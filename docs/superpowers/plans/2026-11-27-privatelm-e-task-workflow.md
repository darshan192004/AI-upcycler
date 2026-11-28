# PrivateLM E — Task Workflow View Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add a deterministic, model-testable DAG workflow executor with SQLite persistence, REST endpoints, tagged WebSocket progress events, an editable desktop task timeline, and a read-only phone progress screen.

**Architecture:** `core/src/workflows/` owns the `WorkflowStep` contract, validation, a synchronous-in-shape but async `StepRunner` seam, a pure topological executor, and a production `RouterBackend` that routes every step through sub-project A's `RoutePolicy` + `ProviderRegistry` + `AppState.backend`. The executor emits `WorkflowEvent`s to an `EventSink`; in production that sink writes `workflow_runs`/`workflow_steps` and publishes `workflow_step_started`/`workflow_step_completed` on the existing `TelemetryBus`, while tests collect events with a fake sink and a scripted runner (no model, no DB, no HTTP). The desktop shell edits and displays runs through three typed REST calls; the phone only reads them through B's LAN/token surface.

**Tech Stack:** Rust 2021 / axum 0.7 / tokio / rusqlite / `futures::StreamExt` in `core`; React 19 + TypeScript strict + zustand + Tailwind in `src`; Jetpack Compose + Kotlin coroutines + `org.json` in `mobile-worker/android`; Playwright MCP for browser checks; wireless ADB at `192.168.1.9:5555` for phone screenshots.

**Spec:** `docs/superpowers/specs/2026-11-27-privatelm-style-extension-design.md` §10 (task workflow), §12 (UI/UX), §13 (verification), §14 (risks).

## Global Constraints

- Work only in `D:\ai-upcycler-worktrees\cloud-provider-bridge`. Write code only in the files named by each task. Do not overwrite the pre-existing uncommitted edits in `core/src/api/backend.rs`, `docs/agents.md`, or `docs/api.md`; apply targeted edits.
- **Dependency A (hard gate for Tasks 1, 3, and 6):** sub-project A must have landed `crate::routing::RoutePolicy::{Auto,Local,Cloud}`, `RoutePolicy::parse(Option<&str>) -> Result<Self,String>`, `crate::providers::ProviderRegistry::{new,resolve,stream}`, `AppState.secrets`, `AppState.providers`, and `GenerationChunk.source`. Do not invent a `RouterBackend` constructor or a second routing table.
- **Dependency B (hard gate for Tasks 8–10):** B must have landed `design-system/ai-upcycler/MASTER.md`, Compose dependencies in `mobile-worker/android/app/build.gradle.kts`, its Compose host screen with a tab slot that calls E's `WorkflowProgressScreen`, and its LAN client/bearer-token source. Read `design-system/ai-upcycler/MASTER.md` before writing any workflow CSS or Compose styling. E does not add a second palette, font family, or navigation host.
- **Dependency D (schema coordination):** D adds a `settings` table and session endpoints. All `core/src/store/schema.sql` edits in this plan are `CREATE TABLE IF NOT EXISTS` and must coexist with D's DDL. E does not read D's settings or session rows; a workflow step's generation session id is derived from its run id.
- Spec §12 tokens, used verbatim in any new workflow surface: background `#0F172A`, card `#1B2336`, border `#475569`, muted text `#94A3B8`, accent `#22C55E`, destructive `#EF4444`. Body text contrast must be ≥ 4.5:1; use `#FCA5A5` for destructive *text* on the dark card so `#EF4444` can stay a border/status accent.
- Keep Fira Sans / Fira Code from `index.html`; no second font family. Desktop hover/press transitions 150–300 ms; `prefers-reduced-motion` honored; visible focus ring on every control.
- Desktop dynamic lists use stable business ids as React keys, never array indexes. Compose lists use `LazyColumn` with `key = { it.stepId }` / `key = { it.runId }`, 48 dp minimum touch targets, 4/8 dp spacing, no layout-shifting pressed state.
- Status is never color-only: every step renders its textual status; decorative icons, if any, are `aria-hidden="true"` / `contentDescription = null`.
- Verification environment (use for every Rust/npm command in this plan; build artifacts stay off the C drive):

```powershell
$env:RUSTUP_TOOLCHAIN = "stable-x86_64-pc-windows-gnu"
$env:CARGO_TARGET_DIR = "D:\ai-upcycler-target"
$env:PATH = "C:\msys64\mingw64\bin;$env:PATH"
$env:npm_config_cache = "D:\ai-upcycler-npm-cache"
$adb = "D:\Android\Sdk\platform-tools\adb.exe"
```

- No live provider keys in tests. Every provider assertion uses A's `UnconfiguredSecretStore` or a mock server. Loopback stays unauthenticated in this phase; the phone uses B's opt-in, token-authenticated LAN listener only.
- Each task ends with an independently runnable check and a checkpoint. Do not proceed past a failing checkpoint.

---

### Task 1: Workflow contract and validation

**Files:**
- Create: `core/src/workflows/mod.rs`
- Create: `core/src/workflows/validation.rs`
- Create: `core/tests/workflows.rs`
- Modify: `core/src/lib.rs:1-13`

**Interfaces:**
- Consumes: `crate::routing::RoutePolicy::parse(Option<&str>) -> Result<RoutePolicy,String>` from A (used only to validate the `provider` token; no model call).
- Produces:
  - `WorkflowStep { id: String, prompt: String, depends_on: Vec<String>, provider: Option<String> }` with `Debug + Clone + PartialEq + Eq + Serialize + Deserialize`.
  - `StepStatus::{Pending,Running,Completed,Failed,Blocked}` with `as_str() -> &'static str` and `Serialize`.
  - `RunStatus::{Running,Completed,Failed}` with `as_str() -> &'static str`.
  - `validate_steps(&[WorkflowStep]) -> Result<Vec<WorkflowStep>,String>` returning normalized steps in declaration order.
  - `core::workflows::{StepOutput, StepFuture, StepRunner, EventSink, WorkflowEvent, StepRecord, ExecutionResult, execute, topological_order}` re-exported in later tasks.

- [ ] **Step 1: Write the failing contract/validation test**

Create `core/tests/workflows.rs` with:

```rust
use ai_upcycler_core::workflows::{validate_steps, WorkflowStep};

fn step(id: &str, deps: &[&str]) -> WorkflowStep {
    WorkflowStep {
        id: id.into(),
        prompt: format!("prompt for {id}"),
        depends_on: deps.iter().map(|d| (*d).into()).collect(),
        provider: None,
    }
}

#[test]
fn validation_normalizes_whitespace_and_keeps_declaration_order() {
    let steps = vec![
        WorkflowStep {
            id: "  b ".into(),
            prompt: "  second  ".into(),
            depends_on: vec![" a ".into()],
            provider: Some(" local ".into()),
        },
        WorkflowStep {
            id: "a".into(),
            prompt: "first".into(),
            depends_on: vec![],
            provider: None,
        },
    ];

    let normalized = validate_steps(&steps).expect("valid");
    assert_eq!(normalized[0].id, "b");
    assert_eq!(normalized[0].prompt, "second");
    assert_eq!(normalized[0].depends_on, vec!["a"]);
    assert_eq!(normalized[0].provider.as_deref(), Some("local"));
    assert_eq!(normalized[1].id, "a");
}

#[test]
fn validation_rejects_duplicate_self_unknown_and_repeated_dependencies() {
    let dup = vec![step("a", &[]), step("a", &[])];
    assert!(validate_steps(&dup).unwrap_err().contains("duplicate step id 'a'"));

    let self_dep = vec![step("a", &["a"])];
    assert!(validate_steps(&self_dep).unwrap_err().contains("cannot depend on itself"));

    let unknown = vec![step("a", &["ghost"])];
    assert!(validate_steps(&unknown).unwrap_err().contains("unknown step 'ghost'"));

    let repeated = vec![step("a", &[]), step("b", &["a", "a"])];
    assert!(validate_steps(&repeated).unwrap_err().contains("repeats dependency 'a'"));
}

#[test]
fn validation_rejects_empty_input_empty_prompt_and_cycles() {
    assert!(validate_steps(&[]).unwrap_err().contains("at least one step"));

    let empty_prompt = vec![WorkflowStep { id: "a".into(), prompt: "  ".into(), ..step("a", &[]) }];
    assert!(validate_steps(&empty_prompt).unwrap_err().contains("empty prompt"));

    let cycle = vec![step("a", &["b"]), step("b", &["a"])];
    assert!(validate_steps(&cycle).unwrap_err().contains("cycle"));
}

#[test]
fn validation_rejects_an_unknown_provider_token() {
    let mut bad = step("a", &[]);
    bad.provider = Some("not-a-provider".into());
    let error = validate_steps(&[bad]).unwrap_err();
    assert!(error.contains("step 'a'"));
    assert!(error.contains("unknown provider"));
}
```

- [ ] **Step 2: Run the test to verify it fails**

Run:

```powershell
$env:RUSTUP_TOOLCHAIN = "stable-x86_64-pc-windows-gnu"
$env:CARGO_TARGET_DIR = "D:\ai-upcycler-target"
$env:PATH = "C:\msys64\mingw64\bin;$env:PATH"
cargo test -p ai-upcycler-core --test workflows
```

Expected: compile failure naming the missing `ai_upcycler_core::workflows` module.

- [ ] **Step 3: Create `core/src/workflows/mod.rs`**

```rust
//! Deterministic task workflow contract (spec 2026-11-27, §10).
//!
//! The executor never talks to a model directly: a [`StepRunner`] seam does.
//! That keeps ordering, failure propagation and event emission testable without
//! SQLite, HTTP, or a provider key.

pub mod executor;
pub mod validation;

use serde::{Deserialize, Serialize};

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
```

- [ ] **Step 4: Create `core/src/workflows/validation.rs`**

```rust
use super::executor::topological_order;
use super::WorkflowStep;
use std::collections::HashSet;

/// Trims ids/prompts/dependencies/provider tokens, checks referential
/// integrity, then proves the graph is acyclic. The returned vector keeps the
/// user's declaration order; execution order is derived separately.
pub fn validate_steps(steps: &[WorkflowStep]) -> Result<Vec<WorkflowStep>, String> {
    if steps.is_empty() {
        return Err("workflow must contain at least one step".into());
    }

    let mut normalized = Vec::with_capacity(steps.len());
    let mut seen = HashSet::new();

    for step in steps {
        let id = step.id.trim();
        if id.is_empty() {
            return Err("step id must not be empty".into());
        }
        if !seen.insert(id.to_string()) {
            return Err(format!("duplicate step id '{id}'"));
        }

        let prompt = step.prompt.trim();
        if prompt.is_empty() {
            return Err(format!("step '{id}' has an empty prompt"));
        }

        let mut depends_on = Vec::with_capacity(step.depends_on.len());
        let mut dep_seen = HashSet::new();
        for dep in &step.depends_on {
            let dep = dep.trim();
            if dep.is_empty() {
                return Err(format!("step '{id}' has an empty dependency"));
            }
            if dep == id {
                return Err(format!("step '{id}' cannot depend on itself"));
            }
            if !dep_seen.insert(dep.to_string()) {
                return Err(format!("step '{id}' repeats dependency '{dep}'"));
            }
            depends_on.push(dep.to_string());
        }

        let provider = step
            .provider
            .as_ref()
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty());
        if let Some(value) = &provider {
            crate::routing::RoutePolicy::parse(Some(value))
                .map_err(|error| format!("step '{id}': {error}"))?;
        }

        normalized.push(WorkflowStep {
            id: id.into(),
            prompt: prompt.into(),
            depends_on,
            provider,
        });
    }

    for step in &normalized {
        for dep in &step.depends_on {
            if !seen.contains(dep) {
                return Err(format!("step '{}' depends on unknown step '{dep}'", step.id));
            }
        }
    }

    topological_order(&normalized)?;
    Ok(normalized)
}
```

- [ ] **Step 5: Add module registration and the remaining stubs needed to compile**

Add to `core/src/lib.rs` after `pub mod provisioning;`:

```rust
pub mod workflows;
```

Create `core/src/workflows/executor.rs` with only the deterministic ordering helper that validation needs; Task 2 replaces this file with the full runner/sink executor.

