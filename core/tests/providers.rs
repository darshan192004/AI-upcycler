//! Routing policy, virtual cloud models, and app-state wiring (Tasks 7–9).
//!
//! Credentials in this file are synthetic markers. No real key is used and no
//! secret value is ever asserted into a log or telemetry payload.

mod common;

use ai_upcycler_core::api::backend::{
    GenerationBackend, GenerationChunk, GenerationRequest, StubBackend,
};
use ai_upcycler_core::api::{router, AppState, HostInfo};
use ai_upcycler_core::discovery::mdns::NodeRegistry;
use ai_upcycler_core::providers::{ProviderId, ProviderRegistry};
use ai_upcycler_core::routing::RoutePolicy;
use ai_upcycler_core::secrets::{SecretScope, SecretStore, UnconfiguredSecretStore};
use ai_upcycler_core::store::{NewModel, Store, StoreError, VIRTUAL_MODEL_PATH_PREFIX};
use ai_upcycler_core::telemetry::{TelemetryBus, TelemetryEvent};
use axum::body::Body;
use axum::http::header::CONTENT_TYPE;
use axum::http::{Request, StatusCode};
use axum::routing::post;
use axum::{Json, Router};
use futures::stream::BoxStream;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use tower::ServiceExt;

const SYNTHETIC_KEY: &str = "sk-test-do-not-leak";

#[derive(Debug, Default)]
struct FakeSecrets;

impl SecretStore for FakeSecrets {
    fn get(&self, scope: SecretScope) -> Option<String> {
        matches!(scope, SecretScope::Provider(ProviderId::OpenAI))
            .then(|| SYNTHETIC_KEY.to_string())
    }

    fn set(&self, _scope: SecretScope, _value: &str) -> Result<(), String> {
        Ok(())
    }

    fn delete(&self, _scope: SecretScope) -> Result<(), String> {
        Ok(())
    }
}

fn state() -> AppState {
    let store = Arc::new(Store::open_in_memory().expect("in-memory store"));
    store
        .upsert_model(&NewModel {
            model_id: "llama-8b".into(),
            file_name: "llama.gguf".into(),
            quantization_format: "Q4_K_M".into(),
            total_layers: 32,
            weight_size_bytes: 32 * 1024 * 1024 * 64,
            local_file_path: "/models/llama.gguf".into(),
        })
        .expect("seed local model");

    AppState::new(
        "cluster-1".into(),
        Arc::new(NodeRegistry::new()),
        store,
        Arc::new(StubBackend::new()),
        TelemetryBus::new(64),
        HostInfo::default(),
    )
}

async fn send(app: Router, uri: &str) -> (StatusCode, Value) {
    let response = app
        .oneshot(Request::builder().uri(uri).body(Body::empty()).unwrap())
        .await
        .unwrap();
    let status = response.status();
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let value = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
    (status, value)
}

#[test]
fn route_policy_parses_auto_local_and_known_providers() {
    assert_eq!(RoutePolicy::parse(None).unwrap(), RoutePolicy::Auto);
    assert_eq!(RoutePolicy::parse(Some("")).unwrap(), RoutePolicy::Auto);
    assert_eq!(
        RoutePolicy::parse(Some(" auto ")).unwrap(),
        RoutePolicy::Auto
    );
    assert_eq!(
        RoutePolicy::parse(Some("local")).unwrap(),
        RoutePolicy::Local
    );
    assert_eq!(
        RoutePolicy::parse(Some("openai")).unwrap(),
        RoutePolicy::Cloud(ProviderId::OpenAI)
    );
    assert_eq!(
        RoutePolicy::parse(Some("anthropic")).unwrap(),
        RoutePolicy::Cloud(ProviderId::Anthropic)
    );
    assert_eq!(
        RoutePolicy::parse(Some("gemini")).unwrap(),
        RoutePolicy::Cloud(ProviderId::Gemini)
    );
    assert_eq!(
        RoutePolicy::parse(Some("kimi")).unwrap(),
        RoutePolicy::Cloud(ProviderId::Kimi)
    );
    let error = RoutePolicy::parse(Some("bogus")).expect_err("unknown provider");
    assert!(error.contains("bogus"), "{error}");
}

