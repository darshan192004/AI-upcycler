use crate::telemetry::{NodeEventData, TelemetryBus, TelemetryEvent};
use mdns_sd::{ServiceDaemon, ServiceEvent, ServiceInfo};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};
use tracing::{info, warn};

/// Service type advertised by AI Upcycler worker nodes over mDNS-SD.
pub const SERVICE_TYPE: &str = "_aiupcycler-worker._tcp.local.";

/// The `node_id` the master assigns to a worker advertising `instance_name`
/// over DNS-SD: the instance fullname, matching
/// [`DiscoveredNode::from_service_info`]'s choice of `info.get_fullname()`.
///
/// The Android worker composes the same string from its NSD-registered
/// instance name before enrolling, so `EnrolledWorkerSource` finds its
/// credentials under the id discovery produces (spec 2026-10-20, D-B6). The
/// exact format is pinned by golden on both sides — it is a cross-language
/// contract, not an implementation detail.
pub fn worker_node_id(instance_name: &str) -> String {
    format!("{instance_name}.{SERVICE_TYPE}")
}

#[allow(dead_code)]
const TXT_PROTOCOL_VERSION: &str = "protocol_version";
const TXT_TOTAL_RAM_BYTES: &str = "total_ram_bytes";
const TXT_FREE_RAM_BYTES: &str = "free_ram_bytes";

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

impl DiscoveredNode {
    /// Projects an mDNS resolved service record onto a cluster node.
    ///
    /// The instance fullname is used as the stable `node_id`; RAM figures are
    /// read from the TXT records published alongside the service (missing or
    /// malformed values degrade to `0`).
    pub fn from_service_info(info: &ServiceInfo) -> Option<Self> {
        let ip_address = preferred_ip(info)?;
        Some(Self {
            node_id: info.get_fullname().to_string(),
            device_name: info.get_hostname().trim_end_matches('.').to_string(),
            ip_address,
            port: info.get_port(),
            total_ram_bytes: txt_u64(info, TXT_TOTAL_RAM_BYTES),
            free_ram_bytes: txt_u64(info, TXT_FREE_RAM_BYTES),
            connection_type: "WIFI_MDNS".to_string(),
            latency_ms: 0.0,
            last_seen_epoch_ms: now_epoch_ms(),
        })
    }
}

#[derive(Clone, Default)]
pub struct NodeRegistry {
    nodes: Arc<Mutex<HashMap<String, DiscoveredNode>>>,
    /// Optional telemetry fan-out; `None` keeps the registry headless (pure
    /// state container). Attached via [`NodeRegistry::with_events`].
    events: Option<TelemetryBus>,
    /// Whether the mDNS browser daemon is currently running. Surfaced on the
    /// topology endpoint so the UI can say discovery is offline instead of
    /// silently showing an empty cluster.
    mdns_active: Arc<AtomicBool>,
}

impl NodeRegistry {
    pub fn new() -> Self {
        Self {
            nodes: Arc::new(Mutex::new(HashMap::new())),
            events: None,
            mdns_active: Arc::new(AtomicBool::new(false)),
        }
    }

    /// Creates a registry that publishes connect/disconnect events onto `bus`.
    pub fn with_events(bus: TelemetryBus) -> Self {
        Self {
            nodes: Arc::new(Mutex::new(HashMap::new())),
            events: Some(bus),
            mdns_active: Arc::new(AtomicBool::new(false)),
        }
    }

    /// Marks the mDNS browser as running (`true`) or stopped (`false`).
    pub fn set_mdns_active(&self, active: bool) {
        self.mdns_active.store(active, Ordering::Relaxed);
    }

    /// Whether the mDNS browser daemon is currently running.
    pub fn mdns_active(&self) -> bool {
        self.mdns_active.load(Ordering::Relaxed)
    }

    pub fn register_or_update(&self, node: DiscoveredNode) {
        {
            let mut map = self.nodes.lock().unwrap();
            info!(
                "mDNS Discovery: Registered node [{}] at {}:{}",
                node.device_name, node.ip_address, node.port
            );
            map.insert(node.node_id.clone(), node.clone());
        }
        self.publish_node_event(&node, true);
    }

    /// Inserts or replaces a node, publishing a connect event only when the
    /// node is new.
    ///
    /// The ADB poll refreshes a device's live capacity every few seconds; a
    /// "connected" event per cycle would make the dashboard refetch for no
    /// reason. The first sighting still announces the node.
    pub fn upsert(&self, node: DiscoveredNode) {
        let is_new = {
            let mut map = self.nodes.lock().unwrap();
            let is_new = !map.contains_key(&node.node_id);
            map.insert(node.node_id.clone(), node.clone());
            is_new
        };
        if is_new {
            self.publish_node_event(&node, true);
        }
    }