```rust
//! Task 1 stub: validation needs cycle detection before the executor lands.
use super::WorkflowStep;
use std::collections::{HashMap, VecDeque};

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
```

Task 3 adds `backend_runner.rs`; no production runner exists in this task.

- [ ] **Step 6: Run the test to verify it passes**

Run the command from Step 2.

Expected: `4 passed` for `core/tests/workflows.rs`.

- [ ] **Step 7: Check formatting and commit**

```powershell
$env:RUSTUP_TOOLCHAIN = "stable-x86_64-pc-windows-gnu"
$env:CARGO_TARGET_DIR = "D:\ai-upcycler-target"
$env:PATH = "C:\msys64\mingw64\bin;$env:PATH"
cargo fmt --all -- --check
git add core/src/lib.rs core/src/workflows/mod.rs core/src/workflows/validation.rs core/src/workflows/executor.rs core/tests/workflows.rs
git commit -m "test: define workflow step contract and validation"
```

**Checkpoint:** `validate_steps` rejects every malformed graph with an exact message, and no test has started a model, opened SQLite, or bound a socket.

---

### Task 2: Pure deterministic DAG executor

**Files:**
- Create/Modify: `core/src/workflows/executor.rs`
- Modify: `core/tests/workflows.rs`

**Interfaces:**
- Consumes: `WorkflowStep`, `StepStatus`, `RunStatus`, `StepOutput` from Task 1; `validate_steps` for input shape (the executor re-checks graph integrity itself so it is safe when called directly).
- Produces:
  - `type StepFuture<'a> = Pin<Box<dyn Future<Output=Result<StepOutput,String>> + Send + 'a>>`
  - `trait StepRunner: Send + Sync { fn run<'a>(&'a self, step: &'a WorkflowStep) -> StepFuture<'a>; }`
  - `trait EventSink: Send + Sync { fn emit(&self, event: WorkflowEvent) -> Result<(),String>; }`
  - `enum WorkflowEvent::{StepStarted{run_id,step_id},StepCompleted{run_id,step_id,status,output,source,error},RunFinished{run_id,status}}`
  - `struct StepRecord { step_id, status, output, source, error }`
  - `struct ExecutionResult { status: RunStatus, steps: Vec<StepRecord> }`
  - `pub(crate) fn topological_order(&[WorkflowStep]) -> Result<Vec<usize>,String>`
  - `pub async fn execute<R: StepRunner,S: EventSink>(run_id:&str, steps:&[WorkflowStep], runner:&R, sink:&S) -> Result<ExecutionResult,String>`

- [ ] **Step 1: Write the failing executor tests**

Replace Task 1's `use ai_upcycler_core::workflows::{validate_steps, WorkflowStep};` line with this single cumulative import, then append the rest of the block to `core/tests/workflows.rs` (never import the same name twice in one test file):

```rust
use ai_upcycler_core::workflows::{
    execute, validate_steps, EventSink, ExecutionResult, StepFuture, StepOutput, StepRecord,
    StepRunner, StepStatus, WorkflowEvent, WorkflowStep,
};
use std::future::Future;
use std::pin::Pin;
use std::sync::Mutex;

struct ScriptedRunner {
    calls: Mutex<Vec<String>>,
    fail: Vec<String>,
}

impl StepRunner for ScriptedRunner {
    fn run<'a>(&'a self, step: &'a WorkflowStep) -> StepFuture<'a> {
        let id = step.id.clone();
        let fail = self.fail.contains(&id);
        Box::pin(async move {
            self.calls.lock().unwrap().push(id.clone());
            if fail {
                Err(format!("boom:{id}"))
            } else {
                Ok(StepOutput {
                    content: format!("out:{id}"),
                    source: "local".into(),
                })
            }
        })
    }
}

#[derive(Default)]
struct CollectingSink {
    events: Mutex<Vec<WorkflowEvent>>,
}

impl EventSink for CollectingSink {
    fn emit(&self, event: WorkflowEvent) -> Result<(), String> {
        self.events.lock().unwrap().push(event);
        Ok(())
    }
}

fn workflow_steps(ids: &[&str]) -> Vec<WorkflowStep> {
    ids.iter()
        .map(|id| WorkflowStep {
            id: (*id).into(),
            prompt: format!("prompt {id}"),
            depends_on: vec![],
            provider: None,
        })
        .collect()
}

#[tokio::test]
async fn executor_runs_independent_steps_in_declaration_order() {
    let steps = workflow_steps(&["a", "b", "c"]);
    let runner = ScriptedRunner { calls: Mutex::new(vec![]), fail: vec![] };
    let sink = CollectingSink::default();

    let result: ExecutionResult = execute("wf-1", &steps, &runner, &sink).await.unwrap();

    assert_eq!(runner.calls.into_inner().unwrap(), vec!["a", "b", "c"]);
    assert_eq!(result.status.as_str(), "completed");
    assert!(result.steps.iter().all(|s| s.status == StepStatus::Completed));
    assert_eq!(result.steps[1].output.as_deref(), Some("out:b"));

    let events = sink.events.into_inner().unwrap();
    let kinds: Vec<&str> = events
        .iter()
        .map(|event| match event {
            WorkflowEvent::StepStarted { .. } => "started",
            WorkflowEvent::StepCompleted { .. } => "completed",
            WorkflowEvent::RunFinished { .. } => "finished",
        })
        .collect();
    assert_eq!(kinds, vec!["started", "completed", "started", "completed", "started", "completed", "finished"]);
}

#[tokio::test]
async fn executor_respects_dependencies_before_running_a_step() {
    let mut steps = workflow_steps(&["b", "a"]);
    steps[0].depends_on = vec!["a".into()];
    let runner = ScriptedRunner { calls: Mutex::new(vec![]), fail: vec![] };
    let sink = CollectingSink::default();

    let result = execute("wf-2", &steps, &runner, &sink).await.unwrap();

    assert_eq!(runner.calls.into_inner().unwrap(), vec!["a", "b"]);
    assert_eq!(result.steps[0].status, StepStatus::Completed);
    assert_eq!(result.steps[1].status, StepStatus::Completed);
}

#[tokio::test]
async fn executor_blocks_transitive_dependents_but_keeps_independent_steps() {
    let mut steps = workflow_steps(&["a", "b", "c", "d"]);
    steps[1].depends_on = vec!["a".into()];
    steps[3].depends_on = vec!["b".into()];
    let runner = ScriptedRunner {
        calls: Mutex::new(vec![]),
        fail: vec!["a".into()],
    };
    let sink = CollectingSink::default();

    let result = execute("wf-3", &steps, &runner, &sink).await.unwrap();

    assert_eq!(runner.calls.into_inner().unwrap(), vec!["a", "c"]);
    assert_eq!(result.status.as_str(), "failed");
    assert_eq!(result.steps[0].status, StepStatus::Failed);
    assert_eq!(result.steps[0].error.as_deref(), Some("boom:a"));
    assert_eq!(result.steps[1].status, StepStatus::Blocked);
    assert_eq!(result.steps[1].error.as_deref(), Some("blocked by failed step 'a'"));
    assert_eq!(result.steps[2].status, StepStatus::Completed);
    assert_eq!(result.steps[3].status, StepStatus::Blocked);
    assert_eq!(result.steps[3].error.as_deref(), Some("blocked by failed step 'b'"));
}

#[tokio::test]
async fn executor_propagates_sink_failures() {
    struct FailingSink;
    impl EventSink for FailingSink {
        fn emit(&self, _event: WorkflowEvent) -> Result<(), String> {
            Err("sink down".into())
        }
    }

    let steps = workflow_steps(&["a"]);
    let runner = ScriptedRunner { calls: Mutex::new(vec![]), fail: vec![] };
    let error = execute("wf-4", &steps, &runner, &FailingSink).await.unwrap_err();
    assert_eq!(error, "sink down");
}
```

- [ ] **Step 2: Run the tests to verify they fail**

Run:

```powershell
$env:RUSTUP_TOOLCHAIN = "stable-x86_64-pc-windows-gnu"
$env:CARGO_TARGET_DIR = "D:\ai-upcycler-target"
$env:PATH = "C:\msys64\mingw64\bin;$env:PATH"
cargo test -p ai-upcycler-core --test workflows
```

Expected: compile failure naming `StepRunner`, `EventSink`, and `execute` as unresolved.

- [ ] **Step 3: Replace the Task 1 stub with the full `core/src/workflows/executor.rs`**

```rust
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

    Ok(ExecutionResult { status, steps: records })
}
```

- [ ] **Step 4: Re-export the event enum from `core/src/workflows/mod.rs`**

The `WorkflowEvent` enum is defined in `core/src/workflows/executor.rs` alongside `EventSink` (Step 3), not in `mod.rs`. Add this re-export beside `pub use validation::validate_steps;` in `core/src/workflows/mod.rs` (do not re-export `topological_order`; it is crate-private):

```rust
pub use executor::{
    execute, EventSink, ExecutionResult, StepFuture, StepRecord, StepRunner, WorkflowEvent,
};
```

- [ ] **Step 5: Run the tests to verify they pass**

Run the command from Step 2.

Expected: all `core/tests/workflows.rs` tests pass, including the transitive-block case.

- [ ] **Step 6: Check formatting and commit**

```powershell
$env:RUSTUP_TOOLCHAIN = "stable-x86_64-pc-windows-gnu"
$env:CARGO_TARGET_DIR = "D:\ai-upcycler-target"
$env:PATH = "C:\msys64\mingw64\bin;$env:PATH"
cargo fmt --all -- --check
git add core/src/workflows/mod.rs core/src/workflows/executor.rs core/tests/workflows.rs
git commit -m "test: add deterministic workflow DAG executor"
```

**Checkpoint:** Ordering, failure propagation, blocking, and event ordering are proven with a scripted runner; the executor never imports `Store`, `TelemetryBus`, or `reqwest`.

---

### Task 3: RouterBackend production step runner

**Files:**
- Create/Modify: `core/src/workflows/backend_runner.rs`
- Modify: `core/tests/workflows.rs`

**Interfaces:**
- Consumes: `StepRunner`, `StepFuture`, `StepOutput`, `WorkflowStep` from Task 1–2; A's `crate::routing::RoutePolicy`, `crate::providers::{ProviderRegistry, CloudRoute}`, `crate::api::backend::{ChatMessage, GenerationBackend, GenerationRequest}`, and `futures::StreamExt`.
- Produces: `RouterBackend::new(backend: Arc<dyn GenerationBackend>, providers: Arc<ProviderRegistry>, model: String, session_id: String) -> Self`, implementing `StepRunner` by calling `state.backend.generate` or `providers.stream` exactly once per step.

- [ ] **Step 1: Write the failing backend-runner tests**

Replace the workflows import in `core/tests/workflows.rs` with this cumulative import (do not duplicate `StepRunner`), then append the new imports and tests:

```rust
use ai_upcycler_core::api::backend::StubBackend;
use ai_upcycler_core::providers::ProviderRegistry;
use ai_upcycler_core::secrets::UnconfiguredSecretStore;
use ai_upcycler_core::workflows::{
    execute, validate_steps, EventSink, ExecutionResult, RouterBackend, StepFuture, StepOutput,
    StepRecord, StepRunner, StepStatus, WorkflowEvent, WorkflowStep,
};
use std::sync::Arc;

fn router_backend() -> RouterBackend {
    RouterBackend::new(
        Arc::new(StubBackend::new()),
        Arc::new(ProviderRegistry::new(Arc::new(UnconfiguredSecretStore))),
        "llama-8b".into(),
        "wf-test".into(),
    )
}

#[tokio::test]
async fn router_backend_runs_an_explicit_local_step_through_the_backend() {
    let backend = router_backend();
    let step = WorkflowStep {
        id: "local".into(),
        prompt: "Say hello".into(),
        depends_on: vec![],
        provider: Some("local".into()),
    };

    let output = backend.run(&step).await.expect("local step");

    assert!(output.content.contains("[stub:llama-8b]"));
    assert!(output.content.contains("Say hello"));
    assert_eq!(output.source, "local");
}

#[tokio::test]
async fn router_backend_refuses_an_unconfigured_cloud_step_without_network() {
    let backend = router_backend();
    let step = WorkflowStep {
        id: "cloud".into(),
        prompt: "Say hello".into(),
        depends_on: vec![],
        provider: Some("openai".into()),
    };

    let error = backend.run(&step).await.unwrap_err();

    assert!(error.contains("openai"), "{error}");
    assert!(error.contains("not configured"), "{error}");
}

#[tokio::test]
async fn router_backend_rejects_an_unknown_provider_before_generating() {
    let backend = router_backend();
    let step = WorkflowStep {
        id: "bad".into(),
        prompt: "Say hello".into(),
        depends_on: vec![],
        provider: Some("ghost".into()),
    };

    let error = backend.run(&step).await.unwrap_err();

    assert!(error.contains("unknown provider 'ghost'"), "{error}");
}
```

