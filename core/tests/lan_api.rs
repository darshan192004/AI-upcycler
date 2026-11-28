//! Task 3: pairing-derived LAN token lifecycle.
//!
//! The token is the bearer credential for the later LAN listener. It is created
//! only at enrollment, persisted on the host as a SHA-256 hash, and dies with
//! revocation or a CA rotation. Every token in this file is generated at test
//! runtime; there are no committed token fixtures or real secrets.

use ai_upcycler_core::api::backend::StubBackend;
use ai_upcycler_core::api::{router, AppState, HostInfo};
use ai_upcycler_core::discovery::mdns::NodeRegistry;
use ai_upcycler_core::security::{generate_lan_token, hash_lan_token, SecurityState};
use ai_upcycler_core::store::Store;
use ai_upcycler_core::telemetry::TelemetryBus;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use axum::Router;
use base64::Engine as _;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::path::PathBuf;
use std::sync::Arc;
use tower::ServiceExt;

const TOKEN_PREFIX: &str = "aiu1_";
const TOKEN_PAYLOAD_LEN: usize = 43;
const LAN_TOKENS_FILE: &str = "lan-tokens.json";

fn temp_dir() -> PathBuf {
    let dir = std::env::temp_dir().join(format!("ai-upcycler-lan-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&dir).expect("temp dir");
    dir
}

async fn send(app: Router, method: &str, uri: &str, body: Option<Value>) -> (StatusCode, Value) {
    let builder = Request::builder().method(method).uri(uri);
    let request = match body {
        Some(body) => builder
            .header("content-type", "application/json")
            .body(Body::from(body.to_string())),
        None => builder.body(Body::empty()),
    }
    .expect("request");
    let response = app.oneshot(request).await.expect("response");
    let status = response.status();
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("body");
    let value = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).expect("json body")
    };
    (status, value)
}

fn plain_state() -> AppState {
    AppState::new(
        "cluster-1".into(),
        Arc::new(NodeRegistry::new()),
        Arc::new(Store::open_in_memory().expect("store")),
        Arc::new(StubBackend::new()),
        TelemetryBus::new(8),
        HostInfo::default(),
    )
}

#[test]
fn token_is_prefixed_32_random_bytes_base64url_without_padding() {
    let first = generate_lan_token();
    let second = generate_lan_token();

    assert!(first.starts_with(TOKEN_PREFIX));
    assert_eq!(first.len(), TOKEN_PREFIX.len() + TOKEN_PAYLOAD_LEN);
    let payload = &first[TOKEN_PREFIX.len()..];
    assert!(
        payload
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_'),
        "base64url alphabet only: {payload:?}"
    );
    assert!(!payload.contains('='), "no padding in a URL-safe token");
    assert_eq!(
        base64::engine::general_purpose::URL_SAFE_NO_PAD
            .decode(payload)
            .expect("decode payload")
            .len(),
        32,
        "32 random bytes encode to 43 base64url characters"
    );
    assert_ne!(first, second, "two enrollments must not share a token");
}

#[test]
fn verify_lan_token_accepts_exact_token_and_rejects_one_character_mutation() {
    let state = SecurityState::default();
    let token = generate_lan_token();
    state
        .enrollments
        .set_lan_token("phone-a", &token)
        .expect("set token");

    assert!(state.enrollments.verify_lan_token(&token));
    assert!(state
        .enrollments
        .verify_lan_token_hash(&hash_lan_token(&token)));

    let mut mutated = token.clone();
    let last = *mutated.as_bytes().last().expect("token has a payload") as char;
    mutated.pop();
    mutated.push(if last == 'A' { 'B' } else { 'A' });
    assert_ne!(mutated, token);
    assert!(
        !state.enrollments.verify_lan_token(&mutated),
        "a one-character change must not authenticate"
    );
    assert!(
        !state.enrollments.verify_lan_token(&generate_lan_token()),
        "an unrelated token must not authenticate"
    );
}

