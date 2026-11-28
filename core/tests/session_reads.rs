//! Read-only shared session API (Task 2 of the mobile-chat plan).
//!
//! Exercises the router in-process. Secrets in this file are synthetic markers:
//! no real credential is used, and the message listing must carry them only as
//! ordinary user content, never in a dedicated field.

use ai_upcycler_core::api::backend::StubBackend;
use ai_upcycler_core::api::{router, AppState, HostInfo};
use ai_upcycler_core::discovery::mdns::NodeRegistry;
use ai_upcycler_core::store::{NewChatMessage, NewModel, Store};
use ai_upcycler_core::telemetry::TelemetryBus;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use axum::Router;
use serde_json::Value;
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tower::ServiceExt;

fn state(store: Arc<Store>) -> AppState {
    AppState::new(
        "cluster-1".into(),
        Arc::new(NodeRegistry::new()),
        store,
        Arc::new(StubBackend::new()),
        TelemetryBus::new(64),
        HostInfo::default(),
    )
}

fn seed_model(store: &Store) {
    store
        .upsert_model(&NewModel {
            model_id: "m1".into(),
            file_name: "m1.gguf".into(),
            quantization_format: "Q4_K_M".into(),
            total_layers: 8,
            weight_size_bytes: 1024,
            local_file_path: "/models/m1.gguf".into(),
        })
        .expect("seed model");
}

fn add_message(store: &Store, session_id: &str, role: &str, content: &str, tokens: i64, secs: f64) {
    store
        .append_message(&NewChatMessage {
            session_id: session_id.into(),
            role: role.into(),
            content: content.into(),
            token_count: tokens,
            generation_time_sec: secs,
        })
        .expect("append message");
}

/// A file-backed store lets the test set explicit `created_at` values so the
/// ordering assertion does not depend on the wall clock's second boundary.
fn file_store() -> (Arc<Store>, PathBuf) {
    let path = std::env::temp_dir().join(format!(
        "ai-upcycler-session-reads-{}.db",
        uuid::Uuid::new_v4()
    ));
    let store = Arc::new(Store::open(&path).expect("file-backed store"));
    (store, path)
}

fn set_created_at(path: &Path, session_id: &str, created_at: &str) {
    let conn = rusqlite::Connection::open(path).expect("second connection to test database");
    conn.execute(
        "UPDATE chat_sessions SET created_at = ?1 WHERE session_id = ?2",
        rusqlite::params![created_at, session_id],
    )
    .expect("set created_at");
}

