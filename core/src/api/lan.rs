//! Opt-in LAN surface (sub-project B): a small, explicit allowlist behind the
//! bearer token minted at pairing.
//!
//! This is deliberately not [`super::router`]. The loopback router keeps every
//! management route and needs no credential; the LAN router exposes only what a
//! paired phone needs and checks a hashed token on every request. Pairing and
//! device revocation are never reachable from this surface.

use crate::api::{chat, images, models, providers, provisioning, sessions, ApiError, AppState};
use crate::security::hash_lan_token;
use axum::body::Body;
use axum::extract::State;
use axum::http::{header::AUTHORIZATION, Request};
use axum::middleware::{from_fn_with_state, Next};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post, put};
use axum::Router;
use tower_http::trace::{DefaultMakeSpan, TraceLayer};

/// Set to exactly `1` to start the LAN listener.
pub const LAN_API_ENV: &str = "AI_UPCYCLER_LAN_API";
/// Optional override for the LAN listener port.
pub const LAN_PORT_ENV: &str = "AI_UPCYCLER_LAN_API_PORT";
/// Port advertised to paired devices in the pairing response.
pub const DEFAULT_LAN_PORT: u16 = 8090;

/// Configuration for the opt-in LAN listener.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LanApiConfig {
    pub enabled: bool,
    pub port: u16,
}

impl Default for LanApiConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            port: DEFAULT_LAN_PORT,
        }
    }
}

impl LanApiConfig {
    /// Reads the two environment knobs. Anything other than the exact string
    /// `1` leaves the LAN listener off; a bad port is logged and ignored.
    pub fn from_env() -> Self {
        Self::from_values(
            std::env::var(LAN_API_ENV).ok(),
            std::env::var(LAN_PORT_ENV).ok(),
        )
    }

    fn from_values(enabled: Option<String>, port: Option<String>) -> Self {
        Self {
            enabled: enabled.as_deref() == Some("1"),
            port: parse_lan_port(port.as_deref()),
        }
    }
}

fn parse_lan_port(value: Option<&str>) -> u16 {
    match value {
        Some(raw) if !raw.trim().is_empty() => match raw.trim().parse::<u16>() {
            Ok(port) if port > 0 => port,
            _ => {
                // The value is a port string from the environment, never a
                // token; log it verbatim without any credential in the line.
                tracing::warn!("ignoring invalid {LAN_PORT_ENV}={raw}; using {DEFAULT_LAN_PORT}");
                DEFAULT_LAN_PORT
            }
        },
        _ => DEFAULT_LAN_PORT,
    }
}

/// Rejects every request that does not carry `Authorization: Bearer <token>`.
///
/// The presented token is hashed and compared through the enrollment registry's
/// constant-time verifier. Neither the token nor any detail about why it failed
/// reaches a log line or the response body.
pub async fn require_lan_token(
    State(state): State<AppState>,
    request: Request<Body>,
    next: Next,
) -> Response {
    let presented = request
        .headers()
        .get(AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.strip_prefix("Bearer "))
        .filter(|token| !token.is_empty() && token.bytes().all(|byte| !byte.is_ascii_whitespace()));

    let authorized = presented
        .map(|token| {
            state
                .security
                .enrollments
                .verify_lan_token_hash(&hash_lan_token(token))
        })
        .unwrap_or(false);

    if !authorized {
        return ApiError::Unauthorized("unauthorized".into()).into_response();
    }
    next.run(request).await
}

/// Builds the LAN router: exactly the paired phone's routes, all behind
/// [`require_lan_token`]. No CORS layer, and the trace span never records
/// headers, so an `authorization` value cannot land in a log.
pub fn lan_router(state: AppState) -> Router {
    Router::new()
        .route("/v1/models", get(models::list_models))
        .route("/v1/chat/completions", post(chat::chat_completions))
        .route("/api/providers", get(providers::list))
        .route("/api/providers/:id/test", post(providers::test))
        .route(
            "/api/settings/secrets/:provider",
            put(providers::put_secret).delete(providers::delete_secret),
        )
        .route("/api/sessions", get(sessions::list))
        .route("/api/sessions/:id", get(sessions::get))
        .route("/api/sessions/:id/messages", get(sessions::messages))
        .route("/api/images", get(images::list_images))
        .route("/api/images/:id/metadata", get(images::get_image_metadata))
        .route("/api/images/:id", get(images::get_image_bytes))
        .route("/api/devices/:id/provision", get(provisioning::status))
        .layer(from_fn_with_state(state.clone(), require_lan_token))
        .layer(
            TraceLayer::new_for_http()
                .make_span_with(DefaultMakeSpan::new().include_headers(false)),
        )
        .with_state(state)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lan_config_is_off_unless_enabled_is_exactly_one() {
        for value in [
            None,
            Some(""),
            Some("0"),
            Some("true"),
            Some("1 "),
            Some("01"),
        ] {
            let config = LanApiConfig::from_values(value.map(str::to_string), None);
            assert!(
                !config.enabled,
                "{value:?} must not enable the LAN listener"
            );
            assert_eq!(config.port, DEFAULT_LAN_PORT);
        }
        let config = LanApiConfig::from_values(Some("1".into()), None);
        assert!(config.enabled);
        assert_eq!(config.port, DEFAULT_LAN_PORT);
    }

    #[test]
    fn lan_config_accepts_a_valid_nonzero_port() {
        let config = LanApiConfig::from_values(Some("1".into()), Some(" 9123 ".into()));
        assert!(config.enabled);
        assert_eq!(config.port, 9123);
    }

    #[test]
    fn lan_config_falls_back_for_bad_ports() {
        for value in [Some("0"), Some("65536"), Some("-1"), Some("abc"), Some("")] {
            let config = LanApiConfig::from_values(None, value.map(str::to_string));
            assert_eq!(config.port, DEFAULT_LAN_PORT, "{value:?}");
            assert!(!config.enabled);
        }
    }
}
