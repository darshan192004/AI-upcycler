//! LAN discovery of phones that can be reached over ADB.
//!
//! A phone can show up here from three places, and the same physical device is
//! frequently seen by two or three of them at once:
//!
//! - **USB** — `adb devices` while a cable is attached.
//! - **Wireless debugging** (Android 11+) — advertised as
//!   `_adb-tls-connect._tcp`, and `_adb-tls-pairing._tcp` while the pairing
//!   dialog is open.
//! - **Port sweep** — an opt-in probe of `x.y.z.1-254:5555` for devices put
//!   into TCP mode by `adb tcpip`. Off by default: it looks like scanning to
//!   network monitors and it is not needed once a device has enrolled.
//!
//! Everything that decides *which* device a user sees is pure and tested
//! ([`merge_sightings`], [`outranks`]); the network parts are thin wrappers so
//! the tests run without a phone, a daemon, or a socket.

use mdns_sd::{ServiceDaemon, ServiceEvent, ServiceInfo};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::net::IpAddr;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tracing::{info, warn};

/// Wireless-debugging connect service (Android 11+).
pub const ADB_CONNECT_SERVICE: &str = "_adb-tls-connect._tcp.local.";

/// Wireless-debugging pairing service; only present while the dialog is open.
pub const ADB_PAIRING_SERVICE: &str = "_adb-tls-pairing._tcp.local.";

/// The port `adb tcpip` listens on by default.
pub const ADB_TCPIP_PORT: u16 = 5555;

/// Concurrent connect attempts during a sweep.
///
/// 64 keeps a /24 sweep under about a second without opening hundreds of
/// sockets at once, which some access points treat as an attack.
pub const SWEEP_MAX_IN_FLIGHT: usize = 64;

/// How long a single sweep probe waits for a TCP handshake.
pub const SWEEP_TIMEOUT: Duration = Duration::from_millis(300);

/// How a device is reachable, best first.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Transport {
    /// A cable is attached right now.
    Usb,
    /// Android 11+ Wireless debugging; survives a reboot.
    WirelessDebugging,
    /// `adb tcpip` over Wi-Fi; lost when the phone reboots.
    WifiAdb,
}

impl Transport {
    /// Lower is better. USB wins because it needs no network and no consent
    /// beyond the cable, and because enrollment over it never puts a private
    /// key on the wire.
    pub fn rank(self) -> u8 {
        match self {
            Self::Usb => 0,
            Self::WirelessDebugging => 1,
            Self::WifiAdb => 2,
        }
    }

    /// What the dashboard shows next to the device name.
    pub fn label(self) -> &'static str {
        match self {
            Self::Usb => "USB",
            Self::WirelessDebugging => "Wireless debugging",
            Self::WifiAdb => "Wi-Fi",
        }
    }
}

/// One source's view of a device, before duplicates are collapsed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Sighting {
    /// What identifies the device for de-duplication: the adb serial when the
    /// source knows it, otherwise `ip:port`.
    pub key: String,
    pub display_name: String,
    /// The `ip:port` to hand to `adb connect`.
    pub address: String,
    pub transport: Transport,
    /// `None` when the source cannot tell (mDNS does not report consent).
    pub authorized: Option<bool>,
}

impl Sighting {
    fn into_device(self) -> AvailableDevice {
        AvailableDevice {
            id: self.key,
            display_name: self.display_name,
            address: self.address,
            transport: self.transport,
            authorized: self.authorized,
        }
    }
}

/// A device the user can act on.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AvailableDevice {
    pub id: String,
    pub display_name: String,
    pub address: String,
    pub transport: Transport,
    pub authorized: Option<bool>,
}

/// Whether `candidate` should replace `current` for the same device.
///
/// Authorization is the strongest signal: a phone that has already accepted
/// this laptop is usable, while one that has not is showing a dialog. After
/// that, the transport decides, and a tie keeps the current entry so repeated
/// mDNS announcements do not make the list flicker.
pub fn outranks(candidate: &AvailableDevice, current: &AvailableDevice) -> bool {
    match (candidate.authorized, current.authorized) {
        (Some(true), Some(true)) | (None, None) | (Some(false), Some(false)) => {}
        (Some(true), _) => return true,
        (_, Some(true)) => return false,
        _ => {}
    }
    candidate.transport.rank() < current.transport.rank()
}

/// Collapses every source's sightings into one entry per physical device.
pub fn merge_sightings(sightings: Vec<Sighting>) -> Vec<AvailableDevice> {
    let mut by_key: HashMap<String, AvailableDevice> = HashMap::new();
    for sighting in sightings {
        let candidate = sighting.into_device();
        match by_key.get(&candidate.id) {
            Some(current) if !outranks(&candidate, current) => {}
            _ => {
                by_key.insert(candidate.id.clone(), candidate);
            }
        }
    }
    let mut devices: Vec<AvailableDevice> = by_key.into_values().collect();
    devices.sort_by(|a, b| {
        a.display_name
            .to_lowercase()
            .cmp(&b.display_name.to_lowercase())
            .then_with(|| a.id.cmp(&b.id))
    });
    devices
}

