//! Wireless ADB: reaching a phone over the network instead of a cable.
//!
//! Two on-ramps exist and they are not interchangeable:
//!
//! - **`adb tcpip`** — works on every Android version, needs one USB plug, and
//!   stops listening when the phone reboots.
//! - **Wireless debugging** (Android 11+) — no cable, advertised over mDNS, and
//!   survives a reboot once enabled.
//!
//! Everything here is a thin wrapper over [`AdbBinary`]. The parsing is split
//! into pure functions ([`parse_devices`], [`classify_connect`],
//! [`parse_version_name`]) so it can be tested against captured real output
//! without a device or a subprocess.

use super::binary::AdbBinary;

/// The package id of the worker app.
pub const WORKER_PACKAGE: &str = "org.aiupcycler.worker";

/// What a connection attempt means for the user.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConnectOutcome {
    /// `adb connect` succeeded *and* `adb devices` reports `device`.
    Connected,
    /// The phone has not authorised this laptop's key, so Android is (or will
    /// be) showing its own "Allow USB debugging?" dialog.
    Unauthorized,
    /// The phone is not listening, refused the connection, or the address is
    /// wrong.
    Refused(String),
    /// adb could not run, or answered something unrecognised.
    Failed(String),
}

impl ConnectOutcome {
    /// The sentence the dashboard shows when this is not a success.
    pub fn remedy(&self) -> Option<String> {
        match self {
            Self::Connected => None,
            Self::Unauthorized => {
                Some("Approve the “Allow USB debugging?” prompt on the phone, then retry.".into())
            }
            Self::Refused(_) => Some(
                "The phone is not listening. If it was rebooted, connect USB once to \
                 re-enable wireless ADB."
                    .into(),
            ),
            Self::Failed(detail) => Some(detail.clone()),
        }
    }
}

/// One line of `adb devices`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeviceEntry {
    pub serial: String,
    pub state: String,
}

/// Parses `adb devices` output.
///
/// The first line is a header; blank lines are padding. Everything else is
/// `serial<TAB>state`, and a serial may itself contain colons (`ip:port`).
pub fn parse_devices(output: &str) -> Vec<DeviceEntry> {
    output
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .filter(|line| !line.starts_with("List of devices"))
        .filter(|line| !line.starts_with('*'))
        .filter_map(|line| {
            let mut fields = line.split_whitespace();
            let serial = fields.next()?;
            let state = fields.next()?;
            Some(DeviceEntry {
                serial: serial.to_string(),
                state: state.to_string(),
            })
        })
        .collect()
}

/// Classifies the immediate output of `adb connect`.
///
/// A `Connected` result here only means adb *accepted* the address; the caller
/// still has to read `adb devices` to learn whether the phone authorised this
/// laptop. [`WirelessAdb::connect`] does exactly that.
pub fn classify_connect(stdout: &str, stderr: &str) -> ConnectOutcome {
    let text = format!("{stdout}\n{stderr}").to_lowercase();

    if text.contains("failed to authenticate") {
        return ConnectOutcome::Unauthorized;
    }
    if text.contains("cannot connect") || text.contains("failed to connect") {
        return ConnectOutcome::Refused(first_meaningful_line(stderr, stdout));
    }
    if text.contains("connected to") {
        return ConnectOutcome::Connected;
    }
    ConnectOutcome::Failed(first_meaningful_line(stderr, stdout))
}

fn first_meaningful_line(stderr: &str, stdout: &str) -> String {
    stderr
        .lines()
        .chain(stdout.lines())
        .map(str::trim)
        .find(|line| !line.is_empty())
        .unwrap_or("adb gave no output")
        .to_string()
}

