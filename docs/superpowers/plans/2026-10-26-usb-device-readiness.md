# USB Device Readiness Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make a USB-paired Android worker reach the existing health path so the dashboard shows its live battery and temperature instead of `—`.

**Architecture:** Record the ADB serial as an enrollment alias when the dashboard pairing session redeems, and make the ADB bootstrap probe use the same enrollment-aware transport source as the health poller. The phone keeps serving mTLS after pairing; the master no longer probes it with a plaintext handshake.

**Tech Stack:** Rust (core + Tauri shell), tonic gRPC, axum, tokio; existing Android worker and React UI unchanged.

**Spec:** `docs/superpowers/specs/2026-10-26-usb-device-readiness-design.md`

## Global Constraints

- Android only; iOS is deferred.
- Do not modify or restart anything on `:8080` except the AI Upcycler app itself.
- Build artifacts go to `D:\ai-upcycler-target`; `C:` has under 1 GB free.
- Host Rust builds use `RUSTUP_TOOLCHAIN=stable-x86_64-pc-windows-gnu`, MSYS2 x86_64 tools, and `CARGO_TARGET_DIR=D:\ai-upcycler-target`.
- Phone taps are manual. ADB input injection is unavailable; never claim a tap was automated.
- Keep tests portable; no test may require a phone.
- Report hardware-gated results only when observed. `null` battery/temperature is honest when the sensor is unreadable.
- No proto change and no Android APK change in this slice.

---

### Task 1: Enroll the ADB serial alias

**Files:**
- Modify: `core/src/adb/mod.rs` (`PairingTunnelRegistry`)
- Modify: `core/src/api/pairing.rs` (`enroll`, helper, tests)

**Interfaces:**
- Produces: `PairingTunnelRegistry::serial_for_pin(&self, pin: &str) -> Option<String>`
- Produces: `enrollment_ids(node_id: &str, serial: Option<&str>) -> Vec<String>`
- Consumes: `AppState::pairing_tunnels` and `AppState::security.enrollments`

- [ ] **Step 1: Write the failing registry test**

Add to the existing `#[cfg(test)] mod tests` in `core/src/adb/mod.rs`:

```rust
#[test]
fn serial_for_pin_finds_the_tracked_device() {
    let tunnels = PairingTunnelRegistry::default();
    tunnels.track("16284c49", "815896");

    assert_eq!(
        tunnels.serial_for_pin("815896"),
        Some("16284c49".to_string())
    );
    assert_eq!(tunnels.serial_for_pin("000000"), None);
}
```

- [ ] **Step 2: Run it to verify it fails**

Run:

```powershell
$env:RUSTUP_TOOLCHAIN="stable-x86_64-pc-windows-gnu"
$env:PATH="C:\msys64\mingw64\bin;" + $env:PATH
$env:DLLTOOL="C:\msys64\mingw64\bin\dlltool.exe"
$env:CC="C:\msys64\mingw64\bin\gcc.exe"
$env:AR="C:\msys64\mingw64\bin\ar.exe"
$env:CARGO_TARGET_X86_64_PC_WINDOWS_GNU_LINKER="C:\msys64\mingw64\bin\gcc.exe"
$env:CARGO_TARGET_DIR="D:\ai-upcycler-target"
cargo test -p ai-upcycler-core serial_for_pin
```

Expected: FAIL with `no method named serial_for_pin`.

- [ ] **Step 3: Implement `serial_for_pin`**

Add to `impl PairingTunnelRegistry` in `core/src/adb/mod.rs`, next to
`is_tracked`:

```rust
/// The ADB serial whose pairing tunnel currently backs `pin`, if any.
///
/// The enrollment handler uses this to record the `adb:<serial>` alias, so a
/// phone that enrolled under its mDNS fullname is also reachable by the USB
/// discovery identity.
pub fn serial_for_pin(&self, pin: &str) -> Option<String> {
    self.tunnels
        .lock()
        .expect("pairing tunnel lock")
        .iter()
        .find_map(|(serial, tracked)| (tracked == pin).then(|| serial.clone()))
}
```

