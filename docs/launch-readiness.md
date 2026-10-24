# Launch readiness

**Status:** 2026-10-23, `main` @ `b141120`.
**Scope assumed:** local-first desktop app (Tauri/React) that pools Android
worker phones on the user's own LAN/USB; no hosted service, no accounts.
If the launch is meant to be a hosted or multi-tenant service, this document
understates the work by a wide margin - start with a new threat model.

This page exists because `docs/tasks.md` tracks **features**, and the project is
feature-complete device-free while still not being launch-ready. It lists what
is actually left, what evidence closes each item, and the decisions only the
maintainer can make.

## 1. Where the product stands

| Area | State | Evidence |
|---|---|---|
| Master core (Rust) | complete, tested | `cargo test` across core + worker; CI lane "Core" |
| Desktop shell | builds on 3 OSes, unsigned | `release.yml` matrix: macOS universal, Linux `.deb/.rpm/.AppImage`, Windows `.msi/.exe` |
| Dashboard + chat UI | complete, typechecked | CI lane "Frontend" |
| Host inference | real `llama-server` subprocess, stub fallback | `docs/api.md`, tasks.md §7 |
| Distributed split | real RPC across Rust/Kotlin workers, mTLS, scheduler, telemetry, thermal cap | PRs #41-#95, specs under `docs/superpowers/specs/` |
| Android worker | debug APK, engine + mTLS + pairing, 133 JVM tests | `android-worker.yml` lane |
| iOS worker | **does not exist** | `mobile-worker/ios/` absent |
| Real-device proof | **never run** | tasks.md §6 checklist |
| License | MIT, file present | `LICENSE`, README badge, `core/Cargo.toml` |
| Auto-update | **none** | no updater plugin/config |
| Installer signing | **none** | no signing config or CI secrets |

## 2. Blocking work

### A. Hardware verification (the honest bar)

Everything here needs a physical Android phone (ideally two) and an 8 GB host.
None of it can be replaced by more code.

1. **Pair -> serve -> drive on one phone** (tasks.md §6, D-B10): QR/PIN enroll,
   tunnel retired, plaintext refused, master `load_secure` dial, distributed
   chat answer.
2. **1000-token continuous inference** on host + 2 phones; record tokens/s and
   whether the split stays up.
3. **RAM headroom under real page faulting** on the 8 GB host; budgeting half is
   already pinned by `never_breaches_the_eighty_percent_head_room_budget`.
4. **Measured benchmark values** from a real phone (`POST /api/cluster/benchmark`),
   including the pre-probe `UNIMPLEMENTED` degradation path.
5. **Measured telemetry and staleness** on the dashboard (battery, temperature or
   honest `-`), plus master RAM.
6. **Thermal cap against reality**: the 40 C constant, visible throttling, and
   the relaxing plan.
7. **mTLS penetration audit on the running cluster** (the master side is already
   audited; the role-confusion bug was found and fixed).

**Exit criteria:** every checkbox above closed with a recorded result and the
exact device/build used. Until then, "works on a real cluster" stays unclaimed.

### B. Distribution and trust (device-free, decision-gated)

1. **`LICENSE`** - done: the file now exists and matches what the README badge,
   the license section and `core/Cargo.toml` already declared (MIT). The desktop
   bundle includes it via `bundle.licenseFile`. Downloaded GGUF models carry
   their own licenses, which are the user's to observe.
2. **Installer signing** - Windows code signing and macOS Developer ID +
   notarization. Needs certificates and CI secrets; without them users meet
   SmartScreen and Gatekeeper.
3. **Auto-update** - `tauri-plugin-updater` with a signed update feed and a
   published public key. Needs a signing keypair and a hosting decision
   (GitHub Releases is the cheapest honest option).
4. **Release APK** - `release.yml` deliberately ships `assembleDebug` because
   the worker has no release key. A globally distributed Android build needs a
   release keystore, `signingConfig`, and a decision on Play Store vs sideload.
5. **Clean-machine install test** - install each signed artifact on a machine
   with no Rust/Node/Python/Android tooling and complete: launch, download a
   model, chat, pair a phone, run a split.

### C. Product surface (device-free, partly decision-gated)

1. **First-run onboarding** - today model setup depends on
   `AI_UPCYCLER_LLAMA_SERVER` / `AI_UPCYCLER_MODEL` or assets under the data
   dir. A launcher that finds-or-downloads llama.cpp and a model, then proves it
   with a local chat, is the difference between a demo and a product.