/// The live set of devices discovery has seen.
///
/// One instance is shared by every source, so an mDNS sighting and a USB
/// sighting of the same phone converge on one entry instead of racing to add
/// two.
#[derive(Clone, Default)]
pub struct LanDiscovery {
    devices: Arc<Mutex<HashMap<String, AvailableDevice>>>,
}

impl LanDiscovery {
    pub fn new() -> Self {
        Self::default()
    }

    /// Adds or improves an entry. A sighting that does not outrank what is
    /// already known is ignored rather than downgrading it.
    pub fn upsert(&self, sighting: Sighting) {
        let candidate = sighting.into_device();
        let mut devices = self.devices.lock().unwrap();
        match devices.get(&candidate.id) {
            Some(current) if !outranks(&candidate, current) => {}
            _ => {
                devices.insert(candidate.id.clone(), candidate);
            }
        }
    }

    pub fn remove(&self, id: &str) {
        self.devices.lock().unwrap().remove(id);
    }

    pub fn clear(&self) {
        self.devices.lock().unwrap().clear();
    }

    /// Every known device, in a stable order.
    pub fn list(&self) -> Vec<AvailableDevice> {
        let mut devices: Vec<AvailableDevice> =
            self.devices.lock().unwrap().values().cloned().collect();
        devices.sort_by(|a, b| {
            a.display_name
                .to_lowercase()
                .cmp(&b.display_name.to_lowercase())
                .then_with(|| a.id.cmp(&b.id))
        });
        devices
    }

