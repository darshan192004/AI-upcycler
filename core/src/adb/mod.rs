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
use std::process::Command;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};
use tracing::{info, warn};

/// Local TCP port bridged to the worker RPC port on each USB-attached device.
pub const ADB_FORWARD_PORT: u16 = 50052;

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

    /// USB bootstrap: forwards every authorized device, probes each tunneled
    /// worker over RPC so only nodes with known RAM capacity are registered,
    /// then stays alive until `shutdown` flips to `true`, at which point every
    /// forward this task established is removed again.
    ///
    /// Devices that fail to handshake are skipped rather than registered with
    /// unknown capacity; their forwards are removed immediately.
    pub async fn start(
        registry: Arc<NodeRegistry>,
        mut shutdown: tokio::sync::watch::Receiver<bool>,
    ) {
        if !Self::is_available() {
            warn!("ADB: binary not found; USB device bridging disabled");
            return;
        }

        let devices = match Self::new().forward_all() {
            Ok(devices) => devices,
            Err(e) => {
                warn!("ADB: {e}");
                return;
            }
        };

        if devices.is_empty() {
            info!("ADB: no authorized USB devices attached");
            return;
        }

        let mut registered = 0;
        let manager = Self::new();
        let mut established: Vec<(AdbDevice, u16)> = Vec::new();
        for (device, local_port) in devices {
            match probe_device(local_port).await {
                Some(handshake) => {
                    registry
                        .register_or_update(node_from_handshake(&device, local_port, handshake));
                    registered += 1;
                    established.push((device, local_port));
                }
                None => {
                    warn!(
                        "ADB: no handshake from {}; leaving device unregistered",
                        device.serial
                    );
                    if let Err(e) = manager.remove_forward(&device.serial, local_port) {
                        warn!("ADB: cleanup failed for {}: {e}", device.serial);
                    }
                }
            }
        }

        info!(
            "ADB: registered {registered} USB device(s); waiting for shutdown to tear down {} tunnel(s)",
            established.len()
        );

        while !*shutdown.borrow() {
            if shutdown.changed().await.is_err() {
                break;
            }
        }

        for (device, local_port) in &established {
            if let Err(e) = manager.remove_forward(&device.serial, *local_port) {
                warn!("ADB: shutdown cleanup failed for {}: {e}", device.serial);
            }
        }
        info!("ADB: removed {} tunnel(s) on shutdown", established.len());
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
        };

        let node = node_from_handshake(&device("FA7B1XYZ", "Pixel_6a"), 50053, handshake);

        assert_eq!(node.device_name, "Pixel_6a");
        assert_eq!(node.port, 50053);
    }

    #[test]
    fn distinct_devices_get_distinct_local_ports() {
        let base = ADB_FORWARD_PORT;
        assert_ne!(base, base + 1);
        assert_eq!(base + 1, 50053);
    }
}
