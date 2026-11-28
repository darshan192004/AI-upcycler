//! Settings, recommendations, config and writable session HTTP surfaces
//! (sub-project D tasks 6 and 7).
//!
//! Every credential-looking value in this file is a synthetic `sk-test-*` marker.
//! No real secret, key, token or file path may appear in any response body.

use ai_upcycler_core::api::backend::{MessageContent, StubBackend};
use ai_upcycler_core::api::downloads::StubFetcher;
use ai_upcycler_core::api::{router, AppState, HostInfo};
use ai_upcycler_core::discovery::mdns::NodeRegistry;
use ai_upcycler_core::settings::AppSettings;
use ai_upcycler_core::store::{NewChatMessage, NewModel, Store};
use ai_upcycler_core::telemetry::{TelemetryBus, WorkerHealth};
use ai_upcycler_core::worker_health::{HostMemorySource, HostMemoryUsage, TelemetryCache};
use axum::body::Body;
use axum::http::{header, Request, StatusCode};
use axum::Router;
use serde_json::{json, Value};
use std::path::PathBuf;
use std::sync::Arc;
use tower::ServiceExt;

struct FixedMemory(Option<HostMemoryUsage>);

impl HostMemorySource for FixedMemory {
    fn utilization_pct(&self) -> Option<f64> {
        self.0
            .map(|usage| (usage.used_bytes as f64 / usage.total_bytes as f64) * 100.0)
    }

    fn usage(&self) -> Option<HostMemoryUsage> {
        self.0
    }
}

fn state() -> AppState {
    let store = Arc::new(Store::open_in_memory().expect("store"));
    store
        .upsert_model(&NewModel {
            model_id: "llama-8b".into(),
            file_name: "llama.gguf".into(),
            quantization_format: "Q4_K_M".into(),
            total_layers: 32,
            weight_size_bytes: 1_024,
            local_file_path: r"D:\ai-upcycler-temp\models\llama.gguf".into(),
        })
        .unwrap();
    let cache = Arc::new(TelemetryCache::new());
    cache.record_worker(WorkerHealth {
        id: "worker-1".into(),
        temp_celsius: None,
        battery_pct: None,
        free_ram_bytes: 8 * 1024 * 1024 * 1024,
        observed_at_epoch_ms: 1_700_000_000_000,
        supports_compute: true,
    });
    AppState::new(
        "cluster-1".into(),
        Arc::new(NodeRegistry::new()),
        store,
        Arc::new(StubBackend::new()),
        TelemetryBus::new(16),
        HostInfo::default(),
    )
    .with_fetcher(Arc::new(StubFetcher::new()))
    .with_models_dir(PathBuf::from(r"D:\ai-upcycler-temp\test-models"))
    .with_host_memory(Arc::new(FixedMemory(Some(HostMemoryUsage {
        total_bytes: 16 * 1024 * 1024 * 1024,
        used_bytes: 0,
        available_bytes: 16 * 1024 * 1024 * 1024,
    }))))
    .with_telemetry_cache(cache)
}

async fn send(
    app: Router,
    method: &str,
    uri: &str,
    body: Option<Value>,
) -> (StatusCode, String, Value) {
    let mut builder = Request::builder().method(method).uri(uri);
    let request = match body {
        Some(value) => {
            builder = builder.header(header::CONTENT_TYPE, "application/json");
            builder
                .body(Body::from(serde_json::to_vec(&value).unwrap()))
                .unwrap()
        }
        None => builder.body(Body::empty()).unwrap(),
    };
    let response = app.oneshot(request).await.unwrap();
    let status = response.status();
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let text = String::from_utf8_lossy(&bytes).to_string();
    let value = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
    (status, text, value)
}

// ---- Task 6: settings and recommendations ---------------------------------

#[tokio::test]
async fn settings_get_put_round_trip() {
    let state = state();
    let app = router(state.clone());
    let body = json!({
        "provider": "openai",
        "model": "gpt-4o",
        "context_tokens": 16384,
        "max_tokens": 2048,
        "show_source_badges": false,
        "compact_mode": true
    });

    let (status, text, saved) = send(app.clone(), "PUT", "/api/settings", Some(body.clone())).await;
    assert_eq!(status, StatusCode::OK, "{text}");
    assert_eq!(saved, body);

    let (status, text, loaded) = send(app, "GET", "/api/settings", None).await;
    assert_eq!(status, StatusCode::OK, "{text}");
    assert_eq!(loaded, body);
}