#[test]
fn lan_token_file_holds_only_a_lowercase_hex_hash() {
    let dir = temp_dir();
    let token = generate_lan_token();
    {
        let state = SecurityState::load_or_generate(&dir);
        state
            .enrollments
            .set_lan_token("phone-a", &token)
            .expect("set token");
    }

    let raw = std::fs::read_to_string(dir.join(LAN_TOKENS_FILE)).expect("token store");
    let expected = format!("{:x}", Sha256::digest(token.as_bytes()));
    assert_eq!(expected.len(), 64);
    assert!(
        expected
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)),
        "the persisted hash is lowercase hex"
    );
    assert!(raw.contains(&expected), "only the hash is persisted");
    assert!(
        !raw.contains(&token),
        "the plaintext token is never on disk"
    );
    assert!(!raw.contains(TOKEN_PREFIX), "no token prefix is on disk");

    // The hash survives a restart with the same CA: the phone keeps its token.
    let restarted = SecurityState::load_or_generate(&dir);
    assert!(restarted.enrollments.verify_lan_token(&token));
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn reenrolling_replaces_the_lan_token_hash() {
    let dir = temp_dir();
    let state = SecurityState::load_or_generate(&dir);
    let first = generate_lan_token();
    let second = generate_lan_token();

    state
        .enrollments
        .set_lan_token("phone-a", &first)
        .expect("set first token");
    assert!(state.enrollments.verify_lan_token(&first));
    state
        .enrollments
        .set_lan_token("phone-a", &second)
        .expect("set second token");

    assert!(
        !state.enrollments.verify_lan_token(&first),
        "a re-enrolled device invalidates its previous token"
    );
    assert!(state.enrollments.verify_lan_token(&second));
    let raw = std::fs::read_to_string(dir.join(LAN_TOKENS_FILE)).expect("token store");
    assert!(!raw.contains(&format!("{:x}", Sha256::digest(first.as_bytes()))));
    assert!(raw.contains(&format!("{:x}", Sha256::digest(second.as_bytes()))));
    std::fs::remove_dir_all(&dir).ok();
}

#[tokio::test]
async fn revoking_a_device_invalidates_its_lan_token() {
    let state = plain_state();
    state
        .security
        .enrollments
        .enroll("phone-a", "Pixel 8", vec!["192.168.1.13".into()]);
    let token = generate_lan_token();
    state
        .security
        .enrollments
        .set_lan_token("phone-a", &token)
        .expect("set token");
    assert!(state.security.enrollments.verify_lan_token(&token));

    let security = state.security.clone();
    let app = router(state);
    let (status, body) = send(app, "DELETE", "/api/devices/phone-a", None).await;

    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["revoked"], true);
    // The same handler must drop the enrollment and its token together.
    assert!(!security.enrollments.is_enrolled("phone-a"));
    assert!(!security.enrollments.verify_lan_token(&token));
}

