//! ADB USB tunnel manager.
//!
//! Bridges Android worker devices into the master node's localhost namespace via
//! `adb forward tcp:50052 tcp:50052` so the RPC client can talk to them as if
//! they were loopback endpoints.

use crate::discovery::mdns::{DiscoveredNode, NodeRegistry};
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

    /// Establishes the port forward for a single device serial.
    pub fn forward(&self, serial: &str) -> Result<(), String> {
        let spec = format!("tcp:{ADB_FORWARD_PORT}");
        let output = Command::new("adb")
            .args(["-s", serial, "forward", &spec, &spec])
            .output()
            .map_err(|e| format!("failed to run 'adb forward' for {serial}: {e}"))?;

        if output.status.success() {
            info!("ADB: forwarded {serial} -> 127.0.0.1:{ADB_FORWARD_PORT}");
            Ok(())
        } else {
            Err(format!(
                "'adb forward' failed for {serial}: {}",
                String::from_utf8_lossy(&output.stderr).trim()
            ))
        }
    }

    /// Forwards every authorized USB device and registers each as a `USB_ADB`
    /// node bound to `127.0.0.1:50052`. Returns the number of tunnels created.
    pub fn forward_all(&self, registry: &Arc<NodeRegistry>) -> Result<usize, String> {
        let devices = Self::list_devices()?;
        let mut established = 0;

        for device in devices.iter().filter(|d| d.state == "device") {
            match self.forward(&device.serial) {
                Ok(()) => {
                    registry.register_or_update(DiscoveredNode {
                        node_id: format!("adb:{}", device.serial),
                        device_name: device.label(),
                        ip_address: "127.0.0.1".to_string(),
                        port: ADB_FORWARD_PORT,
                        total_ram_bytes: 0,
                        free_ram_bytes: 0,
                        connection_type: "USB_ADB".to_string(),
                        latency_ms: 0.0,
                        last_seen_epoch_ms: now_epoch_ms(),
                    });
                    established += 1;
                }
                Err(e) => warn!("ADB: {e}"),
            }
        }

        Ok(established)
    }
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
}