#[test]
fn ensure_virtual_model_is_idempotent_and_rejects_local_collisions() {
    let store = Store::open_in_memory().expect("store");
    store
        .ensure_virtual_model("openai:gpt-4o", "openai")
        .expect("first write");
    store
        .ensure_virtual_model("openai:gpt-4o", "openai")
        .expect("idempotent write");

    let row = store
        .get_model("openai:gpt-4o")
        .expect("lookup")
        .expect("virtual row");
    assert!(
        row.local_file_path.starts_with(VIRTUAL_MODEL_PATH_PREFIX),
        "{}",
        row.local_file_path
    );
    assert_eq!(row.quantization_format, "cloud");

    store
        .upsert_model(&NewModel {
            model_id: "llama-8b".into(),
            file_name: "llama.gguf".into(),
            quantization_format: "Q4_K_M".into(),
            total_layers: 32,
            weight_size_bytes: 1024,
            local_file_path: "/models/llama.gguf".into(),
        })
        .expect("local model");
    match store.ensure_virtual_model("llama-8b", "openai") {
        Err(StoreError::Conflict(message)) => {
            assert!(message.contains("llama-8b"), "{message}");
        }
        other => panic!("expected Conflict, got {other:?}"),
    }
    let local = store
        .get_model("llama-8b")
        .expect("lookup")
        .expect("local row survived");
    assert_eq!(local.local_file_path, "/models/llama.gguf");
}

#[tokio::test]
async fn virtual_cloud_models_are_hidden_from_the_model_listing() {
    let state = state();
    state
        .store
        .ensure_virtual_model("openai:gpt-4o", "openai")
        .expect("virtual model row");

    let (status, body) = send(router(state.clone()), "/v1/models").await;
    assert_eq!(status, StatusCode::OK);
    let ids: Vec<&str> = body["data"]
        .as_array()
        .expect("data array")
        .iter()
        .map(|entry| entry["id"].as_str().expect("string id"))
        .collect();
    assert_eq!(ids, vec!["llama-8b"], "virtual rows must not be listed");
    assert!(
        state
            .store
            .get_model("openai:gpt-4o")
            .expect("lookup")
            .is_some(),
        "routing must still be able to resolve the virtual row"
    );
}

#[test]
fn app_state_secret_builder_rebuilds_provider_statuses() {
    let state = state();
    assert!(state
        .secrets
        .get(SecretScope::Provider(ProviderId::OpenAI))
        .is_none());
    assert!(state
        .providers
        .statuses()
        .iter()
        .all(|status| !status.configured));

    let with_secrets = state.clone().with_secrets(Arc::new(FakeSecrets));
    assert_eq!(
        with_secrets
            .secrets
            .get(SecretScope::Provider(ProviderId::OpenAI))
            .as_deref(),
        Some(SYNTHETIC_KEY)
    );
    let statuses = with_secrets.providers.statuses();
    assert!(
        statuses
            .iter()
            .find(|status| status.id == ProviderId::OpenAI)
            .expect("OpenAI status")
            .configured,
        "OpenAI status should flip"
    );
    assert!(
        statuses
            .iter()
            .filter(|status| status.id != ProviderId::OpenAI)
            .all(|status| !status.configured),
        "other providers stay unconfigured"
    );
    let wire = serde_json::to_string(&statuses).expect("serialize statuses");
    assert!(
        !wire.contains(SYNTHETIC_KEY),
        "secret leaked into status JSON"
    );

    let explicit = ProviderRegistry::new(Arc::new(FakeSecrets))
        .with_base_url(ProviderId::OpenAI, "http://127.0.0.1:1234/v1");
    let with_providers = state.clone().with_providers(Arc::new(explicit));
    assert_eq!(
        with_providers.providers.base_url(ProviderId::OpenAI),
        "http://127.0.0.1:1234/v1"
    );
    assert!(
        with_providers
            .secrets
            .get(SecretScope::Provider(ProviderId::OpenAI))
            .is_none(),
        "with_providers must not replace the secret store"
    );
}

