//! Local HTTP surface for the master node.
//!
//! Exposes the OpenAI-compatible REST API plus cluster management endpoints and
//! a telemetry WebSocket (see `docs/api.md`). Everything is bound to loopback
//! by the desktop shell.

pub mod backend;
pub mod chat;
pub mod cluster;
pub mod distributed;
pub mod downloads;
mod error;
pub mod llama;
pub mod models;
pub mod pairing;
pub mod ws;

pub use error::{ApiError, ApiResult};

use crate::adb::PairingTunnelRegistry;
use crate::api::backend::{GenerationBackend, StubBackend};
use crate::api::downloads::{HfFetcher, ModelFetcher};
use crate::discovery::mdns::NodeRegistry;
use crate::security::SecurityState;
use crate::store::Store;
use crate::telemetry::TelemetryBus;
use axum::http::{header, HeaderValue, Method};
use axum::routing::{get, post};
use axum::Router;
use std::collections::HashSet;
use std::ffi::OsString;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;
use tower_http::cors::{AllowOrigin, CorsLayer};
use tower_http::trace::TraceLayer;

/// Shared, cheaply-cloneable server state handed to every handler.
#[derive(Clone)]
pub struct AppState {
    pub cluster_id: String,
    pub registry: Arc<NodeRegistry>,
    pub store: Arc<Store>,
    pub backend: Arc<dyn GenerationBackend>,
    pub fetcher: Arc<dyn ModelFetcher>,
    pub telemetry: TelemetryBus,
    pub host: HostInfo,
    /// Destination directory for GGUF downloads.
    pub models_dir: PathBuf,
    /// File names with a download currently in flight (guards against two
    /// runners writing the same path).
    pub active_downloads: Arc<tokio::sync::Mutex<HashSet<String>>>,
    /// Ephemeral CA + QR/PIN pairing state for mTLS worker enrollment.
    pub security: Arc<SecurityState>,
    /// Devices holding a pairing reverse tunnel, keyed to the session that
    /// tunnel exists for (spec 2026-10-20, D-B8 cleanup).
    pub pairing_tunnels: Arc<PairingTunnelRegistry>,
    /// Optional override for how the master dials each node — the benchmark
    /// probe and the health poller share it, so the "enrolled → mTLS, else
    /// plaintext" rule cannot drift between them (spec 2026-10-22, D-E4).
    /// `None` (production) derives enrollment-aware transports from
    /// `security` at call time, so a `with_security` swap can never leave the
    /// dials holding a stale CA. Tests set an explicit source for loopback
    /// mock workers (spec 2026-10-21, D-C3).
    pub worker_transports: Option<Arc<dyn crate::rpc::engine::WorkerTransportSource>>,
    /// The freshest worker health and master RAM reading (spec 2026-10-22,
    /// D-E2). Written by the poll loop, read by the chat path and the
    /// dashboard.
    pub telemetry_cache: Arc<crate::worker_health::TelemetryCache>,
    /// How the master reads its own memory (spec 2026-10-22, D-E6).
    pub host_memory: Arc<dyn crate::worker_health::HostMemorySource>,
}

/// Master-host facts that cannot be discovered over mDNS.
#[derive(Debug, Clone)]
pub struct HostInfo {
    pub node_id: String,
    pub device_name: String,
    pub total_ram_mb: u64,
    pub cpu_cores: u32,
}

impl Default for HostInfo {
    fn default() -> Self {
        Self {
            node_id: "master-host-pc".into(),
            device_name: host_name(),
            total_ram_mb: 8192,
            cpu_cores: std::thread::available_parallelism()
                .map(|n| n.get() as u32)
                .unwrap_or(1),
        }
    }
}