- [ ] **Step 2: Run the tests to verify they fail**

Run:

```powershell
$env:RUSTUP_TOOLCHAIN = "stable-x86_64-pc-windows-gnu"
$env:CARGO_TARGET_DIR = "D:\ai-upcycler-target"
$env:PATH = "C:\msys64\mingw64\bin;$env:PATH"
cargo test -p ai-upcycler-core --test workflows router_backend
```

Expected: compile failure naming `RouterBackend` as unresolved.

- [ ] **Step 3: Implement `core/src/workflows/backend_runner.rs`**

First add these two lines to `core/src/workflows/mod.rs`:

```rust
pub mod backend_runner;
pub use backend_runner::RouterBackend;
```

Then create the file:

```rust
use super::{StepFuture, StepOutput, StepRunner, WorkflowStep};
use crate::api::backend::{ChatMessage, GenerationBackend, GenerationRequest};
use crate::providers::{CloudRoute, ProviderRegistry};
use crate::routing::RoutePolicy;
use futures::StreamExt;
use std::sync::Arc;

/// The single production bridge from a workflow step to generation. Routing
/// decisions are delegated to sub-project A; this struct owns no provider
/// table, no secret store and no API keys of its own.
pub struct RouterBackend {
    backend: Arc<dyn GenerationBackend>,
    providers: Arc<ProviderRegistry>,
    model: String,
    session_id: String,
}

impl RouterBackend {
    pub fn new(
        backend: Arc<dyn GenerationBackend>,
        providers: Arc<ProviderRegistry>,
        model: String,
        session_id: String,
    ) -> Self {
        Self { backend, providers, model, session_id }
    }

    fn route(&self, step: &WorkflowStep) -> Result<(RoutePolicy, Option<CloudRoute>), String> {
        let policy = RoutePolicy::parse(step.provider.as_deref())?;
        let cloud = self
            .providers
            .resolve(&self.model, step.provider.as_deref())?;
        Ok((policy, cloud))
    }
}

impl StepRunner for RouterBackend {
    fn run<'a>(&'a self, step: &'a WorkflowStep) -> StepFuture<'a> {
        Box::pin(async move {
            let (policy, cloud) = self.route(step)?;
            let selected = match (policy, cloud) {
                (RoutePolicy::Local, _) => None,
                (RoutePolicy::Cloud(_), Some(route)) => Some(route),
                (RoutePolicy::Cloud(_), None) => {
                    return Err(format!(
                        "workflow step '{}': provider '{}' is not configured for model '{}'",
                        step.id,
                        step.provider.as_deref().unwrap_or("auto"),
                        self.model
                    ));
                }
                (RoutePolicy::Auto, Some(route)) if !self.backend.is_real() => Some(route),
                (RoutePolicy::Auto, None) if !self.backend.is_real() => {
                    return Err(format!(
                        "workflow step '{}': no local engine and no cloud provider configured for model '{}'",
                        step.id, self.model
                    ));
                }
                (RoutePolicy::Auto, _) => None,
            };

            let generation_model = selected
                .as_ref()
                .map(|route| route.model.clone())
                .unwrap_or_else(|| self.model.clone());
            let request = GenerationRequest {
                model: generation_model,
                messages: vec![ChatMessage {
                    role: "user".into(),
                    content: step.prompt.clone(),
                    ..Default::default()
                }],
                temperature: 0.2,
                max_tokens: 512,
                session_id: self.session_id.clone(),
                ..Default::default()
            };

            let mut stream = match selected {
                Some(route) => self.providers.stream(route, request),
                None => self.backend.generate(request),
            };

            let mut content = String::new();
            let mut source = None;
            while let Some(chunk) = stream.next().await {
                let chunk = chunk?;
                content.push_str(&chunk.content);
                if let Some(value) = chunk.source {
                    source = Some(value);
                }
            }

            Ok(StepOutput {
                content,
                source: source.unwrap_or_else(|| "local".into()),
            })
        })
    }
}
```

- [ ] **Step 4: Run the tests to verify they pass**

Run the command from Step 2.

Expected: `3 passed`, all without contacting a real provider.

- [ ] **Step 5: Check formatting and commit**

```powershell
$env:RUSTUP_TOOLCHAIN = "stable-x86_64-pc-windows-gnu"
$env:CARGO_TARGET_DIR = "D:\ai-upcycler-target"
$env:PATH = "C:\msys64\mingw64\bin;$env:PATH"
cargo fmt --all -- --check
git add core/src/workflows/mod.rs core/src/workflows/backend_runner.rs core/tests/workflows.rs
git commit -m "test: route workflow steps through the provider bridge"
```

**Checkpoint:** Every step that reaches a model goes through A's `RoutePolicy`/`ProviderRegistry` or `AppState.backend`; there is no second provider switch and no key material in the workflow module.

---

### Task 4: Workflow SQLite schema and store API

**Files:**
- Modify: `core/src/store/schema.sql`
- Modify: `core/src/store/mod.rs`
- Modify: `core/tests/workflows.rs`

**Interfaces:**
- Consumes: existing `Store::{open_in_memory,lock}` and `StoreError` from `core/src/store/mod.rs`.
- Produces:
  - `NewWorkflowStep { step_id, prompt, depends_on_json, provider, position }`
  - `WorkflowRunRow { run_id, model, status, created_at, finished_at }`
  - `WorkflowStepRow { run_id, step_id, prompt, depends_on: Vec<String>, provider, status, output, source, error, position }`
  - `Store::{create_workflow_run, list_workflow_runs, get_workflow_run, list_workflow_steps, set_workflow_step_status, set_workflow_run_status}`

- [ ] **Step 1: Write the failing store tests**

Append to `core/tests/workflows.rs`:

```rust
use ai_upcycler_core::store::{NewWorkflowStep, Store};

fn new_step(id: &str, deps: &[&str], position: i64) -> NewWorkflowStep {
    NewWorkflowStep {
        step_id: id.into(),
        prompt: format!("prompt {id}"),
        depends_on_json: serde_json::to_string(&deps.to_vec()).unwrap(),
        provider: Some("local".into()),
        position,
    }
}

#[test]
fn workflow_run_and_steps_round_trip_and_update() {
    let store = Store::open_in_memory().unwrap();
    store
        .create_workflow_run(
            "wf-1",
            "llama-8b",
            &[new_step("a", &[], 0), new_step("b", &["a"], 1)],
        )
        .unwrap();

    let runs = store.list_workflow_runs().unwrap();
    assert_eq!(runs.len(), 1);
    assert_eq!(runs[0].status, "running");
    assert_eq!(runs[0].model, "llama-8b");
    assert!(runs[0].finished_at.is_none());

    let steps = store.list_workflow_steps("wf-1").unwrap();
    assert_eq!(steps.len(), 2);
    assert_eq!(steps[1].depends_on, vec!["a"]);
    assert_eq!(steps[1].position, 1);

    store
        .set_workflow_step_status(
            "wf-1",
            "a",
            "completed",
            Some("answer"),
            Some("local"),
            None,
        )
        .unwrap();
    store
        .set_workflow_run_status("wf-1", "completed", true)
        .unwrap();

    let step = &store.list_workflow_steps("wf-1").unwrap()[0];
    assert_eq!(step.status, "completed");
    assert_eq!(step.output.as_deref(), Some("answer"));
    let run = store.get_workflow_run("wf-1").unwrap().unwrap();
    assert_eq!(run.status, "completed");
    assert!(run.finished_at.is_some());
}

#[test]
fn workflow_steps_are_scoped_to_their_run() {
    let store = Store::open_in_memory().unwrap();
    store.create_workflow_run("wf-1", "m", &[new_step("a", &[], 0)]).unwrap();
    store.create_workflow_run("wf-2", "m", &[new_step("a", &[], 0)]).unwrap();

    assert_eq!(store.list_workflow_steps("wf-1").unwrap().len(), 1);
    assert_eq!(store.list_workflow_steps("wf-2").unwrap().len(), 1);
    assert!(store.get_workflow_run("missing").unwrap().is_none());
}
```

- [ ] **Step 2: Run the tests to verify they fail**

Run:

```powershell
$env:RUSTUP_TOOLCHAIN = "stable-x86_64-pc-windows-gnu"
$env:CARGO_TARGET_DIR = "D:\ai-upcycler-target"
$env:PATH = "C:\msys64\mingw64\bin;$env:PATH"
cargo test -p ai-upcycler-core --test workflows workflow_
```

Expected: compile failure naming `NewWorkflowStep` and the new `Store` methods as unresolved.

- [ ] **Step 3: Add idempotent DDL to `core/src/store/schema.sql`**

Append to the end of the file:

```sql
-- Task workflow runs (spec 2026-11-27, §10). All DDL remains idempotent so
-- this file can be applied on every open alongside sub-project D's settings.
CREATE TABLE IF NOT EXISTS workflow_runs (
    run_id TEXT PRIMARY KEY,
    model TEXT NOT NULL,
    status TEXT NOT NULL,          -- 'running', 'completed', 'failed'
    created_at DATETIME DEFAULT CURRENT_TIMESTAMP,
    finished_at DATETIME
);

CREATE TABLE IF NOT EXISTS workflow_steps (
    run_id TEXT NOT NULL,
    step_id TEXT NOT NULL,
    prompt TEXT NOT NULL,
    depends_on TEXT NOT NULL,      -- JSON array of stable step ids
    provider TEXT,                 -- NULL/'auto'/'local'/provider id
    status TEXT NOT NULL,          -- pending|running|completed|failed|blocked
    output TEXT,
    source TEXT,
    error TEXT,
    position INTEGER NOT NULL,
    PRIMARY KEY (run_id, step_id),
    FOREIGN KEY (run_id) REFERENCES workflow_runs(run_id) ON DELETE CASCADE
);
```

- [ ] **Step 4: Add store structs and methods in `core/src/store/mod.rs`**

Insert after `NewChatMessage`:

```rust
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct NewWorkflowStep {
    pub step_id: String,
    pub prompt: String,
    pub depends_on_json: String,
    pub provider: Option<String>,
    pub position: i64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WorkflowRunRow {
    pub run_id: String,
    pub model: String,
    pub status: String,
    pub created_at: String,
    pub finished_at: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WorkflowStepRow {
    pub run_id: String,
    pub step_id: String,
    pub prompt: String,
    pub depends_on: Vec<String>,
    pub provider: Option<String>,
    pub status: String,
    pub output: Option<String>,
    pub source: Option<String>,
    pub error: Option<String>,
    pub position: i64,
}
```

Insert before the closing `}` of `impl Store`:

