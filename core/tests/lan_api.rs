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
    state.enrollments.set_lan_token("phone-a", &token);

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
        state.enrollments.set_lan_token("phone-a", &token);
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

    state.enrollments.set_lan_token("phone-a", &first);
    assert!(state.enrollments.verify_lan_token(&first));
    state.enrollments.set_lan_token("phone-a", &second);

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
    state.security.enrollments.set_lan_token("phone-a", &token);
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
        state.enrollments.set_lan_token("phone-a", &token);
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
