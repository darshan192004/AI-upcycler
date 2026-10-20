# Android Worker mTLS / QR Pairing — Design (2026-10-20)

Status: **ACCEPTED** (2026-10-20; epic: tasks.md §5 "Implement the worker half
of mTLS/QR pairing", line 94). Implementation lands as sequential slice PRs
after this spec merges; this document is stamped IMPLEMENTED with PR numbers
when the epic closes.

Predecessors: `2026-10-19-android-compute-rpcs-design.md` (the compute RPCs
this transport will carry) and the master-side pairing work already merged in
`core/src/security.rs` + `core/src/api/pairing.rs` (tasks.md line 104).

## Intent

Close the last half of the pairing epic. The master side is complete and
tested: an ephemeral CA, single-use 6-digit PIN sessions,
`POST /api/pairing/session` + `POST /api/pairing/enroll` (docs/api.md §3.4),
an mTLS gRPC client (`WorkerRpcClient::connect_tls`), and an
`EnrolledWorkerSource` that resolves a transport only for a paired `node_id`.
The Android worker, however, serves plaintext `ServerBuilder.forPort` and has
no pairing code at all, so none of that is reachable from a real phone: the
worker cannot enroll, cannot store credentials, and cannot serve behind mTLS.

This epic implements the worker half — enrollment client, credential
persistence, an mTLS gRPC server, and `node_id` consistency with mDNS — plus
the minimal master-side enablers without which the flow cannot be demonstrated
at all (a pairing-only listener, the `adb reverse` lifecycle, and a dashboard
panel that renders the QR). The result: a phone pairs once over USB, then
serves TLS-1.3-only gRPC with client-auth required, which is exactly the
contract `load_secure` and `EnrolledWorkerSource` already demand.

## Decisions (D-B1…D-B10)

### D-B1 — Scope: worker half plus four small master enablers

In scope (worker): QR/PIN enrollment client, credential storage, mTLS server
mode, `node_id` composition, address declaration, pairing UI states.

In scope (master, minimal and list-closed):

1. A pairing-only loopback listener serving **only** `POST /api/pairing/enroll`
   (D-B2).
2. `adb reverse` lifecycle in `AdbTunnelManager` + a Tauri command that pairs
   it to a session (D-B2).
3. A dashboard "Pair worker" panel: create session, render the QR, show the
   PIN + fingerprint, offer a copyable `aiupcycler://` URI (D-B3). No pairing
   UI exists anywhere today (`src/` has zero references), so without this the
   epic is undemonstrable; tasks.md line 94 lists "QR display" inside this
   epic's bullet.
4. Doc updates: api.md §3.4 (worker-side note + pairing port), security.md §3
   status paragraph, tasks.md ticks.

Deliberately out of scope:

- **Shell request-path wiring**: security.md's known gap ("nothing yet passes
  the pairing-backed source") closes when the chat-path epic wires a live
  request path; there is nothing to attach `EnrolledWorkerSource` to today.
  This epic ships the source-backed transport contract, proven device-free.
- **QR scanning / CAMERA permission** (D-B3).
- **On-device validation**: the JVM lane cannot exercise NSD, the camera, or a
  real ADB tunnel; the on-device demo is a documented, hardware-gated
  checklist, never a CI claim.

### D-B2 — Enrollment transport: the ADB tunnel, not a LAN listener

The master's API binds `127.0.0.1:8080` only (src-tauri lib.rs), so a phone
cannot reach `/api/pairing/enroll` over Wi-Fi. Options considered:

- **LAN-facing listener** (rejected for now): a 6-digit PIN with no rate
  limiting anywhere in `PairingManager` is brute-forceable in minutes from
  anything on the LAN; exposing it would demand rate limiting first, and
  plaintext-on-LAN of the enroll response would leak the worker's fresh
  private key to any eavesdropper (the response carries `server_key_pem` /
  `client_key_pem`). The honest wireless follow-up — a TLS pairing listener
  pinned to the QR fingerprint plus PIN rate limiting — is recorded in Open
  Questions, not half-shipped here.
- **Full API on the LAN** (rejected outright): violates the loopback-only
  posture for no benefit.
- **ADB tunnel** (chosen): core already shells out to `adb` for
  `adb forward` (`core/src/adb/mod.rs`); `adb -s <serial> reverse tcp:8081
  tcp:8081` makes the phone's own loopback reach a **pairing-only** listener
  bound `127.0.0.1:8081` that serves exactly one route. security.md's policy
  is explicit: "Plaintext stays acceptable only on loopback or an ADB tunnel".
  Exposure is bounded twice: only the pairing route, and only to the device
  the user just plugged in. The reverse tunnel lives and dies with the
  session TTL (created on session start, removed on expiry/completion).

The phone enrolls to `http://127.0.0.1:8081/api/pairing/enroll` — its own
loopback, reversed. No new TLS-serving dependencies, no new attack surface on
the LAN, and the flow reuses the ADB machinery that already exists.

### D-B3 — Trust and out-of-band channel: QR payload pasted, fingerprint pinned

The QR encodes `aiupcycler://pair?fp=<ca-fingerprint>&token=<pin>`
(`PairingManager::create_session`). The worker gets that payload two ways, in
order of preference:

1. **Paste the URI** (primary): the dashboard offers "copy pairing URI"; the
   user pastes it into the app's pairing field. Full fidelity — the app
   parses `fp` and `token` from the URI. Zero permissions, zero new deps.
2. **Typed PIN + fingerprint prefix** (fallback): user types the 6-digit PIN
   and the first 8 hex chars of the fingerprint; the app compares that prefix
   against the `fingerprint` field of the enroll response and aborts on
   mismatch.

QR **scanning** is deliberately deferred, not designed away: any scanner
needs the CAMERA permission (a scope expansion of security.md §4's minimal
list) plus camera deps, and its behavior is hardware-gated on a preview
surface the JVM lane cannot drive. The two paths above carry the same
security property — the fingerprint arrives over an out-of-band channel the
enroll response cannot tamper with — so deferring the camera costs UX, not
security. The scanner is recorded as the follow-up that turns the paste field
into a scan field behind the same parser.

Regardless of entry path: the app sends `{pin, device_name, node_id,
addresses}` (api.md §3.4), then **verifies the response's `fingerprint`
equals the QR's `fp`** before storing anything. Mismatch → abort, wipe any
partial state, surface "pairing fingerprint mismatch". `InvalidPin`,
`Expired` and `AlreadyUsed` map to distinct user-visible errors (the HTTP
codes come from api.md §3.4's error table; the Kotlin client pins them in
tests against the documented contract, engine-fixture style).

### D-B4 — Credential storage: app-private files, excluded from backup

The four PEMs (server cert/key, client cert/key) plus `ca_pem` and the
trusted `fingerprint` persist as files under `filesDir/pairing/` (plain
`File.writeText`, MODE_PRIVATE by the app sandbox). SharedPreferences is
rejected: PEM blobs are large strings and the type system gives no reason to
prefer them; files keep the store trivially testable on the JVM lane.

The manifest flips `android:allowBackup="false"` (currently `true`,
AndroidManifest.xml line 13): backing up private keys to the user's cloud
backup is exactly the exfiltration the pairing flow exists to prevent. One
line, documented in security.md §4; per-app backup rules are not worth the
XML surface for a single app with no other backup-worthy state.

Re-pairing overwrites in place. A "forget pairing" affordance in the UI
deletes the directory and returns the worker to plaintext-ADB-only mode; the
server never serves TLS without stored credentials, and never serves
plaintext on the mTLS port once credentials exist (D-B5).

### D-B5 — mTLS server: grpc-netty `SslContext`, TLS 1.3, client auth REQUIRED

`grpc-netty:1.63.0` is already a dependency (build.gradle.kts line 86), so
`NettyServerBuilder` + netty's `SslContextBuilder` is available with zero new
dependencies:

- Credentials present → `SslContextBuilder.forServer(serverCertChain,
  serverKey).trustManager(caPem).clientAuth(ClientAuth.REQUIRE)` and
  `TLSv1_3` only; `NettyServerBuilder.forPort(port).sslContext(...)`.
  A plaintext client fails the handshake — verified by test, not assumed.
- No credentials → today's plaintext `ServerBuilder` (the ADB path stays
  exactly as it is; security.md's "plaintext on an ADB tunnel" carve-out).
- `WorkerRuntime` restarts the server after a successful enrollment
  (stop → start in TLS mode); the port and the mDNS advertisement do not
  change.

**Honest platform gap**: TLS 1.3 requires API 29+ (Android 10) in the field
JDK/Conscrypt stack; the app's minSdk is 26. On API 26–28 the pairing UI
reports "pairing requires Android 10+" and the worker stays on the plaintext
ADB path — rustls on the master side is TLS-1.3-only, so there is no 1.2
middle ground to bend toward. The JVM test lane (JDK 17) exercises the TLS
path fully; low-API devices are recorded, not faked.

### D-B6 — `node_id` is the NSD-registered service full name

`DiscoveredNode::node_id` is `info.get_fullname()` (core/src/discovery/mdns.rs
line 40) — `<instance>._aiupcycler-worker._tcp.local.`. The worker's NSD
registration may *rename* the instance on collision (Android appends " (2)"),
so the advertiser must expose the **actually registered** name from
`onServiceRegistered`, not the requested one. `MdnsAdvertiser` gains a
listener callback publishing it; enrollment blocks on it (short timeout with
an honest error) because the enroll payload's `node_id` must match what the
master will discover, or `EnrolledWorkerSource` refuses the dial — a loud
failure by design.

The full-name format is pinned cross-language: a Rust helper
`worker_node_id(instance_name)` in `core/src/discovery/mdns.rs` composes the
string the master will see (single source of truth), its unit test pins the
golden, and the Kotlin mirror is pinned against the same golden constant.
Renaming after enrollment (collision, different network) invalidates the
pairing; the UI says "re-pair required" rather than silently degrading.

### D-B7 — Declared addresses: LAN IPv4s plus loopback, capped at 8

Enrollment `addresses` become the server certificate's SANs, which
`load_secure` verifies against the address it dials. The worker enumerates
non-loopback IPv4s (`NetworkInterface.getNetworkInterfaces()`, no permission
needed) and prepends the loopback defaults (`127.0.0.1`, `localhost`) so the
ADB-dialed path verifies too. Cap 8 (the server's `MAX_ADDRESSES`); an empty
list is a 400 by contract, so the loopback defaults guarantee non-empty. DHCP
rotation can invalidate SANs; re-pair is the documented remedy for this
epic (SAN refresh without re-pairing is a follow-up).

### D-B8 — Master enablers in detail

- **Pairing-only listener** (`core/src/api/pairing.rs`): a second axum router
  with the single `POST /api/pairing/enroll` route, served by a
  `serve_pairing(addr, state)` entry point bound `127.0.0.1:8081` from the
  shell. `/api/pairing/session` stays on the main API only — the phone never
  creates sessions, so the tunnel never exposes session minting (a phone that
  could mint sessions could pair itself TOFU without the user's QR).
- **Reverse-tunnel lifecycle** (`core/src/adb/mod.rs`): extend
  `AdbTunnelManager` with `reverse_pairing(serial)` /
  `remove_reverse_pairing(serial)` mirroring the existing forward-tunnel
  command construction (pure string-building unit tests, no device needed).
  Tunnels are removed when the session completes or expires.
- **Tauri command**: `prepare_pairing(serial)` — creates the session via the
  existing pairing manager, sets up the reverse tunnel for that device, and
  returns `{pin, qr_payload, fingerprint}` to the frontend. The dashboard
  renders the QR from `qr_payload` with the `qrcode` npm package (MIT,
  tiny); `frontend` CI lane (pnpm build) covers typecheck/build.

### D-B9 — Zero proto change

`proto/cluster_rpc.proto` stays byte-identical (`checkProtoInSync` keeps
gating); pairing is transport, not contract. D5 of the compute epic carries
over unchanged.

### D-B10 — Epic edge

The epic ends at: device-free proof on both sides (Rust pairing-listener +
tunnel tests; Kotlin enroll-client + mTLS-server tests against committed
fixture credentials), the dashboard panel building in CI, and a written
on-device demo checklist (plug in → pair → verify TLS-only serving → master
`load_secure` dial) marked hardware-gated. The live cross-cluster mTLS chat
run — real phone, real driver, real request path — is the chat-path epic's
milestone, not a claim here.

## Commands

Android (from `mobile-worker/android/`):

- `./gradlew --no-daemon :app:testDebugUnitTest` — the device-free lane
  (enrollment client, credential store, mTLS server, node_id goldens).
- `./gradlew --no-daemon :app:assembleDebug` — APK builds after the change.

Rust (repo root, w64devkit wrapper env as established):

- `cargo test -p ai-upcycler-core` — pairing listener, reverse-tunnel
  command construction, `worker_node_id` golden.
- `cargo fmt --all -- --check` and `cargo clippy -p ai-upcycler-core
  --all-targets -- -D warnings` — unchanged gates.

Fixture regeneration (opt-in, not per-build):

- `cargo run -p ai-upcycler-core --example pairing_fixtures --
  mobile-worker/android/app/src/test/resources/pairing` — regenerates the
  committed test CA + server/client/rogue credentials with explicit
  ~100-year validity. Unlike the engine fixtures these are **not**
  byte-regenerable (rcgen keys are random), so there is no byte-sync guard;
  a Kotlin test pins structure (parse, EKU roles, SANs, chain trust,
  not-expired) and the generator is run only to replace them deliberately.

Frontend: `pnpm build` (typecheck + build, as the CI `frontend` lane).

## Project Structure

```
core/src/api/pairing.rs        — + serve_pairing() (enroll-only router, loopback)
core/src/adb/mod.rs            — + reverse_pairing / remove_reverse_pairing
core/src/discovery/mdns.rs     — + worker_node_id(instance_name) helper + golden test
core/tests/api.rs              — + pairing-listener tests (enroll over the tunnel port)
src-tauri/src/lib.rs           — serve_pairing spawn; prepare_pairing(serial) command
src/components/                — + PairingPanel.tsx (session, QR, PIN, copy-URI)
mobile-worker/android/.../pairing/
    PairingUri.kt              — aiupcycler:// URI parser (fp + token)
    EnrollmentClient.kt        — POST /api/pairing/enroll (HttpURLConnection),
                                 fingerprint verification, error mapping
    CredentialStore.kt         — filesDir/pairing persistence, load/clear/exists
    WorkerCredentials.kt       — the six persisted strings as a data class
…/mdns/MdnsAdvertiser.kt       — registered-name callback (node_id source)
…/rpc/WorkerRpcServer.kt       — TLS mode via NettyServerBuilder + SslContextBuilder
…/WorkerRuntime.kt             — pairing orchestration, server restart in TLS mode
…/MainActivity.kt              — pairing UI states (paste/typed entry, status, forget)
mobile-worker/android/app/src/test/resources/pairing/
    ca.pem, server-cert.pem, server-key.pem,
    client-cert.pem, client-key.pem,
    rogue-ca.pem, rogue-cert.pem, rogue-key.pem
```

No changes to `proto/`, `worker/` (Rust), or the engine code; the Kotlin
`rpc` package gains only transport-mode logic.

## Testing Strategy

- **Pairing listener (Rust, `core/tests/api.rs`)**: enroll over the
  pairing-only port succeeds with a valid single-use PIN; the same port
  refuses anything but `POST /api/pairing/enroll` (session minting is not
  reachable through the tunnel); main API behavior unchanged; already-used /
  expired / wrong-PIN error shapes match the documented contract.
- **Reverse tunnel (Rust)**: command construction for
  `reverse`/`remove` pinned by unit tests; lifecycle behavior on the real
  ADB binary is part of the hardware-gated checklist.
- **node_id (both sides)**: `worker_node_id` golden in Rust; the Kotlin
  composition pinned to the same golden; advertiser callback unit-tested with
  a fake `NsdManager`-shaped listener (JVM).
- **Enrollment client (Kotlin)**: a JDK `HttpServer` on loopback serves the
  documented JSON (fixtures mirroring api.md §3.4); tests cover success,
  fingerprint-mismatch abort (both entry paths), 403/410/409 mapping, and
  the "never store on failure" invariant (credential dir absent after every
  failed attempt).
- **Credential store (Kotlin)**: persist, overwrite, clear, `exists`;
  files land only under the injected storage root.
- **mTLS server (Kotlin, JVM)**: with the committed fixtures — a gRPC client
  presenting the fixture client cert (trusted CA) completes `Handshake`; a
  plaintext client fails; a client presenting the rogue-CA cert fails; TLS
  context negotiates 1.3. The unpaired plaintext server behaves as today.
- **CI**: `core`/`desktop` lanes for the Rust slices, `frontend` for the
  panel, `android-worker` for the Kotlin slices; PR-Agent prompt size stays
  bounded by extending `.pr_agent.toml`'s ignore globs to
  `mobile-worker/android/app/src/test/resources/pairing/**` (PEMs, same
  rationale as the engine fixtures).

## Boundaries

- **Always**: local gates before every commit (gradle unit tests + proto
  guard; cargo fmt/clippy/test); conventional commits authored by the AI
  Upcycler Bot; per-PR reviewer-guide comment + resolution comment; honest
  claims only.
- **Ask first**: new dependencies (none expected — netty, HttpURLConnection,
  and the frontend's `qrcode` are the only additions, the first two are
  already present); the `allowBackup` flip; any change to api.md §3.4's
  request/response shapes (this epic consumes them, never changes them).
- **Never**: expose `/api/pairing/session` through the tunnel; serve
  plaintext where credentials exist; claim on-device or ADB-tunnel behavior
  that only ran on the JVM lane; weaken `load_secure`'s no-fallback stance;
  touch `proto/cluster_rpc.proto`.

## Success Criteria

1. Kotlin: a device-free test proves the enrollment client completes the
   documented contract and stores verified credentials, and that the server
   serves TLS-1.3-only with client auth when they exist.
2. Rust: the pairing-only listener serves enroll, refuses session minting,
   and the reverse-tunnel commands are pinned.
3. `node_id` matches the master's discovery golden on both sides.
4. The dashboard panel creates a session and renders a scannable QR + PIN +
   copyable URI; `pnpm build` green.
5. security.md §3 status paragraph and api.md §3.4 updated to state the
   worker half is implemented over the ADB tunnel, with the LAN-TLS
   follow-up and the API-29+ TLS gap recorded; tasks.md line 94 ticked with
   evidence pointers; this spec stamped IMPLEMENTED with PR numbers.
6. No proto change, no new Rust or Kotlin runtime dependency (the frontend
   `qrcode` package excepted and named).

## Open Questions

1. **Wireless pairing** (follow-up, not this epic): a TLS pairing listener
   pinned to the QR fingerprint, plus PIN rate limiting in `PairingManager` —
   both prerequisites before any LAN-facing exposure. Settled as deferred.
2. **QR scanning**: CAMERA permission + scanner behind the same URI parser
   once the paste path has been validated on-device. Settled as deferred.
3. **SAN refresh on DHCP change** without a full re-pair (e.g. re-enroll
   with a stored long-lived session) — needs a contract change; deferred.
4. On-device demo results (NSD rename behavior in practice, TLS handshake
   against the real master, API 26–28 error surface) are collected from the
   hardware checklist and folded back into this spec's status paragraph when
   the epic closes.