```rust
    // --- workflows ----------------------------------------------------------

    pub fn create_workflow_run(
        &self,
        run_id: &str,
        model: &str,
        steps: &[NewWorkflowStep],
    ) -> Result<(), StoreError> {
        let mut conn = self.lock()?;
        let tx = conn.transaction()?;
        tx.execute(
            "INSERT INTO workflow_runs (run_id, model, status) VALUES (?1, ?2, 'running')",
            params![run_id, model],
        )?;
        for step in steps {
            tx.execute(
                "INSERT INTO workflow_steps \
                 (run_id, step_id, prompt, depends_on, provider, status, position) \
                 VALUES (?1, ?2, ?3, ?4, ?5, 'pending', ?6)",
                params![
                    run_id,
                    step.step_id,
                    step.prompt,
                    step.depends_on_json,
                    step.provider,
                    step.position,
                ],
            )?;
        }
        tx.commit()?;
        Ok(())
    }

    pub fn list_workflow_runs(&self) -> Result<Vec<WorkflowRunRow>, StoreError> {
        let conn = self.lock()?;
        let mut stmt = conn.prepare(
            "SELECT run_id, model, status, created_at, finished_at \
             FROM workflow_runs ORDER BY created_at, run_id",
        )?;
        let rows = stmt.query_map([], |row| {
            Ok(WorkflowRunRow {
                run_id: row.get(0)?,
                model: row.get(1)?,
                status: row.get(2)?,
                created_at: row.get(3)?,
                finished_at: row.get(4)?,
            })
        })?;
        rows.collect::<Result<Vec<_>, _>>().map_err(Into::into)
    }

    pub fn get_workflow_run(&self, run_id: &str) -> Result<Option<WorkflowRunRow>, StoreError> {
        let conn = self.lock()?;
        conn.query_row(
            "SELECT run_id, model, status, created_at, finished_at \
             FROM workflow_runs WHERE run_id = ?1",
            params![run_id],
            |row| {
                Ok(WorkflowRunRow {
                    run_id: row.get(0)?,
                    model: row.get(1)?,
                    status: row.get(2)?,
                    created_at: row.get(3)?,
                    finished_at: row.get(4)?,
                })
            },
        )
        .optional()
        .map_err(Into::into)
    }

    pub fn list_workflow_steps(&self, run_id: &str) -> Result<Vec<WorkflowStepRow>, StoreError> {
        let conn = self.lock()?;
        let mut stmt = conn.prepare(
            "SELECT run_id, step_id, prompt, depends_on, provider, status, \
             output, source, error, position FROM workflow_steps \
             WHERE run_id = ?1 ORDER BY position, step_id",
        )?;
        let rows = stmt.query_map(params![run_id], |row| {
            let raw: String = row.get(3)?;
            let depends_on: Vec<String> = serde_json::from_str(&raw).map_err(|error| {
                rusqlite::Error::FromSqlConversionFailure(
                    3,
                    rusqlite::types::Type::Text,
                    Box::new(error),
                )
            })?;
            Ok(WorkflowStepRow {
                run_id: row.get(0)?,
                step_id: row.get(1)?,
                prompt: row.get(2)?,
                depends_on,
                provider: row.get(4)?,
                status: row.get(5)?,
                output: row.get(6)?,
                source: row.get(7)?,
                error: row.get(8)?,
                position: row.get(9)?,
            })
        })?;
        rows.collect::<Result<Vec<_>, _>>().map_err(Into::into)
    }

    pub fn set_workflow_step_status(
        &self,
        run_id: &str,
        step_id: &str,
        status: &str,
        output: Option<&str>,
        source: Option<&str>,
        error: Option<&str>,
    ) -> Result<(), StoreError> {
        let conn = self.lock()?;
        conn.execute(
            "UPDATE workflow_steps SET status = ?1, output = ?2, source = ?3, error = ?4 \
             WHERE run_id = ?5 AND step_id = ?6",
            params![status, output, source, error, run_id, step_id],
        )?;
        Ok(())
    }

    pub fn set_workflow_run_status(
        &self,
        run_id: &str,
        status: &str,
        finished: bool,
    ) -> Result<(), StoreError> {
        let conn = self.lock()?;
        if finished {
            conn.execute(
                "UPDATE workflow_runs SET status = ?1, finished_at = CURRENT_TIMESTAMP \
                 WHERE run_id = ?2",
                params![status, run_id],
            )?;
        } else {
            conn.execute(
                "UPDATE workflow_runs SET status = ?1 WHERE run_id = ?2",
                params![status, run_id],
            )?;
        }
        Ok(())
    }
}
```

- [ ] **Step 5: Run the tests to verify they pass**

Run the command from Step 2.

Expected: `2 passed` for the workflow store tests; `cargo test -p ai-upcycler-core --lib store::tests` still passes with the new DDL.

- [ ] **Step 6: Check formatting and commit**

```powershell
$env:RUSTUP_TOOLCHAIN = "stable-x86_64-pc-windows-gnu"
$env:CARGO_TARGET_DIR = "D:\ai-upcycler-target"
$env:PATH = "C:\msys64\mingw64\bin;$env:PATH"
cargo fmt --all -- --check
git add core/src/store/schema.sql core/src/store/mod.rs core/tests/workflows.rs
git commit -m "test: persist workflow runs and steps"
```

**Checkpoint:** Runs and steps round-trip in declaration order, dependents are stored as JSON arrays, and opening the DB twice does not duplicate or rebuild tables.

---

### Task 5: Tagged workflow WebSocket events

**Files:**
- Modify: `core/src/telemetry.rs`
- Modify: `core/tests/workflows.rs`

**Interfaces:**
- Consumes: `crate::workflows::StepStatus` from Task 1; existing `TelemetryBus::publish` and `#[serde(tag="event", rename_all="snake_case")]` on `TelemetryEvent`.
- Produces: `WorkflowStepStartedData { run_id, step_id }`, `WorkflowStepCompletedData { run_id, step_id, status, output, source, error }`, `TelemetryEvent::workflow_step_started`, `TelemetryEvent::workflow_step_completed`.

- [ ] **Step 1: Write the failing serialization tests**

Append these imports to `core/tests/workflows.rs`; `StepStatus` is already in the cumulative workflows import from Task 3, so do not import it again:

```rust
use ai_upcycler_core::telemetry::{
    TelemetryBus, TelemetryEvent, WorkflowStepCompletedData, WorkflowStepStartedData,
};

#[test]
fn workflow_step_events_use_the_tagged_wire_format() {
    let started = TelemetryEvent::workflow_step_started(WorkflowStepStartedData {
        run_id: "wf-1".into(),
        step_id: "a".into(),
    });
    let value = serde_json::to_value(&started).unwrap();
    assert_eq!(value["event"], "workflow_step_started");
    assert_eq!(value["data"]["run_id"], "wf-1");
    assert_eq!(value["data"]["step_id"], "a");
    assert!(value["timestamp"].as_u64().is_some());

    let completed = TelemetryEvent::workflow_step_completed(WorkflowStepCompletedData {
        run_id: "wf-1".into(),
        step_id: "a".into(),
        status: StepStatus::Completed,
        output: Some("answer".into()),
        source: Some("local".into()),
        error: None,
    });
    let value = serde_json::to_value(&completed).unwrap();
    assert_eq!(value["event"], "workflow_step_completed");
    assert_eq!(value["data"]["status"], "completed");
    assert_eq!(value["data"]["output"], "answer");
    assert_eq!(value["data"]["source"], "local");

    let blocked = TelemetryEvent::workflow_step_completed(WorkflowStepCompletedData {
        run_id: "wf-1".into(),
        step_id: "b".into(),
        status: StepStatus::Blocked,
        output: None,
        source: None,
        error: Some("blocked by failed step 'a'".into()),
    });
    let value = serde_json::to_value(&blocked).unwrap();
    assert_eq!(value["data"]["status"], "blocked");
    assert_eq!(value["data"]["error"], "blocked by failed step 'a'");
}

#[test]
fn workflow_events_broadcast_to_subscribers() {
    let bus = TelemetryBus::new(8);
    let mut receiver = bus.subscribe();
    let sent = bus.publish(TelemetryEvent::workflow_step_started(WorkflowStepStartedData {
        run_id: "wf-1".into(),
        step_id: "a".into(),
    }));
    assert_eq!(sent, 1);
    assert!(matches!(
        receiver.try_recv().unwrap(),
        TelemetryEvent::WorkflowStepStarted { .. }
    ));
}
```

- [ ] **Step 2: Run the tests to verify they fail**

Run:

```powershell
$env:RUSTUP_TOOLCHAIN = "stable-x86_64-pc-windows-gnu"
$env:CARGO_TARGET_DIR = "D:\ai-upcycler-target"
$env:PATH = "C:\msys64\mingw64\bin;$env:PATH"
cargo test -p ai-upcycler-core --test workflows workflow_step_events
```

Expected: compile failure naming `WorkflowStepStartedData` and `workflow_step_started` as unresolved.

- [ ] **Step 3: Add the payload structs and enum variants to `core/src/telemetry.rs`**

Insert after `DownloadProgressData`:

```rust
/// Payload of `workflow_step_started` (spec 2026-11-27, §10).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct WorkflowStepStartedData {
    pub run_id: String,
    pub step_id: String,
}

/// Payload of `workflow_step_completed`, including failure and blocked
/// outcomes. Clients must read `status`, not infer it from an error string.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct WorkflowStepCompletedData {
    pub run_id: String,
    pub step_id: String,
    pub status: crate::workflows::StepStatus,
    pub output: Option<String>,
    pub source: Option<String>,
    pub error: Option<String>,
}
```

Add these two variants at the end of `enum TelemetryEvent`:

```rust
    WorkflowStepStarted {
        timestamp: u64,
        data: WorkflowStepStartedData,
    },
    WorkflowStepCompleted {
        timestamp: u64,
        data: WorkflowStepCompletedData,
    },
```

Add these two constructors to `impl TelemetryEvent` before its closing `}`:

```rust
    pub fn workflow_step_started(data: WorkflowStepStartedData) -> Self {
        Self::WorkflowStepStarted {
            timestamp: now_epoch_secs(),
            data,
        }
    }

    pub fn workflow_step_completed(data: WorkflowStepCompletedData) -> Self {
        Self::WorkflowStepCompleted {
            timestamp: now_epoch_secs(),
            data,
        }
    }
```

- [ ] **Step 4: Run the tests to verify they pass**

Run:

```powershell
$env:RUSTUP_TOOLCHAIN = "stable-x86_64-pc-windows-gnu"
$env:CARGO_TARGET_DIR = "D:\ai-upcycler-target"
$env:PATH = "C:\msys64\mingw64\bin;$env:PATH"
cargo test -p ai-upcycler-core --test workflows workflow_step_events
cargo test -p ai-upcycler-core --lib telemetry::tests
```

Expected: both commands pass; existing telemetry tests still pass with the new variants.

- [ ] **Step 5: Check formatting and commit**

```powershell
$env:RUSTUP_TOOLCHAIN = "stable-x86_64-pc-windows-gnu"
$env:CARGO_TARGET_DIR = "D:\ai-upcycler-target"
$env:PATH = "C:\msys64\mingw64\bin;$env:PATH"
cargo fmt --all -- --check
git add core/src/telemetry.rs core/tests/workflows.rs
git commit -m "test: publish tagged workflow progress events"
```

**Checkpoint:** `serde_json::to_value` produces exactly `{"event":"workflow_step_started"|"workflow_step_completed","timestamp":<secs>,"data":{...}}`; no new WebSocket route is added.

---

### Task 6: Workflow REST endpoints and router wiring

**Files:**
- Create: `core/src/api/workflows.rs`
- Modify: `core/src/api/mod.rs`
- Modify: `core/tests/api.rs`

**Interfaces:**
- Consumes: all of Tasks 1–5; `AppState::{store,backend,providers,telemetry}`; `ApiError::{BadRequest,NotFound,Store}`.
- Produces:
  - `POST /api/workflows` → `202` `{run_id,status}`
  - `GET /api/workflows` → `200` `{runs:[WorkflowRunRow]}`
  - `GET /api/workflows/:id` → `200` `{run,steps}` or `404`
  - `run_workflow(state, run_id, model, steps)` background task and `StoreSink`.

- [ ] **Step 1: Write failing in-process router tests**

Append to `core/tests/api.rs`:

