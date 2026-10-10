// Prevents additional console window on Windows in release, DO NOT REMOVE!!
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod discovery;
mod rpc;
mod scheduler;

use discovery::mdns::{DiscoveredNode, NodeRegistry};
use rpc::engine::{ExecutionResult, RpcPipelineEngine, TensorPayload};
use scheduler::planner::{LayerAssignment, LayerScheduler, ModelSpec, PartitioningPlan};
use std::sync::Arc;
use tracing::info;

#[tauri::command]
fn get_cluster_status() -> Result<String, String> {
    Ok("Active - Master Node Ready (8GB Host Budget)".into())
}

#[tauri::command]
fn get_discovered_nodes(
    registry: tauri::State<'_, Arc<NodeRegistry>>,
) -> Result<Vec<DiscoveredNode>, String> {
    Ok(registry.list_nodes())
}

#[tauri::command]
fn compute_layer_plan(
    model: ModelSpec,
    registry: tauri::State<'_, Arc<NodeRegistry>>,
) -> Result<PartitioningPlan, String> {
    let nodes = registry.list_nodes();
    Ok(LayerScheduler::compute_plan(&model, &nodes))
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

    // Seed mock nodes for initial desktop GUI state
    registry.register_or_update(DiscoveredNode {
        node_id: "master-pc-01".into(),
        device_name: "Laptop Host (Master)".into(),
        ip_address: "127.0.0.1".into(),
        port: 50051,
        total_ram_bytes: 8 * 1024 * 1024 * 1024,
        free_ram_bytes: 5 * 1024 * 1024 * 1024,
        connection_type: "LOCAL".into(),
        latency_ms: 0.1,
        last_seen_epoch_ms: 1700000000,
    });

    registry.register_or_update(DiscoveredNode {
        node_id: "android-phone-01".into(),
        device_name: "Galaxy S10 (USB Phone 1)".into(),
        ip_address: "127.0.0.1".into(),
        port: 50052,
        total_ram_bytes: 4 * 1024 * 1024 * 1024,
        free_ram_bytes: 3.8 * 1024 * 1024 * 1024 as u64,
        connection_type: "USB_ADB".into(),
        latency_ms: 1.2,
        last_seen_epoch_ms: 1700000000,
    });

    registry.register_or_update(DiscoveredNode {
        node_id: "android-phone-02".into(),
        device_name: "Pixel 4a (Wi-Fi Phone 2)".into(),
        ip_address: "192.168.1.145".into(),
        port: 50051,
        total_ram_bytes: 4 * 1024 * 1024 * 1024,
        free_ram_bytes: 3.6 * 1024 * 1024 * 1024 as u64,
        connection_type: "WIFI_MDNS".into(),
        latency_ms: 8.4,
        last_seen_epoch_ms: 1700000000,
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
