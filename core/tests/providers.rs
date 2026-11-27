//! Routing policy, virtual cloud models, and app-state wiring (Tasks 7–9).
//!
//! Credentials in this file are synthetic markers. No real key is used and no
//! secret value is ever asserted into a log or telemetry payload.

mod common;

use ai_upcycler_core::api::backend::StubBackend;
use ai_upcycler_core::api::{router, AppState, HostInfo};
use ai_upcycler_core::discovery::mdns::NodeRegistry;
use ai_upcycler_core::providers::{ProviderId, ProviderRegistry};
use ai_upcycler_core::routing::RoutePolicy;
use ai_upcycler_core::secrets::{SecretScope, SecretStore, UnconfiguredSecretStore};
use ai_upcycler_core::store::{NewModel, Store, StoreError, VIRTUAL_MODEL_PATH_PREFIX};
use ai_upcycler_core::telemetry::TelemetryBus;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use axum::Router;
use serde_json::Value;
use std::sync::Arc;
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
    assert_eq!(RoutePolicy::parse(Some(" auto ")).unwrap(), RoutePolicy::Auto);
    assert_eq!(RoutePolicy::parse(Some("local")).unwrap(), RoutePolicy::Local);
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
        state.store.get_model("openai:gpt-4o").expect("lookup").is_some(),
        "routing must still be able to resolve the virtual row"
    );
}

#[test]
fn app_state_secret_builder_rebuilds_provider_statuses() {
    let state = state();
    assert!(
        state
            .secrets
            .get(SecretScope::Provider(ProviderId::OpenAI))
            .is_none()
    );
    assert!(
        state
            .providers
            .statuses()
            .iter()
            .all(|status| !status.configured)
    );

    let with_secrets = state.with_secrets(Arc::new(FakeSecrets));
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
    assert!(!wire.contains(SYNTHETIC_KEY), "secret leaked into status JSON");

    let explicit = ProviderRegistry::new(Arc::new(FakeSecrets))
        .with_base_url(ProviderId::OpenAI, "http://127.0.0.1:1234/v1");
    let with_providers = state.with_providers(explicit);
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
    let _ = UnconfiguredSecretStore::default();
}