- [ ] **Step 4: Write the failing enrollment helper test**

Add to `core/src/api/pairing.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::enrollment_ids;

    #[test]
    fn enrollment_ids_include_the_adb_alias_when_a_tunnel_is_tracked() {
        assert_eq!(
            enrollment_ids("Redmi._aiupcycler-worker._tcp.local.", Some("16284c49")),
            vec![
                "Redmi._aiupcycler-worker._tcp.local.".to_string(),
                "adb:16284c49".to_string(),
            ]
        );
    }

    #[test]
    fn enrollment_ids_do_not_duplicate_an_adb_node_id() {
        assert_eq!(
            enrollment_ids("adb:16284c49", Some("16284c49")),
            vec!["adb:16284c49".to_string()]
        );
        assert_eq!(
            enrollment_ids("phone.local.", None),
            vec!["phone.local.".to_string()]
        );
    }
}
```

- [ ] **Step 5: Run it to verify it fails**

Run:

```powershell
cargo test -p ai-upcycler-core enrollment_ids
```

Expected: FAIL with `cannot find function enrollment_ids`.

- [ ] **Step 6: Implement the helper and use it in `enroll`**

Add below `declared_addresses` in `core/src/api/pairing.rs`:

```rust
/// The enrollment keys for one completed pairing: the worker's declared
/// `node_id` plus the ADB serial alias when the dashboard created the session.
fn enrollment_ids(node_id: &str, serial: Option<&str>) -> Vec<String> {
    let mut ids = vec![node_id.to_string()];
    if let Some(serial) = serial {
        let alias = format!("adb:{serial}");
        if alias != node_id {
            ids.push(alias);
        }
    }
    ids
}
```

In `enroll`, capture the tracked serial **before** redeeming the PIN (the
cleanup task may purge a tunnel as soon as the session is redeemed), then
replace the single `state.security.enrollments.enroll(...)` call with:

```rust
    let serial = state.pairing_tunnels.serial_for_pin(req.pin.trim());

    state
        .security
        .pairing
        .redeem(req.pin.trim())
        .map_err(map_pairing_error)?;

    // ... issue certificates ...

    for id in enrollment_ids(node_id, serial.as_deref()) {
        state
            .security
            .enrollments
            .enroll(&id, device_name, addresses.clone());
    }
```

Keep the response construction unchanged.

- [ ] **Step 7: Run both tests**

```powershell
cargo test -p ai-upcycler-core serial_for_pin
cargo test -p ai-upcycler-core enrollment_ids
```

Expected: PASS.

- [ ] **Step 8: Commit**

```powershell
git add core/src/adb/mod.rs core/src/api/pairing.rs
git commit -m "fix(pairing): enroll the adb serial alias on redemption"
```

---

### Task 2: Dial the ADB probe with the enrollment-aware transport

**Files:**
- Modify: `core/src/adb/mod.rs` (imports, `start`, `probe_device`, tests)
- Modify: `src-tauri/src/lib.rs` (spawn the ADB task after `AppState` exists)

**Interfaces:**
- Consumes: `crate::rpc::engine::{dial_worker, WorkerTransportSource}`
- Produces: `AdbTunnelManager::start(registry, transports, shutdown)`
- Produces: `probe_device(transports, serial, local_port) -> Option<HandshakeResponse>`

- [ ] **Step 1: Write the failing probe test**

Add to the existing `#[cfg(test)] mod tests` in `core/src/adb/mod.rs`:

