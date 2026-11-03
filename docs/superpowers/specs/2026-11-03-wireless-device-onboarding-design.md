# Wireless Device Onboarding (Slice A) — Design

Date: 2026-11-03
Status: accepted
Supersedes: nothing
Related: `2026-10-20-android-mtls-pairing-design.md`, `docs/security.md`

## 1. Summary

Today a phone joins the cluster only over a USB cable. This slice makes the
cable a *setup* step rather than a permanent tether: the desktop app discovers
phones on the same network, connects to them over ADB, installs or updates the
worker app, and enrols them — after which the cluster runs over the existing
mTLS channel with no cable and no typed PIN.

The user-facing promise is: **click Connect once, tap Allow on the phone, and
the device is in the cluster.**

## 2. Goals

- Discover Android devices on the LAN that can be reached over ADB.
- Connect to one with a single click from the dashboard.
- Surface the Android "Allow USB debugging?" consent instead of pretending we
  can show our own dialog.
- Install or update the worker APK over ADB when it is missing or stale.
- Enrol the device into the cluster without the user typing a PIN.
- Let the user revoke a device, which removes its enrolment and disconnects it.
- Keep working on Android versions that predate Wireless debugging.

## 3. Non-goals

- **Model provisioning** (Slice B): pushing the GGUF, checking it on-device,
  resuming interrupted transfers. Explicitly out of scope.
- **Multi-device pooling** (Slice C): making the planner provisioning-aware and
  proving N-device inference. Out of scope.
- iOS. Out of scope as before.
- Replacing PIN pairing for users who prefer it. The PIN path stays.

## 4. What exists today

| Piece | Where | State |
|---|---|---|
| USB device list + port forward/reverse | `core/src/adb/mod.rs` | works |
| mDNS worker discovery | `core/src/discovery/mdns.rs` | works |
| PIN/QR pairing, mTLS identities | `core/src/security.rs` | works, verified on-device |
| Enrollment registry + transport rule | `core/src/security.rs` | works |
| Worker gRPC service | `mobile-worker/.../rpc/` | works |
| Dashboard topology + pairing panel | `src/components/` | works |
| **LAN discovery of ADB devices** | — | **missing** |
| **`adb connect` / `adb tcpip` / `adb pair`** | — | **missing** |
| **APK install/version check over ADB** | — | **missing** |
| **Intent-driven enrollment** | — | **missing** |
| **Bundled `adb` binary** | — | **missing** (dev env sets PATH) |

## 5. Constraints that shape the design

1. **We cannot show a custom popup on the phone.** Android raises its own
   "Allow USB debugging?" dialog when an unauthorised host connects. We wait for
   it and report progress. Any UI copy implying otherwise is wrong.
2. **`adb tcpip` does not survive a reboot.** After a phone restarts, it stops
   listening until `adb tcpip` runs again (USB) or the user re-enables Wireless
   debugging (Android 11+, persists).
3. **ADB over Wi-Fi is unencrypted.** The RSA handshake authenticates the host;
   the bytes are plaintext. Enrollment responses contain private keys.
4. **A phone with no worker app cannot be enrolled.** There is nothing to hold
   the certificate, so install must precede enrolment.
5. **mDNS does not cross subnets.** Same-LAN only; VPNs and guest networks that
   isolate clients will not work and must fail with a clear message.

## 6. Architecture

**ADB is the control plane. mTLS is the data plane.**

```
 Desktop (master)                                   Phone
 ┌──────────────────────────┐                  ┌───────────────────────┐
 │ OnboardingService        │   adb (control)  │  adbd                 │
 │  discover → connect →    │ ───────────────► │  install / am start   │
 │  install → enrol         │                  │                       │
 │                          │                  │  worker app           │
 │ SecurityState (CA)       │   mTLS (data)    │  WorkerService :50062 │
 │  enrollments             │ ◄──────────────► │                       │
 └──────────────────────────┘                  └───────────────────────┘
```

ADB is used only for actions that need shell access. Once a device is enrolled,
all cluster traffic uses the existing mTLS path and ADB can drop away.

### Components