#[tokio::test]
async fn settings_recommendations_use_injected_sources() {
    let (status, text, body) = send(
        router(state()),
        "GET",
        "/api/settings/recommendations",
        None,
    )
    .await;

    assert_eq!(status, StatusCode::OK, "{text}");
    assert_eq!(body["context_tokens"], 32768);
    assert_eq!(body["max_tokens"], 8192);
    assert_eq!(body["reasons"].as_array().unwrap().len(), 3);
}

#[tokio::test]
async fn settings_put_rejects_secret_fields_without_persisting_them() {
    let state = state();
    let app = router(state.clone());

    for field in ["api_key", "token", "secret", "password"] {
        let marker = format!("sk-test-{field}");
        let mut body = json!({
            "provider": "openai",
            "model": "gpt-4o",
            "context_tokens": 4096,
            "max_tokens": 1024,
            "show_source_badges": true,
            "compact_mode": false
        });
        body[field] = json!(marker);

        let (status, text, error) = send(app.clone(), "PUT", "/api/settings", Some(body)).await;

        assert_eq!(status, StatusCode::BAD_REQUEST, "{field}: {text}");
        assert!(
            error["error"]["message"]
                .as_str()
                .unwrap_or_default()
                .starts_with("unknown field"),
            "{field}: {text}"
        );
        assert!(!text.contains(&marker), "secret marker echoed: {text}");
        assert_eq!(state.store.setting_json(field).unwrap(), None);
    }

    let (status, text, loaded) = send(app, "GET", "/api/settings", None).await;
    assert_eq!(status, StatusCode::OK, "{text}");
    assert_eq!(
        loaded,
        serde_json::to_value(AppSettings::default()).unwrap()
    );
}

#[tokio::test]
async fn config_reports_multimodal_flag_without_leaking_state() {
    let state = state();
    state
        .store
        .put_settings(&AppSettings {
            provider: Some("openai".into()),
            model: Some("gpt-4o".into()),
            ..AppSettings::default()
        })
        .unwrap();

    for enabled in [false, true] {
        let (status, text, body) = send(
            router(state.clone().with_multimodal(enabled)),
            "GET",
            "/api/config",
            None,
        )
        .await;

        assert_eq!(status, StatusCode::OK, "{text}");
        assert_eq!(body, json!({ "multimodal_enabled": enabled }));
        for forbidden in [
            "api_key",
            "token",
            "secret",
            "password",
            "sk-test",
            "gpt-4o",
            "ai-upcycler-temp",
        ] {
            assert!(
                !text.contains(forbidden),
                "/api/config leaked {forbidden}: {text}"
            );
        }
    }
}

// ---- Task 7: writable sessions --------------------------------------------

