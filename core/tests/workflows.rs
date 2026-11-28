use ai_upcycler_core::api::backend::StubBackend;
use ai_upcycler_core::providers::ProviderRegistry;
use ai_upcycler_core::secrets::UnconfiguredSecretStore;
use ai_upcycler_core::workflows::{
    execute, validate_steps, EventSink, ExecutionResult, RouterBackend, StepFuture, StepOutput,
    StepRecord, StepRunner, StepStatus, WorkflowEvent, WorkflowStep,
};
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::sync::Mutex;

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
    assert!(validate_steps(&dup)
        .unwrap_err()
        .contains("duplicate step id 'a'"));

    let self_dep = vec![step("a", &["a"])];
    assert!(validate_steps(&self_dep)
        .unwrap_err()
        .contains("cannot depend on itself"));

    let unknown = vec![step("a", &["ghost"])];
    assert!(validate_steps(&unknown)
        .unwrap_err()
        .contains("unknown step 'ghost'"));

    let repeated = vec![step("a", &[]), step("b", &["a", "a"])];
    assert!(validate_steps(&repeated)
        .unwrap_err()
        .contains("repeats dependency 'a'"));
}

#[test]
fn validation_rejects_empty_input_empty_prompt_and_cycles() {
    assert!(validate_steps(&[])
        .unwrap_err()
        .contains("at least one step"));

    let empty_prompt = vec![WorkflowStep {
        id: "a".into(),
        prompt: "  ".into(),
        ..step("a", &[])
    }];
    assert!(validate_steps(&empty_prompt)
        .unwrap_err()
        .contains("empty prompt"));

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
    let runner = ScriptedRunner {
        calls: Mutex::new(vec![]),
        fail: vec![],
    };
    let sink = CollectingSink::default();

    let result: ExecutionResult = execute("wf-1", &steps, &runner, &sink).await.unwrap();

    assert_eq!(runner.calls.into_inner().unwrap(), vec!["a", "b", "c"]);
    assert_eq!(result.status.as_str(), "completed");
    assert!(result
        .steps
        .iter()
        .all(|s| s.status == StepStatus::Completed));
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
    assert_eq!(
        kinds,
        vec![
            "started",
            "completed",
            "started",
            "completed",
            "started",
            "completed",
            "finished"
        ]
    );
}

#[tokio::test]
async fn executor_respects_dependencies_before_running_a_step() {
    let mut steps = workflow_steps(&["b", "a"]);
    steps[0].depends_on = vec!["a".into()];
    let runner = ScriptedRunner {
        calls: Mutex::new(vec![]),
        fail: vec![],
    };
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
    assert_eq!(
        result.steps[1].error.as_deref(),
        Some("blocked by failed step 'a'")
    );
    assert_eq!(result.steps[2].status, StepStatus::Completed);
    assert_eq!(result.steps[3].status, StepStatus::Blocked);
    assert_eq!(
        result.steps[3].error.as_deref(),
        Some("blocked by failed step 'b'")
    );
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
    let runner = ScriptedRunner {
        calls: Mutex::new(vec![]),
        fail: vec![],
    };
    let error = execute("wf-4", &steps, &runner, &FailingSink)
        .await
        .unwrap_err();
    assert_eq!(error, "sink down");
}

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
