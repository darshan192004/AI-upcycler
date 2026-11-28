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