```rust
#[tokio::test]
async fn probe_uses_the_adb_alias_for_transport_lookup() {
    struct RecordingTransports {
        seen: std::sync::Mutex<Vec<String>>,
    }

    impl crate::rpc::engine::WorkerTransportSource for RecordingTransports {
        fn transport_for(
            &self,
            node_id: &str,
        ) -> Result<crate::rpc::engine::WorkerTransport, String> {
            self.seen.lock().unwrap().push(node_id.to_string());
            Err("stop before dialing".to_string())
        }
    }

    let transports = RecordingTransports {
        seen: std::sync::Mutex::new(Vec::new()),
    };
    let result = probe_device(&transports, "16284c49", 50052).await;

    assert!(result.is_none());
    assert_eq!(
        *transports.seen.lock().unwrap(),
        vec!["adb:16284c49".to_string()]
    );
}
```

- [ ] **Step 2: Run it to verify it fails**

```powershell
cargo test -p ai-upcycler-core probe_uses_the_adb_alias
```

Expected: FAIL because `probe_device` still takes only `local_port`.

- [ ] **Step 3: Change the imports and probe**

In `core/src/adb/mod.rs`, replace:

```rust
use crate::rpc::client::{WorkerRpcClient, MASTER_ID, PROTOCOL_VERSION};
use crate::rpc::engine::http_endpoint;
```

with:

```rust
use crate::rpc::client::{MASTER_ID, PROTOCOL_VERSION};
use crate::rpc::engine::{dial_worker, WorkerTransportSource};
```

Replace `probe_device` with:

```rust
async fn probe_device(
    transports: &dyn WorkerTransportSource,
    serial: &str,
    local_port: u16,
) -> Option<HandshakeResponse> {
    let node = DiscoveredNode {
        node_id: format!("adb:{serial}"),
        device_name: String::new(),
        ip_address: "127.0.0.1".to_string(),
        port: local_port,
        total_ram_bytes: 0,
        free_ram_bytes: 0,
        connection_type: "USB_ADB".to_string(),
        latency_ms: 0.0,
        last_seen_epoch_ms: now_epoch_ms(),
    };
    let mut client = dial_worker(transports, &node).await.ok()?;
    client.handshake(MASTER_ID, PROTOCOL_VERSION).await.ok()
}
```

- [ ] **Step 4: Pass the transport source through `start`**

Change the signature of `AdbTunnelManager::start`:

```rust
pub async fn start(
    registry: Arc<NodeRegistry>,
    transports: Arc<dyn WorkerTransportSource>,
    mut shutdown: tokio::sync::watch::Receiver<bool>,
) {
```

Change the probe call site to:

```rust
if let Some(handshake) =
    probe_device(transports.as_ref(), &device.serial, local_port).await
{
```

- [ ] **Step 5: Run the probe test**

```powershell
cargo test -p ai-upcycler-core probe_uses_the_adb_alias
```

Expected: PASS.

- [ ] **Step 6: Move the ADB task after `AppState` in the Tauri shell**

In `src-tauri/src/lib.rs`, remove the early ADB spawn:

```rust
let adb_registry = registry.clone();
tauri::async_runtime::spawn(async move {
    ai_upcycler_core::adb::AdbTunnelManager::start(adb_registry, adb_shutdown_rx).await;
});
```

Inside `.setup(...)`, after `app.manage(state.clone());`, add:

```rust
// The ADB probe must resolve the same enrollment-aware transport as the
// health poller; spawn it after the shared AppState (and its security state)
// exists.
let adb_registry = server_registry.clone();
let adb_transports = state.transports();
tauri::async_runtime::spawn(async move {
    ai_upcycler_core::adb::AdbTunnelManager::start(
        adb_registry,
        adb_transports,
        adb_shutdown_rx,
    )
    .await;
});
```

The existing `adb_shutdown_tx` stays outside for the exit handler.

- [ ] **Step 7: Check the workspace**

```powershell
cargo check -p ai-upcycler-core
cargo check -p ai-upcycler-desktop
```

Expected: both PASS. If the Tauri check complains about the moved
`adb_shutdown_rx`, confirm it is captured by the `move` setup closure and is
not used elsewhere.

- [ ] **Step 8: Commit**