#[tokio::test]
async fn session_list_rename_and_delete() {
    let state = state();
    let app = router(state.clone());
    state
        .store
        .create_session("s1", "First prompt", "llama-8b")
        .unwrap();

    let (status, text, listed) = send(app.clone(), "GET", "/api/sessions", None).await;
    assert_eq!(status, StatusCode::OK, "{text}");
    assert_eq!(listed["sessions"][0]["session_id"], "s1");
    assert_eq!(listed["sessions"][0]["title"], "First prompt");

    let (status, text, renamed) = send(
        app.clone(),
        "PUT",
        "/api/sessions/s1",
        Some(json!({ "title": "Renamed in D" })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{text}");
    assert_eq!(renamed["title"], "Renamed in D");

    let (status, _, _) = send(app.clone(), "DELETE", "/api/sessions/s1", None).await;
    assert_eq!(status, StatusCode::NO_CONTENT);

    let (status, text, empty) = send(app, "GET", "/api/sessions", None).await;
    assert_eq!(status, StatusCode::OK, "{text}");
    assert_eq!(empty["sessions"].as_array().unwrap().len(), 0);
}

#[tokio::test]
async fn session_messages_are_read_from_the_host_store() {
    let state = state();
    state
        .store
        .create_session("s1", "First prompt", "llama-8b")
        .unwrap();
    state
        .store
        .append_message(&NewChatMessage {
            session_id: "s1".into(),
            role: "user".into(),
            content: MessageContent::Text("persisted message".into()),
            token_count: 2,
            generation_time_sec: 0.0,
        })
        .unwrap();

    let (status, text, body) = send(router(state), "GET", "/api/sessions/s1/messages", None).await;

    assert_eq!(status, StatusCode::OK, "{text}");
    assert_eq!(body["messages"][0]["content"], "persisted message");
    assert_eq!(body["messages"][0]["message_id"], 1);
}

#[tokio::test]
async fn rename_rejects_empty_and_secret_fields() {
    let (status, text, body) = send(
        router(state()),
        "PUT",
        "/api/sessions/missing",
        Some(json!({ "title": "   " })),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{text}");
    assert_eq!(body["error"]["message"], "title must be 1..=120 characters");

    let marker = "sk-test-rename-never";
    let (status, text, body) = send(
        router(state()),
        "PUT",
        "/api/sessions/missing",
        Some(json!({ "title": "ok", "api_key": marker })),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{text}");
    assert!(
        body["error"]["message"]
            .as_str()
            .unwrap_or_default()
            .starts_with("unknown field"),
        "{text}"
    );
    assert!(!text.contains(marker), "secret marker echoed: {text}");
}

#[tokio::test]
async fn missing_session_writes_use_the_not_found_envelope() {
    let app = router(state());
    let cases: [(&str, &str, Option<Value>); 2] = [
        (
            "PUT",
            "/api/sessions/missing",
            Some(json!({ "title": "Renamed in D" })),
        ),
        ("DELETE", "/api/sessions/missing", None),
    ];

    for (method, uri, body) in cases {
        let (status, text, error) = send(app.clone(), method, uri, body).await;
        assert_eq!(status, StatusCode::NOT_FOUND, "{method} {uri}: {text}");
        assert_eq!(error["error"]["type"], "not_found", "{text}");
        assert_eq!(error["error"]["code"], 404, "{text}");
        assert!(
            error["error"]["message"]
                .as_str()
                .unwrap_or_default()
                .contains("session missing not found"),
            "{text}"
        );
    }
}

#[tokio::test]
async fn rename_wrong_type_rejection_is_generic_and_echoes_nothing() {
    let app = router(state());
    let bodies = [
        json!({ "title": { "sk-test-invalid value-RENAME": "sk-\"RENAME\"" } }),
        json!({ "title": ["sk-test-RENAME"] }),
    ];

    for body in bodies {
        let (status, text, error) =
            send(app.clone(), "PUT", "/api/sessions/missing", Some(body)).await;

        assert_eq!(status, StatusCode::BAD_REQUEST, "{text}");
        assert!(!text.contains("RENAME"), "wrong-type value echoed: {text}");
        assert_eq!(error["error"]["message"], "invalid request body", "{text}");
    }
}

#[tokio::test]
async fn settings_put_rejects_wrong_types_without_echoing_value_fragments() {
    let state = state();
    let app = router(state.clone());

    // Serde renders a wrong-typed string with `{:?}`. The first marker makes
    // "invalid value" appear inside the quoted caller value; the second adds
    // an escaped quote. Both used to survive the quote-toggling sanitizer.
    for marker in ["sk-test-invalid value-LEAK", "sk-\"LEAK\""] {
        let body = json!({
            "provider": "openai",
            "model": "gpt-4o",
            "context_tokens": marker,
            "max_tokens": 1024,
            "show_source_badges": true,
            "compact_mode": false
        });

        let (status, text, error) = send(app.clone(), "PUT", "/api/settings", Some(body)).await;

        assert_eq!(status, StatusCode::BAD_REQUEST, "{text}");
        assert!(!text.contains("LEAK"), "wrong-type value echoed: {text}");
        assert!(!text.contains(marker), "wrong-type marker echoed: {text}");
        assert_eq!(error["error"]["message"], "invalid request body", "{text}");
    }

    let (status, text, loaded) = send(app, "GET", "/api/settings", None).await;
    assert_eq!(status, StatusCode::OK, "{text}");
    assert_eq!(
        loaded,
        serde_json::to_value(AppSettings::default()).unwrap()
    );
}

#[tokio::test]
async fn settings_recommendations_ignore_virtual_cloud_models() {
    let state = state();
    state
        .store
        .upsert_model(&NewModel {
            model_id: "virtual-huge".into(),
            file_name: "virtual-huge.gguf".into(),
            quantization_format: "virtual".into(),
            total_layers: 1_024,
            weight_size_bytes: 0,
            local_file_path: "virtual:kimi-k2".into(),
        })
        .unwrap();

    let (status, text, body) =
        send(router(state), "GET", "/api/settings/recommendations", None).await;

    assert_eq!(status, StatusCode::OK, "{text}");
    assert_eq!(body["context_tokens"], 32_768, "{text}");
    assert_eq!(body["max_tokens"], 8_192, "{text}");
    assert_eq!(body["reasons"].as_array().unwrap().len(), 3, "{text}");
    assert!(
        !text.contains("virtual-huge"),
        "virtual catalog row leaked into recommendations: {text}"
    );
}
