//! Task workflow REST surface (spec 2026-11-27, §10).

use super::{ApiError, ApiResult, AppState};
use crate::store::NewWorkflowStep;
use crate::telemetry::{WorkflowStepCompletedData, WorkflowStepStartedData};
use crate::workflows::{execute, EventSink, RouterBackend, WorkflowEvent, WorkflowStep};
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::Json;
use serde::{Deserialize, Serialize};
use std::sync::Arc;

#[derive(Debug, Deserialize)]
pub struct WorkflowRequest {
    pub model: String,
    pub steps: Vec<WorkflowStep>,
}

#[derive(Debug, Serialize)]
pub struct CreateWorkflowResponse {
    pub run_id: String,
    pub status: &'static str,
}

#[derive(Debug, Serialize)]
pub struct ListWorkflowsResponse {
    pub runs: Vec<crate::store::WorkflowRunRow>,
}

#[derive(Debug, Serialize)]
pub struct WorkflowDetailResponse {
    pub run: crate::store::WorkflowRunRow,
    pub steps: Vec<crate::store::WorkflowStepRow>,
}

pub async fn create_workflow(
    State(state): State<AppState>,
    Json(request): Json<WorkflowRequest>,
) -> ApiResult<(StatusCode, Json<CreateWorkflowResponse>)> {
    let model = request.model.trim();
    if model.is_empty() {
        return Err(ApiError::BadRequest("model must not be empty".into()));
    }
    let steps = crate::workflows::validate_steps(&request.steps).map_err(ApiError::BadRequest)?;

    let run_id = format!("wf-{}", uuid::Uuid::new_v4());
    let new_steps = steps
        .iter()
        .enumerate()
        .map(|(position, step)| {
            Ok(NewWorkflowStep {
                step_id: step.id.clone(),
                prompt: step.prompt.clone(),
                depends_on_json: serde_json::to_string(&step.depends_on)
                    .map_err(|error| ApiError::BadRequest(error.to_string()))?,
                provider: step.provider.clone(),
                position: position as i64,
            })
        })
        .collect::<ApiResult<Vec<_>>>()?;

    state
        .store
        .create_workflow_run(&run_id, model, &new_steps)?;
    tokio::spawn(run_workflow(
        state.clone(),
        run_id.clone(),
        model.to_string(),
        steps,
    ));

    Ok((
        StatusCode::ACCEPTED,
        Json(CreateWorkflowResponse {
            run_id,
            status: "running",
        }),
    ))
}

pub async fn list_workflows(
    State(state): State<AppState>,
) -> ApiResult<Json<ListWorkflowsResponse>> {
    Ok(Json(ListWorkflowsResponse {
        runs: state.store.list_workflow_runs()?,
    }))
}

pub async fn get_workflow(
    State(state): State<AppState>,
    Path(run_id): Path<String>,
) -> ApiResult<Json<WorkflowDetailResponse>> {
    let run = state
        .store
        .get_workflow_run(&run_id)?
        .ok_or_else(|| ApiError::NotFound(format!("workflow run '{run_id}'")))?;
    Ok(Json(WorkflowDetailResponse {
        run,
        steps: state.store.list_workflow_steps(&run_id)?,
    }))
}

/// Persists every transition before publishing it, so a subscriber that reads
/// REST immediately after a WS event always sees the same status.
struct StoreSink {
    store: Arc<crate::store::Store>,
    telemetry: crate::telemetry::TelemetryBus,
}

impl EventSink for StoreSink {
    fn emit(&self, event: WorkflowEvent) -> Result<(), String> {
        match event {
            WorkflowEvent::StepStarted { run_id, step_id } => {
                self.store
                    .set_workflow_step_status(&run_id, &step_id, "running", None, None, None)
                    .map_err(|error| error.to_string())?;
                self.telemetry
                    .publish(crate::telemetry::TelemetryEvent::workflow_step_started(
                        WorkflowStepStartedData { run_id, step_id },
                    ));
            }
            WorkflowEvent::StepCompleted {
                run_id,
                step_id,
                status,
                output,
                source,
                error,
            } => {
                self.store
                    .set_workflow_step_status(
                        &run_id,
                        &step_id,
                        status.as_str(),
                        output.as_deref(),
                        source.as_deref(),
                        error.as_deref(),
                    )
                    .map_err(|error| error.to_string())?;
                self.telemetry
                    .publish(crate::telemetry::TelemetryEvent::workflow_step_completed(
                        WorkflowStepCompletedData {
                            run_id,
                            step_id,
                            status,
                            output,
                            source,
                            error,
                        },
                    ));
            }
            WorkflowEvent::RunFinished { run_id, status } => {
                self.store
                    .set_workflow_run_status(&run_id, status.as_str(), true)
                    .map_err(|error| error.to_string())?;
            }
        }
        Ok(())
    }
}

async fn run_workflow(state: AppState, run_id: String, model: String, steps: Vec<WorkflowStep>) {
    let router = RouterBackend::new(
        state.backend.clone(),
        state.providers.clone(),
        model,
        format!("workflow-{run_id}"),
    );
    let sink = StoreSink {
        store: state.store.clone(),
        telemetry: state.telemetry.clone(),
    };
    if let Err(error) = execute(&run_id, &steps, &router, &sink).await {
        tracing::error!("workflow {run_id} stopped before completion: {error}");
        let _ = sink.emit(WorkflowEvent::RunFinished {
            run_id,
            status: crate::workflows::RunStatus::Failed,
        });
    }
}