```powershell
git add core/src/adb/mod.rs src-tauri/src/lib.rs
git commit -m "fix(adb): probe paired workers over the enrollment-aware transport"
```

---

### Task 3: Live USB verification with the connected phone

**Files:** none (runtime verification)

**Interfaces:**
- Consumes: the built app, `adb`, the installed `org.aiupcycler.worker` APK.

- [ ] **Step 1: Confirm the phone is attached and the worker app is installed**

```powershell
$adb = "D:\Android\Sdk\platform-tools\adb.exe"
& $adb devices -l
& $adb shell dumpsys package org.aiupcycler.worker | Select-String "versionName"
```

Expected: serial `16284c49` in `device` state; versionName `0.1.0`.

- [ ] **Step 2: Start the host app**

Use the documented environment, with ADB on `PATH`:

```powershell
$env:RUSTUP_TOOLCHAIN="stable-x86_64-pc-windows-gnu"
$env:RUSTUP_SKIP_UPDATE_CHECK="1"
$env:PATH="D:\Android\Sdk\platform-tools;C:\msys64\mingw64\bin;" + $env:PATH
$env:DLLTOOL="C:\msys64\mingw64\bin\dlltool.exe"
$env:CC="C:\msys64\mingw64\bin\gcc.exe"
$env:AR="C:\msys64\mingw64\bin\ar.exe"
$env:CARGO_TARGET_X86_64_PC_WINDOWS_GNU_LINKER="C:\msys64\mingw64\bin\gcc.exe"
$env:CARGO_TARGET_DIR="D:\ai-upcycler-target"
$env:CARGO_BUILD_JOBS="2"
$env:AI_UPCYCLER_MODELS_DIR="D:\ai-upcycler-models"
$env:AI_UPCYCLER_MODEL="D:\ai-upcycler-models\qwen2.5-coder-3b-instruct-q4_k_m.gguf"
$env:AI_UPCYCLER_LLAMA_SERVER="E:\AI Upcycler\runtime\engine\llama-server.exe"
$env:AI_UPCYCLER_CONTEXT="8192"
pnpm tauri dev
```

Run it in the background. Wait for the API and Vite to report ready.

- [ ] **Step 3: Launch the worker app and pair manually**

```powershell
& $adb shell am start -n org.aiupcycler.worker/.MainActivity
```

If the phone was paired to the previous host process, tap **Forget pairing**
on the phone. In the dashboard, enter serial `16284c49`, create the session,
and have the user enter the new PIN and the first 8 fingerprint characters in
the phone app. Phone taps are manual; do not inject input.

- [ ] **Step 4: Confirm the tunnel and the upgraded node**

```powershell
& $adb forward --list
Invoke-RestMethod http://127.0.0.1:8080/api/cluster/topology |
  ConvertTo-Json -Depth 8
```

Expected: a forward for `16284c49`; the `adb:16284c49` node has a non-zero
`ram_available_mb` and `worker_state` `paired` or `ready`.

- [ ] **Step 5: Capture a real `worker_health` event**

With the app running, use a WebSocket client (Node 22 has a global
`WebSocket`) and print the first health event:

```powershell
node -e "const ws=new WebSocket('ws://127.0.0.1:8080/api/telemetry'); ws.onmessage=(e)=>{const m=JSON.parse(e.data); if(m.type==='worker_health'){console.log(JSON.stringify(m.data,null,2)); ws.close();}}; setTimeout(()=>{ws.close(); process.exit(1)}, 15000);"
```

Expected: a `worker_health` payload containing the `adb:16284c49` node with
`battery_pct` and `temp_celsius` numbers, or `null` where the phone's sensor
is unreadable. Record exactly what was observed.

**OBSERVED 2026-10-26 22:23** (note: the envelope field is `event`, not
`type`):

```json
{"master_ram_utilization_pct":57.018998744690954,
 "nodes":[{"id":"adb:16284c49","temp_celsius":35.29999923706055,
           "battery_pct":100,"free_ram_bytes":2402373632,
           "observed_at_epoch_ms":1793033637962,"supports_compute":true}]}
```

