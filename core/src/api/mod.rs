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

use crate::api::backend::{GenerationBackend, StubBackend};
use crate::api::downloads::{HfFetcher, ModelFetcher};
use crate::discovery::mdns::NodeRegistry;
use crate::security::SecurityState;
use crate::store::Store;
use crate::telemetry::TelemetryBus;
use axum::routing::{get, post};
use axum::Router;
use std::collections::HashSet;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;
use tower_http::cors::CorsLayer;
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
            security: Arc::new(SecurityState::default()),
        }
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
        let backend: Arc<dyn GenerationBackend> = Arc::new(distributed::AdaptiveBackend::new(
            registry.clone(),
            store.clone(),
            security.clone(),
            fallback,
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
        .layer(CorsLayer::permissive())
        .with_state(state)
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
        .layer(CorsLayer::permissive())
        .with_state(state)
}

/// Binds `addr` and serves until the future is dropped.
pub async fn serve(addr: SocketAddr, state: AppState) -> std::io::Result<()> {
    let listener = tokio::net::TcpListener::bind(addr).await?;
    tracing::info!("AI Upcycler API listening on http://{addr}");
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
}
