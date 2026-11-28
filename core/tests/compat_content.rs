use ai_upcycler_core::api::backend::{ChatMessage, GenerationRequest, MessageContent};
use serde_json::json;

#[test]
fn plain_string_content_round_trips_unchanged() {
    let msg: ChatMessage =
        serde_json::from_value(json!({"role":"user","content":"hello"})).unwrap();
    assert_eq!(msg.content, MessageContent::Text("hello".into()));
    assert_eq!(
        serde_json::to_value(&msg).unwrap(),
        json!({"role":"user","content":"hello"})
    );
}

#[test]
fn missing_and_null_content_become_empty_text() {
    for value in [
        json!({"role":"user"}),
        json!({"role":"user","content":null}),
    ] {
        let msg: ChatMessage = serde_json::from_value(value).unwrap();
        assert_eq!(msg.content, MessageContent::Text(String::new()));
    }
}

#[test]
fn agent_request_omitting_provider_still_deserializes() {
    let req: GenerationRequest = serde_json::from_value(json!({
        "messages":[{"role":"user","content":"hello"}],
        "max_tokens":64
    }))
    .unwrap();
    assert!(req.provider.is_none());
}