    /// Removes a node that has left the cluster (e.g. an mDNS `ServiceRemoved`
    /// event). Returns the removed entry if it was known.
    pub fn remove(&self, node_id: &str) -> Option<DiscoveredNode> {
        let removed = {
            let mut map = self.nodes.lock().unwrap();
            map.remove(node_id)
        };
        if let Some(node) = &removed {
            warn!(
                "mDNS Discovery: node [{}] at {}:{} left the cluster",
                node.device_name, node.ip_address, node.port
            );
            self.publish_node_event(node, false);
        }
        removed
    }

    pub fn list_nodes(&self) -> Vec<DiscoveredNode> {
        let map = self.nodes.lock().unwrap();
        map.values().cloned().collect()
    }

    pub fn get_node(&self, node_id: &str) -> Option<DiscoveredNode> {
        let map = self.nodes.lock().unwrap();
        map.get(node_id).cloned()
    }

    fn publish_node_event(&self, node: &DiscoveredNode, connected: bool) {
        let Some(bus) = &self.events else {
            return;
        };
        let data = NodeEventData {
            node_id: node.node_id.clone(),
            device_name: node.device_name.clone(),
            connection_type: node.connection_type.clone(),
            total_ram_bytes: node.total_ram_bytes,
            free_ram_bytes: node.free_ram_bytes,
        };
        let event = if connected {
            TelemetryEvent::node_connected(data)
        } else {
            TelemetryEvent::node_disconnected(data)
        };
        bus.publish(event);
    }
}

/// Starts an mDNS browser that keeps `registry` in sync with the cluster.
///
/// The returned [`ServiceDaemon`] owns the background responder/querier; keep it
/// alive for as long as discovery should run and call [`ServiceDaemon::shutdown`]
/// to stop it.
pub fn spawn_mdns_browser(registry: Arc<NodeRegistry>) -> Result<ServiceDaemon, String> {
    let daemon = ServiceDaemon::new().map_err(|e| format!("failed to start mDNS daemon: {e}"))?;
    let receiver = daemon
        .browse(SERVICE_TYPE)
        .map_err(|e| format!("failed to browse {SERVICE_TYPE}: {e}"))?;

    registry.set_mdns_active(true);
    std::thread::spawn(move || {
        info!("mDNS Discovery: browsing for {SERVICE_TYPE}");
        while let Ok(event) = receiver.recv() {
            match event {
                ServiceEvent::ServiceResolved(info) => {
                    match DiscoveredNode::from_service_info(&info) {
                        Some(node) => registry.register_or_update(node),
                        None => warn!(
                            "mDNS Discovery: '{}' resolved without a usable address; ignoring",
                            info.get_fullname()
                        ),
                    }
                }
                ServiceEvent::ServiceRemoved(_service_type, fullname) => {
                    registry.remove(&fullname);
                }
                ServiceEvent::ServiceFound(_service_type, fullname) => {
                    info!("mDNS Discovery: found '{fullname}', resolving...");
                }
                ServiceEvent::SearchStarted(service_type) => {
                    info!("mDNS Discovery: search started for {service_type}");
                }
                ServiceEvent::SearchStopped(service_type) => {
                    info!("mDNS Discovery: search stopped for {service_type}");
                }
            }
        }
        registry.set_mdns_active(false);
        info!("mDNS Discovery: browser for {SERVICE_TYPE} stopped");
    });

    Ok(daemon)
}

fn preferred_ip(info: &ServiceInfo) -> Option<String> {
    let addresses = info.get_addresses();
    addresses
        .iter()
        .find(|addr| addr.is_ipv4())
        .or_else(|| addresses.iter().next())
        .map(|addr| addr.to_string())
}

fn txt_u64(info: &ServiceInfo, key: &str) -> u64 {
    info.get_property_val_str(key)
        .and_then(|value| value.parse::<u64>().ok())
        .unwrap_or(0)
}