```rust
/// Polls the detail endpoint until the background executor finishes.
async fn wait_for_workflow(app: Router, run_id: &str) -> serde_json::Value {
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let (_, body) = send(app.clone(), "GET", &format!("/api/workflows/{run_id}"), None).await;
            if body["run"]["status"] != "running" {
                return body;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("workflow finished within 5 seconds")
}

#[tokio::test]
async fn creates_and_reads_a_two_step_workflow() {
    let state = state();
    let app = router(state.clone());
    let (status, created) = send(
        app.clone(),
        "POST",
        "/api/workflows",
        Some(json!({
            "model": "llama-8b",
            "steps": [
                {"id":"a","prompt":"first","depends_on":[],"provider":"local"},
                {"id":"b","prompt":"second","depends_on":["a"],"provider":"local"}
            ]
        })),
    )
    .await;

    assert_eq!(status, StatusCode::ACCEPTED);
    let run_id = created["run_id"].as_str().unwrap().to_string();
    assert_eq!(created["status"], "running");

    let detail = wait_for_workflow(app.clone(), &run_id).await;
    assert_eq!(detail["run"]["status"], "completed");
    assert_eq!(detail["steps"].as_array().unwrap().len(), 2);
    assert_eq!(detail["steps"][0]["status"], "completed");
    assert_eq!(detail["steps"][1]["depends_on"][0], "a");
    assert!(detail["steps"][0]["output"].as_str().unwrap().contains("[stub:llama-8b]"));

    let (status, list) = send(app, "GET", "/api/workflows", None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(list["runs"].as_array().unwrap().len(), 1);
}

#[tokio::test]
async fn workflow_validation_rejects_bad_graphs_without_creating_a_run() {
    let state = state();
    let app = router(state.clone());
    let (status, body) = send(
        app.clone(),
        "POST",
        "/api/workflows",
        Some(json!({
            "model": "llama-8b",
            "steps": [
                {"id":"a","prompt":"one","depends_on":["b"]},
                {"id":"b","prompt":"two","depends_on":["a"]}
            ]
        })),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert!(body["error"]["message"].as_str().unwrap().contains("cycle"));

    let (status, list) = send(app, "GET", "/api/workflows", None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(list["runs"].as_array().unwrap().len(), 0);
}

#[tokio::test]
async fn workflow_detail_for_unknown_run_is_not_found() {
    let (status, body) = send(router(state()), "GET", "/api/workflows/wf-missing", None).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert!(body["error"]["message"].as_str().unwrap().contains("wf-missing"));
}

#[tokio::test]
async fn workflow_progress_is_published_over_the_existing_telemetry_bus() {
    let state = state();
    let mut receiver = state.telemetry.subscribe();
    let app = router(state);
    let (status, created) = send(
        app,
        "POST",
        "/api/workflows",
        Some(json!({
            "model": "llama-8b",
            "steps": [{"id":"a","prompt":"one","depends_on":[],"provider":"local"}]
        })),
    )
    .await;
    assert_eq!(status, StatusCode::ACCEPTED);
    let run_id = created["run_id"].as_str().unwrap().to_string();

    let mut saw_started = false;
    let saw_completed = tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            match receiver.recv().await.expect("bus open") {
                TelemetryEvent::WorkflowStepStarted { data, .. } if data.run_id == run_id => {
                    assert_eq!(data.step_id, "a");
                    saw_started = true;
                }
                TelemetryEvent::WorkflowStepCompleted { data, .. }
                    if data.run_id == run_id && data.step_id == "a" =>
                {
                    return data.status.as_str().to_string();
                }
                _ => {}
            }
        }
    })
    .await
    .expect("progress events arrive");

    assert!(saw_started, "started must precede completed");
    assert_eq!(saw_completed, "completed");
}
```

- [ ] **Step 2: Run the tests to verify they fail**

Run:

```powershell
$env:RUSTUP_TOOLCHAIN = "stable-x86_64-pc-windows-gnu"
$env:CARGO_TARGET_DIR = "D:\ai-upcycler-target"
$env:PATH = "C:\msys64\mingw64\bin;$env:PATH"
cargo test -p ai-upcycler-core --test api workflow_
```

Expected: `404` assertions fail because the routes do not exist yet.

- [ ] **Step 3: Implement `core/src/api/workflows.rs`**

```rust
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

async fn run_workflow(
    state: AppState,
    run_id: String,
    model: String,
    steps: Vec<WorkflowStep>,
) {
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
```

Do not import `StepStatus` here; `status.as_str()` resolves on the value carried by `WorkflowEvent` without a separate import. Rust clippy with `-D warnings` is part of final verification.

- [ ] **Step 4: Register routes in `core/src/api/mod.rs`**

Add `pub mod workflows;` after `pub mod ws;`.

Add these routes to `router()` before `.route("/api/telemetry", get(ws::handler))`:

```rust
        .route(
            "/api/workflows",
            get(workflows::list_workflows).post(workflows::create_workflow),
        )
        .route("/api/workflows/:id", get(workflows::get_workflow))
```

No CORS change is needed: `cors_layer()` already allows `GET` and `POST` from the shell and localhost.

- [ ] **Step 5: Run the tests to verify they pass**

Run:

```powershell
$env:RUSTUP_TOOLCHAIN = "stable-x86_64-pc-windows-gnu"
$env:CARGO_TARGET_DIR = "D:\ai-upcycler-target"
$env:PATH = "C:\msys64\mingw64\bin;$env:PATH"
cargo test -p ai-upcycler-core --test api
cargo test -p ai-upcycler-core --test workflows
```

Expected: all API and workflow tests pass, including the telemetry ordering assertion.

- [ ] **Step 6: Check formatting, lint, and commit**

```powershell
$env:RUSTUP_TOOLCHAIN = "stable-x86_64-pc-windows-gnu"
$env:CARGO_TARGET_DIR = "D:\ai-upcycler-target"
$env:PATH = "C:\msys64\mingw64\bin;$env:PATH"
cargo fmt --all -- --check
cargo clippy -p ai-upcycler-core --all-targets -- -D warnings
git add core/src/api/workflows.rs core/src/api/mod.rs core/tests/api.rs
git commit -m "test: add workflow REST endpoints and live progress"
```

**Checkpoint:** Backend is complete: a client can POST a DAG, poll two GETs, and observe `workflow_step_started`/`workflow_step_completed` on `/api/telemetry` without any UI code.

---

### Task 7: Desktop typed API client and workflow store

**Files:**
- Modify: `src/lib/types.ts`
- Modify: `src/lib/api.ts`
- Create: `src/store/workflows.ts`

**Interfaces:**
- Consumes: REST contract from Task 6; existing `request<T>` in `src/lib/api.ts`; zustand `create` pattern from `src/store/cluster.ts`.
- Produces: `createWorkflow`, `listWorkflows`, `getWorkflow`; `useWorkflowStore` with `refreshRuns`, `selectRun`, `startWorkflow`, `applyWorkflowTelemetry`.

- [ ] **Step 1: Add the failing type-contract check**

Create `src/store/workflows.ts` with an intentionally incomplete stub:

```ts
export const useWorkflowStore = {};
```

Do not add the types yet; run this check while the store is still a stub:

```powershell
$env:npm_config_cache = "D:\ai-upcycler-npm-cache"
npx tsc --noEmit -p tsconfig.json
```

Expected: TypeScript errors naming the missing `WorkflowRequest`/`CreateWorkflowResponse` types and the missing `startWorkflow`/`refreshRuns` store members. To keep this task independently verifiable, add this compile-time assertion file temporarily at `src/store/workflows.contract.ts`:

```ts
import { useWorkflowStore } from './workflows';

const refreshRuns: () => Promise<void> = useWorkflowStore.refreshRuns;
const startWorkflow: (req: import('../lib/types').WorkflowRequest) => Promise<import('../lib/types').CreateWorkflowResponse> =
  useWorkflowStore.startWorkflow;

void refreshRuns;
void startWorkflow;
```

- [ ] **Step 2: Run the contract check to verify it fails**

Run:

```powershell
$env:npm_config_cache = "D:\ai-upcycler-npm-cache"
npx tsc --noEmit -p tsconfig.json
```

Expected: TypeScript errors naming `WorkflowRequest`, `CreateWorkflowResponse`, and the missing store members.

- [ ] **Step 3: Append the shared types to `src/lib/types.ts`**

```ts
export type WorkflowRunStatus = 'running' | 'completed' | 'failed';
export type WorkflowStepStatus = 'pending' | 'running' | 'completed' | 'failed' | 'blocked';
export type WorkflowProvider = 'auto' | 'local' | 'openai' | 'anthropic' | 'gemini' | 'kimi';

export interface WorkflowStepDraft {
  id: string;
  prompt: string;
  depends_on: string[];
  provider: WorkflowProvider;
}

export interface WorkflowRequest {
  model: string;
  steps: WorkflowStepDraft[];
}

export interface WorkflowRunRow {
  run_id: string;
  model: string;
  status: WorkflowRunStatus;
  created_at: string;
  finished_at: string | null;
}

export interface WorkflowStepRow {
  run_id: string;
  step_id: string;
  prompt: string;
  depends_on: string[];
  provider: string | null;
  status: WorkflowStepStatus;
  output: string | null;
  source: string | null;
  error: string | null;
  position: number;
}

export interface WorkflowRunDetail {
  run: WorkflowRunRow;
  steps: WorkflowStepRow[];
}

export interface CreateWorkflowResponse {
  run_id: string;
  status: WorkflowRunStatus;
}

export interface WorkflowListResponse {
  runs: WorkflowRunRow[];
}
```

Replace the final `| { event: 'model_download_progress'; timestamp: number; data: DownloadProgressData };` line of the `TelemetryEvent` union with the same member followed by these two new members. Keep the semicolon only on the last line:

```ts
  | { event: 'model_download_progress'; timestamp: number; data: DownloadProgressData }
  | { event: 'workflow_step_started'; timestamp: number; data: { run_id: string; step_id: string } }
  | { event: 'workflow_step_completed'; timestamp: number; data: {
      run_id: string;
      step_id: string;
      status: WorkflowStepStatus;
      output: string | null;
      source: string | null;
      error: string | null;
    } };
```

- [ ] **Step 4: Add the API client calls to `src/lib/api.ts`**

Extend the import type list with `CreateWorkflowResponse`, `WorkflowListResponse`, `WorkflowRequest`, `WorkflowRunDetail`, then append:

```ts
// ---- task workflows (spec 2026-11-27, §10) --------------------------------

export function createWorkflow(req: WorkflowRequest): Promise<CreateWorkflowResponse> {
  return request<CreateWorkflowResponse>('/api/workflows', {
    method: 'POST',
    body: JSON.stringify(req),
  });
}

export function listWorkflows(): Promise<WorkflowListResponse> {
  return request<WorkflowListResponse>('/api/workflows');
}

export function getWorkflow(runId: string): Promise<WorkflowRunDetail> {
  return request<WorkflowRunDetail>(`/api/workflows/${encodeURIComponent(runId)}`);
}
```

- [ ] **Step 5: Implement `src/store/workflows.ts`**

```ts
import { create } from 'zustand';
import { createWorkflow, getWorkflow, listWorkflows } from '../lib/api';
import type {
  CreateWorkflowResponse,
  TelemetryEvent,
  WorkflowRequest,
  WorkflowRunDetail,
  WorkflowRunRow,
  WorkflowStepRow,
  WorkflowStepStatus,
} from '../lib/types';

function deriveRunStatus(steps: WorkflowStepRow[]): WorkflowRunRow['status'] {
  if (steps.some((step) => step.status === 'failed' || step.status === 'blocked')) return 'failed';
  if (steps.length > 0 && steps.every((step) => step.status === 'completed')) return 'completed';
  return 'running';
}

interface WorkflowState {
  runs: WorkflowRunRow[];
  selected: WorkflowRunDetail | null;
  error: string | null;
  refreshRuns: () => Promise<void>;
  selectRun: (runId: string) => Promise<void>;
  startWorkflow: (req: WorkflowRequest) => Promise<CreateWorkflowResponse>;
  applyWorkflowTelemetry: (event: TelemetryEvent) => void;
}

export const useWorkflowStore = create<WorkflowState>((set, get) => ({
  runs: [],
  selected: null,
  error: null,

  refreshRuns: async () => {
    try {
      const { runs } = await listWorkflows();
      set({ runs, error: null });
    } catch (error) {
      set({ error: error instanceof Error ? error.message : String(error) });
    }
  },

  selectRun: async (runId) => {
    try {
      const selected = await getWorkflow(runId);
      set({ selected, error: null });
    } catch (error) {
      set({ error: error instanceof Error ? error.message : String(error) });
    }
  },

  startWorkflow: async (req) => {
    try {
      const created = await createWorkflow(req);
      set({ error: null });
      await get().refreshRuns();
      await get().selectRun(created.run_id);
      return created;
    } catch (error) {
      const message = error instanceof Error ? error.message : String(error);
      set({ error: message });
      throw new Error(message);
    }
  },

  applyWorkflowTelemetry: (event) => {
    if (
      event.event !== 'workflow_step_started' &&
      event.event !== 'workflow_step_completed'
    ) {
      return;
    }
    const { run_id: runId, step_id: stepId } = event.data;
    const selected = get().selected;
    if (!selected || selected.run.run_id !== runId) return;

    const status: WorkflowStepStatus =
      event.event === 'workflow_step_started' ? 'running' : event.data.status;
    const steps = selected.steps.map((step): WorkflowStepRow => {
      if (step.step_id !== stepId) return step;
      if (event.event === 'workflow_step_started') return { ...step, status };
      return {
        ...step,
        status,
        output: event.data.output,
        source: event.data.source,
        error: event.data.error,
      };
    });
    set({
      selected: {
        run: { ...selected.run, status: deriveRunStatus(steps) },
        steps,
      },
    });
  },
}));
```

