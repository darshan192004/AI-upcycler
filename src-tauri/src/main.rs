// Prevents additional console window on Windows in release, DO NOT REMOVE!!
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use tracing::info;

#[tauri::command]
fn get_cluster_status() -> Result<String, String> {
    Ok("Active - Master Node Ready (8GB Host Budget)".into())
}

fn main() {
    tracing_subscriber::fmt::init();
    info!("Starting AI Upcycler Master Orchestrator Core...");

    tauri::Builder::default()
        .invoke_handler(tauri::generate_handler![get_cluster_status])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
