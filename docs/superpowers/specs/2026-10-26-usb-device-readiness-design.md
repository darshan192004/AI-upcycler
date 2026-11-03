# USB Device Readiness — First Slice (2026-10-26)

Status: **ACCEPTED for implementation** (first slice of the approved unified
identity / persistent trust architecture; user requested a live USB test).

Predecessors: `2026-10-20-android-mtls-pairing-design.md` (mTLS pairing) and
`2026-10-22-live-worker-telemetry-design.md` (health polling and battery /
temperature rendering).

## Intent

A phone paired through the dashboard currently can show as `paired` or
`usb_detected` with no battery or temperature. Three facts combine:

1. ADB discovery registers the phone as `adb:<serial>`.
2. The Android worker enrolls under its mDNS fullname, so the master never
   records the ADB alias as enrolled.
3. The ADB bootstrap probe dials plaintext. After pairing the worker serves
   mTLS only, so the handshake fails and the node's port stays `0`. The health
   poll skips port `0`, so no `WorkerHealth` snapshot reaches the dashboard.

This slice closes that loop without changing the protocol, the UI, or the
worker APK:

- The pairing tunnel already knows which ADB serial a PIN belongs to. The
  enrollment handler records the same device under both the declared
  `node_id` and the `adb:<serial>` alias.
- The ADB bootstrap probe uses the same enrollment-aware transport source as
  the health poller and benchmark probe. An enrolled alias is dialed over
  mTLS; an unpaired phone stays plaintext until it pairs.
- The existing health poll then gets a real port and a real battery /
  temperature snapshot. `NodeCard` already renders those fields.

This is deliberately a vertical slice, not the full architecture. The
canonical worker UUID, alias merging, persisted CA/enrollments, one-click USB
pairing, QR pairing, and automatic model provisioning remain follow-up slices.

## Decisions

### D-H1 — ADB alias enrollment

`prepare_pairing(serial)` already calls `pairing_tunnels.track(serial, pin)`.
Add `PairingTunnelRegistry::serial_for_pin(pin)`. When `POST
/api/pairing/enroll` handles the request, look up the tracked serial **before**
redeeming the PIN (the cleanup task may purge the tunnel as soon as the session
is redeemed), then enroll both:

- the worker-declared `node_id` (its mDNS fullname today), and
- `adb:<serial>`.

Both records carry the same device name and certificate addresses. The
enrollment response is unchanged.

The main API's manual `POST /api/pairing/session` path has no tracked serial,
so it does not get the alias. The dashboard `prepare_pairing` flow is the
supported USB path; this is recorded rather than silently implied.

### D-H2 — One dial rule for the ADB probe

`AdbTunnelManager::start` receives the same `WorkerTransportSource` as the
health poller (`AppState::transports()`). `probe_device` builds the
`adb:<serial>` node and calls `dial_worker`, so:

- enrolled alias → mTLS, matching the worker's paired mode;
- unenrolled phone → plaintext, preserving the pre-pairing bootstrap.

This removes the second, plaintext-only dial rule from the ADB path.

### D-H3 — The health poll and UI stay unchanged

The health poll already resolves the transport per cycle and skips only
`port == 0`. Once the probe upgrades the port and the alias is enrolled, the
existing path publishes battery, temperature, free RAM, and
`supports_compute`. `NodeCard` already renders temperature/battery with `—`
for unknown and a stale marker after 15 seconds.

### D-H4 — Honest scope

No proto change, no Android APK change, no frontend change. A phone that has
never loaded a model still reports `supports_compute: false` and stays
`paired`, not `ready`; battery/temperature can still display. Unreadable
sensors still render `—`.

### D-H5 — Re-assert the adb tunnels, never trust our own record

Found during live verification, when pairing failed with no error on either
side. `adb` drops **every** forward and reverse when its server restarts or a
device re-enumerates, while this process's record of those bindings survives.
The USB poll loop trusted that record:

```rust
let local_port = match forwards.get(&device.serial) {
    Some(local_port) => *local_port,   // binding may be long gone
```

so it kept dialing a tunnel that no longer existed, and never re-probed either
(the node already carries a port, so `needs_probe` is false). The pairing
tunnel was weaker still: opened once at session creation and afterwards only
ever *removed*.

Both symptoms traced to this one cause:

- The worker's enroll POST to `127.0.0.1:8081` was refused at its own loopback
  while the master still showed a live PIN — pairing failed silently.
- `worker health poll timed out for adb:16284c49 after 2s`, 73 consecutive
  times and never once succeeding, so battery and temperature never appeared.

The decision: **re-issue the command every tick rather than reconcile against
a listing.** `adb forward <serial> tcp:P tcp:Q` and `adb reverse <serial>
tcp:P tcp:Q` are both idempotent (verified: re-issuing an existing binding
exits 0 and disturbs nothing), so the cost is one subprocess per device per
5 s poll — the same as reading `adb forward --list` would — with no parsing
and no second opinion to trust. Concretely:

- `ensure_forward` / `ensure_reverse_pairing` are the quiet primitives;
  `forward` / `reverse_pairing` wrap them and keep the first-time `info!`, so
  the create and re-assert paths cannot drift apart.
- The poll loop re-asserts every tick instead of reading its map.
- `pairing_tunnel_cleanup` re-asserts for every session still redeemable, via
  the new `PairingTunnelRegistry::active`.

Parsing was rejected for the reverse specifically: `adb forward --list` keys by
the real serial (`16284c49`), but `adb reverse --list` keys by the device
*transport* (`UsbFfs tcp:8081 tcp:8081`), so a reverse cannot be matched
against that listing by serial at all.

Observed effect: removing the forward by hand now has it recreated
automatically in ~1.4 s (`22:21:27.557`), where before the identical teardown
left it gone for 40+ minutes until repaired by hand.

## Out of scope (follow-up slices)

- Canonical worker UUID and alias merging of USB + mDNS into one card.
- Persistent CA/enrollments across host restarts.
- One-click USB pairing and same-LAN QR.
- Automatic GGUF inventory/transfer/load.
- Per-device token/s.
- Persistent chat sessions.

## Verification

Device-free:

- `cargo test -p ai-upcycler-core serial_for_pin`
- `cargo test -p ai-upcycler-core enrollment_ids`
- `cargo test -p ai-upcycler-core probe_uses_the_adb_alias`
- `cargo test -p ai-upcycler-core adb::tests` (includes
  `forward_args_pin_the_host_port_to_the_worker_rpc_port`,
  `active_lists_only_sessions_still_awaiting_enrollment`,
  `active_is_empty_once_every_session_is_spent`)
- `cargo clippy -p ai-upcycler-core --all-targets -- -D warnings`
- `cargo check -p ai-upcycler-desktop`
- `cargo fmt --all -- --check`

Live USB (hardware-gated, recorded honestly):

1. Host app running with ADB on `PATH`; phone authorized and worker app
   installed.
2. Pair through the dashboard `prepare_pairing` flow (or the documented manual
   fallback) and confirm the phone switches to mTLS.
3. Topology shows the ADB node with `worker_state` `paired` or `ready` and a
   non-zero port/available RAM.
4. The telemetry WebSocket emits `worker_health` with real `battery_pct` and
   `temp_celsius` (or `null` where the sensor is unavailable).
5. The dashboard `NodeCard` shows the same values.

## Security notes

- An unenrolled node is still plaintext only before pairing, matching the
  existing ADB-tunnel carve-out.
- An enrolled `adb:<serial>` resolves to mTLS. A plaintext impostor at the
  same tunnel cannot satisfy the client handshake.
- The alias is recorded only from a redeemed, single-use PIN whose reverse
  tunnel was created for that serial by `prepare_pairing`.