Two consecutive events ~6 s apart plus one `node_connected`. Cross-checked
against the phone's own `BatteryService` log (`batteryTemperature351`,
`batteryLevel100`): 35.1 °C / 100 % agrees with 35.3 °C / 100 %.

**Scope caveat:** this was the *unpaired* plaintext path — the health poller
dials plaintext while `adb:16284c49` has no enrollment. The paired mTLS path
(Tasks 1 and 2) is not exercised until a pairing completes.

- [ ] **Step 6: Confirm the dashboard**

Open the dashboard and verify the phone card shows the same battery and
temperature values, or `—` for an unreadable sensor. Take a screenshot if
possible. Do not claim the values are shown if the UI was not observed.

- [ ] **Step 7: Record the result**

If the phone reaches `paired`/`ready` with a real health snapshot, update
`docs/tasks.md` §6's battery/temperature line with the observed values and
the date. If the probe still fails, keep the task open and capture the host
log line plus `adb logcat` output for the failure.

---

### Task 4: Local gates and commit

**Files:** none beyond the docs update from Task 3.

- [ ] **Step 1: Format**

```powershell
cargo fmt --all -- --check
```

Expected: PASS. If it fails, run `cargo fmt --all`, re-run the check, and
include the formatting-only diff in the commit.

- [ ] **Step 2: Run the core tests**

```powershell
cargo test -p ai-upcycler-core
```

Expected: PASS. The phone is not required for this command.

- [ ] **Step 3: Run clippy on the changed crates**

```powershell
cargo clippy -p ai-upcycler-core -p ai-upcycler-desktop --all-targets -- -D warnings
```

Expected: PASS. Fix warnings introduced by this change; do not silence
pre-existing unrelated warnings with new `allow` attributes.

- [ ] **Step 4: Commit the docs and any formatting changes**

```powershell
git add docs/tasks.md docs/superpowers/specs/2026-10-26-usb-device-readiness-design.md docs/superpowers/plans/2026-10-26-usb-device-readiness.md
git commit -m "docs: record USB device readiness evidence"
```

---

### Task 6: Re-assert the ADB tunnels (defect found in live verification)

Found while diagnosing "not able to pair": both adb tunnels had been dropped
behind the app's back, and nothing in the app ever recreated one.

**Files:**
- Modify: `core/src/adb/mod.rs` (`forward`, `ensure_forward`,
  `reverse_pairing`, `ensure_reverse_pairing`, `start`, `forward_args`,
  `pairing_tunnel_cleanup`, `PairingTunnelRegistry::active`, tests)

**Interfaces:**
- Produces: `AdbTunnelManager::ensure_forward(&self, serial, local_port) -> Result<(), String>`
- Produces: `AdbTunnelManager::ensure_reverse_pairing(&self, serial) -> Result<(), String>`
- Produces: `PairingTunnelRegistry::active(&self, pairing) -> Vec<String>`
- Produces: `forward_args(serial, local_port) -> Vec<String>`
- Consumes: `PairingManager::session_usable`

**Observed evidence (2026-10-26):**
- `adb forward --list` and `adb reverse --list` both empty while the phone
  still had `*:50062 LISTEN` and the host still had `8080`/`8081`.
- Host log: reverse opened at `16:09:42`, then at `16:14:44` the cleanup task
  found it already gone — `listener 'tcp:8081' not found`.
- `worker health poll timed out for adb:16284c49 after 2s`, 73 consecutive
  times and never a success.
- Restoring `adb forward tcp:50062 tcp:50062` by hand stopped the timeouts
  within one poll cycle: last timeout `16:18:35`, none in the 7 minutes to
  `16:25:34`.