- [ ] **Step 6: Delete the temporary contract file and run the type check**

Remove `src/store/workflows.contract.ts`, then run:

```powershell
$env:npm_config_cache = "D:\ai-upcycler-npm-cache"
npx tsc --noEmit -p tsconfig.json
```

Expected: no errors under strict mode with `noUnusedLocals` and `noImplicitReturns`.

- [ ] **Step 7: Commit**

```bash
git add src/lib/types.ts src/lib/api.ts src/store/workflows.ts
git commit -m "test: type desktop workflow REST and progress store"
```

**Checkpoint:** The desktop has a typed client and a store whose telemetry reducer updates only the selected run; strict `tsc` is green and no browser check is needed yet.

---

### Task 8: Desktop editable task timeline and tab wiring

**Files:**
- Create: `src/components/WorkflowView.tsx`
- Modify: `src/components/AppShell.tsx`
- Modify: `src/App.tsx`

**Interfaces:**
- Consumes: `useWorkflowStore`, `useClusterStore` models, `connectTelemetry`, and all Task 7 types.
- Produces: `WorkflowView` component and a `workflows` value in `AppTab`; no new REST/WS contract.

- [ ] **Step 1: Add the failing UI contract check**

Create `src/components/WorkflowView.tsx` with `export const WorkflowView = () => null;`. Create a temporary `src/components/workflow-view.contract.ts`:

```ts
import type React from 'react';
import { WorkflowView } from './WorkflowView';
import type { AppTab } from './AppShell';

const tab: AppTab = 'workflows';
const view: React.FC = WorkflowView;
void tab;
void view;
```

Run:

```powershell
$env:npm_config_cache = "D:\ai-upcycler-npm-cache"
npx tsc --noEmit -p tsconfig.json
```

Expected: `'workflows'` is not assignable to `AppTab`.

- [ ] **Step 2: Implement `WorkflowView.tsx`**

Replace the stub with:

```tsx
import { useRef, useState } from 'react';
import {
  AlertTriangle,
  Ban,
  Check,
  Circle,
  Loader2,
  Plus,
  Trash2,
} from 'lucide-react';
import { useClusterStore } from '../store/cluster';
import { useWorkflowStore } from '../store/workflows';
import type {
  WorkflowProvider,
  WorkflowRunDetail,
  WorkflowStepDraft,
  WorkflowStepStatus,
} from '../lib/types';

const PROVIDERS: WorkflowProvider[] = ['auto', 'local', 'openai', 'anthropic', 'gemini', 'kimi'];

const STATUS_LABEL: Record<WorkflowStepStatus, string> = {
  pending: 'Pending',
  running: 'Running',
  completed: 'Completed',
  failed: 'Failed',
  blocked: 'Blocked',
};

function StatusIcon({ status }: { status: WorkflowStepStatus }) {
  const props = { className: 'wf-status-icon', 'aria-hidden': true } as const;
  switch (status) {
    case 'completed':
      return <Check {...props} />;
    case 'running':
      return <Loader2 {...props} />;
    case 'failed':
      return <AlertTriangle {...props} />;
    case 'blocked':
      return <Ban {...props} />;
    case 'pending':
      return <Circle {...props} />;
  }
}

export function WorkflowView() {
  const models = useClusterStore((state) => state.models);
  const runs = useWorkflowStore((state) => state.runs);
  const selected = useWorkflowStore((state) => state.selected);
  const error = useWorkflowStore((state) => state.error);
  const refreshRuns = useWorkflowStore((state) => state.refreshRuns);
  const selectRun = useWorkflowStore((state) => state.selectRun);
  const startWorkflow = useWorkflowStore((state) => state.startWorkflow);

  const [model, setModel] = useState('');
  const [steps, setSteps] = useState<WorkflowStepDraft[]>(() => [
    { id: 's1', prompt: '', depends_on: [], provider: 'auto' },
  ]);
  const nextId = useRef(2);

  const updateStep = (id: string, patch: Partial<WorkflowStepDraft>) => {
    setSteps((current) =>
      current.map((step) => (step.id === id ? { ...step, ...patch } : step)),
    );
  };

  const addStep = () => {
    const id = `s${nextId.current++}`;
    setSteps((current) => [
      ...current,
      { id, prompt: '', depends_on: [], provider: 'auto' },
    ]);
  };

  const removeStep = (id: string) => {
    setSteps((current) =>
      current
        .filter((step) => step.id !== id)
        .map((step) => ({
          ...step,
          depends_on: step.depends_on.filter((dep) => dep !== id),
        })),
    );
  };

  const submit = async () => {
    await startWorkflow({ model, steps });
  };

  return (
    <section className="workflow-view" aria-labelledby="workflow-heading">
      <header className="wf-header">
        <h2 id="workflow-heading">Task workflow</h2>
        <button type="button" className="wf-button" onClick={() => void refreshRuns()}>
          Refresh runs
        </button>
      </header>

      {error && <p className="wf-error" role="alert">{error}</p>}

      <form
        className="wf-editor"
        onSubmit={(event) => {
          event.preventDefault();
          void submit();
        }}
      >
        <label className="wf-field">
          <span>Model</span>
          <select
            value={model}
            onChange={(event) => setModel(event.target.value)}
            disabled={models.length === 0}
            required
          >
            <option value="">Select a model</option>
            {models.map((entry) => (
              <option key={entry.id} value={entry.id}>{entry.id}</option>
            ))}
          </select>
        </label>
        {models.length === 0 && <p className="wf-hint">Load a model on the Models tab first.</p>}

        <ol className="wf-step-list">
          {steps.map((step) => (
            <li key={step.id} className="wf-step-editor">
              <div className="wf-step-row">
                <span className="wf-step-id">{step.id}</span>
                <label className="wf-field wf-grow">
                  <span>Prompt</span>
                  <input
                    type="text"
                    value={step.prompt}
                    onChange={(event) => updateStep(step.id, { prompt: event.target.value })}
                    required
                  />
                </label>
                <label className="wf-field">
                  <span>Provider</span>
                  <select
                    value={step.provider}
                    onChange={(event) =>
                      updateStep(step.id, { provider: event.target.value as WorkflowProvider })
                    }
                  >
                    {PROVIDERS.map((provider) => (
                      <option key={provider} value={provider}>{provider}</option>
                    ))}
                  </select>
                </label>
                <label className="wf-field">
                  <span>Depends on</span>
                  <select
                    multiple
                    value={step.depends_on}
                    onChange={(event) =>
                      updateStep(step.id, {
                        depends_on: Array.from(event.target.selectedOptions, (option) => option.value),
                      })
                    }
                  >
                    {steps
                      .filter((candidate) => candidate.id !== step.id)
                      .map((candidate) => (
                        <option key={candidate.id} value={candidate.id}>{candidate.id}</option>
                      ))}
                  </select>
                </label>
                <button
                  type="button"
                  className="wf-button wf-danger"
                  aria-label={`Remove step ${step.id}`}
                  onClick={() => removeStep(step.id)}
                >
                  <Trash2 className="wf-status-icon" aria-hidden="true" />
                  Remove
                </button>
              </div>
            </li>
          ))}
        </ol>

        <div className="wf-actions">
          <button type="button" className="wf-button" onClick={addStep}>
            <Plus className="wf-status-icon" aria-hidden="true" />
            Add step
          </button>
          <button type="submit" className="wf-button wf-primary" disabled={!model}>
            Start run
          </button>
        </div>
      </form>

      <section className="wf-history" aria-labelledby="workflow-runs-heading">
        <h3 id="workflow-runs-heading">Runs</h3>
        <ul className="wf-run-list">
          {runs.map((run) => (
            <li key={run.run_id}>
              <button
                type="button"
                className="wf-button wf-run"
                aria-current={selected?.run.run_id === run.run_id}
                onClick={() => void selectRun(run.run_id)}
              >
                {run.run_id} · {run.model} · {run.status}
              </button>
            </li>
          ))}
        </ul>
      </section>

      {selected && <WorkflowTimeline detail={selected} />}
    </section>
  );
}

function WorkflowTimeline({ detail }: { detail: WorkflowRunDetail }) {
  return (
    <section className="wf-timeline" aria-labelledby="workflow-timeline-heading">
      <h3 id="workflow-timeline-heading">Timeline · {detail.run.run_id}</h3>
      <ol className="wf-timeline-list">
        {detail.steps.map((step) => (
          <li
            key={step.step_id}
            className="wf-timeline-step"
            aria-busy={step.status === 'running'}
            aria-current={step.status === 'running' ? 'step' : undefined}
          >
            <span className={`wf-status wf-status-${step.status}`}>
              <StatusIcon status={step.status} />
              {STATUS_LABEL[step.status]}
            </span>
            <strong>{step.step_id}</strong>
            <span className="wf-muted">{step.prompt}</span>
            {step.source && <span className="wf-muted">source: {step.source}</span>}
            {step.output && <p className="wf-output">{step.output}</p>}
            {step.error && <p className="wf-error">{step.error}</p>}
          </li>
        ))}
      </ol>
    </section>
  );
}

export default WorkflowView;
```

- [ ] **Step 3: Add the tab in `src/components/AppShell.tsx`**

Add `Workflow` to the lucide icon import list, then change:

```ts
export type AppTab = 'dashboard' | 'models' | 'chat';
```

to:

```ts
export type AppTab = 'dashboard' | 'models' | 'chat' | 'workflows';
```

Add this entry to `NAV` after the Chat item:

```tsx
  { id: 'workflows', label: 'Tasks', icon: <Workflow className="h-4 w-4" /> },
```

- [ ] **Step 4: Wire the view and second telemetry subscription in `src/App.tsx`**

Replace the whole file with:

```tsx
import { useEffect, useState } from 'react';
import { AppShell, type AppTab } from './components/AppShell';
import { ChatView } from './components/ChatView';
import { DashboardView } from './components/DashboardView';
import { ModelsView } from './components/ModelsView';
import { WorkflowView } from './components/WorkflowView';
import { connectTelemetry } from './lib/telemetry';
import { useClusterStore } from './store/cluster';
import { useWorkflowStore } from './store/workflows';

export const App: React.FC = () => {
  const [activeTab, setActiveTab] = useState<AppTab>('dashboard');
  const refreshTopology = useClusterStore((s) => s.refreshTopology);
  const refreshModels = useClusterStore((s) => s.refreshModels);
  const applyTelemetry = useClusterStore((s) => s.applyTelemetry);
  const applyWorkflowTelemetry = useWorkflowStore((s) => s.applyWorkflowTelemetry);
  const refreshRuns = useWorkflowStore((s) => s.refreshRuns);

  useEffect(() => {
    void refreshTopology();
    void refreshModels();
    void refreshRuns();
    // Topology reads live host/phone memory at request time; the telemetry
    // socket carries health/temperature and workflow step transitions.
    const topologyTimer = window.setInterval(() => {
      void refreshTopology();
    }, 5000);
    const disconnectCluster = connectTelemetry(applyTelemetry);
    const disconnectWorkflows = connectTelemetry(applyWorkflowTelemetry);
    return () => {
      window.clearInterval(topologyTimer);
      disconnectCluster();
      disconnectWorkflows();
    };
  }, [refreshTopology, refreshModels, refreshRuns, applyTelemetry, applyWorkflowTelemetry]);

  return (
    <AppShell activeTab={activeTab} onTabChange={setActiveTab}>
      {activeTab === 'dashboard' && <DashboardView />}
      {activeTab === 'models' && <ModelsView />}
      {activeTab === 'chat' && <ChatView />}
      {activeTab === 'workflows' && <WorkflowView />}
    </AppShell>
  );
};

export default App;
```