fn now_epoch_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::IpAddr;

    fn sample_service_info() -> ServiceInfo {
        let mut properties = HashMap::new();
        properties.insert(TXT_PROTOCOL_VERSION.to_string(), "1".to_string());
        properties.insert(TXT_TOTAL_RAM_BYTES.to_string(), "8589934592".to_string());
        properties.insert(TXT_FREE_RAM_BYTES.to_string(), "4294967296".to_string());

        let ip: IpAddr = "192.168.1.50".parse().unwrap();
        ServiceInfo::new(
            SERVICE_TYPE,
            "worker-1",
            "worker-1.local.",
            ip,
            50052,
            properties,
        )
        .expect("valid service info")
    }

    #[test]
    fn builds_node_from_resolved_service_info() {
        let info = sample_service_info();

        let node = DiscoveredNode::from_service_info(&info).expect("node built");

        assert_eq!(node.ip_address, "192.168.1.50");
        assert_eq!(node.port, 50052);
        assert_eq!(node.total_ram_bytes, 8_589_934_592);
        assert_eq!(node.free_ram_bytes, 4_294_967_296);
        assert_eq!(node.connection_type, "WIFI_MDNS");
        assert!(node.device_name.starts_with("worker-1"));
    }

    #[test]
    fn worker_node_id_matches_the_discovered_id_format() {
        // Cross-language contract (spec 2026-10-20, D-B6): the string a
        // worker composes from its registered instance name must equal the
        // `node_id` the master's browser assigns — checked against the real
        // mdns-sd fullname, not a hand-copied constant.
        let info = sample_service_info();
        assert_eq!(worker_node_id("worker-1"), info.get_fullname());
        assert_eq!(
            worker_node_id("worker-1"),
            "worker-1._aiupcycler-worker._tcp.local."
        );
    }

    #[test]
    fn missing_txt_records_degrade_to_zero() {
        let ip: IpAddr = "10.0.0.9".parse().unwrap();
        let info = ServiceInfo::new(
            SERVICE_TYPE,
            "worker-2",
            "worker-2.local.",
            ip,
            50052,
            HashMap::<String, String>::new(),
        )
        .expect("valid service info");

        let node = DiscoveredNode::from_service_info(&info).expect("node built");
        assert_eq!(node.total_ram_bytes, 0);
        assert_eq!(node.free_ram_bytes, 0);
    }

    #[test]
    fn registry_tracks_mdns_browser_status() {
        let registry = NodeRegistry::new();
        assert!(!registry.mdns_active(), "no browser has been spawned");

        registry.set_mdns_active(true);
        assert!(registry.mdns_active());
        assert!(
            registry.clone().mdns_active(),
            "status is shared across clones"
        );

        registry.set_mdns_active(false);
        assert!(!registry.mdns_active());
    }

    #[test]
    fn registry_add_update_and_remove() {
        let registry = NodeRegistry::new();
        let info = sample_service_info();
        let node = DiscoveredNode::from_service_info(&info).expect("node built");
        let node_id = node.node_id.clone();

        registry.register_or_update(node.clone());
        assert_eq!(registry.list_nodes().len(), 1);
        assert_eq!(registry.get_node(&node_id).unwrap().port, 50052);

        let mut updated = node;
        updated.port = 60000;
        registry.register_or_update(updated);
        assert_eq!(registry.list_nodes().len(), 1, "update must not duplicate");
        assert_eq!(registry.get_node(&node_id).unwrap().port, 60000);

        assert!(registry.remove(&node_id).is_some());
        assert!(registry.get_node(&node_id).is_none());
        assert!(
            registry.remove(&node_id).is_none(),
            "second remove is a no-op"
        );
    }

    #[test]
    fn publishes_connect_and_disconnect_telemetry() {
        let bus = crate::telemetry::TelemetryBus::new(8);
        let mut rx = bus.subscribe();
        let registry = NodeRegistry::with_events(bus);

        let node = DiscoveredNode::from_service_info(&sample_service_info()).expect("node built");
        let node_id = node.node_id.clone();

        registry.register_or_update(node);
        assert!(matches!(
            rx.try_recv().expect("connect event"),
            crate::telemetry::TelemetryEvent::NodeConnected { .. }
        ));

        registry.remove(&node_id);
        assert!(matches!(
            rx.try_recv().expect("disconnect event"),
            crate::telemetry::TelemetryEvent::NodeDisconnected { .. }
        ));
    }

    #[test]
    fn upsert_announces_only_the_first_sighting() {
        let bus = crate::telemetry::TelemetryBus::new(8);
        let mut rx = bus.subscribe();
        let registry = NodeRegistry::with_events(bus);
        let mut node =
            DiscoveredNode::from_service_info(&sample_service_info()).expect("node built");

        registry.upsert(node.clone());
        assert!(matches!(
            rx.try_recv().expect("first sighting event"),
            crate::telemetry::TelemetryEvent::NodeConnected { .. }
        ));

        node.free_ram_bytes = 1;
        registry.upsert(node);
        assert!(
            rx.try_recv().is_err(),
            "a capacity refresh is not a new connection"
        );
        assert_eq!(
            registry
                .get_node("worker-1._aiupcycler-worker._tcp.local.")
                .unwrap()
                .free_ram_bytes,
            1
        );
    }

    #[test]
    fn registry_without_events_publishes_nothing() {
        let registry = NodeRegistry::new();
        let node = DiscoveredNode::from_service_info(&sample_service_info()).expect("node built");
        let node_id = node.node_id.clone();
        registry.register_or_update(node);
        registry.remove(&node_id);
    }
}
