//! The device onboarding state machine.
//!
//! ADB is the control plane: this module drives `adb connect`, the worker
//! install, and the version check. The mTLS channel is the data plane and is
//! not touched here.
//!
//! The machine stops at [`OnboardingState::NeedsEnrol`] rather than claiming
//! `Ready`, because enrolment needs the pairing session that the API layer
//! owns. A device that is reachable and running the right app is not yet in the
//! cluster, and the UI should not say it is.
//!
//! Every method here blocks: `adb` is a subprocess and the consent wait is a
//! poll loop. Callers run them on a blocking thread.

use crate::adb::wireless::{AdbControl, ConnectOutcome};
use crate::discovery::lan::{LanDiscovery, Transport};
use serde::Serialize;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

/// How long to wait for the user to approve Android's debugging prompt.
pub const CONSENT_TIMEOUT: Duration = Duration::from_secs(120);

/// How often the consent wait re-reads `adb devices`.
pub const CONSENT_POLL: Duration = Duration::from_secs(2);

/// What the dashboard shows for one device.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum OnboardingState {
    /// Seen, but nothing has been tried.
    Discovered,
    /// `adb connect` is running.
    Connecting,
    /// Android is showing its own "Allow USB debugging?" dialog. We cannot
    /// draw that dialog; we can only wait for it.
    AwaitingConsent,
    /// adb reaches the phone, but the worker app's state is not known yet.
    Connected,
    /// The worker app is missing or stale. Installing is always the user's
    /// explicit choice.
    NeedsInstall {
        installed: Option<String>,
        available: String,
    },
    Installing,
    /// The right worker app is running. The device still has to enrol before
    /// it is part of the cluster.
    NeedsEnrol {
        installed: String,
    },
    /// Enrolled and serving.
    Ready {
        node_id: String,
    },
    Failed {
        reason: String,
        remedy: String,
    },
}

impl OnboardingState {
    /// The label the dashboard puts on the primary button.
    pub fn action_label(&self) -> &'static str {
        match self {
            Self::Discovered => "Connect",
            Self::Connecting => "Connectingâ€¦",
            Self::AwaitingConsent => "Waiting for Allowâ€¦",
            Self::Connected => "Check app",
            Self::NeedsInstall { .. } => "Install",
            Self::Installing => "Installingâ€¦",
            Self::NeedsEnrol { .. } => "Enrol",
            Self::Ready { .. } => "Ready",
            Self::Failed { .. } => "Retry",
        }
    }

    pub fn is_terminal(&self) -> bool {
        matches!(self, Self::Ready { .. } | Self::Failed { .. })
    }
}

/// One device as the dashboard sees it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct DeviceOnboarding {
    pub id: String,
    pub display_name: String,
    pub address: String,
    pub transport: Transport,
    pub state: OnboardingState,
    pub updated_epoch_ms: u64,
}

/// Drives onboarding for every discovered device.
pub struct OnboardingService {
    adb: Arc<dyn AdbControl>,
    discovery: Arc<LanDiscovery>,
    records: Mutex<HashMap<String, DeviceOnboarding>>,
    /// The APK to install, when the desktop app ships one.
    worker_apk: Option<String>,
    /// The version that APK carries, as read at build time.
    worker_version: String,
    consent_timeout: Duration,
    consent_poll: Duration,
}

impl OnboardingService {
    pub fn new(
        adb: Arc<dyn AdbControl>,
        discovery: Arc<LanDiscovery>,
        worker_version: impl Into<String>,
    ) -> Self {
        Self {
            adb,
            discovery,
            records: Mutex::new(HashMap::new()),
            worker_apk: None,
            worker_version: worker_version.into(),
            consent_timeout: CONSENT_TIMEOUT,
            consent_poll: CONSENT_POLL,
        }
    }

    /// The APK the Install action pushes. Without it, Install reports that the
    /// build is missing rather than pretending to work.
    pub fn with_worker_apk(mut self, path: impl Into<String>) -> Self {
        self.worker_apk = Some(path.into());
        self
    }

    /// Shrinks the consent wait. Only tests should need this.
    pub fn with_consent_wait(mut self, timeout: Duration, poll: Duration) -> Self {
        self.consent_timeout = timeout;
        self.consent_poll = poll;
        self
    }