async fn send(app: Router, method: &str, uri: &str) -> (StatusCode, String, Value) {
    let response = app
        .oneshot(
            Request::builder()
                .method(method)
                .uri(uri)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let status = response.status();
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let text = String::from_utf8_lossy(&bytes).to_string();
    let value = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
    (status, text, value)
}

#[tokio::test]
async fn lists_sessions_ordered_by_created_at_then_session_id() {
    let (store, path) = file_store();
    seed_model(&store);
    store.create_session("z-early", "Early", "m1").unwrap();
    store.create_session("b-late", "Late B", "m1").unwrap();
    store.create_session("a-late", "Late A", "m1").unwrap();
    set_created_at(&path, "z-early", "2026-01-01 00:00:00");
    set_created_at(&path, "b-late", "2026-01-02 00:00:00");
    set_created_at(&path, "a-late", "2026-01-02 00:00:00");

    let app = router(state(store));
    let (status, text, body) = send(app.clone(), "GET", "/api/sessions").await;
    drop(app);
    let _ = std::fs::remove_file(&path);

    assert_eq!(status, StatusCode::OK, "{text}");
    let sessions = body["sessions"].as_array().expect("sessions array");
    let ids: Vec<&str> = sessions
        .iter()
        .map(|row| row["session_id"].as_str().expect("session_id"))
        .collect();
    assert_eq!(ids, vec!["z-early", "a-late", "b-late"]);
    for row in sessions {
        let keys: HashSet<&str> = row
            .as_object()
            .expect("session object")
            .keys()
            .map(String::as_str)
            .collect();
        assert_eq!(
            keys,
            HashSet::from(["session_id", "title", "model_id", "created_at"]),
            "a session row must not carry message bodies"
        );
    }
}

#[tokio::test]
async fn gets_one_session_as_a_chat_session_row() {
    let store = Arc::new(Store::open_in_memory().expect("store"));
    seed_model(&store);
    store.create_session("s1", "Hello", "m1").unwrap();
    let expected = serde_json::to_value(store.get_session("s1").unwrap().unwrap()).unwrap();

    let (status, text, body) = send(router(state(store)), "GET", "/api/sessions/s1").await;

    assert_eq!(status, StatusCode::OK, "{text}");
    assert_eq!(body, expected);
}

#[tokio::test]
async fn missing_session_uses_the_error_envelope_and_leaks_no_messages() {
    const SECRET: &str = "sk-TEST-ONLY-session-reads-7c41";
    let store = Arc::new(Store::open_in_memory().expect("store"));
    seed_model(&store);
    store.create_session("s1", "Hello", "m1").unwrap();
    add_message(&store, "s1", "user", SECRET, 3, 0.0);

    let (status, text, body) = send(router(state(store)), "GET", "/api/sessions/missing").await;

    assert_eq!(status, StatusCode::NOT_FOUND, "{text}");
    assert_eq!(
        body["error"]["message"].as_str(),
        Some("session missing not found")
    );
    assert_eq!(body["error"]["type"].as_str(), Some("not_found"));
    assert_eq!(body["error"]["code"].as_u64(), Some(404));
    assert_eq!(
        body.as_object()
            .expect("error object")
            .keys()
            .collect::<Vec<_>>(),
        vec!["error"],
        "a 404 must not carry a session or message body"
    );
    assert!(
        !text.contains(SECRET),
        "404 leaked a stored message: {text}"
    );
}

#[tokio::test]
async fn lists_messages_with_the_full_chat_message_row_shape() {
    let store = Arc::new(Store::open_in_memory().expect("store"));
    seed_model(&store);
    store.create_session("s1", "Hello", "m1").unwrap();
    add_message(&store, "s1", "user", "hi", 1, 0.0);
    add_message(&store, "s1", "assistant", "hello!", 2, 0.25);
    let expected = serde_json::to_value(store.list_messages("s1").unwrap()).unwrap();

    let (status, text, body) = send(router(state(store)), "GET", "/api/sessions/s1/messages").await;

    assert_eq!(status, StatusCode::OK, "{text}");
    assert_eq!(body["messages"], expected);
    let messages = body["messages"].as_array().expect("messages array");
    assert_eq!(messages.len(), 2);
    let reply = &messages[1];
    assert_eq!(reply["message_id"].as_i64(), Some(2));
    assert_eq!(reply["role"].as_str(), Some("assistant"));
    assert_eq!(reply["content"].as_str(), Some("hello!"));
    assert_eq!(reply["token_count"].as_i64(), Some(2));
    assert_eq!(reply["generation_time_sec"].as_f64(), Some(0.25));
    assert!(
        !reply["created_at"].as_str().unwrap_or_default().is_empty(),
        "created_at must be present on every message"
    );
}

#[tokio::test]
async fn secret_looking_content_appears_only_as_user_content() {
    const SECRET: &str = "sk-TEST-ONLY-session-reads-5a3f";
    const ENCRYPTED: &str = "-----BEGIN ENCRYPTED PAYLOAD-----";
    let store = Arc::new(Store::open_in_memory().expect("store"));
    seed_model(&store);
    store.create_session("s1", "Hello", "m1").unwrap();
    add_message(&store, "s1", "user", SECRET, 4, 0.0);
    add_message(&store, "s1", "user", ENCRYPTED, 6, 0.0);

    let (status, text, body) = send(router(state(store)), "GET", "/api/sessions/s1/messages").await;

    assert_eq!(status, StatusCode::OK, "{text}");
    assert_eq!(
        text.matches(SECRET).count(),
        1,
        "the marker must appear once, as content only: {text}"
    );
    assert_eq!(
        text.matches(ENCRYPTED).count(),
        1,
        "the encrypted-looking marker must appear once, as content only: {text}"
    );
    assert_eq!(
        body.as_object()
            .expect("response object")
            .keys()
            .collect::<Vec<_>>(),
        vec!["messages"],
        "no sibling field may duplicate message payloads"
    );
    let messages = body["messages"].as_array().expect("messages array");
    let allowed = HashSet::from([
        "message_id",
        "session_id",
        "role",
        "content",
        "token_count",
        "generation_time_sec",
        "created_at",
    ]);
    for message in messages {
        let object = message.as_object().expect("message object");
        for key in object.keys() {
            assert!(
                allowed.contains(key.as_str()),
                "unexpected message field {key:?}"
            );
            let lower = key.to_ascii_lowercase();
            assert!(
                !lower.contains("secret")
                    && !lower.contains("encrypt")
                    && !lower.contains("credential"),
                "message field {key:?} looks like a secret carrier"
            );
        }
        assert_eq!(message["role"].as_str(), Some("user"));
    }
    assert_eq!(messages[0]["content"].as_str(), Some(SECRET));
    assert_eq!(messages[1]["content"].as_str(), Some(ENCRYPTED));
}

#[tokio::test]
async fn messages_for_a_missing_session_return_the_same_not_found_envelope() {
    let store = Arc::new(Store::open_in_memory().expect("store"));
    seed_model(&store);

    let (status, text, body) = send(
        router(state(store)),
        "GET",
        "/api/sessions/missing/messages",
    )
    .await;

    assert_eq!(status, StatusCode::NOT_FOUND, "{text}");
    assert_eq!(
        body["error"]["message"].as_str(),
        Some("session missing not found")
    );
    assert_eq!(body["error"]["code"].as_u64(), Some(404));
    assert!(body.get("messages").is_none(), "{text}");
}

#[tokio::test]
async fn the_session_surface_has_no_write_routes() {
    let store = Arc::new(Store::open_in_memory().expect("store"));
    seed_model(&store);
    store.create_session("s1", "Hello", "m1").unwrap();
    let app = router(state(store));

    for (method, uri) in [
        ("POST", "/api/sessions"),
        ("PUT", "/api/sessions/s1"),
        ("DELETE", "/api/sessions/s1"),
        ("POST", "/api/sessions/s1/messages"),
    ] {
        let (status, text, _) = send(app.clone(), method, uri).await;
        assert_eq!(
            status,
            StatusCode::METHOD_NOT_ALLOWED,
            "{method} {uri}: {text}"
        );
    }
    assert_eq!(
        app.clone()
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri("/api/sessions")
                    .body(Body::empty())
                    .unwrap()
            )
            .await
            .unwrap()
            .status(),
        StatusCode::OK
    );
}