- [ ] **Step 5: Delete the temporary contract file and verify types**

Remove `src/components/workflow-view.contract.ts`, then run:

```powershell
$env:npm_config_cache = "D:\ai-upcycler-npm-cache"
npx tsc --noEmit -p tsconfig.json
```

Expected: no errors.

- [ ] **Step 6: Verify behavior with Playwright MCP**

Start the app with the API on `127.0.0.1:8080` and use Playwright MCP in this order:

1. `browser_navigate` to `http://127.0.0.1:1420` (or the running Tauri dev URL).
2. `browser_click` the `Tasks` tab; `browser_snapshot` must show the `Task workflow` heading, one `s1` editor row, and a `Start run` button.
3. `browser_type` into the `s1` prompt field: `Write the release notes`.
4. `browser_select_option` on Model: the model id reported by `GET /v1/models`.
5. `browser_click` `Add step`; after typing into `s2` and `s3`, remove `s1`. The snapshot must still show `s2` with its original prompt: stable `step.id` keys keep React state attached to the business id, not the array index.
6. `browser_click` `Start run`; snapshot must show a new run id and a timeline whose first status text is `Running`, then `Completed`.
7. `browser_take_screenshot` at each of 375, 768, 1024, and 1440 px viewport widths. No horizontal overflow and every control has a visible focus ring when tabbed to.

Expected: all seven checks pass; no console errors in the Playwright console snapshot.

- [ ] **Step 7: Commit**

```bash
git add src/components/WorkflowView.tsx src/components/AppShell.tsx src/App.tsx
git commit -m "feat: add editable desktop task timeline"
```

**Checkpoint:** A user can build a two-step DAG, submit it, watch stable-key status rows update, and the layout holds at all four required widths.

---

### Task 9: Desktop workflow tokens, accessibility, and reduced motion

**Files:**
- Modify: `src/index.css`
- Modify: `src/components/WorkflowView.tsx`

**Interfaces:**
- Consumes: `design-system/ai-upcycler/MASTER.md` from B (read before editing), spec §12 tokens, existing global focus/reduced-motion rules in `src/index.css:200-213`.
- Produces: scoped `.workflow-view` / `.wf-*` styles only; no changes to global app tokens.

- [ ] **Step 1: Confirm the design-system dependency**

Run:

```powershell
Get-Content design-system\ai-upcycler\MASTER.md -TotalCount 120
```

Expected: the file exists and lists the §12 tokens. If B has not landed it, stop this task and report the dependency; do not substitute a new palette.

- [ ] **Step 2: Append scoped styles to `src/index.css`**

```css
/* Task workflow surface — spec 2026-11-27 §12. Scoped so existing shells keep
   their tokens; values are copied verbatim from the design system. */
.workflow-view {
  --wf-bg: #0F172A;
  --wf-card: #1B2336;
  --wf-border: #475569;
  --wf-muted: #94A3B8;
  --wf-accent: #22C55E;
  --wf-danger: #EF4444;
  --wf-danger-text: #FCA5A5;
  background: var(--wf-bg);
  color: #F8FAFC;
  border-radius: 0.75rem;
  padding: 1rem;
}

.wf-header,
.wf-actions,
.wf-step-row {
  display: flex;
  flex-wrap: wrap;
  gap: 0.5rem;
  align-items: flex-end;
}

.wf-header {
  justify-content: space-between;
  align-items: center;
}

.wf-grow { flex: 1 1 16rem; }

.wf-field {
  display: flex;
  flex-direction: column;
  gap: 0.25rem;
  font-size: 0.875rem;
  color: var(--wf-muted);
}

.wf-field input,
.wf-field select {
  background: var(--wf-card);
  color: #F8FAFC;
  border: 1px solid var(--wf-border);
  border-radius: 0.375rem;
  min-height: 2.75rem;
  padding: 0.4rem 0.6rem;
  font: inherit;
}

.wf-button {
  display: inline-flex;
  align-items: center;
  gap: 0.4rem;
  min-height: 2.75rem;
  padding: 0.4rem 0.75rem;
  border: 1px solid var(--wf-border);
  border-radius: 0.375rem;
  background: var(--wf-card);
  color: #F8FAFC;
  cursor: pointer;
  transition: background-color 200ms ease, border-color 200ms ease;
}

.wf-button:hover { border-color: var(--wf-accent); }
.wf-button:active { background: #24304A; }
.wf-button:disabled { opacity: 0.5; cursor: not-allowed; }
.wf-primary { border-color: var(--wf-accent); }
.wf-danger { color: var(--wf-danger-text); border-color: var(--wf-danger); }

.wf-step-list,
.wf-run-list,
.wf-timeline-list {
  list-style: none;
  margin: 0.75rem 0;
  padding: 0;
  display: grid;
  gap: 0.5rem;
}

.wf-step-editor,
.wf-timeline-step {
  background: var(--wf-card);
  border: 1px solid var(--wf-border);
  border-radius: 0.5rem;
  padding: 0.75rem;
}

.wf-step-id { font-family: var(--font-mono); color: var(--wf-muted); }

.wf-status {
  display: inline-flex;
  align-items: center;
  gap: 0.35rem;
  font-weight: 600;
}

.wf-status-icon { width: 1rem; height: 1rem; }
.wf-status-running { color: var(--wf-accent); }
.wf-status-failed,
.wf-status-blocked { color: var(--wf-danger-text); }
.wf-muted { color: var(--wf-muted); }
.wf-hint { color: var(--wf-muted); font-size: 0.875rem; }
.wf-output { white-space: pre-wrap; overflow-wrap: anywhere; }
.wf-error { color: var(--wf-danger-text); }

.wf-run[aria-current='true'] { border-color: var(--wf-accent); }

@media (prefers-reduced-motion: reduce) {
  .wf-button { transition: none; }
}
```

- [ ] **Step 3: Verify contrast and a11y with Playwright MCP**

Run:

```powershell
$env:npm_config_cache = "D:\ai-upcycler-npm-cache"
npx tsc --noEmit -p tsconfig.json
```

Then in Playwright MCP, on the Tasks tab:

1. `browser_snapshot`: every step has a visible text status, not just a colored dot.
2. `browser_press_key` `Tab` through the editor; `browser_snapshot` must show a focus outline on each control, including `Add step`, `Remove`, `Start run`, and each run button.
3. Toggle the browser's `prefers-reduced-motion: reduce` emulation and take `browser_take_screenshot`: no shimmer or transition movement is visible.
4. At 375 px, confirm the step editor wraps instead of overflowing; at 1440 px, confirm the timeline keeps the same token colors as the card.

Expected: all four checks pass; no new font family appears in the computed styles.

- [ ] **Step 4: Commit**

```bash
git add src/index.css src/components/WorkflowView.tsx
git commit -m "style: apply workflow design tokens and a11y states"
```

**Checkpoint:** The workflow surface uses the six §12 tokens verbatim for their named roles, keeps status legible without color, and passes focus/reduced-motion/responsive checks.

---

### Task 10: Phone read-only workflow progress

**Files:**
- Create: `mobile-worker/android/app/src/main/java/org/aiupcycler/workflow/WorkflowApi.kt`
- Create: `mobile-worker/android/app/src/main/java/org/aiupcycler/workflow/LanWorkflowApi.kt`
- Create: `mobile-worker/android/app/src/main/java/org/aiupcycler/workflow/WorkflowProgressScreen.kt`
- Create: `mobile-worker/android/app/src/test/java/org/aiupcycler/workflow/WorkflowStatusTest.kt`

**Interfaces:**
- Consumes: B's Compose host and LAN settings (base URL + bearer token provider); REST contract from Task 6; spec §12 Compose rules.
- Produces: `WorkflowApi`, `LanWorkflowApi`, `WorkflowProgressScreen(api: WorkflowApi, modifier: Modifier = Modifier)`, `displayStatus`.

- [ ] **Step 1: Write the failing JVM test**

Create `mobile-worker/android/app/src/test/java/org/aiupcycler/workflow/WorkflowStatusTest.kt`:

```kotlin
package org.aiupcycler.workflow

import org.junit.Assert.assertEquals
import org.junit.Test

class WorkflowStatusTest {
    @Test
    fun maps_known_workflow_statuses_to_readable_text() {
        assertEquals("Running", displayStatus("running"))
        assertEquals("Completed", displayStatus("completed"))
        assertEquals("Failed", displayStatus("failed"))
        assertEquals("Blocked", displayStatus("blocked"))
        assertEquals("Pending", displayStatus("pending"))
    }

    @Test
    fun maps_an_unknown_status_without_crashing() {
        assertEquals("Queued later", displayStatus("queued_later"))
    }
}
```

- [ ] **Step 2: Run the test to verify it fails**

Run:

```powershell
$env:RUSTUP_TOOLCHAIN = "stable-x86_64-pc-windows-gnu"
$env:CARGO_TARGET_DIR = "D:\ai-upcycler-target"
$env:PATH = "C:\msys64\mingw64\bin;$env:PATH"
Push-Location mobile-worker\android
.\gradlew.bat :app:testDebugUnitTest --tests "org.aiupcycler.workflow.WorkflowStatusTest"
Pop-Location
```

Expected: compile failure naming `displayStatus` as unresolved.

- [ ] **Step 3: Create the phone contract file**

Create `mobile-worker/android/app/src/main/java/org/aiupcycler/workflow/WorkflowApi.kt`:

```kotlin
package org.aiupcycler.workflow

/** Read-only surface. This phase has no phone-side workflow editing. */
interface WorkflowApi {
    suspend fun listRuns(): List<WorkflowRunSummary>
    suspend fun getRun(runId: String): WorkflowRunDetail
}

data class WorkflowRunSummary(
    val runId: String,
    val model: String,
    val status: String,
    val createdAt: String?,
)

data class WorkflowStepState(
    val stepId: String,
    val status: String,
    val output: String?,
    val source: String?,
    val error: String?,
)

data class WorkflowRunDetail(
    val runId: String,
    val model: String,
    val status: String,
    val steps: List<WorkflowStepState>,
)

/** Text is the primary status indicator; colour is only a secondary cue. */
fun displayStatus(status: String): String = when (status) {
    "running" -> "Running"
    "completed" -> "Completed"
    "failed" -> "Failed"
    "blocked" -> "Blocked"
    "pending" -> "Pending"
    else -> status.replace('_', ' ').replaceFirstChar { it.uppercase() }
}
```

- [ ] **Step 4: Create the LAN adapter**

Create `mobile-worker/android/app/src/main/java/org/aiupcycler/workflow/LanWorkflowApi.kt`:

```kotlin
package org.aiupcycler.workflow

import java.net.HttpURLConnection
import java.net.URLEncoder
import java.net.URL
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.withContext
import org.json.JSONObject

/**
 * Uses B's opt-in LAN listener and bearer token. No token or base URL is
 * stored by E; both are supplied by B's settings surface at call time.
 */
class LanWorkflowApi(
    private val baseUrl: String,
    private val bearerToken: () -> String?,
) : WorkflowApi {

    override suspend fun listRuns(): List<WorkflowRunSummary> = withContext(Dispatchers.IO) {
        val array = get("/api/workflows").getJSONArray("runs")
        (0 until array.length()).map { index ->
            val row = array.getJSONObject(index)
            WorkflowRunSummary(
                runId = row.getString("run_id"),
                model = row.getString("model"),
                status = row.getString("status"),
                createdAt = row.optString("created_at", null),
            )
        }
    }

    override suspend fun getRun(runId: String): WorkflowRunDetail = withContext(Dispatchers.IO) {
        val body = get("/api/workflows/${encode(runId)}")
        val run = body.getJSONObject("run")
        val array = body.getJSONArray("steps")
        val steps = (0 until array.length()).map { index ->
            val row = array.getJSONObject(index)
            WorkflowStepState(
                stepId = row.getString("step_id"),
                status = row.getString("status"),
                output = row.optString("output", null),
                source = row.optString("source", null),
                error = row.optString("error", null),
            )
        }
        WorkflowRunDetail(
            runId = run.getString("run_id"),
            model = run.getString("model"),
            status = run.getString("status"),
            steps = steps,
        )
    }

    private fun get(path: String): JSONObject {
        val connection = URL(baseUrl.trimEnd('/') + path).openConnection() as HttpURLConnection
        connection.requestMethod = "GET"
        connection.setRequestProperty("Accept", "application/json")
        bearerToken()?.let { connection.setRequestProperty("Authorization", "Bearer $it") }
        connection.connectTimeout = 5_000
        connection.readTimeout = 5_000
        try {
            if (connection.responseCode !in 200..299) {
                throw IllegalStateException("workflow API returned ${connection.responseCode}")
            }
            return JSONObject(connection.inputStream.bufferedReader().use { it.readText() })
        } finally {
            connection.disconnect()
        }
    }
}

private fun encode(value: String): String = URLEncoder.encode(value, "UTF-8")
```

