# Wireless Onboarding (Slice A) — Implementation Plan

Spec: `docs/superpowers/specs/2026-11-03-wireless-device-onboarding-design.md`
Branch: `feat/wireless-onboarding` (from `feat/usb-device-readiness`)

Each task is independently committable and leaves the tree green. Tasks 1–5 are
host-only and fully testable without a phone; 6–8 need the Android build; 9 needs
the device.

## Task 0 — Land persistent trust (prerequisite)

The CA currently rotates on every host start, which would make every wireless
reconnect require re-enrollment. Work is already written in worktree
`persistent-trust` (branch `feat/persistent-trust`, based on `0dd11d2`):
`core/src/trust.rs`, `NodeCa::from_parts`, `SecurityState::load_or_generate`,
DPAPI-wrapped CA key, persisted enrollments, `bootstrap(..., trust_dir)`.

- [ ] `cargo test -p ai-upcycler-core` green (229 + 7 new tests)
- [ ] `cargo fmt --check` green
- [ ] Commit, fast-forward `feat/usb-device-readiness`, merge into the feature branch
- [ ] Verify: restart the host twice, fingerprint unchanged, enrollment survives

## Task 1 — `adb` resolver + fake-adb harness

- `core/src/adb/resolve.rs`: `AdbBinary::resolve()` — `AI_UPCYCLER_ADB` env,
  then bundled path, then `PATH`. Every call site goes through it.
- Test harness: `FakeAdb` that writes a script to a temp dir, records argv lines
  to a log, and replays queued stdout/stderr/exit codes.
- Refactor `AdbTunnelManager` to take the resolved path.

**Done when:** existing adb tests pass through `FakeAdb`; a test proves the
resolver order and the error text when nothing is found.

## Task 2 — Wireless ADB wrappers

`core/src/adb/wireless.rs`:

- `connect(addr) -> ConnectOutcome` (`Connected`, `Unauthorized`, `Refused`,
  `Timeout`) parsing `adb connect` output and `adb devices`.
- `disconnect(addr)`, `tcpip(serial, port)`, `pair(addr, code)`.
- `install(apk)` with `INSTALL_FAILED_*` mapped to remedy text.
- `shell(serial, cmd)` and `version_name(package)`.

**Done when:** every parser has a table-driven test over captured real output,
including the `cannot connect` and `failed to authenticate` shapes.

## Task 3 — LAN discovery

`core/src/discovery/lan.rs`:

- Browse `_adb-tls-connect._tcp` and `_adb-tls-pairing._tcp`.
- `sweep(subnet, timeout)` — bounded concurrency (64), 300 ms, opt-in.
- Merge into `Vec<AvailableDevice>` keyed by serial else `ip:port`; dedupe
  against USB and mDNS worker sightings.

**Done when:** merge/dedupe tests cover all three sources and the
one-device-three-sightings case.

## Task 4 — `OnboardingService` state machine

`core/src/onboarding/mod.rs`: states from the spec, transitions driven by
injected `Adb` + `SecurityState`, progress published on `TelemetryBus` as
`device_onboarding` events, remedy strings on every failure.

**Done when:** transition tests cover the happy path, consent timeout, reboot
refusal, and revoke.

## Task 5 — HTTP surface

`core/src/api/devices.rs` + router wiring: the six endpoints from spec §12.

**Done when:** `core/tests/devices.rs` drives connect → install → enroll → ready
and revoke against `FakeAdb`, asserting status codes and event order.

## Task 6 — Android intent hand-off

`MainActivity`: read `aiupcycler.auto_enroll`, `aiupcycler.pin`,
`aiupcycler.api_base`, `aiupcycler.ca_fingerprint` on create and `onNewIntent`;
drive the existing enrollment client with no input. Ignore extras when
`auto_enroll != "true"`.

**Done when:** JVM tests cover extra parsing and the ignore path; manual run on
device completes enrollment with no typing.

## Task 7 — APK bundling + version check

Bundle the worker APK under `src-tauri/resources/worker/`, expose its version to
`core` at build time, compare against `dumpsys package`, gate on explicit
Install/Update.

**Done when:** a stale version yields `NeedsInstall`, a matching one proceeds,
and `INSTALL_FAILED_*` maps to readable text.

## Task 8 — Dashboard UI

`AvailableDevices.tsx`: list, transport badge, state-driven primary action,
inline remedy text, revoke with confirmation.

**Done when:** component tests cover each state and the revoked device leaves the
list.

## Task 9 — On-device verification

- [ ] USB → Connect → `tcpip` → unplug → device stays `Ready`
- [ ] Reboot phone → status shows "plug in USB once" → replug → `Ready`
- [ ] Android 11+ wireless debugging → discovered over mDNS → enroll
- [ ] Fresh phone with no worker app → `NeedsInstall` → Install → `Ready`
- [ ] Revoke → enrolment gone, transport refused, `adb disconnect` issued
- [ ] Restart host → device reconnects without re-enrollment

Record results in `docs/tasks.md` §6. Nothing above is claimed until executed.

## Task 10 — PR

- `gh pr create` against `main`, body links the spec and the issues
- Address PR-Agent findings
- CI green (`ci.yml`, `android-worker.yml`)
- Merge only on the owner's explicit go