/// Extracts `versionName=` from `dumpsys package <pkg>`.
pub fn parse_version_name(output: &str) -> Option<String> {
    output
        .lines()
        .map(str::trim)
        .find_map(|line| line.strip_prefix("versionName="))
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

/// Turns an `adb install` failure into something a person can act on.
pub fn install_remedy(stderr: &str) -> String {
    let text = stderr.to_lowercase();
    if text.contains("install_failed_version_downgrade") {
        return "The phone has a newer worker app installed. Uninstall it, or use a build \
                with a higher versionCode."
            .into();
    }
    if text.contains("install_failed_insufficient_storage") {
        return "The phone is out of storage; free some space and retry.".into();
    }
    if text.contains("install_failed_update_incompatible") {
        return "The installed worker app was signed with a different key. Uninstall it first."
            .into();
    }
    if text.contains("install_failed_internal_error") {
        return "Android rejected the install (INSTALL_FAILED_INTERNAL_ERROR). Reboot the phone \
                and retry."
            .into();
    }
    first_meaningful_line(stderr, "")
}

/// Wireless operations against one resolved adb binary.
pub struct WirelessAdb {
    adb: AdbBinary,
}

impl WirelessAdb {
    pub fn new(adb: AdbBinary) -> Self {
        Self { adb }
    }

    /// Convenience for a binary at a known path.
    pub fn at(path: impl Into<std::path::PathBuf>) -> Self {
        Self::new(AdbBinary::at(path))
    }

    /// Resolves adb the way the app does at startup.
    pub fn resolve() -> Result<Self, String> {
        AdbBinary::resolve().map(Self::new)
    }

    /// Connects to `addr` (`ip:port`) and reports whether the phone is usable.
    ///
    /// `adb connect` succeeds even when the phone is unauthorized, so the
    /// device list is the authority on the result.
    pub fn connect(&self, addr: &str) -> ConnectOutcome {
        let output = match self.adb.run(&["connect", addr]) {
            Ok(output) => output,
            Err(error) => return ConnectOutcome::Failed(error),
        };
        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);

        match classify_connect(&stdout, &stderr) {
            // adb accepted the address; the phone decides whether it is usable.
            ConnectOutcome::Connected => self.settled_state(addr),
            other => other,
        }
    }

    /// Reads `adb devices` and reports the state `addr` settled into.
    fn settled_state(&self, addr: &str) -> ConnectOutcome {
        match self.adb.run_text(&["devices"]) {
            Ok(output) => match parse_devices(&output)
                .into_iter()
                .find(|entry| entry.serial == addr)
            {
                Some(entry) if entry.state == "device" => ConnectOutcome::Connected,
                Some(entry) if entry.state == "unauthorized" => ConnectOutcome::Unauthorized,
                Some(entry) => {
                    ConnectOutcome::Refused(format!("the phone reports state {:?}", entry.state))
                }
                None => ConnectOutcome::Refused(
                    "adb accepted the address but does not list the device".into(),
                ),
            },
            Err(error) => ConnectOutcome::Failed(error),
        }
    }

    pub fn disconnect(&self, addr: &str) -> Result<(), String> {
        self.adb.run_text(&["disconnect", addr]).map(|_| ())
    }

    /// Puts an attached device into TCP mode on `port`.
    ///
    /// This is the cable-assisted path: it needs the device to be attached over
    /// USB right now, and the setting is lost when the phone reboots.
    pub fn tcpip(&self, serial: &str, port: u16) -> Result<(), String> {
        self.adb
            .run_text(&["-s", serial, "tcpip", &port.to_string()])
            .map(|_| ())
    }

    /// Pairs with Android 11+ Wireless debugging.
    ///
    /// `addr` is the pairing address the phone displays, and `code` is the
    /// six-digit code shown beside it.
    pub fn pair(&self, addr: &str, code: &str) -> Result<(), String> {
        self.adb.run_text(&["pair", addr, code]).map(|_| ())
    }

    /// Installs or updates the worker app, mapping Android's error codes to
    /// remedies.
    pub fn install(&self, apk: &str) -> Result<(), String> {
        let output = self.adb.run(&["install", "-r", "-d", apk])?;
        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);
        if output.status.success() && stdout.contains("Success") {
            return Ok(());
        }
        Err(install_remedy(&format!("{stderr}\n{stdout}")))
    }

    /// The installed worker version, or `None` when the package is absent.
    pub fn version_name(&self) -> Result<Option<String>, String> {
        let output = self
            .adb
            .run(&["shell", "dumpsys", "package", WORKER_PACKAGE])?;
        let stdout = String::from_utf8_lossy(&output.stdout);
        Ok(parse_version_name(&stdout))
    }

    /// Every device adb currently knows about, USB and network alike.
    pub fn devices(&self) -> Result<Vec<DeviceEntry>, String> {
        self.adb
            .run_text(&["devices"])
            .map(|output| parse_devices(&output))
    }
}

/// The ADB operations onboarding needs.
///
/// A trait rather than concrete calls so the state machine is tested against a
/// `FakeAdb` in `cargo test` — no phone, no subprocess, and the same code paths
/// on every platform.
pub trait AdbControl: Send + Sync {
    fn connect(&self, addr: &str) -> ConnectOutcome;
    fn disconnect(&self, addr: &str) -> Result<(), String>;
    fn tcpip(&self, serial: &str, port: u16) -> Result<(), String>;
    fn install(&self, apk: &str) -> Result<(), String>;
    fn version_name(&self) -> Result<Option<String>, String>;
    fn devices(&self) -> Result<Vec<DeviceEntry>, String>;
}

