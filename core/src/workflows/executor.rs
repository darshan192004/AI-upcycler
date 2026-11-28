//! Deterministic DAG execution for task workflows.
//!
//! The executor reaches models only through [`StepRunner`] and reports every
//! transition through [`EventSink`], so ordering and failure propagation stay
//! testable without SQLite, HTTP, or a provider key.
use super::{RunStatus, StepOutput, StepStatus, WorkflowStep};
use std::collections::{HashMap, VecDeque};
use std::future::Future;
use std::pin::Pin;

/// Boxed future returned by a step runner. Kept as an alias instead of an
/// `async_trait` dependency so the core crate's dependency set is unchanged.
pub type StepFuture<'a> = Pin<Box<dyn Future<Output = Result<StepOutput, String>> + Send + 'a>>;

/// The only way the executor reaches a model. Tests inject a scripted runner;
/// production injects `RouterBackend` (Task 3).
pub trait StepRunner: Send + Sync {
    fn run<'a>(&'a self, step: &'a WorkflowStep) -> StepFuture<'a>;
}

/// Receives progress in real time. Production's sink persists and broadcasts;
/// tests collect events in a `Mutex<Vec<_>>`.
pub trait EventSink: Send + Sync {
    fn emit(&self, event: WorkflowEvent) -> Result<(), String>;
}

/// Progress emitted before a step starts, when it finishes, and when the run
/// finishes. Events are emitted in execution order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WorkflowEvent {
    StepStarted {
        run_id: String,
        step_id: String,
    },
    StepCompleted {
        run_id: String,
        step_id: String,
        status: StepStatus,
        output: Option<String>,
        source: Option<String>,
        error: Option<String>,
    },
    RunFinished {
        run_id: String,
        status: RunStatus,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StepRecord {
    pub step_id: String,
    pub status: StepStatus,
    pub output: Option<String>,
    pub source: Option<String>,
    pub error: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExecutionResult {
    pub status: RunStatus,
    pub steps: Vec<StepRecord>,
}

/// Kahn's algorithm with a declaration-order ready queue: DAG order is
/// deterministic and independent of `HashMap` iteration order.
pub(crate) fn topological_order(steps: &[WorkflowStep]) -> Result<Vec<usize>, String> {
    let mut by_id = HashMap::with_capacity(steps.len());
    for (index, step) in steps.iter().enumerate() {
        if by_id.insert(step.id.as_str(), index).is_some() {
            return Err(format!("duplicate step id '{}'", step.id));
        }
    }

    let mut indegree = vec![0usize; steps.len()];
    let mut dependents: Vec<Vec<usize>> = vec![Vec::new(); steps.len()];
    for (index, step) in steps.iter().enumerate() {
        for dep in &step.depends_on {
            let dep_index = *by_id
                .get(dep.as_str())
                .ok_or_else(|| format!("step '{}' depends on unknown step '{dep}'", step.id))?;
            indegree[index] += 1;
            dependents[dep_index].push(index);
        }
    }

    let mut ready: VecDeque<usize> = (0..steps.len()).filter(|&i| indegree[i] == 0).collect();
    let mut order = Vec::with_capacity(steps.len());
    while let Some(index) = ready.pop_front() {
        order.push(index);
        for &next in &dependents[index] {
            indegree[next] -= 1;
            if indegree[next] == 0 {
                ready.push_back(next);
            }
        }
    }

    if order.len() != steps.len() {
        return Err("workflow contains a dependency cycle".into());
    }
    Ok(order)
}

/// Executes steps sequentially in deterministic topological order. A failed
/// step blocks its transitive dependents in declaration order; unrelated ready
/// steps still run. Every state transition goes through `sink` before the next
/// step starts, so live progress is real rather than reconstructed afterwards.
pub async fn execute<R, S>(
    run_id: &str,
    steps: &[WorkflowStep],
    runner: &R,
    sink: &S,
) -> Result<ExecutionResult, String>
where
    R: StepRunner,
    S: EventSink,
{
    let order = topological_order(steps)?;
    let mut by_id = HashMap::with_capacity(steps.len());
    for (index, step) in steps.iter().enumerate() {
        by_id.insert(step.id.as_str(), index);
    }

    let mut records: Vec<StepRecord> = steps
        .iter()
        .map(|step| StepRecord {
            step_id: step.id.clone(),
            status: StepStatus::Pending,
            output: None,
            source: None,
            error: None,
        })
        .collect();

    for index in order {
        let step = &steps[index];
        let blocker = step.depends_on.iter().find_map(|dep| {
            let dep_index = *by_id.get(dep.as_str()).expect("validated dependency");
            match records[dep_index].status {
                StepStatus::Failed | StepStatus::Blocked => Some(dep.clone()),
                _ => None,
            }
        });

        if let Some(dep) = blocker {
            let reason = format!("blocked by failed step '{dep}'");
            let record = &mut records[index];
            record.status = StepStatus::Blocked;
            record.error = Some(reason.clone());
            sink.emit(WorkflowEvent::StepCompleted {
                run_id: run_id.into(),
                step_id: step.id.clone(),
                status: StepStatus::Blocked,
                output: None,
                source: None,
                error: Some(reason),
            })?;
            continue;
        }

        records[index].status = StepStatus::Running;
        sink.emit(WorkflowEvent::StepStarted {
            run_id: run_id.into(),
            step_id: step.id.clone(),
        })?;

        match runner.run(step).await {
            Ok(output) => {
                let record = &mut records[index];
                record.status = StepStatus::Completed;
                record.output = Some(output.content);
                record.source = Some(output.source.clone());
                sink.emit(WorkflowEvent::StepCompleted {
                    run_id: run_id.into(),
                    step_id: step.id.clone(),
                    status: StepStatus::Completed,
                    output: record.output.clone(),
                    source: Some(output.source),
                    error: None,
                })?;
            }
            Err(error) => {
                let record = &mut records[index];
                record.status = StepStatus::Failed;
                record.error = Some(error.clone());
                sink.emit(WorkflowEvent::StepCompleted {
                    run_id: run_id.into(),
                    step_id: step.id.clone(),
                    status: StepStatus::Failed,
                    output: None,
                    source: None,
                    error: Some(error),
                })?;
            }
        }
    }

    let status = if records
        .iter()
        .any(|record| matches!(record.status, StepStatus::Failed | StepStatus::Blocked))
    {
        RunStatus::Failed
    } else {
        RunStatus::Completed
    };
    sink.emit(WorkflowEvent::RunFinished {
        run_id: run_id.into(),
        status,
    })?;

    Ok(ExecutionResult {
        status,
        steps: records,
    })
}