impl AppState {
    pub fn new(
        cluster_id: String,
        registry: Arc<NodeRegistry>,
        store: Arc<Store>,
        backend: Arc<dyn GenerationBackend>,
        telemetry: TelemetryBus,
        host: HostInfo,
    ) -> Self {
        let security = Arc::new(SecurityState::default());
        Self {
            cluster_id,
            registry,
            store,
            backend,
            fetcher: Arc::new(HfFetcher::default()),
            telemetry,
            host,
            models_dir: default_models_dir(),
            active_downloads: Arc::new(tokio::sync::Mutex::new(HashSet::new())),
            security,
            pairing_tunnels: Arc::new(PairingTunnelRegistry::default()),
            worker_transports: None,
            telemetry_cache: Arc::new(crate::worker_health::TelemetryCache::new()),
            host_memory: Arc::new(crate::worker_health::SysinfoHostMemory),
        }
    }

    /// The transport to use for each node: the test override when one is set,
    /// otherwise the enrollment-aware rule (mTLS once paired, plaintext until
    /// then — spec 2026-10-21, D-C3). Resolved at call time so a
    /// `with_security` swap is always honoured.
    pub fn transports(&self) -> Arc<dyn crate::rpc::engine::WorkerTransportSource> {
        self.worker_transports.clone().unwrap_or_else(|| {
            Arc::new(crate::security::EnrollmentAwareTransports::new(
                self.security.clone(),
            ))
        })
    }

    /// Overrides how nodes are dialed (tests inject a plaintext loopback
    /// source; spec 2026-10-21, D-C3 / spec 2026-10-22, D-E4).
    pub fn with_worker_transports(
        mut self,
        transports: Arc<dyn crate::rpc::engine::WorkerTransportSource>,
    ) -> Self {
        self.worker_transports = Some(transports);
        self
    }

    /// Overrides how the master reads its own memory (tests inject a fixed
    /// reading; spec 2026-10-22, D-E6).
    pub fn with_host_memory(
        mut self,
        host_memory: Arc<dyn crate::worker_health::HostMemorySource>,
    ) -> Self {
        self.host_memory = host_memory;
        self
    }

    /// Overrides the telemetry cache the poll loop writes into (tests inject
    /// their own so they can inspect it; spec 2026-10-22, D-E2).
    pub fn with_telemetry_cache(
        mut self,
        cache: Arc<crate::worker_health::TelemetryCache>,
    ) -> Self {
        self.telemetry_cache = cache;
        self
    }

    /// Overrides the GGUF fetcher (tests inject [`downloads::StubFetcher`]).
    pub fn with_fetcher(mut self, fetcher: Arc<dyn ModelFetcher>) -> Self {
        self.fetcher = fetcher;
        self
    }

    /// Overrides the security state (bootstrap shares one CA/enrollment
    /// registry between [`AppState`] and the adaptive chat backend).
    pub fn with_security(mut self, security: Arc<SecurityState>) -> Self {
        self.security = security;
        self
    }

    /// Overrides the GGUF download directory.
    pub fn with_models_dir(mut self, models_dir: PathBuf) -> Self {
        self.models_dir = models_dir;
        self
    }

    /// Standard wiring for the desktop shell: real registry/store/telemetry.
    ///
    /// Uses the real llama.cpp engine when [`llama::LlamaCppBackend::discover`]
    /// finds a server binary + GGUF; otherwise falls back to the clearly
    /// labelled [`StubBackend`] placeholder.
    pub fn bootstrap(
        cluster_id: String,
        registry: Arc<NodeRegistry>,
        store: Arc<Store>,
        telemetry: TelemetryBus,
        models_dir: PathBuf,
    ) -> Self {
        let fallback: Arc<dyn GenerationBackend> =
            match llama::LlamaCppBackend::discover(&models_dir) {
                Some(engine) => {
                    tracing::info!("fallback generation backend: {}", engine.describe());
                    Arc::new(engine)
                }
                None => {
                    tracing::warn!(
                        "llama.cpp engine not found (set AI_UPCYCLER_LLAMA_SERVER and \
                         AI_UPCYCLER_MODEL, or place llama-server + {model} under the data \
                         dir); chat will use the stub backend",
                        model = llama::DEFAULT_MODEL_FILE,
                    );
                    Arc::new(StubBackend::new())
                }
            };
        // Cluster-first chat serving (design D1): the pairing-registry-backed
        // adaptive backend serves through LoadedPipeline when enrolled workers
        // exist; llama-server stays the fallback of last resort.
        let security = Arc::new(SecurityState::default());
        // One cache for the worker health the poll loop writes and the
        // planner reads (spec 2026-10-22, D-F1): a hot worker gets a smaller
        // share of the model.
        let telemetry_cache = Arc::new(crate::worker_health::TelemetryCache::new());
        let backend: Arc<dyn GenerationBackend> = Arc::new(distributed::AdaptiveBackend::new(
            registry.clone(),
            store.clone(),
            security.clone(),
            fallback,
            telemetry_cache.clone(),
        ));
        Self::new(
            cluster_id,
            registry,
            store,
            backend,
            telemetry,
            HostInfo::default(),
        )
        .with_security(security)
        .with_telemetry_cache(telemetry_cache)
        .with_models_dir(models_dir)
    }
}