impl AdbControl for WirelessAdb {
    fn connect(&self, addr: &str) -> ConnectOutcome {
        WirelessAdb::connect(self, addr)
    }

    fn disconnect(&self, addr: &str) -> Result<(), String> {
        WirelessAdb::disconnect(self, addr)
    }

    fn tcpip(&self, serial: &str, port: u16) -> Result<(), String> {
        WirelessAdb::tcpip(self, serial, port)
    }

    fn install(&self, apk: &str) -> Result<(), String> {
        WirelessAdb::install(self, apk)
    }

    fn version_name(&self) -> Result<Option<String>, String> {
        WirelessAdb::version_name(self)
    }

    fn devices(&self) -> Result<Vec<DeviceEntry>, String> {
        WirelessAdb::devices(self)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_the_device_list() {
        let output = "List of devices attached\r\n\
                      16284c49\tdevice\r\n\
                      192.168.1.13:5555\tunauthorized\r\n\
                      \r\n";
        assert_eq!(
            parse_devices(output),
            vec![
                DeviceEntry {
                    serial: "16284c49".into(),
                    state: "device".into()
                },
                DeviceEntry {
                    serial: "192.168.1.13:5555".into(),
                    state: "unauthorized".into()
                },
            ]
        );
    }

    #[test]
    fn a_device_list_with_no_devices_is_empty_not_an_error() {
        assert!(parse_devices("List of devices attached\n\n").is_empty());
        assert!(parse_devices("").is_empty());
    }

    #[test]
    fn daemon_startup_noise_is_not_a_device() {
        let output = "* daemon not running; starting now at tcp:5037\n\
                      * daemon started successfully\n\
                      List of devices attached\n\
                      16284c49\tdevice\n";
        let devices = parse_devices(output);
        assert_eq!(devices.len(), 1, "{devices:?}");
        assert_eq!(devices[0].serial, "16284c49");
    }

    #[test]
    fn classifies_connection_output() {
        assert_eq!(
            classify_connect("connected to 192.168.1.13:5555\n", ""),
            ConnectOutcome::Connected
        );
        assert_eq!(
            classify_connect("already connected to 192.168.1.13:5555\n", ""),
            ConnectOutcome::Connected
        );
        assert!(matches!(
            classify_connect(
                "",
                "cannot connect to 192.168.1.13:5555: No connection could be made because the \
                 target machine actively refused it. (10061)"
            ),
            ConnectOutcome::Refused(detail) if detail.contains("cannot connect")
        ));
        assert_eq!(
            classify_connect("", "failed to authenticate to 192.168.1.13:5555"),
            ConnectOutcome::Unauthorized
        );
    }

    #[test]
    fn an_unrecognised_answer_is_a_failure_not_a_success() {
        // Silence must never be read as "connected": that would send the UI on
        // to install against a device that is not there.
        assert!(matches!(
            classify_connect("", ""),
            ConnectOutcome::Failed(_)
        ));
    }

    #[test]
    fn unauthorized_explains_the_prompt_on_the_phone() {
        let remedy = ConnectOutcome::Unauthorized.remedy().expect("has a remedy");
        assert!(remedy.contains("Allow USB debugging"), "{remedy}");
    }

    #[test]
    fn a_refusal_tells_the_user_about_the_reboot_case() {
        let remedy = ConnectOutcome::Refused("refused".into())
            .remedy()
            .expect("has a remedy");
        assert!(remedy.contains("reboot"), "{remedy}");
    }

    #[test]
    fn parses_the_installed_version() {
        let output = "Packages:\n  Package [org.aiupcycler.worker] (abc):\n    \
                      versionCode=3 minSdk=26 targetSdk=34\n    versionName=0.1.0\n";
        assert_eq!(parse_version_name(output), Some("0.1.0".to_string()));
        assert_eq!(parse_version_name("Packages:"), None);
        assert_eq!(parse_version_name("versionName=\n"), None);
    }

    #[test]
    fn install_failures_become_actionable_sentences() {
        assert!(
            install_remedy("Failure [INSTALL_FAILED_INSUFFICIENT_STORAGE]").contains("storage")
        );
        assert!(
            install_remedy("Failure [INSTALL_FAILED_UPDATE_INCOMPATIBLE]")
                .contains("different key")
        );
        assert!(install_remedy("Failure [INSTALL_FAILED_VERSION_DOWNGRADE]")
            .contains("newer worker app"));
        // An unknown failure still says something rather than an empty string.
        assert!(!install_remedy("something odd").is_empty());
    }
}