- **`core/src/onboarding/mod.rs`** (new) — `OnboardingService`: owns the state
  machine, drives ADB commands, emits progress events. One instance per
  `AppState`.
- **`core/src/adb/wireless.rs`** (new) — thin, testable wrappers for
  `adb connect`, `adb disconnect`, `adb tcpip`, `adb pair`, `adb install`,
  `adb shell`, plus parsers for their output.
- **`core/src/discovery/lan.rs`** (new) — mDNS browsers for
  `_adb-tls-connect._tcp` and `_adb-tls-pairing._tcp`, and a bounded TCP sweep
  of port 5555.
- **`core/src/api/devices.rs`** (new) — HTTP surface.
- **`src/components/AvailableDevices.tsx`** (new) — the Connect UI.
- **Android `MainActivity`** — reads enrollment intent extras.

## 7. Discovery

Three sources, merged into one list keyed by the strongest available identity.

1. **USB** — existing `adb devices -l`.
2. **mDNS** — the worker app already advertises `_aiupcycler-worker._tcp`.
   Additionally browse `_adb-tls-connect._tcp` (Android 11+ Wireless debugging
   connect port) and `_adb-tls-pairing._tcp` (present only while the pairing
   dialog is open).
3. **Port sweep** — opt-in, per network, remembered. Concurrent connect attempts
   against `x.y.z.1-254:5555` with a 300 ms timeout, at most 64 in flight.
   Disabled by default because it looks like scanning to network monitors.

A device is shown once even when several sources see it. The merge key is
`adb serial` when known, else `ip:port`.

**Deliberately not shown:** phones with no ADB reachability and no worker app.
Listing them would imply we can connect, and we cannot.

## 8. Connect state machine

```
Discovered
   │ connect()
   ▼
Connecting ──► Unauthorized ──► (user taps Allow) ──► Connected
   │                │
   │                └── timeout 120 s ──► Failed
   ▼
Connected
   │ version check
   ▼
NeedsInstall ──► (user taps Install) ──► Installing ──► Connected
   │
   ▼
Enrolling ──► Ready
   │
   └── any step ──► Failed { reason, remedy }
```

Every failure carries a **remedy string** the UI shows verbatim, e.g.
"Phone stopped listening: reconnect the USB cable to re-enable wireless ADB",
"Android 10 has no Wireless debugging — plug in the cable once", or
"adb is not installed; set AI_UPCYCLER_ADB or bundle platform-tools".

Progress is published on the existing telemetry bus as `device_onboarding`
events, so the dashboard updates without polling.

## 9. Enrollment hand-off

The existing PIN flow is reused unchanged. What changes is who types the PIN:
nobody.

1. `POST /api/pairing/session?serial=<serial>` creates a single-use PIN and
   registers the pairing tunnel (this already exists).
2. The host launches the worker app with the PIN and host coordinates:

```
adb shell am start -n org.aiupcycler.worker/.MainActivity \
  --es aiupcycler.auto_enroll true \
  --es aiupcycler.pin <pin> \
  --es aiupcycler.api_base http://127.0.0.1:8081 \
  --es aiupcycler.ca_fingerprint <hex>
```

3. `MainActivity` reads the extras on create and on `onNewIntent`, and drives
   the existing enrollment client (`PairingState`/credential store) with no user
   input.
4. The phone posts to `/api/pairing/enroll` over the `adb reverse` tunnel, which
   the existing code already sets up.

**Why reuse the PIN rather than add a new endpoint:** the PIN is already
single-use, TTL-bound, and covered by tests. The only new thing is delivery, and
delivery is gated by the Android Allow tap — device-side consent, stronger than
typing a code on the laptop.

### USB-first policy

- If the device is attached over USB, enrollment runs over the USB tunnel.
  Nothing secret crosses the network.
- If the device is only reachable over Wi-Fi ADB, the UI must state that
  enrollment credentials travel unencrypted on the LAN, and the action is
  labelled accordingly. The user opts in per device.
- The Android 11+ Wireless debugging path (no cable ever) is supported but the
  first enrollment is always classified as Wi-Fi.

