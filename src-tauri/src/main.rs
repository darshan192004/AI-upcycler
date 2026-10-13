// Prevents additional console window on Windows in release, DO NOT REMOVE!!
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use ai_upcycler_core::discovery::mdns::NodeRegistry;
use ai_upcycler_core::rpc::engine::{ExecutionResult, RpcPipelineEngine, TensorPayload};
use ai_upcycler_core::scheduler::planner::{LayerAssignment, ModelSpec, PartitioningPlan};
use std::sync::Arc;
use tracing::info;

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

fn main() {
    tracing_subscriber::fmt::init();
    info!("Starting AI Upcycler Master Orchestrator Core...");

    let registry = Arc::new(NodeRegistry::new());

    // Start mDNS discovery; keep the daemon alive for the lifetime of the app.
    let _mdns_daemon = ai_upcycler_core::discovery::mdns::spawn_mdns_browser(registry.clone())
        .map_err(|e| tracing::warn!("mDNS discovery unavailable: {e}"))
        .ok();

    // Bridge USB-attached Android workers on a background task.
    let adb_registry = registry.clone();
    tauri::async_runtime::spawn(async move {
        ai_upcycler_core::adb::AdbTunnelManager::start(adb_registry).await;
    });

    tauri::Builder::default()
        .manage(registry)
        .invoke_handler(tauri::generate_handler![
            get_cluster_status,
            get_discovered_nodes,
            compute_layer_plan,
            dispatch_rpc_step
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