**Root cause:** `start` trusted its own `forwards: HashMap`
(`Some(port) => *port`) and never checked the binding `adb` actually serves.
`adb` clears every forward and reverse when its server restarts or a device
re-enumerates; the map outlives them, so the loop kept dialing a dead tunnel
and never re-probed (the node already carries a port).

- [x] **Step 1: Pin the exact `adb` invocations**

Factor `forward_args` out of `forward`, alongside the existing
`reverse_pairing_args`, and test both ends of the forward:

```rust
#[test]
fn forward_args_pin_the_host_port_to_the_worker_rpc_port() {
    assert_eq!(
        forward_args("16284c49", 50062),
        vec!["-s", "16284c49", "forward", "tcp:50062", "tcp:50062"]
    );
}
```

- [x] **Step 2: Add the idempotent re-assert primitives**

`ensure_forward` / `ensure_reverse_pairing` run the same command as
`forward` / `reverse_pairing` but stay silent on success, because the poll
and cleanup loops call them on every tick where a live binding is the
expected result. `forward` and `reverse_pairing` now delegate to them and
keep the first-time `info!` line, so the create and re-assert paths cannot
drift apart.

- [x] **Step 3: Re-assert on every USB poll tick**

In `start`, take the port from the map (or `next_free_port` when new), then
call `ensure_forward` unconditionally before using it; `continue` on failure,
and log the first-time open only when the device was not already recorded.

- [x] **Step 4: Re-assert the reverse for live sessions**

Add `PairingTunnelRegistry::active` — the mirror of `purge_inactive`, the
sessions still redeemable — and have `pairing_tunnel_cleanup` call
`ensure_reverse_pairing` for each, alongside the existing teardown pass.

```rust
#[test]
fn active_lists_only_sessions_still_awaiting_enrollment() {
    let pairing = PairingManager::new(PAIRING_TTL);
    let (live, _, _) = pairing.create_session("fp");
    let (done, _, _) = pairing.create_session("fp");
    let registry = PairingTunnelRegistry::default();
    registry.track("16284c49", &live);
    registry.track("emulator-5554", &done);
    pairing.redeem(&done).expect("redeem");

    assert_eq!(registry.active(&pairing), vec!["16284c49"]);
}
```

- [x] **Step 5: Run the tests** — `cargo test -p ai-upcycler-core
  adb::tests` → **19 passed** (16 prior + the 3 added).

- [x] **Step 6: Prove the repair live** — OBSERVED 2026-10-26 22:21: with the
  rebuilt app running, `adb -s 16284c49 forward --remove tcp:50062` returned
  exit 0 and the immediate listing was empty; 0.9 s later still empty; at
  `22:21:27.557` (≈1.4 s) the app had recreated
  `16284c49 tcp:50062 tcp:50062` on its own. Contrast with the pre-fix
  observation, where the same teardown left the forward gone for 40+ minutes
  and produced 73 consecutive health timeouts until it was restored by hand.

**Note on why the two loops reconcile differently:** `adb forward --list`
keys a forward by the real serial (`16284c49`), but `adb reverse --list` keys
a reverse by the device *transport* (`UsbFfs tcp:8081 tcp:8081`). A reverse
therefore cannot be matched against that listing by serial, so both loops
simply re-issue the idempotent command instead of parsing it — the same one
subprocess a listing would cost.

---

## Self-Review

**Spec coverage:**
- D-H1 alias enrollment → Task 1.
- D-H2 enrollment-aware probe → Task 2.
- D-H3 health/UI unchanged → Task 3 verification.
- D-H4 honest scope → Global Constraints and Task 3 Step 7.
- Tunnel resilience (defect found during live verification) → Task 6.
- Live USB evidence → Task 3.
- Local gates → Task 4.

**Placeholder scan:** no TBD/TODO steps; every code step includes the code.

**Type consistency:** `serial_for_pin` returns `Option<String>`;
`enrollment_ids` takes `Option<&str>`; `probe_device` takes
`&dyn WorkerTransportSource`; `start` takes
`Arc<dyn WorkerTransportSource>`.