    /// Every device, whether or not onboarding has started.
    pub fn records(&self) -> Vec<DeviceOnboarding> {
        self.sync_from_discovery();
        let mut records: Vec<DeviceOnboarding> =
            self.records.lock().unwrap().values().cloned().collect();
        records.sort_by(|a, b| {
            a.display_name
                .to_lowercase()
                .cmp(&b.display_name.to_lowercase())
                .then_with(|| a.id.cmp(&b.id))
        });
        records
    }

    pub fn record(&self, id: &str) -> Option<DeviceOnboarding> {
        self.sync_from_discovery();
        self.records.lock().unwrap().get(id).cloned()
    }

    /// Adds entries for devices discovery has seen but onboarding has not.
    /// Existing states are never overwritten: a `NeedsInstall` device must not
    /// slide back to `Discovered` because mDNS re-announced it.
    fn sync_from_discovery(&self) {
        let discovered = self.discovery.list();
        let mut records = self.records.lock().unwrap();
        for device in discovered {
            records
                .entry(device.id.clone())
                .or_insert_with(|| DeviceOnboarding {
                    id: device.id,
                    display_name: device.display_name,
                    address: device.address,
                    transport: device.transport,
                    state: OnboardingState::Discovered,
                    updated_epoch_ms: now_epoch_ms(),
                });
        }
    }

    /// Connects to a device and works out whether its worker app is usable.
    ///
    /// Blocks for up to [`CONSENT_TIMEOUT`] while the user decides whether to
    /// trust this laptop.
    pub fn connect(&self, id: &str) -> DeviceOnboarding {
        let device = match self.lookup(id) {
            Some(device) => device,
            None => return self.unknown(id),
        };

        self.store(&device, OnboardingState::Connecting);

        // A USB device is already reachable; `adb connect` would be a no-op at
        // best and a wrong-address failure at worst.
        if device.transport != Transport::Usb {
            match self.adb.connect(&device.address) {
                ConnectOutcome::Connected => {}
                ConnectOutcome::Unauthorized => return self.await_consent(device),
                other => {
                    let remedy = other
                        .remedy()
                        .unwrap_or_else(|| "adb could not reach the phone.".into());
                    return self.store(
                        &device,
                        OnboardingState::Failed {
                            reason: "The phone did not answer over ADB.".into(),
                            remedy,
                        },
                    );
                }
            }
        }

        self.check_worker(device)
    }

    /// Waits for the phone's own consent dialog, then continues.
    fn await_consent(&self, device: DeviceOnboarding) -> DeviceOnboarding {
        self.store(&device, OnboardingState::AwaitingConsent);
        let deadline = Instant::now() + self.consent_timeout;

        loop {
            if !self.consent_poll.is_zero() {
                std::thread::sleep(self.consent_poll);
            }
            if let Ok(entries) = self.adb.devices() {
                if let Some(entry) = entries
                    .iter()
                    .find(|entry| entry.serial == device.address || entry.serial == device.id)
                {
                    if entry.state == "device" {
                        return self.check_worker(device);
                    }
                }
            }
            if Instant::now() >= deadline {
                return self.store(
                    &device,
                    OnboardingState::Failed {
                        reason: "The phone never approved this laptop.".into(),
                        remedy: "Approve the â€œAllow USB debugging?â€ prompt on the phone, then \
                                 retry."
                            .into(),
                    },
                );
            }
        }
    }

    /// Compares the installed worker with the one this build ships.
    fn check_worker(&self, device: DeviceOnboarding) -> DeviceOnboarding {
        self.store(&device, OnboardingState::Connected);

        match self.adb.version_name() {
            Ok(Some(installed)) if installed == self.worker_version => {
                self.store(&device, OnboardingState::NeedsEnrol { installed })
            }
            Ok(installed) => self.store(
                &device,
                OnboardingState::NeedsInstall {
                    installed,
                    available: self.worker_version.clone(),
                },
            ),
            Err(error) => self.store(
                &device,
                OnboardingState::Failed {
                    reason: "Could not read the worker app's version.".into(),
                    remedy: error,
                },
            ),
        }
    }

    /// Installs or updates the worker app. Always a deliberate action.
    pub fn install(&self, id: &str) -> DeviceOnboarding {
        let device = match self.lookup(id) {
            Some(device) => device,
            None => return self.unknown(id),
        };
        let Some(apk) = self.worker_apk.clone() else {
            return self.store(
                &device,
                OnboardingState::Failed {
                    reason: "This build does not ship the worker app.".into(),
                    remedy: "Build the worker APK and bundle it, or install it by hand with \
                             adb install."
                        .into(),
                },
            );
        };

        self.store(&device, OnboardingState::Installing);
        match self.adb.install(&apk) {
            Ok(()) => self.check_worker(device),
            Err(remedy) => self.store(
                &device,
                OnboardingState::Failed {
                    reason: "The worker app could not be installed.".into(),
                    remedy,
                },
            ),
        }
    }