#[test]
fn default_state_uses_the_unconfigured_secret_store() {
    let state = state();
    assert!(
        state
            .secrets
            .set(SecretScope::Provider(ProviderId::OpenAI), SYNTHETIC_KEY)
            .is_err(),
        "platform secrets are never written from the default state"
    );
    let _ = UnconfiguredSecretStore;
}

// ---------------------------------------------------------------------------
// Task 8: router-level chat routing. Mock providers are loopback servers; the
// credential is a synthetic marker and is never written to logs or telemetry.
// ---------------------------------------------------------------------------

const OPENAI_SSE: &str = concat!(
    "data: {\"choices\":[{\"delta\":{\"content\":\"Hello \"},\"finish_reason\":null}]}\n\n",
    "data: {\"choices\":[{\"delta\":{\"content\":\"from OpenAI\"},\"finish_reason\":null}]}\n\n",
    "data: {\"choices\":[{\"delta\":{},\"finish_reason\":\"stop\"}]}\n\n",
    "data: [DONE]\n\n",
);

#[derive(Debug, Default, Clone)]
struct KeyedSecrets {
    key: String,
}

impl SecretStore for KeyedSecrets {
    fn get(&self, scope: SecretScope) -> Option<String> {
        matches!(scope, SecretScope::Provider(ProviderId::OpenAI))
            .then(|| self.key.clone())
            .filter(|value| !value.is_empty())
    }

    fn set(&self, _scope: SecretScope, _value: &str) -> Result<(), String> {
        Ok(())
    }

    fn delete(&self, _scope: SecretScope) -> Result<(), String> {
        Ok(())
    }
}

fn cloud_state(base_url: &str) -> AppState {
    let secrets: Arc<dyn SecretStore> = Arc::new(KeyedSecrets {
        key: SYNTHETIC_KEY.into(),
    });
    let providers = ProviderRegistry::new(secrets.clone())
        .with_base_url(ProviderId::OpenAI, format!("{base_url}/v1"))
        .with_http_client(
            reqwest::Client::builder()
                .no_proxy()
                .build()
                .expect("no-proxy test client"),
        );
    state()
        .with_secrets(secrets)
        .with_providers(Arc::new(providers))
}

fn mock_openai(captured: Arc<Mutex<Option<Value>>>) -> Router {
    Router::new().route(
        "/v1/chat/completions",
        post(move |Json(body): Json<Value>| {
            let captured = captured.clone();
            async move {
                *captured.lock().expect("capture lock") = Some(body);
                (
                    [(CONTENT_TYPE, "text/event-stream")],
                    OPENAI_SSE.to_string(),
                )
            }
        }),
    )
}

