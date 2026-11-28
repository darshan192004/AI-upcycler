//! Deterministic task workflow contract (spec 2026-11-27, §10).
//!
//! The executor never talks to a model directly: a [`StepRunner`] seam does.
//! That keeps ordering, failure propagation and event emission testable without
//! SQLite, HTTP, or a provider key.

pub mod backend_runner;
pub mod executor;
pub mod validation;

use serde::{Deserialize, Serialize};

pub use backend_runner::RouterBackend;
pub use executor::{
    execute, EventSink, ExecutionResult, StepFuture, StepRecord, StepRunner, WorkflowEvent,
};
pub use validation::validate_steps;

/// One editable step of a workflow. `id` is the stable business key used by
/// both UI lists and the database; it is never an array index.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkflowStep {
    pub id: String,
    pub prompt: String,
    #[serde(default)]
    pub depends_on: Vec<String>,
    #[serde(default)]
    pub provider: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum StepStatus {
    Pending,
    Running,
    Completed,
    Failed,
    Blocked,
}

impl StepStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Running => "running",
            Self::Completed => "completed",
            Self::Failed => "failed",
            Self::Blocked => "blocked",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RunStatus {
    Running,
    Completed,
    Failed,
}

impl RunStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Running => "running",
            Self::Completed => "completed",
            Self::Failed => "failed",
        }
    }
}

/// What a successful runner invocation produced. `source` is `local`,
/// `adb:<serial>`, or a provider id and is forwarded to telemetry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StepOutput {
    pub content: String,
    pub source: String,
}