/// Download destination when no explicit directory was configured: `models/`
/// under the current working directory.
fn default_models_dir() -> PathBuf {
    std::env::current_dir()
        .map(|dir| dir.join("models"))
        .unwrap_or_else(|_| PathBuf::from("models"))
}

/// Whether a browser may use the loopback API (spec 2026-10-24, D-G1).
///
/// The bundled desktop shell is served from a Tauri custom protocol, and
/// `tauri dev` from a localhost port; every other origin is a web page the user
/// happens to have open and gets no CORS grant. Matching is on the parsed host,
/// never a prefix or suffix, so `http://localhost.evil.com` is not localhost and
/// `http://tauri.localhost.evil.com` is not the shell.
pub(crate) fn browser_origin_is_allowed(origin: &str) -> bool {
    let origin = origin.trim();
    if origin.eq_ignore_ascii_case("tauri://localhost") {
        return true;
    }
    let Some((scheme, rest)) = origin.split_once("://") else {
        return false;
    };
    if !scheme.eq_ignore_ascii_case("http") && !scheme.eq_ignore_ascii_case("https") {
        return false;
    }

    let authority = rest.split(['/', '?', '#']).next().unwrap_or_default();
    if authority.is_empty() || authority.contains('@') {
        return false;
    }

    // An IPv6 literal is bracketed; keep it whole so the port split below does
    // not trip over the colons inside the address.
    let host = if let Some(after_bracket) = authority.strip_prefix('[') {
        match after_bracket.split_once(']') {
            Some((address, _)) => format!("[{address}]"),
            None => return false,
        }
    } else {
        authority.split(':').next().unwrap_or_default().to_string()
    };

    matches!(
        host.to_ascii_lowercase().as_str(),
        "localhost" | "127.0.0.1" | "[::1]" | "tauri.localhost"
    )
}

/// The CORS policy shared by the main and pairing listeners (spec 2026-10-24,
/// D-G2). Refused origins get no `Access-Control-Allow-Origin` at all, which is
/// what makes the browser drop the response.
fn cors_layer() -> CorsLayer {
    CorsLayer::new()
        .allow_origin(AllowOrigin::predicate(|origin: &HeaderValue, _| {
            origin
                .to_str()
                .map(browser_origin_is_allowed)
                .unwrap_or(false)
        }))
        .allow_methods([Method::GET, Method::POST])
        .allow_headers([header::CONTENT_TYPE])
        // Some Chromium builds send a Private Network Access preflight when a
        // page asks for a loopback address; grant it only to allowed origins.
        .allow_private_network(true)
}

/// Builds the application router (used by the server and by integration tests).
pub fn router(state: AppState) -> Router {
    Router::new()
        .route("/v1/models", get(models::list_models))
        .route("/v1/chat/completions", post(chat::chat_completions))
        .route("/api/cluster/topology", get(cluster::topology))
        .route("/api/cluster/benchmark", post(cluster::benchmark))
        .route("/api/models/download", post(downloads::download))
        .route("/api/pairing/session", post(pairing::create_session))
        .route("/api/pairing/enroll", post(pairing::enroll))
        .route("/api/telemetry", get(ws::handler))
        .layer(TraceLayer::new_for_http())
        .layer(cors_layer())
        .with_state(state)
}