    /// Marks a device enrolled. Called by the API layer once the PIN hand-off
    /// succeeds, so `Ready` is never claimed on the strength of an install.
    pub fn mark_ready(&self, id: &str, node_id: &str) -> Option<DeviceOnboarding> {
        let device = self.lookup(id)?;
        Some(self.store(
            &device,
            OnboardingState::Ready {
                node_id: node_id.to_string(),
            },
        ))
    }

    /// Drops the ADB connection and forgets the record.
    ///
    /// Removing the enrolment itself belongs to the API layer, which owns the
    /// trust store; this only stops the control-plane connection.
    pub fn disconnect(&self, id: &str) -> Result<(), String> {
        let device = self
            .lookup(id)
            .ok_or_else(|| format!("unknown device {id}"))?;
        self.adb.disconnect(&device.address)?;
        self.records.lock().unwrap().remove(id);
        Ok(())
    }

    /// Puts a USB-attached device into TCP mode so it can be unplugged.
    pub fn enable_wireless(&self, id: &str, port: u16) -> Result<(), String> {
        let device = self
            .lookup(id)
            .ok_or_else(|| format!("unknown device {id}"))?;
        if device.transport != Transport::Usb {
            return Err("the phone must be attached over USB to enable wireless ADB".into());
        }
        self.adb.tcpip(&device.id, port)
    }

    fn lookup(&self, id: &str) -> Option<DeviceOnboarding> {
        self.sync_from_discovery();
        if let Some(record) = self.records.lock().unwrap().get(id).cloned() {
            return Some(record);
        }
        self.discovery
            .list()
            .into_iter()
            .find(|device| device.id == id)
            .map(|device| DeviceOnboarding {
                id: device.id,
                display_name: device.display_name,
                address: device.address,
                transport: device.transport,
                state: OnboardingState::Discovered,
                updated_epoch_ms: now_epoch_ms(),
            })
    }

    fn unknown(&self, id: &str) -> DeviceOnboarding {
        DeviceOnboarding {
            id: id.to_string(),
            display_name: id.to_string(),
            address: String::new(),
            transport: Transport::Usb,
            state: OnboardingState::Failed {
                reason: "That device is no longer visible.".into(),
                remedy: "Run discovery again and pick it from the list.".into(),
            },
            updated_epoch_ms: now_epoch_ms(),
        }
    }

    fn store(&self, device: &DeviceOnboarding, state: OnboardingState) -> DeviceOnboarding {
        let record = DeviceOnboarding {
            id: device.id.clone(),
            display_name: device.display_name.clone(),
            address: device.address.clone(),
            transport: device.transport,
            state,
            updated_epoch_ms: now_epoch_ms(),
        };
        self.records
            .lock()
            .unwrap()
            .insert(record.id.clone(), record.clone());
        record
    }
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
    use crate::adb::wireless::DeviceEntry;
    use crate::discovery::lan::Sighting;
    use std::collections::VecDeque;

    /// A scriptable adb. Every branch of the state machine is reachable from a
    /// test without a phone or a subprocess.
    #[derive(Default)]
    struct FakeAdb {
        connect: Mutex<VecDeque<ConnectOutcome>>,
        devices: Mutex<VecDeque<Vec<DeviceEntry>>>,
        version: Mutex<Option<String>>,
        install: Mutex<Option<Result<(), String>>>,
        disconnected: Mutex<Vec<String>>,
        tcpip: Mutex<Vec<(String, u16)>>,
    }

    impl FakeAdb {
        fn with_connect(outcome: ConnectOutcome) -> Self {
            let fake = Self::default();
            fake.connect.lock().unwrap().push_back(outcome);
            fake
        }

        fn with_version(self, version: Option<&str>) -> Self {
            *self.version.lock().unwrap() = version.map(str::to_string);
            self
        }

        fn with_devices(self, rounds: Vec<Vec<DeviceEntry>>) -> Self {
            let mut queue = self.devices.lock().unwrap();
            for round in rounds {
                queue.push_back(round);
            }
            drop(queue);
            self
        }

