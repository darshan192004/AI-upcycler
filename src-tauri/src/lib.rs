use ai_upcycler_core::api::pairing::PairingSessionResponse;
use ai_upcycler_core::api::{self, AppState};
use ai_upcycler_core::discovery::mdns::NodeRegistry;
use ai_upcycler_core::rpc::engine::{ExecutionResult, RpcPipelineEngine, TensorPayload};
use ai_upcycler_core::scheduler::planner::{LayerAssignment, ModelSpec, PartitioningPlan};
use ai_upcycler_core::store::Store;
use ai_upcycler_core::telemetry::TelemetryBus;
use std::net::SocketAddr;
use std::sync::Arc;
use tauri::Manager;
use tracing::{error, info};

/// Loopback port for the local REST/WebSocket API.
const API_PORT: u16 = 8080;

#[tauri::command]
fn get_cluster_status() -> Result<String, String> {
    Ok("Active - Master Node Ready (8GB Host Budget)".into())
}

#[tauri::command]
fn get_discovered_nodes(
    registry: tauri::State<'_, Arc<NodeRegistry>>,
) -> Result<Vec<ai_upcycler_core::discovery::mdns::DiscoveredNode>, String> {
    Ok(registry.list_nodes())
}

#[tauri::command]
fn compute_layer_plan(
    model: ModelSpec,
    registry: tauri::State<'_, Arc<NodeRegistry>>,
    state: tauri::State<'_, AppState>,
) -> Result<PartitioningPlan, String> {
    let nodes = registry.list_nodes();
    // The plan is thermal-aware: a worker that is already hot gets a smaller
    // share so it can rest (spec 2026-10-22, D-F1/D-F2).
    let thermal = ai_upcycler_core::scheduler::planner::ThermalHints::from_temperatures(
        state.telemetry_cache.temperatures(),
    );
    Ok(ai_upcycler_core::scheduler::planner::LayerScheduler::compute_plan(
        &model, &nodes, &thermal,
    ))
}

#[tauri::command]
async fn dispatch_rpc_step(
    assignment: LayerAssignment,
    payload: TensorPayload,
) -> Result<ExecutionResult, String> {
    RpcPipelineEngine::dispatch_forward_step(&assignment, payload).await
}

/// Prepares one device for pairing (spec 2026-10-20, D-B8): mints a
/// single-use session from the shell's shared pairing state, bridges the
/// pairing-only listener to the device with `adb reverse`, tracks the tunnel
/// against that session so the cleanup task can retire it, and returns what
/// the dashboard panel renders — `{pin, qr_payload, fingerprint}`.
///
/// The command is async and pushes the `adb` subprocess onto a blocking
/// thread: Tauri runs synchronous commands on the main thread, and `adb`
/// blocks while its server starts.
#[tauri::command]
async fn prepare_pairing(
    serial: String,
    state: tauri::State<'_, AppState>,
) -> Result<PairingSessionResponse, String> {
    let fingerprint = state.security.ca.fingerprint().to_string();
    let (pin, qr_payload, expires_in_secs) = state.security.pairing.create_session(&fingerprint);

    let device = serial.clone();
    tauri::async_runtime::spawn_blocking(move || {
        ai_upcycler_core::adb::AdbTunnelManager::new().reverse_pairing(&device)
    })
    .await
    .map_err(|e| format!("adb tunnel task failed: {e}"))??;

    state.pairing_tunnels.track(&serial, &pin);
    Ok(PairingSessionResponse {
        pin,
        qr_payload,
        expires_in_secs,
        fingerprint,
    })
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tracing_subscriber::fmt::init();
    info!("Starting AI Upcycler Master Orchestrator Core...");

    let telemetry = TelemetryBus::default();
    let registry = Arc::new(NodeRegistry::with_events(telemetry.clone()));

    // Start mDNS discovery; keep the daemon alive for the lifetime of the app.
    let _mdns_daemon = ai_upcycler_core::discovery::mdns::spawn_mdns_browser(registry.clone())
        .map_err(|e| tracing::warn!("mDNS discovery unavailable: {e}"))
        .ok();

    // Bridge USB-attached Android workers on a background task. The task stays
    // alive after bootstrap so it can remove `adb forward` tunnels on shutdown.
    let (adb_shutdown_tx, adb_shutdown_rx) = tokio::sync::watch::channel(false);
    let adb_registry = registry.clone();
    tauri::async_runtime::spawn(async move {
        ai_upcycler_core::adb::AdbTunnelManager::start(adb_registry, adb_shutdown_rx).await;
    });

    let server_registry = registry.clone();
    let app = tauri::Builder::default()
        .manage(registry)
        .setup(move |app| {
            let data_dir = app
                .path()
                .app_data_dir()
                .map_err(|e| format!("cannot resolve app data dir: {e}"))?;
            std::fs::create_dir_all(&data_dir)
                .map_err(|e| format!("cannot create app data dir: {e}"))?;

            let store = Arc::new(
                Store::open(data_dir.join("ai-upcycler.db"))
                    .map_err(|e| format!("cannot open database: {e}"))?,
            );

            let state = AppState::bootstrap(
                "cluster-local".into(),
                server_registry.clone(),
                store,
                telemetry.clone(),
                data_dir.join("models"),
            );

            // `prepare_pairing` resolves this shared state; its Arcs are the
            // same instances the API listeners below serve from.
            app.manage(state.clone());

            let state_for_api = state.clone();
            tauri::async_runtime::spawn(async move {
                let addr = SocketAddr::from(([127, 0, 0, 1], API_PORT));
                if let Err(e) = api::serve(addr, state_for_api).await {
                    error!("local API server stopped: {e}");
                }
            });

            // Pairing tunnels die with their session (spec 2026-10-20, D-B8):
            // a poll retires the `adb reverse` tunnel once its session has
            // been redeemed or has expired.
            tauri::async_runtime::spawn(ai_upcycler_core::adb::pairing_tunnel_cleanup(
                state.security.clone(),
                state.pairing_tunnels.clone(),
            ));

            // Pairing-only listener (spec 2026-10-20, D-B2): one enroll route
            // on loopback, reached by a phone through `adb reverse` — never
            // bound beyond loopback itself.
            tauri::async_runtime::spawn(async move {
                let addr = SocketAddr::from(([127, 0, 0, 1], api::PAIRING_PORT));
                if let Err(e) = api::serve_pairing(addr, state).await {
                    error!("pairing listener stopped: {e}");
                }
            });

            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            get_cluster_status,
            get_discovered_nodes,
            compute_layer_plan,
            dispatch_rpc_step,
            prepare_pairing
        ])
        .build(tauri::generate_context!())
        .expect("error while running tauri application");

    app.run(move |_app, event| {
        if let tauri::RunEvent::ExitRequested { .. } = event {
            let _ = adb_shutdown_tx.send(true);
            // Brief grace window so the ADB task can remove its port forwards
            // before the runtime tears down.
            std::thread::sleep(std::time::Duration::from_millis(300));
        }
    });
}