/// Overrides the GGUF download directory (`AI_UPCYCLER_MODELS_DIR`).
///
/// An unset or empty variable keeps the shell's default (the app-data
/// `models/` directory). Models are large and the system drive is often the
/// smallest one, so hosts can point this at another volume.
pub fn resolve_models_dir(default: PathBuf) -> PathBuf {
    resolve_models_dir_with(default, std::env::var_os("AI_UPCYCLER_MODELS_DIR"))
}

fn resolve_models_dir_with(default: PathBuf, override_: Option<OsString>) -> PathBuf {
    match override_ {
        Some(value) if !value.is_empty() => PathBuf::from(value),
        _ => default,
    }
}

/// Overrides the loopback API port (`AI_UPCYCLER_API_PORT`).
///
/// The desktop shell's default is 8080. A host that already has a process on
/// that port (another local model server, for example) can move this one.
pub fn resolve_api_port(default: u16) -> u16 {
    let value = std::env::var("AI_UPCYCLER_API_PORT").ok();
    parse_api_port(value.as_deref(), default)
}

fn parse_api_port(value: Option<&str>, default: u16) -> u16 {
    match value.map(str::trim) {
        Some(text) if !text.is_empty() => match text.parse::<u16>() {
            Ok(port) if port > 0 => port,
            _ => {
                tracing::warn!("ignoring invalid AI_UPCYCLER_API_PORT={text:?}; using {default}");
                default
            }
        },
        _ => default,
    }
}

/// Loopback port for the pairing-only listener (spec 2026-10-20, D-B2): one
/// route, reachable from a phone through `adb reverse` only.
pub const PAIRING_PORT: u16 = 8081;

/// Builds the pairing-only router: exactly `POST /api/pairing/enroll`, no
/// session minting.
///
/// A phone holding only the ADB reverse tunnel can redeem a single-use PIN
/// and nothing else — session creation stays on the main loopback API so a
/// device on the far side of the tunnel can never mint its own pairing
/// session (spec 2026-10-20, D-B8).
pub fn pairing_router(state: AppState) -> Router {
    Router::new()
        .route("/api/pairing/enroll", post(pairing::enroll))
        .layer(TraceLayer::new_for_http())
        .layer(cors_layer())
        .with_state(state)
}

/// Binds `addr` and serves until the future is dropped.
pub async fn serve(addr: SocketAddr, state: AppState) -> std::io::Result<()> {
    let listener = tokio::net::TcpListener::bind(addr).await?;
    tracing::info!("AI Upcycler API listening on http://{addr}");

    // Worker health polling (spec 2026-10-22, D-E1): one background task
    // asks every discovered worker how it is doing, so the dashboard shows
    // measured battery/temperature instead of placeholders. It lives here
    // rather than in the desktop shell so every embedder of the core API gets
    // live telemetry.
    tokio::spawn(crate::worker_health::worker_health_poll(state.clone()));

    axum::serve(listener, router(state)).await
}

/// Binds the pairing-only listener ([`pairing_router`]) and serves until the
/// future is dropped. Bind address stays loopback: the phone reaches it
/// through `adb reverse`, never over the LAN.
pub async fn serve_pairing(addr: SocketAddr, state: AppState) -> std::io::Result<()> {
    let listener = tokio::net::TcpListener::bind(addr).await?;
    tracing::info!("AI Upcycler pairing listener on http://{addr}");
    axum::serve(listener, pairing_router(state)).await
}

fn host_name() -> String {
    std::env::var("HOSTNAME")
        .or_else(|_| std::env::var("COMPUTERNAME"))
        .unwrap_or_else(|_| "master-host-pc".into())
}

