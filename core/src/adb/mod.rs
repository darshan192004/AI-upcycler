//! ADB USB tunnel manager.
//!
//! Bridges Android worker devices into the master node's localhost namespace via
//! `adb forward tcp:50052 tcp:50052`, then probes each tunneled worker over RPC
//! so its real RAM/device metadata is known before scheduling. Workers reached
//! this way are registered as `USB_ADB` nodes bound to `127.0.0.1:50052`.

use crate::discovery::mdns::{DiscoveredNode, NodeRegistry};
use crate::rpc::client::{WorkerRpcClient, MASTER_ID, PROTOCOL_VERSION};
use crate::rpc::engine::http_endpoint;
use crate::rpc::proto::aiupcycler::rpc::HandshakeResponse;
use std::collections::{HashMap, HashSet};
use std::process::Command;
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tracing::{info, warn};

/// Local TCP port bridged to the worker RPC port on each USB-attached device.
pub const ADB_FORWARD_PORT: u16 = 50052;

/// How often the USB device list and each phone's live memory are refreshed.
///
/// The worker health poll already runs at 5 s; matching it keeps the dashboard
/// on one cadence. `adb shell cat /proc/meminfo` is read-only and does not wake
/// or launch the worker app.
pub const ADB_POLL_INTERVAL: Duration = Duration::from_secs(5);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct MemInfo {
    total_bytes: u64,
    available_bytes: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AdbDevice {
    pub serial: String,
    pub state: String,
    pub model: String,
}

impl AdbDevice {
    /// Human-readable label, falling back to the serial when no model is
    /// reported by `adb devices -l`.
    pub fn label(&self) -> String {
        if self.model.is_empty() {
            self.serial.clone()
        } else {
            self.model.clone()
        }
    }
}

#[derive(Debug, Default)]
pub struct AdbTunnelManager;

impl AdbTunnelManager {
    pub fn new() -> Self {
        Self
    }

    /// Whether the `adb` binary is installed and runnable.
    pub fn is_available() -> bool {
        Command::new("adb")
            .arg("version")
            .output()
            .map(|output| output.status.success())
            .unwrap_or(false)
    }

    /// Lists USB-attached devices via `adb devices -l`.
    pub fn list_devices() -> Result<Vec<AdbDevice>, String> {
        let output = Command::new("adb")
            .args(["devices", "-l"])
            .output()
            .map_err(|e| format!("failed to run 'adb devices -l': {e}"))?;

        if !output.status.success() {
            return Err(format!(
                "'adb devices -l' failed: {}",
                String::from_utf8_lossy(&output.stderr).trim()
            ));
        }

        Ok(parse_devices(&String::from_utf8_lossy(&output.stdout)))
    }

    /// Establishes a port forward for a single device serial.
    ///
    /// `local_port` is the loopback port the master connects to; the worker's
    /// own RPC port is always [`ADB_FORWARD_PORT`] on the device side.
    pub fn forward(&self, serial: &str, local_port: u16) -> Result<(), String> {
        let remote = format!("tcp:{ADB_FORWARD_PORT}");
        let local = format!("tcp:{local_port}");
        let output = Command::new("adb")
            .args(["-s", serial, "forward", &local, &remote])
            .output()
            .map_err(|e| format!("failed to run 'adb forward' for {serial}: {e}"))?;

        if output.status.success() {
            info!("ADB: forwarded {serial} -> 127.0.0.1:{local_port}");
            Ok(())
        } else {
            Err(format!(
                "'adb forward' failed for {serial}: {}",
                String::from_utf8_lossy(&output.stderr).trim()
            ))
        }
    }

    /// Tears down a previously established port forward.
    pub fn remove_forward(&self, serial: &str, local_port: u16) -> Result<(), String> {
        let local = format!("tcp:{local_port}");
        let output = Command::new("adb")
            .args(["-s", serial, "forward", "--remove", &local])
            .output()
            .map_err(|e| format!("failed to run 'adb forward --remove' for {serial}: {e}"))?;

        if output.status.success() {
            info!("ADB: removed forward for {serial} on 127.0.0.1:{local_port}");
            Ok(())
        } else {
            Err(format!(
                "'adb forward --remove' failed for {serial}: {}",
                String::from_utf8_lossy(&output.stderr).trim()
            ))
        }
    }

    /// Exposes the master's pairing-only listener on the device's own loopback
    /// (`adb -s <serial> reverse tcp:8081 tcp:8081`), so the worker app can
    /// POST `/api/pairing/enroll` at `127.0.0.1:8081` without the API leaving
    /// host loopback (spec 2026-10-20, D-B2; docs/security.md §3).
    pub fn reverse_pairing(&self, serial: &str) -> Result<(), String> {
        let args = reverse_pairing_args(serial);
        let output = Command::new("adb")
            .args(&args)
            .output()
            .map_err(|e| format!("failed to run 'adb reverse' for {serial}: {e}"))?;

        if output.status.success() {
            info!(
                "ADB: pairing reverse tunnel up for {serial} -> 127.0.0.1:{}",
                crate::api::PAIRING_PORT
            );
            Ok(())
        } else {
            Err(format!(
                "'adb reverse' failed for {serial}: {}",
                String::from_utf8_lossy(&output.stderr).trim()
            ))
        }
    }

    /// Removes the pairing reverse tunnel (session completed or expired).
    pub fn remove_reverse_pairing(&self, serial: &str) -> Result<(), String> {
        let args = remove_reverse_pairing_args(serial);
        let output = Command::new("adb")
            .args(&args)
            .output()
            .map_err(|e| format!("failed to run 'adb reverse --remove' for {serial}: {e}"))?;

        if output.status.success() {
            info!("ADB: removed pairing reverse tunnel for {serial}");
            Ok(())
        } else {
            Err(format!(
                "'adb reverse --remove' failed for {serial}: {}",
                String::from_utf8_lossy(&output.stderr).trim()
            ))
        }
    }

    /// Sets up a port forward for every authorized USB device, giving each
    /// device a distinct loopback port so multiple phones can be bridged at
    /// once. Returns the devices that now have a tunnel alongside their local
    /// port.
    pub fn forward_all(&self) -> Result<Vec<(AdbDevice, u16)>, String> {
        let devices = Self::list_devices()?;
        let mut forwarded = Vec::new();

        for (index, device) in devices
            .into_iter()
            .filter(|d| d.state == "device")
            .enumerate()
        {
            let local_port = ADB_FORWARD_PORT + index as u16;
            match self.forward(&device.serial, local_port) {
                Ok(()) => forwarded.push((device, local_port)),
                Err(e) => warn!("ADB: {e}"),
            }
        }

        Ok(forwarded)
    }

    /// USB bootstrap: polls every authorized device, keeps a forward per
    /// device, and registers a node as soon as ADB can read its memory — even
    /// before the worker app answers. The node is upgraded with worker-reported
    /// capacity when the handshake succeeds.
    ///
    /// Polling (rather than a one-shot startup pass) is what makes a phone
    /// attached after launch appear, and what keeps its live RAM current. The
    /// only device-side work is read-only `adb shell cat /proc/meminfo`; the
    /// worker app is never launched here.
    pub async fn start(
        registry: Arc<NodeRegistry>,
        mut shutdown: tokio::sync::watch::Receiver<bool>,
    ) {
        if !Self::is_available() {
            warn!("ADB: binary not found; USB device bridging disabled");
            return;
        }

        let manager = Self::new();
        let mut forwards: HashMap<String, u16> = HashMap::new();
        info!(
            "ADB: polling USB devices every {}s; unpaired phones are listed with live capacity",
            ADB_POLL_INTERVAL.as_secs()
        );

        loop {
            if *shutdown.borrow() {
                break;
            }

            match Self::list_devices() {
                Ok(devices) => {
                    let devices: Vec<AdbDevice> = devices
                        .into_iter()
                        .filter(|device| device.state == "device")
                        .collect();
                    let present: HashSet<String> =
                        devices.iter().map(|device| device.serial.clone()).collect();

                    let departed: Vec<String> = forwards
                        .keys()
                        .filter(|serial| !present.contains(*serial))
                        .cloned()
                        .collect();
                    for serial in departed {
                        if let Some(local_port) = forwards.remove(&serial) {
                            if let Err(e) = manager.remove_forward(&serial, local_port) {
                                warn!("ADB: cleanup failed for {serial}: {e}");
                            }
                        }
                        registry.remove(&format!("adb:{serial}"));
                    }

                    for device in devices {
                        let local_port = match forwards.get(&device.serial) {
                            Some(local_port) => *local_port,
                            None => {
                                let local_port = next_free_port(&forwards);
                                match manager.forward(&device.serial, local_port) {
                                    Ok(()) => {
                                        forwards.insert(device.serial.clone(), local_port);
                                        local_port
                                    }
                                    Err(e) => {
                                        warn!("ADB: {e}");
                                        continue;
                                    }
                                }
                            }
                        };

                        let node_id = format!("adb:{}", device.serial);
                        let existing = registry.get_node(&node_id);
                        let node = merge_usb_node(
                            existing.as_ref(),
                            usb_detected_node(&device, read_meminfo(&device.serial)),
                        );
                        if existing.is_some() {
                            registry.upsert(node);
                        } else {
                            registry.register_or_update(node);
                        }

                        // The phone is listed before its worker app runs. Probe
                        // the RPC port until it answers; success upgrades this
                        // same node with worker-reported capacity and a
                        // dialable port, which puts it on the health-poll path.
                        let needs_probe = registry
                            .get_node(&node_id)
                            .map(|node| node.port == 0)
                            .unwrap_or(false);
                        if needs_probe {
                            if let Some(handshake) = probe_device(local_port).await {
                                registry.register_or_update(node_from_handshake(
                                    &device, local_port, handshake,
                                ));
                            }
                        }
                    }
                }
                Err(e) => warn!("ADB: {e}"),
            }

            tokio::select! {
                _ = tokio::time::sleep(ADB_POLL_INTERVAL) => {}
                changed = shutdown.changed() => {
                    if changed.is_err() {
                        break;
                    }
                }
            }
        }

        for (serial, local_port) in forwards {
            if let Err(e) = manager.remove_forward(&serial, local_port) {
                warn!("ADB: shutdown cleanup failed for {serial}: {e}");
            }
            registry.remove(&format!("adb:{serial}"));
        }
        info!("ADB: USB polling stopped");
    }
}

/// Which device serials currently hold a pairing reverse tunnel, keyed to the
/// single-use session that tunnel exists for (spec 2026-10-20, D-B8: tunnels
/// are removed when the session completes or expires).
#[derive(Default)]
pub struct PairingTunnelRegistry {
    tunnels: std::sync::Mutex<HashMap<String, String>>,
}

impl PairingTunnelRegistry {
    /// Records that `serial`'s reverse tunnel now backs `pin`'s session,
    /// replacing any previous entry for that device.
    pub fn track(&self, serial: &str, pin: &str) {
        self.tunnels
            .lock()
            .expect("pairing tunnel lock")
            .insert(serial.to_string(), pin.to_string());
    }

    /// Drops and returns every serial whose session has been redeemed or has
    /// expired, so its tunnel no longer exposes `POST /api/pairing/enroll`.
    pub fn purge_inactive(&self, pairing: &crate::security::PairingManager) -> Vec<String> {
        let mut tunnels = self.tunnels.lock().expect("pairing tunnel lock");
        let inactive: Vec<String> = tunnels
            .iter()
            .filter(|(_, pin)| !pairing.session_usable(pin))
            .map(|(serial, _)| serial.clone())
            .collect();
        tunnels.retain(|serial, _| !inactive.contains(serial));
        inactive
    }

    /// Whether a tunnel for `serial` is currently tracked.
    pub fn is_tracked(&self, serial: &str) -> bool {
        self.tunnels
            .lock()
            .expect("pairing tunnel lock")
            .contains_key(serial)
    }
}

/// Polls the registry and removes pairing reverse tunnels whose session has
/// completed or expired (spec 2026-10-20, D-B8). Removal is best-effort: a
/// failed `adb reverse --remove` is logged and the entry dropped so the poll
/// does not retry a device that is gone forever; the device-side entry itself
/// only ever reaches a listener that refuses every redemption (the session
/// behind it is already dead).
pub async fn pairing_tunnel_cleanup(
    security: Arc<crate::security::SecurityState>,
    tunnels: Arc<PairingTunnelRegistry>,
) {
    let mut tick = tokio::time::interval(std::time::Duration::from_secs(2));
    loop {
        tick.tick().await;
        for serial in tunnels.purge_inactive(&security.pairing) {
            if let Err(e) = AdbTunnelManager::new().remove_reverse_pairing(&serial) {
                warn!("ADB: pairing tunnel cleanup for {serial}: {e}");
            }
        }
    }
}

async fn probe_device(local_port: u16) -> Option<HandshakeResponse> {
    let endpoint = http_endpoint("127.0.0.1", local_port);
    let mut client = WorkerRpcClient::connect(&endpoint).await.ok()?;
    client.handshake(MASTER_ID, PROTOCOL_VERSION).await.ok()
}

fn node_from_handshake(
    device: &AdbDevice,
    local_port: u16,
    handshake: HandshakeResponse,
) -> DiscoveredNode {
    let device_name = if handshake.device_name.is_empty() {
        device.label()
    } else {
        handshake.device_name
    };

    DiscoveredNode {
        node_id: format!("adb:{}", device.serial),
        device_name,
        ip_address: "127.0.0.1".to_string(),
        port: local_port,
        total_ram_bytes: handshake.total_ram_bytes.max(0) as u64,
        free_ram_bytes: handshake.free_ram_bytes.max(0) as u64,
        connection_type: "USB_ADB".to_string(),
        latency_ms: 0.0,
        last_seen_epoch_ms: now_epoch_ms(),
    }
}

/// A phone ADB can see, but whose worker app has not answered the RPC
/// handshake yet. Port 0 means "not dialable": the health poll skips it, and
/// the topology still shows its real total/available memory.
fn usb_detected_node(device: &AdbDevice, mem: Option<MemInfo>) -> DiscoveredNode {
    DiscoveredNode {
        node_id: format!("adb:{}", device.serial),
        device_name: device.label(),
        ip_address: "127.0.0.1".to_string(),
        port: 0,
        total_ram_bytes: mem.map(|mem| mem.total_bytes).unwrap_or(0),
        free_ram_bytes: mem.map(|mem| mem.available_bytes).unwrap_or(0),
        connection_type: "USB_ADB".to_string(),
        latency_ms: 0.0,
        last_seen_epoch_ms: now_epoch_ms(),
    }
}

/// Keeps the worker handshake's dialable port and name while refreshing the
/// phone's live memory from `/proc/meminfo`. Without this, every ADB poll
/// would reset the port to 0 and knock the worker off the health-poll path.
fn merge_usb_node(existing: Option<&DiscoveredNode>, fresh: DiscoveredNode) -> DiscoveredNode {
    let Some(existing) = existing else {
        return fresh;
    };
    let mut node = fresh;
    node.port = existing.port;
    node.latency_ms = existing.latency_ms;
    if node.total_ram_bytes == 0 {
        node.total_ram_bytes = existing.total_ram_bytes;
        node.free_ram_bytes = existing.free_ram_bytes;
    }
    if !existing.device_name.is_empty() {
        node.device_name = existing.device_name.clone();
    }
    node
}

/// Reads the phone's live memory without touching the worker app.
fn read_meminfo(serial: &str) -> Option<MemInfo> {
    let output = Command::new("adb")
        .args(["-s", serial, "shell", "cat", "/proc/meminfo"])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    parse_meminfo(&String::from_utf8_lossy(&output.stdout))
}

/// Parses the `MemTotal`/`MemAvailable` lines from Android's `/proc/meminfo`.
/// Older devices may omit `MemAvailable`; `MemFree` is the honest fallback.
fn parse_meminfo(output: &str) -> Option<MemInfo> {
    let mut total_kb = None;
    let mut available_kb = None;
    let mut free_kb = None;
    for line in output.lines() {
        let Some((key, rest)) = line.split_once(':') else {
            continue;
        };
        let Some(value) = rest.split_whitespace().next() else {
            continue;
        };
        let Ok(value) = value.parse::<u64>() else {
            continue;
        };
        match key.trim() {
            "MemTotal" => total_kb = Some(value),
            "MemAvailable" => available_kb = Some(value),
            "MemFree" => free_kb = Some(value),
            _ => {}
        }
    }
    let total_kb = total_kb.filter(|total| *total > 0)?;
    let available_kb = available_kb.or(free_kb).unwrap_or(0).min(total_kb);
    Some(MemInfo {
        total_bytes: total_kb.saturating_mul(1024),
        available_bytes: available_kb.saturating_mul(1024),
    })
}

/// Lowest loopback port not already assigned to another attached device.
fn next_free_port(forwards: &HashMap<String, u16>) -> u16 {
    let mut port = ADB_FORWARD_PORT;
    while forwards.values().any(|used| *used == port) {
        port = port.saturating_add(1);
    }
    port
}

/// Argument vector for the pairing reverse tunnel, factored out so unit tests
/// pin the exact `adb` invocation without a device (spec 2026-10-20, D-B2).
fn reverse_pairing_args(serial: &str) -> Vec<String> {
    let port = crate::api::PAIRING_PORT;
    vec![
        "-s".into(),
        serial.into(),
        "reverse".into(),
        format!("tcp:{port}"),
        format!("tcp:{port}"),
    ]
}

/// Argument vector for removing the pairing reverse tunnel.
fn remove_reverse_pairing_args(serial: &str) -> Vec<String> {
    let port = crate::api::PAIRING_PORT;
    vec![
        "-s".into(),
        serial.into(),
        "reverse".into(),
        "--remove".into(),
        format!("tcp:{port}"),
    ]
}

fn parse_devices(output: &str) -> Vec<AdbDevice> {
    output
        .lines()
        .skip_while(|line| line.starts_with("List of devices"))
        .filter_map(|line| {
            let mut parts = line.split_whitespace();
            let serial = parts.next()?.to_string();
            let state = parts.next()?.to_string();
            let model = parts
                .find_map(|field| field.strip_prefix("model:").map(str::to_string))
                .unwrap_or_default();
            Some(AdbDevice {
                serial,
                state,
                model,
            })
        })
        .collect()
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

    const SAMPLE: &str = "List of devices attached\n\
        R3CN30ABCDE\tdevice product:x1q model:SM_G986B device:x1q transport_id:1\n\
        emulator-5554\tunauthorized\n\
        FA7B1XYZ\toffline\n";

    #[test]
    fn parses_adb_devices_output() {
        let devices = parse_devices(SAMPLE);

        assert_eq!(devices.len(), 3);
        assert_eq!(devices[0].serial, "R3CN30ABCDE");
        assert_eq!(devices[0].state, "device");
        assert_eq!(devices[0].model, "SM_G986B");
        assert_eq!(devices[0].label(), "SM_G986B");
        assert_eq!(devices[1].state, "unauthorized");
        assert_eq!(devices[2].state, "offline");
        assert_eq!(devices[2].model, "");
        assert_eq!(devices[2].label(), "FA7B1XYZ");
    }

    #[test]
    fn ignores_header_and_blank_lines() {
        let devices = parse_devices("List of devices attached\n\n");
        assert!(devices.is_empty());
    }

    #[test]
    fn forward_port_matches_worker_rpc_port() {
        assert_eq!(ADB_FORWARD_PORT, 50052);
    }

    use crate::security::{PairingManager, PAIRING_TTL};
    use std::time::Duration;

    #[test]
    fn tracked_tunnel_lives_until_its_session_ends() {
        let pairing = PairingManager::new(PAIRING_TTL);
        let (pin, _, _) = pairing.create_session("fp");
        let registry = PairingTunnelRegistry::default();

        registry.track("R3CN30ABCDE", &pin);
        assert!(registry.purge_inactive(&pairing).is_empty());
        assert!(registry.is_tracked("R3CN30ABCDE"));

        pairing.redeem(&pin).expect("redeem");
        assert_eq!(registry.purge_inactive(&pairing), vec!["R3CN30ABCDE"]);
        assert!(!registry.is_tracked("R3CN30ABCDE"));
        // A second purge must not hand the same serial out again.
        assert!(registry.purge_inactive(&pairing).is_empty());
    }

    #[test]
    fn expired_session_purges_its_tunnel() {
        let pairing = PairingManager::new(Duration::ZERO);
        let (pin, _, _) = pairing.create_session("fp");
        let registry = PairingTunnelRegistry::default();

        registry.track("emulator-5554", &pin);
        assert_eq!(registry.purge_inactive(&pairing), vec!["emulator-5554"]);
    }

    #[test]
    fn retracked_serial_follows_its_newest_session() {
        // A second "create session" on the same device must not let the old
        // session's teardown drop the new session's tunnel.
        let pairing = PairingManager::new(PAIRING_TTL);
        let (old_pin, _, _) = pairing.create_session("fp");
        let (new_pin, _, _) = pairing.create_session("fp");
        let registry = PairingTunnelRegistry::default();
        registry.track("R3CN30ABCDE", &old_pin);
        registry.track("R3CN30ABCDE", &new_pin);

        pairing.redeem(&old_pin).expect("old redeem");
        assert!(registry.purge_inactive(&pairing).is_empty());

        pairing.redeem(&new_pin).expect("new redeem");
        assert_eq!(registry.purge_inactive(&pairing), vec!["R3CN30ABCDE"]);
    }

    #[test]
    fn reverse_pairing_args_pin_the_device_and_host_ports() {
        // `adb -s <serial> reverse tcp:8081 tcp:8081`: the phone's own
        // loopback 8081 lands on the master's pairing-only listener.
        assert_eq!(
            reverse_pairing_args("R3CN30ABCDE"),
            vec!["-s", "R3CN30ABCDE", "reverse", "tcp:8081", "tcp:8081"]
        );
        assert_eq!(
            remove_reverse_pairing_args("R3CN30ABCDE"),
            vec!["-s", "R3CN30ABCDE", "reverse", "--remove", "tcp:8081"]
        );
        // The tunnel must land on the pairing listener's port, whatever it is.
        assert_eq!(crate::api::PAIRING_PORT, 8081);
    }

    fn device(serial: &str, model: &str) -> AdbDevice {
        AdbDevice {
            serial: serial.to_string(),
            state: "device".to_string(),
            model: model.to_string(),
        }
    }

    #[test]
    fn node_from_handshake_uses_worker_reported_capacity() {
        let handshake = HandshakeResponse {
            worker_id: "w1".to_string(),
            device_name: "Pixel 8".to_string(),
            total_ram_bytes: 8_589_934_592,
            free_ram_bytes: 4_294_967_296,
            cpu_cores: 8,
            has_npu_acceleration: true,
            supports_last_row_logits: false,
        };

        let node = node_from_handshake(&device("R3CN30ABCDE", "SM_G986B"), 50052, handshake);

        assert_eq!(node.node_id, "adb:R3CN30ABCDE");
        assert_eq!(node.device_name, "Pixel 8");
        assert_eq!(node.ip_address, "127.0.0.1");
        assert_eq!(node.port, 50052);
        assert_eq!(node.total_ram_bytes, 8_589_934_592);
        assert_eq!(node.free_ram_bytes, 4_294_967_296);
        assert_eq!(node.connection_type, "USB_ADB");
    }

    #[test]
    fn node_from_handshake_falls_back_to_adb_label() {
        let handshake = HandshakeResponse {
            worker_id: String::new(),
            device_name: String::new(),
            total_ram_bytes: 0,
            free_ram_bytes: 0,
            cpu_cores: 0,
            has_npu_acceleration: false,
            supports_last_row_logits: false,
        };

        let node = node_from_handshake(&device("FA7B1XYZ", "Pixel_6a"), 50053, handshake);

        assert_eq!(node.device_name, "Pixel_6a");
        assert_eq!(node.port, 50053);
    }

    #[test]
    fn parses_android_meminfo() {
        let info = parse_meminfo(
            "MemTotal:        5714584 kB\n\
             MemFree:          164068 kB\n\
             MemAvailable:    2527724 kB\n",
        )
        .expect("meminfo parsed");

        assert_eq!(info.total_bytes, 5_714_584 * 1024);
        assert_eq!(info.available_bytes, 2_527_724 * 1024);
    }

    #[test]
    fn meminfo_falls_back_to_mem_free_and_rejects_unknown_total() {
        let info = parse_meminfo("MemTotal: 1000 kB\nMemFree: 250 kB\n").expect("parsed");
        assert_eq!(info.available_bytes, 250 * 1024);

        assert!(parse_meminfo("MemFree: 250 kB\n").is_none());
        assert!(parse_meminfo("MemTotal: 0 kB\n").is_none());
    }

    #[test]
    fn a_usb_device_is_listed_before_its_worker_app_answers() {
        let device = device("16284c49", "Redmi_Note_9_Pro_Max");
        let node = usb_detected_node(
            &device,
            Some(MemInfo {
                total_bytes: 5_714_584 * 1024,
                available_bytes: 2_527_724 * 1024,
            }),
        );

        assert_eq!(node.node_id, "adb:16284c49");
        assert_eq!(node.device_name, "Redmi_Note_9_Pro_Max");
        assert_eq!(node.port, 0, "not dialable until the handshake");
        assert_eq!(node.total_ram_bytes, 5_714_584 * 1024);
        assert_eq!(node.free_ram_bytes, 2_527_724 * 1024);
        assert_eq!(node.connection_type, "USB_ADB");
    }

    #[test]
    fn a_meminfo_refresh_keeps_the_handshake_port_and_name() {
        let handshake = HandshakeResponse {
            worker_id: "w1".to_string(),
            device_name: "Redmi Note 9 Pro Max".to_string(),
            total_ram_bytes: 6 * 1024 * 1024 * 1024,
            free_ram_bytes: 3 * 1024 * 1024 * 1024,
            cpu_cores: 8,
            has_npu_acceleration: false,
            supports_last_row_logits: false,
        };
        let existing = node_from_handshake(
            &device("16284c49", "Redmi_Note_9_Pro_Max"),
            50052,
            handshake,
        );
        let fresh = usb_detected_node(
            &device("16284c49", "Redmi_Note_9_Pro_Max"),
            Some(MemInfo {
                total_bytes: 5 * 1024 * 1024 * 1024,
                available_bytes: 2 * 1024 * 1024 * 1024,
            }),
        );

        let merged = merge_usb_node(Some(&existing), fresh);

        assert_eq!(merged.port, 50052, "the handshake port survives polling");
        assert_eq!(merged.device_name, "Redmi Note 9 Pro Max");
        assert_eq!(merged.total_ram_bytes, 5 * 1024 * 1024 * 1024);
        assert_eq!(merged.free_ram_bytes, 2 * 1024 * 1024 * 1024);
    }

    #[test]
    fn next_free_port_skips_ports_already_in_use() {
        let mut forwards = HashMap::new();
        forwards.insert("a".to_string(), ADB_FORWARD_PORT);
        assert_eq!(next_free_port(&forwards), ADB_FORWARD_PORT + 1);

        forwards.insert("b".to_string(), ADB_FORWARD_PORT + 1);
        assert_eq!(next_free_port(&forwards), ADB_FORWARD_PORT + 2);
    }
}
