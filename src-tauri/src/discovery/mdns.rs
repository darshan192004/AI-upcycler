use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use tracing::{info, warn};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DiscoveredNode {
    pub node_id: String,
    pub device_name: String,
    pub ip_address: String,
    pub port: u16,
    pub total_ram_bytes: u64,
    pub free_ram_bytes: u64,
    pub connection_type: String, // "USB_ADB" or "WIFI_MDNS"
    pub latency_ms: f32,
    pub last_seen_epoch_ms: u64,
}

#[derive(Clone, Default)]
pub struct NodeRegistry {
    nodes: Arc<Mutex<HashMap<String, DiscoveredNode>>>,
}

impl NodeRegistry {
    pub fn new() -> Self {
        Self {
            nodes: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    pub fn register_or_update(&self, node: DiscoveredNode) {
        let mut map = self.nodes.lock().unwrap();
        info!(
            "mDNS Discovery: Registered node [{}] at {}:{}",
            node.device_name, node.ip_address, node.port
        );
        map.insert(node.node_id.clone(), node);
    }

    pub fn list_nodes(&self) -> Vec<DiscoveredNode> {
        let map = self.nodes.lock().unwrap();
        map.values().cloned().collect()
    }

    pub fn get_node(&self, node_id: &str) -> Option<DiscoveredNode> {
        let map = self.nodes.lock().unwrap();
        map.get(node_id).cloned()
    }
}