#[test]
fn an_unusable_ca_clears_lan_tokens_with_enrollments() {
    let dir = temp_dir();
    let token = generate_lan_token();
    {
        let state = SecurityState::load_or_generate(&dir);
        state
            .enrollments
            .enroll("phone-a", "Pixel 8", vec!["192.168.1.13".into()]);
        state
            .enrollments
            .set_lan_token("phone-a", &token)
            .expect("set token");
    }
    std::fs::write(dir.join("ca-cert.pem"), "not a certificate").expect("corrupt the CA");

    let recovered = SecurityState::load_or_generate(&dir);

    assert!(
        !dir.join(LAN_TOKENS_FILE).exists(),
        "a new CA must not inherit tokens issued under the old one"
    );
    assert!(recovered.enrollments.is_empty());
    assert!(!recovered.enrollments.verify_lan_token(&token));
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn a_missing_ca_clears_lan_tokens_with_enrollments() {
    let dir = temp_dir();
    let stale_hash = "a".repeat(64);
    std::fs::write(
        dir.join(LAN_TOKENS_FILE),
        format!("{{\"phone-a\":\"{stale_hash}\"}}"),
    )
    .expect("stale token store");
    std::fs::write(dir.join("enrollments.json"), "{\"phone-a\":{}}")
        .expect("stale enrollment store");

    let state = SecurityState::load_or_generate(&dir);

    assert!(!dir.join(LAN_TOKENS_FILE).exists());
    assert!(!dir.join("enrollments.json").exists());
    assert!(state.enrollments.is_empty());
    std::fs::remove_dir_all(&dir).ok();
}

#[tokio::test]
async fn pairing_response_carries_a_token_and_hashes_every_enrollment_id() {
    let dir = temp_dir();
    let security = Arc::new(SecurityState::load_or_generate(&dir));
    let app = router(plain_state().with_security(security.clone()));
    let (status, session) = send(
        app.clone(),
        "POST",
        "/api/pairing/session?serial=16284c49",
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let pin = session["pin"].as_str().expect("pin").to_string();

    let (status, body) = send(
        app,
        "POST",
        "/api/pairing/enroll",
        Some(json!({
            "pin": pin,
            "device_name": "Pixel 8",
            "node_id": "phone.local.",
            "addresses": ["192.168.1.50"],
        })),
    )
    .await;

    assert_eq!(status, StatusCode::OK);
    let token = body["lan_token"].as_str().expect("lan_token").to_string();
    assert!(token.starts_with(TOKEN_PREFIX));
    assert_eq!(token.len(), TOKEN_PREFIX.len() + TOKEN_PAYLOAD_LEN);
    assert_eq!(body["lan_port"].as_u64(), Some(8090));
    assert_eq!(body["node_id"].as_str(), Some("phone.local."));
    for id in ["phone.local.", "adb:16284c49"] {
        assert!(
            security.enrollments.verify_lan_token(&token),
            "token must authenticate every enrolled id, including {id}"
        );
        assert!(security.enrollments.get_lan_token(id).is_some());
    }

    let raw = std::fs::read_to_string(dir.join(LAN_TOKENS_FILE)).expect("token store");
    assert!(
        !raw.contains(&token),
        "the pairing response token is hash-only on disk"
    );
    assert!(raw.contains(&format!("{:x}", Sha256::digest(token.as_bytes()))));
    std::fs::remove_dir_all(&dir).ok();
}

#[tokio::test]
async fn pairing_response_reports_the_configured_lan_port() {
    let _lock = LAN_ENV_LOCK
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    let _restore = RestoreLanEnv {
        enabled: std::env::var_os(LAN_API_ENV),
        port: std::env::var_os(LAN_PORT_ENV),
    };
    std::env::set_var(LAN_PORT_ENV, "8099");

    let app = router(plain_state());
    let (status, session) = send(app.clone(), "POST", "/api/pairing/session", None).await;
    assert_eq!(status, StatusCode::OK);
    let pin = session["pin"].as_str().expect("pin").to_string();
    let (status, body) = send(
        app,
        "POST",
        "/api/pairing/enroll",
        Some(json!({
            "pin": pin,
            "device_name": "Pixel 8",
            "node_id": "phone.local.",
        })),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["lan_port"].as_u64(), Some(8099));
}

// ---- Task 4: LAN allowlist router and bearer middleware -----------------

use ai_upcycler_core::api::lan::lan_router;
use ai_upcycler_core::secrets::{SecretScope, SecretStore};
use ai_upcycler_core::store::NewModel;
use std::collections::HashMap;
use std::sync::Mutex as StdMutex;

/// Injected credential store for the LAN routes: only a synthetic value is
/// ever written, and nothing touches the host keychain or the network.
#[derive(Default)]
struct TestSecretStore {
    values: StdMutex<HashMap<String, String>>,
}

fn scope_key(scope: SecretScope) -> String {
    match scope {
        SecretScope::Provider(provider) => format!("provider:{}", provider.as_str()),
        SecretScope::LanToken => "lan-token".into(),
    }
}

impl SecretStore for TestSecretStore {
    fn get(&self, scope: SecretScope) -> Option<String> {
        self.values.lock().ok()?.get(&scope_key(scope)).cloned()
    }

    fn set(&self, scope: SecretScope, value: &str) -> Result<(), String> {
        let mut values = self.values.lock().map_err(|_| "lock failed".to_string())?;
        values.insert(scope_key(scope), value.to_string());
        Ok(())
    }

    fn delete(&self, scope: SecretScope) -> Result<(), String> {
        let mut values = self.values.lock().map_err(|_| "lock failed".to_string())?;
        values.remove(&scope_key(scope));
        Ok(())
    }
}

fn lan_state() -> AppState {
    let store = Arc::new(Store::open_in_memory().expect("store"));
    store
        .upsert_model(&NewModel {
            model_id: "llama-8b".into(),
            file_name: "llama.gguf".into(),
            quantization_format: "Q4_K_M".into(),
            total_layers: 32,
            weight_size_bytes: 1,
            local_file_path: "/llama.gguf".into(),
        })
        .expect("seed model");
    AppState::new(
        "cluster-1".into(),
        Arc::new(NodeRegistry::new()),
        store,
        Arc::new(StubBackend::new()),
        TelemetryBus::new(8),
        HostInfo::default(),
    )
    .with_secrets(Arc::new(TestSecretStore::default()))
}

fn enrolled_lan_state() -> (AppState, String) {
    let state = lan_state();
    let token = generate_lan_token();
    state
        .security
        .enrollments
        .enroll("phone-a", "Pixel 8", vec!["192.168.1.13".into()]);
    state
        .security
        .enrollments
        .set_lan_token("phone-a", &token)
        .expect("set token");
    (state, token)
}

fn unauthorized_envelope() -> String {
    json!({"error": {
        "message": "unauthorized",
        "type": "unauthorized",
        "code": 401,
    }})
    .to_string()
}

async fn send_lan(
    app: Router,
    method: &str,
    uri: &str,
    body: Option<Value>,
    authorization: Option<&str>,
) -> (StatusCode, String) {
    let mut builder = Request::builder().method(method).uri(uri);
    if let Some(value) = authorization {
        builder = builder.header("authorization", value);
    }
    let request = match body {
        Some(body) => builder
            .header("content-type", "application/json")
            .body(Body::from(body.to_string())),
        None => builder.body(Body::empty()),
    }
    .expect("request");
    let response = app.oneshot(request).await.expect("response");
    let status = response.status();
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("body");
    (
        status,
        String::from_utf8(bytes.to_vec()).expect("utf8 body"),
    )
}

async fn send_lan_json(
    app: Router,
    method: &str,
    uri: &str,
    body: Option<Value>,
    authorization: Option<&str>,
) -> (StatusCode, Value) {
    let (status, text) = send_lan(app, method, uri, body, authorization).await;
    let value = if text.is_empty() {
        Value::Null
    } else {
        serde_json::from_str(&text).expect("json body")
    };
    (status, value)
}

#[tokio::test]
async fn unauthorized_without_header_returns_fixed_envelope() {
    let (state, _token) = enrolled_lan_state();
    let app = lan_router(state);

    let (status, body) = send_lan(app, "GET", "/v1/models", None, None).await;

    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert_eq!(body, unauthorized_envelope());
    for leaked in ["model", "session", "provider"] {
        assert!(!body.contains(leaked), "401 body leaked {leaked:?}: {body}");
    }
}

#[tokio::test]
async fn unauthorized_malformed_bearer_header_is_rejected() {
    let (state, token) = enrolled_lan_state();
    let app = lan_router(state);
    let headers = vec![
        "Bearer".to_string(),
        "Bearer ".to_string(),
        format!("bearer {token}"),
        "Basic abc".to_string(),
        format!("Bearer  {token}"),
        format!("Bearer {token} extra"),
    ];

    for header in headers {
        let (status, body) = send_lan(app.clone(), "GET", "/v1/models", None, Some(&header)).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED, "header {header:?}");
        assert_eq!(body, unauthorized_envelope(), "header {header:?}");
    }
}

#[tokio::test]
async fn allowlist_valid_token_reaches_every_route() {
    let (state, token) = enrolled_lan_state();
    let app = lan_router(state);
    let auth = format!("Bearer {token}");

    let (status, models) = send_lan_json(app.clone(), "GET", "/v1/models", None, Some(&auth)).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(models["object"], "list");

    let chat_body = json!({
        "model": "llama-8b",
        "messages": [{"role": "user", "content": "hello there"}],
        "provider": "local",
        "max_tokens": 16,
        "session_id": "s1",
    });
    let (status, chat) = send_lan_json(
        app.clone(),
        "POST",
        "/v1/chat/completions",
        Some(chat_body),
        Some(&auth),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{chat}");
    assert_eq!(chat["choices"][0]["message"]["role"], "assistant");

    let (status, providers) =
        send_lan_json(app.clone(), "GET", "/api/providers", None, Some(&auth)).await;
    assert_eq!(status, StatusCode::OK);
    assert!(providers.as_array().is_some_and(|rows| !rows.is_empty()));

    // A bogus model is refused by validation before any provider is dialed.
    let (status, _) = send_lan_json(
        app.clone(),
        "POST",
        "/api/providers/openai/test",
        Some(json!({"model": "not-a-provider-model"})),
        Some(&auth),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);

    let (status, _) = send_lan_json(
        app.clone(),
        "PUT",
        "/api/settings/secrets/openai",
        Some(json!({"value": "sk-synthetic-not-real"})),
        Some(&auth),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let (status, _) = send_lan_json(
        app.clone(),
        "DELETE",
        "/api/settings/secrets/openai",
        None,
        Some(&auth),
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    let (status, sessions) =
        send_lan_json(app.clone(), "GET", "/api/sessions", None, Some(&auth)).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(sessions["sessions"].as_array().map(Vec::len), Some(1));
    let (status, session) =
        send_lan_json(app.clone(), "GET", "/api/sessions/s1", None, Some(&auth)).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(session["session_id"], "s1");
    let (status, messages) = send_lan_json(
        app.clone(),
        "GET",
        "/api/sessions/s1/messages",
        None,
        Some(&auth),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(messages["messages"].as_array().map(Vec::len), Some(2));

    let (status, provision) = send_lan_json(
        app,
        "GET",
        "/api/devices/phone-a/provision",
        None,
        Some(&auth),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(provision["state"], "idle");
}

#[tokio::test]
async fn allowlist_admits_token_authenticated_gallery_reads() {
    let (state, token) = enrolled_lan_state();
    let app = lan_router(state);
    let auth = format!("Bearer {token}");

    let (status, body) = send_lan(app.clone(), "GET", "/api/images", None, None).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert_eq!(body, unauthorized_envelope());

    let (status, body) = send_lan_json(app.clone(), "GET", "/api/images", None, Some(&auth)).await;
    assert_eq!(status, StatusCode::OK);
    assert!(body["images"].is_array());

    let (status, _) = send_lan_json(
        app,
        "GET",
        "/api/images/missing/metadata",
        None,
        Some(&auth),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn allowlist_revoked_token_is_unauthorized() {
    let (state, token) = enrolled_lan_state();
    let security = state.security.clone();
    let app = lan_router(state);
    assert!(security
        .enrollments
        .revoke_lan_token("phone-a")
        .expect("revoke token"));

    let (status, body) = send_lan(
        app,
        "GET",
        "/v1/models",
        None,
        Some(&format!("Bearer {token}")),
    )
    .await;

    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert_eq!(body, unauthorized_envelope());
}

#[tokio::test]
async fn allowlist_excludes_pairing_and_device_control() {
    let (state, token) = enrolled_lan_state();
    let app = lan_router(state);
    let auth = format!("Bearer {token}");

    let (status, body) = send_lan(
        app.clone(),
        "POST",
        "/api/pairing/enroll",
        Some(json!({
            "pin": "000000",
            "device_name": "Pixel 8",
            "node_id": "phone.local.",
            "addresses": ["192.168.1.50"],
        })),
        Some(&auth),
    )
    .await;
    assert!(
        matches!(
            status,
            StatusCode::NOT_FOUND | StatusCode::METHOD_NOT_ALLOWED
        ),
        "LAN enroll status {status}: {body}"
    );
    assert_ne!(status, StatusCode::OK);

    let (status, body) = send_lan(app, "DELETE", "/api/devices/phone-a", None, Some(&auth)).await;
    assert!(
        matches!(
            status,
            StatusCode::NOT_FOUND | StatusCode::METHOD_NOT_ALLOWED
        ),
        "LAN device control status {status}: {body}"
    );
    assert_ne!(status, StatusCode::OK);
}

#[tokio::test]
async fn allowlist_loopback_router_still_serves_pairing_session_without_token() {
    let app = router(lan_state());

    let (status, body) = send_lan(
        app,
        "POST",
        "/api/pairing/session?serial=16284c49",
        None,
        None,
    )
    .await;

    assert_eq!(status, StatusCode::OK, "{body}");
    let session: Value = serde_json::from_str(&body).expect("json body");
    assert!(session["pin"]
        .as_str()
        .is_some_and(|pin| pin.len() == 6 && pin.bytes().all(|b| b.is_ascii_digit())));
}

// ---- Task 5: LAN listener config and master mDNS advert ------------------

use ai_upcycler_core::api::{LanApiConfig, DEFAULT_LAN_PORT, LAN_API_ENV, LAN_PORT_ENV};
use ai_upcycler_core::discovery::mdns::{
    resolve_lan_bind_addr, MasterServiceInfo, MASTER_SERVICE_TYPE,
};
use std::fmt;
use std::net::Ipv4Addr;
use tracing::field::{Field, Visit};
use tracing::span::{Attributes, Id, Record};
use tracing::{Event, Metadata, Subscriber};

/// Serializes the two env-mutating tests in this file.
static LAN_ENV_LOCK: StdMutex<()> = StdMutex::new(());

struct RestoreLanEnv {
    enabled: Option<std::ffi::OsString>,
    port: Option<std::ffi::OsString>,
}

impl Drop for RestoreLanEnv {
    fn drop(&mut self) {
        match self.enabled.take() {
            Some(value) => std::env::set_var(LAN_API_ENV, value),
            None => std::env::remove_var(LAN_API_ENV),
        }
        match self.port.take() {
            Some(value) => std::env::set_var(LAN_PORT_ENV, value),
            None => std::env::remove_var(LAN_PORT_ENV),
        }
    }
}

#[derive(Default)]
struct MessageVisitor {
    messages: Arc<StdMutex<Vec<String>>>,
}

impl Visit for MessageVisitor {
    fn record_debug(&mut self, field: &Field, value: &dyn fmt::Debug) {
        if field.name() == "message" {
            self.messages
                .lock()
                .expect("message collector")
                .push(format!("{value:?}"));
        }
    }
}

struct CapturingSubscriber {
    messages: Arc<StdMutex<Vec<String>>>,
}

impl Subscriber for CapturingSubscriber {
    fn enabled(&self, _metadata: &Metadata<'_>) -> bool {
        true
    }

    fn new_span(&self, _attributes: &Attributes<'_>) -> Id {
        Id::from_u64(1)
    }

    fn record(&self, _span: &Id, _values: &Record<'_>) {}

    fn record_follows_from(&self, _span: &Id, _follows: &Id) {}

    fn event(&self, event: &Event<'_>) {
        event.record(&mut MessageVisitor {
            messages: self.messages.clone(),
        });
    }

    fn enter(&self, _span: &Id) {}

    fn exit(&self, _span: &Id) {}
}

fn capture_warnings<F: FnOnce()>(run: F) -> Vec<String> {
    let messages = Arc::new(StdMutex::new(Vec::new()));
    let subscriber = CapturingSubscriber {
        messages: messages.clone(),
    };
    tracing::subscriber::with_default(subscriber, run);
    let mut collected = messages.lock().expect("message collector");
    std::mem::take(&mut *collected)
}

#[test]
fn config_env_gate_and_port_warnings_are_exact() {
    let _lock = LAN_ENV_LOCK
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    let _restore = RestoreLanEnv {
        enabled: std::env::var_os(LAN_API_ENV),
        port: std::env::var_os(LAN_PORT_ENV),
    };

    for value in [None, Some("0"), Some("true")] {
        match value {
            Some(value) => std::env::set_var(LAN_API_ENV, value),
            None => std::env::remove_var(LAN_API_ENV),
        }
        std::env::remove_var(LAN_PORT_ENV);
        let config = LanApiConfig::from_env();
        assert!(
            !config.enabled,
            "{value:?} must not enable the LAN listener"
        );
        assert_eq!(config.port, DEFAULT_LAN_PORT);
    }

    std::env::set_var(LAN_API_ENV, "1");
    std::env::set_var(LAN_PORT_ENV, "8099");
    let config = LanApiConfig::from_env();
    assert!(config.enabled);
    assert_eq!(config.port, 8099);

    for invalid in ["70000", "0"] {
        std::env::set_var(LAN_PORT_ENV, invalid);
        let warnings = capture_warnings(|| {
            let config = LanApiConfig::from_env();
            assert!(config.enabled);
            assert_eq!(config.port, DEFAULT_LAN_PORT, "{invalid}");
        });
        let expected =
            format!("ignoring invalid {LAN_PORT_ENV}={invalid}; using {DEFAULT_LAN_PORT}");
        assert!(
            warnings
                .iter()
                .any(|message| message.trim_matches('"') == expected),
            "expected exact warning {expected:?}, got {warnings:?}"
        );
    }
}

fn synthetic_master_info() -> MasterServiceInfo {
    MasterServiceInfo {
        instance_name: "master-host-pc".into(),
        node_id: "node-synthetic-01".into(),
        fingerprint: "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef".into(),
        port: 8099,
    }
}

#[test]
fn advert_txt_records_carry_public_metadata_without_token() {
    let records = synthetic_master_info().txt_records();

    assert!(
        records
            .iter()
            .any(|(key, value)| key == "version" && value == "1"),
        "records: {records:?}"
    );
    assert!(
        records
            .iter()
            .any(|(key, value)| key == "node_id" && value == "node-synthetic-01"),
        "records: {records:?}"
    );
    assert!(
        records
            .iter()
            .any(|(key, value)| key == "api_port" && value == "8099"),
        "records: {records:?}"
    );
    assert!(
        records.iter().any(|(key, value)| key == "fingerprint"
            && value == "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef"),
        "records: {records:?}"
    );
    assert!(
        records
            .iter()
            .all(|(key, _)| !key.eq_ignore_ascii_case("token") && !key.contains("token")),
        "a TXT key leaked a token: {records:?}"
    );
    assert!(
        records
            .iter()
            .all(|(_, value)| !value.contains("synthetic-token-must-never-appear")),
        "a TXT value leaked a token: {records:?}"
    );
}

#[test]
fn advert_service_type_is_exact() {
    assert_eq!(MASTER_SERVICE_TYPE, "_aiupcycler-master._tcp.local.");

    let service = synthetic_master_info().service_info("192.168.1.20".parse().expect("valid IPv4"));
    assert_eq!(service.get_type(), "_aiupcycler-master._tcp.local.");
    assert_eq!(service.get_port(), 8099);
    assert_eq!(
        service.get_fullname(),
        "master-host-pc._aiupcycler-master._tcp.local."
    );
    assert!(
        service
            .get_addresses()
            .contains(&"192.168.1.20".parse::<std::net::IpAddr>().expect("ip")),
        "advertised addresses: {:?}",
        service.get_addresses()
    );
    assert!(service.get_property_val_str("token").is_none());
}

#[test]
#[should_panic(expected = "concrete LAN IPv4")]
fn advert_refuses_to_advertise_an_unspecified_address() {
    let _ = synthetic_master_info().service_info(Ipv4Addr::UNSPECIFIED);
}

#[test]
fn advert_resolve_lan_bind_addr_prefers_first_private_ipv4() {
    let candidates: Vec<Ipv4Addr> = [
        "127.0.0.1",
        "169.254.9.9",
        "0.0.0.0",
        "172.16.4.5",
        "192.168.1.2",
    ]
    .iter()
    .map(|text| text.parse().expect("valid IPv4"))
    .collect();

    assert_eq!(
        resolve_lan_bind_addr(&candidates),
        Some("172.16.4.5".parse().expect("valid IPv4"))
    );

    let loopback_only: Vec<Ipv4Addr> = vec![
        "127.0.0.1".parse().expect("valid IPv4"),
        "127.0.0.2".parse().expect("valid IPv4"),
    ];
    assert_eq!(resolve_lan_bind_addr(&loopback_only), None);

    let link_local_only: Vec<Ipv4Addr> = vec!["169.254.1.1".parse().expect("valid IPv4")];
    assert_eq!(resolve_lan_bind_addr(&link_local_only), None);

    let public_only: Vec<Ipv4Addr> = vec!["8.8.8.8".parse().expect("valid IPv4")];
    assert_eq!(resolve_lan_bind_addr(&public_only), None);
}