- [ ] **Step 5: Create the read-only Compose screen**

Create `mobile-worker/android/app/src/main/java/org/aiupcycler/workflow/WorkflowProgressScreen.kt`:

```kotlin
package org.aiupcycler.workflow

import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.material3.Button
import androidx.compose.material3.Card
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableIntStateOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.unit.dp

/**
 * B's Compose host owns navigation; E supplies this tab. It only reads runs:
 * no add/edit/delete controls exist here in this phase.
 */
@Composable
fun WorkflowProgressScreen(api: WorkflowApi, modifier: Modifier = Modifier) {
    var runs by remember { mutableStateOf<List<WorkflowRunSummary>>(emptyList()) }
    var detail by remember { mutableStateOf<WorkflowRunDetail?>(null) }
    var error by remember { mutableStateOf<String?>(null) }
    var refreshKey by remember { mutableIntStateOf(0) }

    LaunchedEffect(api, refreshKey) {
        try {
            val loaded = api.listRuns()
            runs = loaded
            detail = loaded.firstOrNull()?.let { api.getRun(it.runId) }
            error = null
        } catch (failure: Exception) {
            error = failure.message ?: "Unable to load workflow progress"
        }
    }

    Column(
        modifier = modifier.padding(16.dp),
        verticalArrangement = Arrangement.spacedBy(8.dp),
    ) {
        Text("Task progress", style = MaterialTheme.typography.titleLarge)
        Button(
            onClick = { refreshKey += 1 },
            modifier = Modifier.heightIn(min = 48.dp),
        ) {
            Text("Refresh")
        }
        error?.let { Text(it, color = MaterialTheme.colorScheme.error) }

        LazyColumn(
            verticalArrangement = Arrangement.spacedBy(8.dp),
            modifier = Modifier.fillMaxWidth(),
        ) {
            items(items = runs, key = { it.runId }) { run ->
                Card(modifier = Modifier.fillMaxWidth()) {
                    Column(modifier = Modifier.padding(12.dp)) {
                        Text("${run.model} · ${displayStatus(run.status)}")
                        Text(run.runId, style = MaterialTheme.typography.bodySmall)
                    }
                }
            }
            detail?.let { selected ->
                items(items = selected.steps, key = { it.stepId }) { step ->
                    Card(modifier = Modifier.fillMaxWidth()) {
                        Row(
                            modifier = Modifier.padding(12.dp),
                            verticalAlignment = Alignment.Top,
                        ) {
                            Text(displayStatus(step.status))
                            Spacer(Modifier.width(8.dp))
                            Column {
                                Text(step.stepId, style = MaterialTheme.typography.bodySmall)
                                step.source?.let { Text("source: $it") }
                                step.output?.let { Text(it, maxLines = 2) }
                                step.error?.let { Text("Error: $it", color = MaterialTheme.colorScheme.error) }
                            }
                        }
                    }
                }
            }
        }
    }
}
```

- [ ] **Step 6: Run the tests and assemble the APK**

Run:

```powershell
$env:RUSTUP_TOOLCHAIN = "stable-x86_64-pc-windows-gnu"
$env:CARGO_TARGET_DIR = "D:\ai-upcycler-target"
$env:PATH = "C:\msys64\mingw64\bin;$env:PATH"
Push-Location mobile-worker\android
.\gradlew.bat :app:testDebugUnitTest --tests "org.aiupcycler.workflow.WorkflowStatusTest"
.\gradlew.bat :app:assembleDebug
Pop-Location
```

Expected: both JVM tests pass and `app-debug.apk` builds with B's Compose dependencies already present.

- [ ] **Step 7: Verify on the wireless device with screenshots**

First confirm B's host wires this screen, then use the LAN listener and the desktop workflow run from Task 8:

```powershell
Get-ChildItem mobile-worker\android\app\src\main\java -Recurse -Filter *.kt |
  Select-String -Pattern 'WorkflowProgressScreen'
```

Expected: one result is this task's file, and a second result is B's Compose host/tab file. If only the definition exists, stop and report Dependency B; do not create a second navigation host.

```powershell
$adb = "D:\Android\Sdk\platform-tools\adb.exe"
& $adb connect 192.168.1.9:5555
Copy-Item mobile-worker\android\app\build\outputs\apk\debug\app-debug.apk D:\ai-upcycler-target\app-debug.apk -Force
& $adb install -r D:\ai-upcycler-target\app-debug.apk
& $adb shell am start -n org.aiupcycler.worker/.MainActivity
& $adb shell screencap -p /sdcard/workflow-progress.png
& $adb pull /sdcard/workflow-progress.png D:\ai-upcycler-target\workflow-progress.png
```

Expected screenshot: the Tasks tab shows `Task progress`, a run row, and step rows with textual statuses (`Running` or `Completed`). No workflow edit control is visible. Confirm 48 dp touch targets on `Refresh` and no layout shift on press.

- [ ] **Step 8: Commit**

```bash
git add mobile-worker/android/app/src/main/java/org/aiupcycler/workflow mobile-worker/android/app/src/test/java/org/aiupcycler/workflow
git commit -m "feat: add read-only phone workflow progress"
```

**Checkpoint:** The phone can display an existing run read-only over B's token-authenticated LAN path; the worker foreground service is untouched.

---

### Task 11: Documentation and final verification

**Files:**
- Modify: `docs/api.md`
- Modify: `docs/agents.md`

**Interfaces:**
- Consumes: the exact REST and telemetry contracts implemented in Tasks 5–6.
- Produces: documented endpoint/WS surface; no interface change.

- [ ] **Step 1: Document the REST endpoints in `docs/api.md`**

Append a new section after §3.5:

````markdown
### 3.6 Task workflow runs (spec 2026-11-27, §10)

`POST /api/workflows` starts a DAG. The body requires a `model` and a non-empty
`steps` array; each step has `id`, `prompt`, `depends_on` (array of step ids),
and optional `provider` (`auto`, `local`, or a configured provider id). Unknown,
duplicate, self-, repeated, or cyclic dependencies are rejected with `400`.

```json
{
  "model": "llama-8b",
  "steps": [
    { "id": "draft", "prompt": "Draft the outline", "depends_on": [], "provider": "local" },
    { "id": "review", "prompt": "Review the outline", "depends_on": ["draft"], "provider": "local" }
  ]
}
```

Success is `202` with `{"run_id":"wf-...","status":"running"}`. Steps execute in
deterministic topological order through the same provider bridge as chat; a
failed step blocks its transitive dependents and unrelated steps still run.

`GET /api/workflows` returns `{"runs":[{"run_id","model","status","created_at","finished_at"}]}`
with statuses `running`, `completed`, `failed`.

`GET /api/workflows/{id}` returns `{"run":{...},"steps":[...]}`. Each step
carries `step_id`, `prompt`, `depends_on`, `provider`, `status`, `output`,
`source`, `error`, and `position`. Step statuses are `pending`, `running`,
`completed`, `failed`, `blocked`. An unknown id returns `404`.
````

- [ ] **Step 2: Document the WS events in `docs/api.md`**

Append to §4:

````markdown
`workflow_step_started` and `workflow_step_completed` carry workflow progress
for the desktop timeline and the phone's read-only view. Both use the tagged
format; `status` in a completed event is `completed`, `failed`, or `blocked`.

```json
{"event":"workflow_step_started","timestamp":1764278400,
 "data":{"run_id":"wf-...","step_id":"draft"}}
{"event":"workflow_step_completed","timestamp":1764278401,
 "data":{"run_id":"wf-...","step_id":"draft","status":"completed",
         "output":"...","source":"local","error":null}}
```

The socket does not replay old events: clients load initial state from
`GET /api/workflows/{id}` and apply WS events on top of it.
````

- [ ] **Step 3: Add the agent note in `docs/agents.md`**

Append a short paragraph to §8:

```markdown
Workflow runs (`POST /api/workflows`) are separate from chat sessions. Steps do
not inherit an agent's session: each run gets its own `workflow-<run_id>`
generation session id. A step's `provider` follows the same `auto`/`local`/
provider-id rules as chat requests; loopback remains unauthenticated.
```

- [ ] **Step 4: Run the full verification suite**

```powershell
$env:RUSTUP_TOOLCHAIN = "stable-x86_64-pc-windows-gnu"
$env:CARGO_TARGET_DIR = "D:\ai-upcycler-target"
$env:PATH = "C:\msys64\mingw64\bin;$env:PATH"
$env:npm_config_cache = "D:\ai-upcycler-npm-cache"

cargo fmt --all -- --check
cargo test -p ai-upcycler-core
cargo clippy -p ai-upcycler-core --all-targets -- -D warnings
cargo build --release -p ai-upcycler-desktop
npx tsc --noEmit -p tsconfig.json
Push-Location mobile-worker\android
.\gradlew.bat :app:testDebugUnitTest
.\gradlew.bat :app:assembleDebug
Pop-Location
```

Expected: every command exits 0. Record the outputs in the task log; do not claim completion if any check is skipped.

- [ ] **Step 5: Re-run the two manual UI checks**

1. Playwright MCP on the Tasks tab at 375/768/1024/1440 px with a completed and a failed run; both status sets must be legible and text-labelled.
2. Wireless ADB screenshot on `192.168.1.9:5555` of the same run's phone screen; it must be read-only and show the same statuses.

- [ ] **Step 6: Commit the docs**

```bash
git add docs/api.md docs/agents.md
git commit -m "docs: describe workflow endpoints and progress events"
```

**Checkpoint:** Spec §10 is implemented end to end; §12 tokens and a11y are verified; §13's four verification layers (unit, router, Playwright, wireless ADB) have all produced passing evidence.

## Self-review

- **Spec §10:** `core/src/workflows/` with `WorkflowStep { id, prompt, depends_on, provider }` (Task 1), model-free deterministic executor (Task 2), steps through the provider bridge (Task 3), `workflow_runs`/`workflow_steps` (Task 4), both WS events (Task 5), all three REST endpoints (Task 6), editable desktop timeline (Tasks 8–9), read-only phone progress (Task 10). Covered.
- **Spec §12:** six named tokens copied verbatim into scoped CSS (Task 9), contrast-safe text/active neutrals documented in Global Constraints, Fira fonts unchanged, stable React keys and Compose `LazyColumn` keys, visible focus, reduced motion, 48 dp phone targets, text status plus decorative icons hidden from a11y. Covered.
- **Spec §13:** unit tests with injected runner/sink and offline provider (Tasks 1–5), in-process axum router tests (Task 6), Playwright MCP (Tasks 8, 9, 11), wireless ADB screenshots (Tasks 10, 11). Covered.
- **Spec §14:** D: artifacts kept on `D:\ai-upcycler-target`; schema DDL is additive/idempotent alongside D's `settings` table; Compose screens are additive and the worker service is untouched; LAN widening stays owned by B's opt-in token model; provider drift is caught by A's mock-server tests used in Task 3. Covered.
- **Dependencies:** A is a hard gate for Tasks 1, 3, and 6; B for Tasks 8–10 (design-system file, Compose host, LAN token); D only for schema coexistence, with no settings/session reads. Stated in Global Constraints and repeated per task.
- **Placeholder scan:** no `TBD`, `TODO`, "implement later", "fill in details", "handle errors" without exact behavior, or "similar to Task N" remains. Every code step carries its full snippet; every verification step carries a command or an explicit Playwright/ADB interaction sequence.