        fn with_install(self, result: Result<(), String>) -> Self {
            *self.install.lock().unwrap() = Some(result);
            self
        }
    }

    impl AdbControl for FakeAdb {
        fn connect(&self, _addr: &str) -> ConnectOutcome {
            self.connect
                .lock()
                .unwrap()
                .pop_front()
                .unwrap_or(ConnectOutcome::Connected)
        }

        fn disconnect(&self, addr: &str) -> Result<(), String> {
            self.disconnected.lock().unwrap().push(addr.to_string());
            Ok(())
        }

        fn tcpip(&self, serial: &str, port: u16) -> Result<(), String> {
            self.tcpip.lock().unwrap().push((serial.to_string(), port));
            Ok(())
        }

        fn install(&self, _apk: &str) -> Result<(), String> {
            self.install.lock().unwrap().clone().unwrap_or(Ok(()))
        }

        fn version_name(&self) -> Result<Option<String>, String> {
            Ok(self.version.lock().unwrap().clone())
        }

        fn devices(&self) -> Result<Vec<DeviceEntry>, String> {
            Ok(self.devices.lock().unwrap().pop_front().unwrap_or_default())
        }
    }

    fn discovery_with(id: &str, transport: Transport) -> Arc<LanDiscovery> {
        let discovery = Arc::new(LanDiscovery::new());
        discovery.upsert(Sighting {
            key: id.to_string(),
            display_name: "redmi".to_string(),
            address: format!("{id}:5555"),
            transport,
            authorized: None,
        });
        discovery
    }

    fn service(adb: FakeAdb, discovery: Arc<LanDiscovery>) -> OnboardingService {
        OnboardingService::new(Arc::new(adb), discovery, "0.1.0")
            .with_consent_wait(Duration::ZERO, Duration::ZERO)
    }

    #[test]
    fn a_usb_device_with_the_right_app_needs_only_enrolment() {
        let adb = FakeAdb::default().with_version(Some("0.1.0"));
        let service = service(adb, discovery_with("16284c49", Transport::Usb));

        let record = service.connect("16284c49");

        assert_eq!(
            record.state,
            OnboardingState::NeedsEnrol {
                installed: "0.1.0".into()
            },
            "installed and reachable is not the same as enrolled"
        );
    }

    #[test]
    fn a_phone_without_the_worker_is_asked_to_install_it() {
        let adb = FakeAdb::default().with_version(None);
        let service = service(adb, discovery_with("16284c49", Transport::Usb));

        let record = service.connect("16284c49");

        assert_eq!(
            record.state,
            OnboardingState::NeedsInstall {
                installed: None,
                available: "0.1.0".into()
            }
        );
    }

    #[test]
    fn an_older_worker_is_flagged_for_update_not_silently_replaced() {
        let adb = FakeAdb::default().with_version(Some("0.0.9"));
        let service = service(adb, discovery_with("16284c49", Transport::Usb));

        let record = service.connect("16284c49");

        assert_eq!(
            record.state,
            OnboardingState::NeedsInstall {
                installed: Some("0.0.9".into()),
                available: "0.1.0".into()
            }
        );
    }

    #[test]
    fn an_unauthorized_phone_waits_and_then_continues_when_consent_arrives() {
        let adb = FakeAdb::with_connect(ConnectOutcome::Unauthorized)
            .with_version(Some("0.1.0"))
            .with_devices(vec![
                // First poll: still undecided.
                vec![DeviceEntry {
                    serial: "192.168.1.13:5555".into(),
                    state: "unauthorized".into(),
                }],
                // Second poll: the user tapped Allow.
                vec![DeviceEntry {
                    serial: "192.168.1.13:5555".into(),
                    state: "device".into(),
                }],
            ]);
        let service = OnboardingService::new(
            Arc::new(adb),
            discovery_with("192.168.1.13:5555", Transport::WifiAdb),
            "0.1.0",
        )
        // Long enough for the second poll, short enough to keep the suite fast.
        .with_consent_wait(Duration::from_secs(1), Duration::ZERO);

        let record = service.connect("192.168.1.13:5555");

        assert_eq!(
            record.state,
            OnboardingState::NeedsEnrol {
                installed: "0.1.0".into()
            }
        );
    }