    pub fn len(&self) -> usize {
        self.devices.lock().unwrap().len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

/// Starts browsers for both ADB service types.
///
/// The returned daemon owns the responder; keep it alive while discovery should
/// run. Pairing devices are included because Android 11+ users start there:
/// the pairing dialog is the only moment the phone advertises the address and
/// code we need.
pub fn spawn_adb_browsers(discovery: Arc<LanDiscovery>) -> Result<ServiceDaemon, String> {
    let daemon = ServiceDaemon::new().map_err(|e| format!("failed to start mDNS daemon: {e}"))?;

    for service in [ADB_CONNECT_SERVICE, ADB_PAIRING_SERVICE] {
        let receiver = daemon
            .browse(service)
            .map_err(|e| format!("failed to browse {service}: {e}"))?;
        let discovery = discovery.clone();
        std::thread::spawn(move || {
            info!("LAN discovery: browsing for {service}");
            while let Ok(event) = receiver.recv() {
                match event {
                    ServiceEvent::ServiceResolved(info) => {
                        match sighting_from_service(&info, service) {
                            Some(sighting) => discovery.upsert(sighting),
                            None => warn!(
                                "LAN discovery: '{}' resolved without a usable address",
                                info.get_fullname()
                            ),
                        }
                    }
                    ServiceEvent::ServiceRemoved(_service_type, fullname) => {
                        // The fullname is not the device key, so the entry is
                        // left to the transport probes to expire; removing by
                        // guess would drop a device that is also on USB.
                        info!("LAN discovery: '{fullname}' withdrawn");
                    }
                    _ => {}
                }
            }
            info!("LAN discovery: browser for {service} stopped");
        });
    }

    Ok(daemon)
}

/// Projects an mDNS record onto a sighting.
///
/// The serial is not in these records, so `ip:port` is the key. It stops being
/// the key the moment `adb connect` succeeds and `adb devices` reports a
/// serial, which is why [`AvailableDevice`] keeps both.
fn sighting_from_service(info: &ServiceInfo, service: &str) -> Option<Sighting> {
    let ip = preferred_ip(info)?;
    let port = info.get_port();
    let address = format!("{ip}:{port}");
    let transport = if service == ADB_PAIRING_SERVICE {
        Transport::WifiAdb
    } else {
        Transport::WirelessDebugging
    };
    let host = info.get_hostname().trim_end_matches('.');
    Some(Sighting {
        key: address.clone(),
        display_name: if host.is_empty() {
            info.get_fullname().to_string()
        } else {
            host.to_string()
        },
        address,
        transport,
        authorized: None,
    })
}

fn preferred_ip(info: &ServiceInfo) -> Option<String> {
    let addresses = info.get_addresses();
    addresses
        .iter()
        .find(|addr| matches!(addr, IpAddr::V4(_)))
        .or_else(|| addresses.iter().next())
        .map(|addr| addr.to_string())
}

/// Probes `prefix.1-254:port` for a listening adbd.
///
/// A completed TCP handshake only means something is listening on that port,
/// not that this laptop is authorized — `adb connect` decides that. Callers
/// must ask the user first: this is a port scan by any reasonable definition.
pub async fn sweep(
    subnet_prefix: &str,
    port: u16,
    timeout: Duration,
    max_in_flight: usize,
) -> Vec<String> {
    let semaphore = Arc::new(tokio::sync::Semaphore::new(max_in_flight.max(1)));
    let mut tasks = Vec::with_capacity(254);

    for host in 1..=254u8 {
        let target = format!("{subnet_prefix}.{host}:{port}");
        let permit_source = semaphore.clone();
        tasks.push(tokio::spawn(async move {
            let _permit = permit_source.acquire_owned().await.ok()?;
            match tokio::time::timeout(timeout, tokio::net::TcpStream::connect(&target)).await {
                Ok(Ok(_)) => Some(target),
                _ => None,
            }
        }));
    }

    let mut listening = Vec::new();
    for task in tasks {
        if let Ok(Some(target)) = task.await {
            listening.push(target);
        }
    }
    listening.sort();
    listening
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sighting(key: &str, transport: Transport, authorized: Option<bool>) -> Sighting {
        Sighting {
            key: key.to_string(),
            display_name: "redmi".to_string(),
            address: format!("{key}:5555"),
            transport,
            authorized,
        }
    }

    #[test]
    fn sightings_with_different_keys_stay_separate_until_enrolment_links_them() {
        // USB reports a serial, mDNS reports ip:port, and nothing in either
        // record says they are the same phone. Discovery therefore shows three
        // entries here; `NodeRegistry::link_alias` collapses them once the
        // device enrols, which is the first moment both ids are known at once.
        // Pretending otherwise would mean guessing from an IP address.
        let merged = merge_sightings(vec![
            sighting("192.168.1.13:5555", Transport::WifiAdb, None),
            sighting("192.168.1.13:37043", Transport::WirelessDebugging, None),
            sighting("16284c49", Transport::Usb, Some(true)),
        ]);
        assert_eq!(merged.len(), 3);
    }

    #[test]
    fn the_same_key_from_two_sources_collapses_to_the_better_transport() {
        let merged = merge_sightings(vec![
            sighting("192.168.1.13:5555", Transport::WifiAdb, None),
            sighting("192.168.1.13:5555", Transport::Usb, None),
        ]);
        assert_eq!(merged.len(), 1);
        assert_eq!(merged[0].transport, Transport::Usb);
    }

    #[test]
    fn an_authorized_sighting_beats_a_newer_transport() {
        let merged = merge_sightings(vec![
            sighting("192.168.1.13:5555", Transport::WifiAdb, Some(true)),
            sighting("192.168.1.13:5555", Transport::Usb, Some(false)),
        ]);
        assert_eq!(merged.len(), 1);
        assert_eq!(
            merged[0].authorized,
            Some(true),
            "a phone that already trusts us is usable; a cable that has not been \
             approved is not"
        );
    }

    #[test]
    fn an_unauthorized_sighting_does_not_downgrade_an_authorized_one() {
        let current = sighting("a", Transport::Usb, Some(true)).into_device();
        let candidate = sighting("a", Transport::Usb, Some(false)).into_device();
        assert!(!outranks(&candidate, &current));
    }

    #[test]
    fn equal_sightings_keep_the_first_so_the_list_does_not_flicker() {
        let current = sighting("a", Transport::WirelessDebugging, None).into_device();
        let candidate = sighting("a", Transport::WirelessDebugging, None).into_device();
        assert!(!outranks(&candidate, &current));
    }

    #[test]
    fn the_store_keeps_one_entry_and_prefers_usb() {
        let store = LanDiscovery::new();
        store.upsert(sighting("192.168.1.13:5555", Transport::WifiAdb, None));
        store.upsert(sighting("192.168.1.13:5555", Transport::Usb, None));
        assert_eq!(store.len(), 1);
        assert_eq!(store.list()[0].transport, Transport::Usb);
    }

    #[test]
    fn devices_are_listed_by_display_name() {
        let mut alpha = sighting("b", Transport::Usb, None);
        alpha.display_name = "alpha".to_string();
        let mut zebra = sighting("a", Transport::Usb, None);
        zebra.display_name = "Zebra".to_string();
        let store = LanDiscovery::new();
        store.upsert(zebra);
        store.upsert(alpha);
        let listed = store.list();
        let names: Vec<&str> = listed.iter().map(|d| d.display_name.as_str()).collect();
        assert_eq!(
            names,
            vec!["alpha", "Zebra"],
            "sorted case-insensitively by the name the user sees"
        );
    }

    #[test]
    fn a_removed_device_is_gone() {
        let store = LanDiscovery::new();
        store.upsert(sighting("a", Transport::Usb, None));
        store.remove("a");
        assert!(store.is_empty());
    }
}
