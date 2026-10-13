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
) -> Result<PartitioningPlan, String> {
    let nodes = registry.list_nodes();
    Ok(ai_upcycler_core::scheduler::planner::LayerScheduler::compute_plan(&model, &nodes))
}

#[tauri::command]
async fn dispatch_rpc_step(
    assignment: LayerAssignment,
    payload: TensorPayload,
) -> Result<ExecutionResult, String> {
    RpcPipelineEngine::dispatch_forward_step(&assignment, payload).await
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

    // Bridge USB-attached Android workers on a background task.
    let adb_registry = registry.clone();
    tauri::async_runtime::spawn(async move {
        ai_upcycler_core::adb::AdbTunnelManager::start(adb_registry).await;
    });

    let server_registry = registry.clone();
    tauri::Builder::default()
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
            );

            tauri::async_runtime::spawn(async move {
                let addr = SocketAddr::from(([127, 0, 0, 1], API_PORT));
                if let Err(e) = api::serve(addr, state).await {
                    error!("local API server stopped: {e}");
                }
            });

            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            get_cluster_status,
            get_discovered_nodes,
            compute_layer_plan,
            dispatch_rpc_step
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