    #[test]
    fn consent_that_never_arrives_fails_with_the_prompt_remedy() {
        let adb = FakeAdb::with_connect(ConnectOutcome::Unauthorized);
        let service = service(adb, discovery_with("192.168.1.13:5555", Transport::WifiAdb));

        let record = service.connect("192.168.1.13:5555");

        match record.state {
            OnboardingState::Failed { remedy, .. } => {
                assert!(remedy.contains("Allow USB debugging"), "{remedy}")
            }
            other => panic!("expected a failure, got {other:?}"),
        }
    }

    #[test]
    fn a_refused_connection_tells_the_user_to_replug_the_cable() {
        let adb = FakeAdb::with_connect(ConnectOutcome::Refused("refused".into()));
        let service = service(adb, discovery_with("192.168.1.13:5555", Transport::WifiAdb));

        let record = service.connect("192.168.1.13:5555");

        match record.state {
            OnboardingState::Failed { remedy, .. } => {
                assert!(remedy.contains("reboot"), "{remedy}")
            }
            other => panic!("expected a failure, got {other:?}"),
        }
    }

    #[test]
    fn installing_without_a_bundled_apk_says_so_instead_of_pretending() {
        let adb = FakeAdb::default();
        let service = service(adb, discovery_with("16284c49", Transport::Usb));

        let record = service.install("16284c49");

        match record.state {
            OnboardingState::Failed { reason, .. } => assert!(reason.contains("does not ship")),
            other => panic!("expected a failure, got {other:?}"),
        }
    }

    #[test]
    fn a_successful_install_rechecks_the_version() {
        let adb = FakeAdb::default()
            .with_version(Some("0.1.0"))
            .with_install(Ok(()));
        let service =
            service(adb, discovery_with("16284c49", Transport::Usb)).with_worker_apk("worker.apk");

        let record = service.install("16284c49");

        assert_eq!(
            record.state,
            OnboardingState::NeedsEnrol {
                installed: "0.1.0".into()
            }
        );
    }

    #[test]
    fn a_failed_install_keeps_adb_remedy_text() {
        let adb = FakeAdb::default().with_install(Err(
            "The phone is out of storage; free some space and retry.".into(),
        ));
        let service =
            service(adb, discovery_with("16284c49", Transport::Usb)).with_worker_apk("worker.apk");

        let record = service.install("16284c49");

        match record.state {
            OnboardingState::Failed { remedy, .. } => assert!(remedy.contains("storage")),
            other => panic!("expected a failure, got {other:?}"),
        }
    }

    #[test]
    fn discovery_seeds_records_without_clobbering_progress() {
        let adb = FakeAdb::default().with_version(None);
        let discovery = discovery_with("16284c49", Transport::Usb);
        let service = service(adb, discovery.clone());

        service.connect("16284c49");
        discovery.upsert(Sighting {
            key: "second".into(),
            display_name: "second".into(),
            address: "second:5555".into(),
            transport: Transport::WifiAdb,
            authorized: None,
        });

        let records = service.records();
        assert_eq!(records.len(), 2);
        let first = records.iter().find(|r| r.id == "16284c49").unwrap();
        assert!(
            matches!(first.state, OnboardingState::NeedsInstall { .. }),
            "a re-announced device must not slide back to Discovered: {:?}",
            first.state
        );
    }

    #[test]
    fn disconnecting_returns_the_device_to_discovered() {
        let adb = FakeAdb::default();
        let service = service(adb, discovery_with("192.168.1.13:5555", Transport::WifiAdb));

        service.connect("192.168.1.13:5555");
        service.disconnect("192.168.1.13:5555").expect("disconnect");

        let record = service
            .record("192.168.1.13:5555")
            .expect("the phone is still on the network");
        assert_eq!(
            record.state,
            OnboardingState::Discovered,
            "the ADB connection is gone, but discovery still sees the phone"
        );
    }

    #[test]
    fn enabling_wireless_refuses_a_device_that_is_not_on_usb() {
        let adb = FakeAdb::default();
        let service = service(adb, discovery_with("192.168.1.13:5555", Transport::WifiAdb));

        let error = service
            .enable_wireless("192.168.1.13:5555", 5555)
            .expect_err("must refuse");
        assert!(error.contains("USB"), "{error}");
    }

    #[test]
    fn an_unknown_device_reports_that_it_is_gone() {
        let service = service(FakeAdb::default(), Arc::new(LanDiscovery::new()));

        let record = service.connect("not-a-device");

        assert!(matches!(record.state, OnboardingState::Failed { .. }));
    }
}