## 10. Worker app install

- Version source of truth: the APK shipped with the desktop app
  (`resources/worker/ai-upcycler-worker.apk`), version read from its manifest at
  build time and exposed to `core` as a constant.
- Installed version: `adb shell dumpsys package org.aiupcycler.worker | grep
  versionName`.
- Missing or older → `NeedsInstall`, with an explicit **Install** / **Update**
  button. Never silent: installing software is a deliberate act.
- `adb install -r -d <apk>`; surface `INSTALL_FAILED_*` codes as remedy text.

## 11. `adb` binary resolution

Order: `AI_UPCYCLER_ADB` env → bundled `resources/platform-tools/adb` → `adb` on
`PATH`. `AdbTunnelManager` currently shells out to `adb` directly; it moves to a
single resolver so tests can inject a fake.

Windows bundle needs `adb.exe`, `AdbWinApi.dll`, `AdbWinUsbApi.dll`. Apache 2.0,
notice file added to `THIRD_PARTY_NOTICES`.

## 12. HTTP surface

| Method | Path | Purpose |
|---|---|---|
| `GET` | `/api/devices/available` | Discovered devices + their state |
| `POST` | `/api/devices/{id}/connect` | Start the state machine |
| `POST` | `/api/devices/{id}/install` | Install/update the worker APK |
| `POST` | `/api/devices/{id}/enroll` | PIN hand-off and enrollment |
| `DELETE` | `/api/devices/{id}` | Revoke enrolment and disconnect |
| `POST` | `/api/devices/scan` | One LAN sweep (opt-in) |

`{id}` is the adb serial when known, else `ip:port`, URL-encoded.

Revoke removes every enrollment record whose canonical id matches the device
(both the `adb:<serial>` alias and the mDNS fullname, using the alias map added
in `0dd11d2`), then runs `adb disconnect`.

## 13. UI

A new **Available on network** section above the cluster cards. Each row shows
device name, transport (USB / Wi-Fi / Wireless debugging), and one primary
action whose label tracks the state (`Connect`, `Waiting for Allow…`, `Install`,
`Enrol`, `Ready`, `Retry`). Failures show the remedy string inline. The existing
`NodeCard` cluster list is unchanged.

## 14. Security posture

- ADB over TCP is plaintext; enrollment over Wi-Fi is opt-in and labelled.
- `adb tcpip 5555` opens a root-equivalent shell to anyone holding an authorised
  key. The UI warns on the action and the port is never opened silently.
- Revoke is authoritative: enrollment removed, transport refused, connection
  dropped.
- No new long-lived secret is introduced; the CA and enrollment records are the
  same ones the PIN flow uses.

## 15. Failure modes

| Failure | Detection | User-visible remedy |
|---|---|---|
| Phone not listening (rebooted) | `adb connect` refused | "Plug in USB once to re-enable wireless ADB" |
| Android < 11, no cable | version probe | "This phone has no Wireless debugging; connect USB" |
| Consent never tapped | 120 s timeout | "Approve the debugging prompt on the phone" |
| Wrong APK version | version compare | "Update" button |
| adb missing | resolver | "Set AI_UPCYCLER_ADB or bundle platform-tools" |
| Subnet isolation | sweep finds nothing + mDNS silent | "Phone and laptop appear to be on different networks" |
| Enrollment rejected | HTTP status | pass through the server's message |

## 16. Testing

- **Unit, no device:** output parsers, state-machine transitions, discovery
  merge/dedupe, remedy selection, version comparison.
- **Integration, no device:** a fake `adb` executable (a script that records
  argv and replays canned stdout) injected through the resolver, so the whole
  state machine runs in `cargo test` and in CI.
- **On-device, manual:** the checklist in `docs/tasks.md`, run against the Redmi
  Note 9 Pro Max. Not claimable as done until executed.

## 17. Rollout

1. `feat/wireless-onboarding` from `feat/usb-device-readiness`.
2. Issues: prerequisites, Slice A, Slice B, Slice C.
3. PR for Slice A; PR-Agent review addressed; CI green; merge to `main` only on
   the owner's explicit go.