2. **iOS agent** - absent. This is the single largest remaining engineering
   item if iOS is in the launch scope; a Swift engine port, RPC client/server,
   mTLS identities and pairing UI, gated on macOS + Xcode.
3. **Privacy and telemetry statement** - the app keeps chat history locally and
   polls worker health; state what is stored, where, and how to delete it, and
   that nothing leaves the LAN.
4. **Support surface** - issue template, crash/error reporting story (even if
   the answer is "write to the issue tracker with the log path"), and a known
   limitations page that repeats the honest gaps from `docs/tasks.md`.

### Host configuration (environment)

The desktop shell reads these before it starts the API:

| Variable | Default | Purpose |
| --- | --- | --- |
| `AI_UPCYCLER_LLAMA_SERVER` | `engine/llama-server[.exe]` beside the models dir | `llama-server` binary |
| `AI_UPCYCLER_MODEL` | `models/qwen2.5-0.5b-instruct-q4_k_m.gguf` in the app data dir | GGUF the host serves |
| `AI_UPCYCLER_MODELS_DIR` | app data dir `models/` | Where `/api/models/download` writes |
| `AI_UPCYCLER_API_PORT` | `8080` | Loopback API port; invalid or `0` keeps the default |

`AI_UPCYCLER_MODELS_DIR` exists because models are large and the system drive is
often the smallest one; pointing it at another volume is supported. The pairing
listener stays on `8081`.

### D. Known correctness limits already documented (not blockers, must be stated)

- The REST API binds `127.0.0.1:8080` and has **no auth**. Fine for local-only; a
  blocker the moment the bind widens. The browser side is bounded as of
  2026-10-24: CORS grants only the bundled shell and localhost origins, and the
  telemetry WebSocket refuses a foreign `Origin`.
- The webview still has **no Content-Security-Policy** (`tauri.conf.json` says
  `"csp": null`). Tightening it needs runtime verification of the shell's inline
  styles; until then a compromised frontend is not sandboxed.
- `storage_mmap` reports `enabled: false`: the host path delegates mapping to
  `llama-server`.
- The displayed topology is the serving plan for paired workers; the master
  never holds layers.
- Prompt length is bounded by worker RAM/attention cost, not the wire (the
  ~430-token wire bound is gone since #94).

## 3. Decisions needed before device-free work can start

| Decision | Why it gates work | Recommended default |
|---|---|---|
| **License** | `LICENSE` must match what the README claims; affects every redistribution channel | **Decided 2026-10-24: MIT**, matching the README badge, the license section and `core/Cargo.toml` |
| **Launch surface** | signing, store listings, updater hosting | Signed GitHub Releases for Windows/Linux/macOS + sideload APK first; stores later |
| **iOS timing** | 3-6 weeks of Swift work; changes the "global" claim | After the Android+desktop beta is real-device verified |
| **Telemetry posture** | privacy text, error reporting | No remote telemetry in v1; local logs only, user-initiated sharing |

## 4. Suggested sequence

**Phase 0 - decisions (this week).** The four items above.

**Phase 1 - device-free launch hardening (~1 week).**
`LICENSE`; signing + updater scaffolding with documented secrets; release APK
signing config; onboarding flow; privacy/limitations docs; clean-install test on
one OS. Each lands as its own PR with the normal gates.

**Phase 2 - real-device proof (~2-4 days once a phone + 8 GB host are in hand).**
Section 2A, in the listed order. Any failure here becomes a fix PR, so budget
for one iteration.

**Phase 3 - public beta.** Signed artifacts, updater feed, onboarding, honest
limitations. Windows/Linux/macOS desktop + sideload Android.

**Phase 4 - global/GA (adds ~2-4 weeks).** iOS agent, store distribution,
external security audit, support rotation.

## 5. Go/no-go checklist

- [x] `LICENSE` present and consistent with README
- [ ] Windows + macOS artifacts signed and notarized; Linux artifacts published
- [ ] Updater feed signed, one successful upgrade from the previous tag
- [ ] Android APK signed with a release key (or sideload-only stated publicly)
- [ ] Clean-machine install passes on each shipped OS
- [ ] Section 2A complete on recorded hardware
- [ ] Privacy/telemetry and limitations pages published
- [ ] First-run path needs no environment variables
- [ ] No device-free test regressions: CI 7/7, Android suite green
- [ ] Known gaps in `docs/tasks.md` re-read and still accurate on launch day