async fn post_chat(app: Router, body: Value) -> (StatusCode, Value) {
    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/v1/chat/completions")
                .header(CONTENT_TYPE, "application/json")
                .body(Body::from(serde_json::to_vec(&body).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();
    let status = response.status();
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let value = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
    (status, value)
}

fn chat_body(model: &str, provider: Option<&str>) -> Value {
    let mut body = json!({
        "model": model,
        "messages": [{ "role": "user", "content": "hello" }],
    });
    if let Some(provider) = provider {
        body["provider"] = json!(provider);
    }
    body
}

fn metric_sources(rx: &mut tokio::sync::broadcast::Receiver<TelemetryEvent>) -> Vec<String> {
    let mut sources = Vec::new();
    while let Ok(event) = rx.try_recv() {
        if let TelemetryEvent::GenerationMetrics { data, .. } = event {
            sources.push(data.source);
        }
    }
    sources
}

#[tokio::test]
async fn explicit_local_turn_stays_on_the_stub_and_reports_local_source() {
    let state = state();
    let mut rx = state.telemetry.subscribe();
    let store = state.store.clone();

    let (status, body) = post_chat(router(state), chat_body("llama-8b", Some("local"))).await;

    assert_eq!(status, StatusCode::OK);
    assert!(body["choices"][0]["message"]["content"]
        .as_str()
        .unwrap()
        .contains("[stub:llama-8b]"));
    assert!(
        metric_sources(&mut rx)
            .iter()
            .any(|source| source == "local"),
        "stub turns must be labelled local"
    );
    assert_eq!(store.list_sessions().unwrap().len(), 1);
}

#[tokio::test]
async fn explicit_cloud_turn_uses_the_provider_and_registers_a_virtual_model() {
    let captured = Arc::new(Mutex::new(None));
    let (base_url, handle) = common::spawn_mock_provider(mock_openai(captured.clone())).await;
    let state = cloud_state(&base_url);
    let mut rx = state.telemetry.subscribe();
    let store = state.store.clone();

    let (status, body) = post_chat(router(state), chat_body("gpt-4o", Some("openai"))).await;
    handle.abort();

    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(
        body["choices"][0]["message"]["content"].as_str(),
        Some("Hello from OpenAI")
    );
    assert_eq!(body["model"].as_str(), Some("openai:gpt-4o"));
    let sent = captured.lock().expect("capture lock").clone().unwrap();
    assert_eq!(sent["model"].as_str(), Some("gpt-4o"));
    assert!(sent.get("tools").is_none(), "cloud adapters are text-only");
    assert!(
        metric_sources(&mut rx)
            .iter()
            .any(|source| source == "openai"),
        "cloud turns must be labelled with the provider id"
    );
    let virtual_row = store
        .get_model("openai:gpt-4o")
        .expect("virtual row lookup")
        .expect("virtual row");
    assert!(virtual_row
        .local_file_path
        .starts_with(VIRTUAL_MODEL_PATH_PREFIX));
}

#[tokio::test]
async fn auto_falls_back_to_a_configured_cloud_when_only_the_stub_is_present() {
    let captured = Arc::new(Mutex::new(None));
    let (base_url, handle) = common::spawn_mock_provider(mock_openai(captured.clone())).await;
    let state = cloud_state(&base_url);

    let (status, body) = post_chat(router(state), chat_body("openai:gpt-4o", None)).await;
    handle.abort();

    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(
        body["choices"][0]["message"]["content"].as_str(),
        Some("Hello from OpenAI")
    );
    assert_eq!(body["model"].as_str(), Some("openai:gpt-4o"));
}

#[tokio::test]
async fn auto_with_a_stub_and_no_cloud_refuses_without_writing_a_session() {
    let state = state();
    let store = state.store.clone();

    let (status, body) = post_chat(router(state), chat_body("llama-8b", None)).await;

    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    let message = body["error"]["message"].as_str().unwrap();
    assert!(
        message.contains("local engine") && message.contains("cloud"),
        "the refusal must name both missing options: {message}"
    );
    assert!(
        store.list_sessions().unwrap().is_empty(),
        "a refused turn must not create a session"
    );
}

#[tokio::test]
async fn tool_turns_are_refused_for_cloud_without_writing_or_calling_the_provider() {
    let captured = Arc::new(Mutex::new(None));
    let (base_url, handle) = common::spawn_mock_provider(mock_openai(captured.clone())).await;
    let state = cloud_state(&base_url);
    let store = state.store.clone();
    let mut body = chat_body("gpt-4o", Some("openai"));
    body["tools"] = json!([{
        "type": "function",
        "function": {
            "name": "read",
            "description": "Read a file",
            "parameters": { "type": "object" }
        }
    }]);

    let (status, error) = post_chat(router(state), body).await;
    handle.abort();

    assert_eq!(status, StatusCode::BAD_REQUEST, "{error}");
    assert!(
        error["error"]["message"]
            .as_str()
            .unwrap()
            .contains("host engine"),
        "the refusal must explain the missing host engine: {error}"
    );
    assert!(store.list_sessions().unwrap().is_empty());
    assert!(
        captured.lock().expect("capture lock").is_none(),
        "a refused tool turn must never reach the provider"
    );
}

#[tokio::test]
async fn cloud_turn_without_a_key_is_refused_before_session_writes() {
    let state = state();

    let (status, body) = post_chat(router(state), chat_body("gpt-4o", Some("openai"))).await;

    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert!(
        body["error"]["message"]
            .as_str()
            .unwrap()
            .contains("openai"),
        "the refusal must name the provider without the key: {body}"
    );
    assert!(!serde_json::to_string(&body)
        .unwrap()
        .contains(SYNTHETIC_KEY));
}

#[tokio::test]
async fn unknown_providers_and_unknown_cloud_models_are_bad_requests() {
    let state = state();
    let store = state.store.clone();

    let (status, _) = post_chat(router(state.clone()), chat_body("llama-8b", Some("bogus"))).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    let (status, body) = post_chat(router(state), chat_body("gpt-9", Some("openai"))).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert!(store.list_sessions().unwrap().is_empty());
}

#[tokio::test]
async fn a_local_model_id_colliding_with_a_virtual_model_returns_conflict() {
    let captured = Arc::new(Mutex::new(None));
    let (base_url, handle) = common::spawn_mock_provider(mock_openai(captured.clone())).await;
    let state = cloud_state(&base_url);
    state
        .store
        .upsert_model(&NewModel {
            model_id: "gpt-4o".into(),
            file_name: "local.gguf".into(),
            quantization_format: "Q4_K_M".into(),
            total_layers: 32,
            weight_size_bytes: 1024,
            local_file_path: "/models/local.gguf".into(),
        })
        .expect("colliding local row");

    let (status, body) = post_chat(router(state), chat_body("gpt-4o", Some("openai"))).await;
    handle.abort();

    assert_eq!(status, StatusCode::CONFLICT, "{body}");
    assert!(body["error"]["message"]
        .as_str()
        .unwrap()
        .contains("gpt-4o"));
}

/// A real backend serving a phone worker, used to prove the cluster source is
/// carried into telemetry without dialing any device in this test.
struct ClusterSourceBackend;

impl GenerationBackend for ClusterSourceBackend {
    fn generate(
        &self,
        _request: GenerationRequest,
    ) -> BoxStream<'static, Result<GenerationChunk, String>> {
        Box::pin(async_stream::stream! {
            yield Ok(GenerationChunk {
                content: "from phone".into(),
                tokens_emitted: 1,
                source: Some("adb:192.168.1.9:5555".into()),
                ..Default::default()
            });
        })
    }
}

#[tokio::test]
async fn a_real_cluster_backend_keeps_auto_local_and_reports_the_worker_source() {
    let mut state = state();
    state.backend = Arc::new(ClusterSourceBackend);
    let mut rx = state.telemetry.subscribe();

    let (status, body) = post_chat(router(state), chat_body("llama-8b", None)).await;

    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(
        body["choices"][0]["message"]["content"].as_str(),
        Some("from phone")
    );
    assert!(
        metric_sources(&mut rx)
            .iter()
            .any(|source| source == "adb:192.168.1.9:5555"),
        "cluster turns must be labelled with the worker serial"
    );
}

// ---------------------------------------------------------------------------
// Task 9: provider status and secret endpoints. The mutable store is local to
// this test binary; the production state keeps the OS keychain store.
// ---------------------------------------------------------------------------

struct MutableSecrets {
    values: Mutex<HashMap<ProviderId, String>>,
}

impl Default for MutableSecrets {
    fn default() -> Self {
        Self {
            values: Mutex::new(HashMap::new()),
        }
    }
}

impl SecretStore for MutableSecrets {
    fn get(&self, scope: SecretScope) -> Option<String> {
        match scope {
            SecretScope::Provider(id) => self.values.lock().ok()?.get(&id).cloned(),
            SecretScope::LanToken => None,
        }
    }

    fn set(&self, scope: SecretScope, value: &str) -> Result<(), String> {
        let SecretScope::Provider(id) = scope else {
            return Err("only provider credentials are supported here".into());
        };
        let mut values = self.values.lock().map_err(|_| "secret lock failed")?;
        values.insert(id, value.to_string());
        Ok(())
    }

    fn delete(&self, scope: SecretScope) -> Result<(), String> {
        let SecretScope::Provider(id) = scope else {
            return Err("only provider credentials are supported here".into());
        };
        let mut values = self.values.lock().map_err(|_| "secret lock failed")?;
        values.remove(&id);
        Ok(())
    }
}

fn secret_state(base_url: Option<&str>) -> (AppState, Arc<MutableSecrets>) {
    let secrets = Arc::new(MutableSecrets::default());
    let dyn_secrets: Arc<dyn SecretStore> = secrets.clone();
    let mut built = state().with_secrets(dyn_secrets.clone());
    if let Some(base_url) = base_url {
        let registry = ProviderRegistry::new(dyn_secrets)
            .with_base_url(ProviderId::OpenAI, format!("{base_url}/v1"))
            .with_http_client(
                reqwest::Client::builder()
                    .no_proxy()
                    .build()
                    .expect("no-proxy test client"),
            );
        built = built.with_providers(Arc::new(registry));
    }
    (built, secrets)
}

async fn call(app: Router, method: &str, uri: &str, body: Body) -> (StatusCode, String, Value) {
    let response = app
        .oneshot(
            Request::builder()
                .method(method)
                .uri(uri)
                .header(CONTENT_TYPE, "application/json")
                .body(body)
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

fn json_body(value: Value) -> Body {
    Body::from(serde_json::to_vec(&value).unwrap())
}

#[tokio::test]
async fn provider_list_is_credential_free_reports_four_unconfigured_providers() {
    let (app_state, _) = secret_state(None);

    let (status, text, body) =
        call(router(app_state), "GET", "/api/providers", Body::empty()).await;

    assert_eq!(status, StatusCode::OK, "{text}");
    let entries = body.as_array().expect("provider list is an array");
    assert_eq!(entries.len(), 4);
    let ids: Vec<&str> = entries
        .iter()
        .map(|entry| entry["id"].as_str().unwrap())
        .collect();
    assert_eq!(ids, vec!["openai", "anthropic", "gemini", "kimi"]);
    assert!(entries.iter().all(|entry| entry["configured"] == false));
    assert!(entries.iter().all(|entry| entry["models"].is_array()));
    assert!(
        !text.contains("sk-"),
        "provider list must never carry a key"
    );
}

#[tokio::test]
async fn saving_a_secret_flips_configured_without_exposing_the_value() {
    let (app_state, _) = secret_state(None);
    let app = router(app_state);

    let (status, text, body) = call(
        app.clone(),
        "PUT",
        "/api/settings/secrets/openai",
        json_body(json!({ "value": "sk-super-secret" })),
    )
    .await;

    assert_eq!(status, StatusCode::OK, "{text}");
    assert_eq!(body["id"], "openai");
    assert_eq!(body["configured"], true);
    assert!(!text.contains("sk-super-secret"), "{text}");

    let (status, list_text, list) = call(app, "GET", "/api/providers", Body::empty()).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(list[0]["configured"], true);
    assert_eq!(list[1]["configured"], false);
    assert!(!list_text.contains("sk-super-secret"));
}

#[tokio::test]
async fn deleting_a_secret_is_idempotent_and_flips_configured_back() {
    let (app_state, _) = secret_state(None);
    let app = router(app_state);

    let (status, _, _) = call(
        app.clone(),
        "PUT",
        "/api/settings/secrets/kimi",
        json_body(json!({ "value": "sk-super-secret" })),
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    let (status, text, body) = call(
        app.clone(),
        "DELETE",
        "/api/settings/secrets/kimi",
        Body::empty(),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{text}");
    assert_eq!(body["configured"], false);

    let (status, text, body) = call(
        app.clone(),
        "DELETE",
        "/api/settings/secrets/kimi",
        Body::empty(),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{text}");
    assert_eq!(body["configured"], false);

    let (_, _, list) = call(app, "GET", "/api/providers", Body::empty()).await;
    assert!(list
        .as_array()
        .expect("provider list")
        .iter()
        .all(|entry| entry["configured"] == false));
}

#[tokio::test]
async fn unknown_provider_paths_and_bad_secret_bodies_are_rejected() {
    let (app_state, _) = secret_state(None);
    let app = router(app_state);

    let (status, _, _) = call(
        app.clone(),
        "PUT",
        "/api/settings/secrets/bogus",
        json_body(json!({ "value": "sk-super-secret" })),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    let (status, _, _) = call(
        app.clone(),
        "POST",
        "/api/providers/bogus/test",
        json_body(json!({ "model": "gpt-4o" })),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    let (status, _, _) = call(
        app.clone(),
        "PUT",
        "/api/settings/secrets/openai",
        json_body(json!({ "value": "" })),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);

    let (status, _, _) = call(
        app,
        "PUT",
        "/api/settings/secrets/openai",
        Body::from("{\"value\":"),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn provider_test_streams_one_turn_and_returns_only_a_preview() {
    let captured = Arc::new(Mutex::new(None));
    let (base_url, handle) = common::spawn_mock_provider(mock_openai(captured.clone())).await;
    let (app_state, _) = secret_state(Some(&base_url));
    let app = router(app_state);

    let (status, _, _) = call(
        app.clone(),
        "PUT",
        "/api/settings/secrets/openai",
        json_body(json!({ "value": "sk-super-secret" })),
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    let (status, text, body) = call(
        app,
        "POST",
        "/api/providers/openai/test",
        json_body(json!({ "model": "gpt-4o" })),
    )
    .await;
    handle.abort();

    assert_eq!(status, StatusCode::OK, "{text}");
    assert_eq!(body["provider"], "openai");
    assert_eq!(body["model"], "gpt-4o");
    assert_eq!(body["ok"], true);
    assert_eq!(body["preview"], "Hello from OpenAI");
    assert!(!text.contains("sk-super-secret"), "{text}");
    assert!(!text.contains("Authorization"), "{text}");
}

#[tokio::test]
async fn provider_test_without_a_key_refuses_before_dialing() {
    let (app_state, _) = secret_state(Some("http://127.0.0.1:1"));

    let (status, text, _) = call(
        router(app_state),
        "POST",
        "/api/providers/openai/test",
        json_body(json!({ "model": "gpt-4o" })),
    )
    .await;

    assert_eq!(status, StatusCode::BAD_REQUEST, "{text}");
    assert!(text.contains("openai"), "{text}");
    assert!(!text.contains("sk-"), "{text}");
}

#[tokio::test]
async fn the_dashboard_can_preflight_a_secret_put() {
    let (app_state, _) = secret_state(None);
    let response = router(app_state)
        .oneshot(
            Request::builder()
                .method("OPTIONS")
                .uri("/api/settings/secrets/openai")
                .header("origin", "http://localhost:1420")
                .header("access-control-request-method", "PUT")
                .header("access-control-request-headers", "content-type")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    let allowed = response
        .headers()
        .get("access-control-allow-methods")
        .expect("CORS preflight grant")
        .to_str()
        .unwrap()
        .to_string();
    assert!(allowed.contains("PUT"), "allowed methods: {allowed}");
    let origin = response
        .headers()
        .get("access-control-allow-origin")
        .and_then(|value| value.to_str().ok())
        .expect("CORS origin grant");
    assert_eq!(origin, "http://localhost:1420");
    let headers = response
        .headers()
        .get("access-control-allow-headers")
        .and_then(|value| value.to_str().ok())
        .expect("CORS header grant")
        .to_ascii_lowercase();
    assert!(
        headers.contains("content-type"),
        "allowed headers: {headers}"
    );
}