/// Converts a SQLite `CURRENT_TIMESTAMP` (`YYYY-MM-DD HH:MM:SS`, UTC) to a Unix
/// epoch. Returns `None` for anything that does not match the expected shape.
pub(crate) fn sqlite_timestamp_to_epoch(ts: &str) -> Option<u64> {
    let (date, time) = ts.split_once(' ')?;
    let mut date_parts = date.split('-');
    let year: i64 = date_parts.next()?.parse().ok()?;
    let month: i64 = date_parts.next()?.parse().ok()?;
    let day: i64 = date_parts.next()?.parse().ok()?;

    let mut time_parts = time.split(':');
    let hour: i64 = time_parts.next()?.parse().ok()?;
    let minute: i64 = time_parts.next()?.parse().ok()?;
    let second: i64 = time_parts.next()?.parse().ok()?;

    let days = days_from_civil(year, month, day)?;
    let secs = days * 86_400 + hour * 3_600 + minute * 60 + second;
    u64::try_from(secs).ok()
}

/// Days since the Unix epoch for a proleptic Gregorian date (Howard Hinnant's
/// `days_from_civil`).
fn days_from_civil(year: i64, month: i64, day: i64) -> Option<i64> {
    if !(1..=12).contains(&month) || !(1..=31).contains(&day) {
        return None;
    }
    let y = if month <= 2 { year - 1 } else { year };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let doy = (153 * (if month > 2 { month - 3 } else { month + 9 }) + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    Some(era * 146_097 + doe - 719_468)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::OsString;

    #[test]
    fn parses_sqlite_timestamps() {
        assert_eq!(sqlite_timestamp_to_epoch("1970-01-01 00:00:00"), Some(0));
        assert_eq!(
            sqlite_timestamp_to_epoch("1970-01-02 00:00:00"),
            Some(86_400)
        );
        // 2025-01-01 00:00:00 UTC
        assert_eq!(
            sqlite_timestamp_to_epoch("2025-01-01 00:00:00"),
            Some(1_735_689_600)
        );
    }

    #[test]
    fn rejects_malformed_timestamps() {
        assert_eq!(sqlite_timestamp_to_epoch("nonsense"), None);
        assert_eq!(sqlite_timestamp_to_epoch("2025-13-01 00:00:00"), None);
    }

    #[test]
    fn accepts_the_shell_and_localhost_origins() {
        for origin in [
            "tauri://localhost",
            "http://tauri.localhost",
            "https://tauri.localhost",
            "http://localhost:1420",
            "https://localhost",
            "http://127.0.0.1:8080",
            "http://[::1]:1420",
        ] {
            assert!(
                browser_origin_is_allowed(origin),
                "{origin} is a shell or localhost origin and must be allowed"
            );
        }
    }

    #[test]
    fn refuses_web_pages_and_lookalike_hosts() {
        for origin in [
            "https://evil.example",
            "https://localtest.me",
            "http://localhost.evil.com",
            "http://tauri.localhost.evil.com",
            "http://127.0.0.1.evil.com",
            "http://localhost@evil.example",
            "tauri://evil",
            "chrome-extension://abcdefghijklmnop",
            "null",
            "file://",
        ] {
            assert!(
                !browser_origin_is_allowed(origin),
                "{origin} must not receive a CORS grant"
            );
        }
    }

    #[test]
    fn models_dir_override_wins_when_non_empty() {
        let default = PathBuf::from("C:/app/models");
        let override_ = PathBuf::from("D:/ai-upcycler-models");

        assert_eq!(
            resolve_models_dir_with(default, Some(override_.into_os_string())),
            PathBuf::from("D:/ai-upcycler-models")
        );
    }

    #[test]
    fn models_dir_empty_override_keeps_default() {
        let default = PathBuf::from("C:/app/models");

        assert_eq!(
            resolve_models_dir_with(default.clone(), Some(OsString::from(""))),
            default
        );
        assert_eq!(resolve_models_dir_with(default.clone(), None), default);
    }

    #[test]
    fn api_port_override_parses_a_valid_port() {
        assert_eq!(parse_api_port(Some("8090"), 8080), 8090);
        assert_eq!(parse_api_port(Some(" 8090 "), 8080), 8090);
    }

    #[test]
    fn api_port_invalid_zero_or_out_of_range_keeps_default() {
        for value in [
            Some("0"),
            Some("65536"),
            Some("-1"),
            Some("abc"),
            Some(""),
            None,
        ] {
            assert_eq!(parse_api_port(value, 8080), 8080, "{value:?}");
        }
    }
}
