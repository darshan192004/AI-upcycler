# Master Development Tasks Checklist - AI Upcycler

## 1. Project Initialization & Documentation Setup
- [x] Create core architectural documents in `/docs/` directory (`prd.md`, `architecture.md`, `spec.md`, `api.md`, `memory.md`, `design.md`, `security.md`, `database.md`, `code_style.md`, `plan.md`, `tasks.md`).
- [x] Create project root `README.md` with overview, download badge placeholders, and architecture diagrams.
- [x] Setup repository `.gitignore` for Rust, Node, Tauri, and Android build artifacts.

## 2. CI/CD Automated Build Workflows
- [x] Create `.github/workflows/ci.yml` for pull request linting and cargo test runs.
- [x] Create `.github/workflows/release.yml` for automated cross-platform desktop installer builds (Windows `.msi`/`.exe`, macOS `.dmg`, Linux `.deb`/`.rpm`/`.AppImage`) on version tag.
- [ ] Package and publish the Android `.apk` worker asset (blocked on the Android worker agent in section 5).

## 3. Master Orchestrator Engine (Rust + Tauri)
- [x] Initialize Tauri 2.0 Rust app in `src-tauri/` (config v2 schema, capability-based permissions).
- [x] Implement mDNS Service Browser (`mdns-sd` crate) to discover mobile workers.
- [x] Implement ADB USB Tunnel Manager to trigger `adb forward tcp:50052 tcp:50052`.
- [x] Build RPC Client module connecting to `cluster_rpc.proto` endpoints.
- [x] Implement SSD `mmap` zero-copy weight pager for Windows/Linux file mapping (`core::mmap::MmapPager`, memmap2-backed, bounds-checked, unit-tested). Note: the host inference path shells out to `llama-server`, which maps the GGUF itself; `MmapPager` remains the tested primitive for the future in-process worker RPC path.
- [x] Implement greedy dynamic programming layer scheduler.
- [x] Implement the host-side distributed split driver — `LoadedPipeline` (`core/src/rpc/engine.rs`) opens a gRPC client per assignment, handshakes, issues `LoadLayers`, threads one activation through every stage in ascending layer order via `ExecuteForward` (feeding each stage's output tensor to the next), and releases the cluster with `UnloadLayers`; a partial load failure rolls back the stages already loaded. Covered device-free by `core/tests/distributed.rs` against two in-process mock workers. The plan is validated before any worker is dialed — inverted, overlapping or gapped layer ranges, or a plan that does not begin at layer 0, are rejected instead of silently duplicating or skipping layers. Not yet wired to a live request path — the real GGUF slicing/weight push and the worker's compute RPCs are still missing (§5). The session lifecycle — plan validity, activation handoff, the wire contract and the worker-side obligations — is written up in `docs/architecture.md` §3.1.
- [x] Implement embedded SQLite store (`rusqlite`) for nodes, models, benchmarks and chat history (`docs/database.md`).
- [x] Implement OpenAI-compatible REST API (`axum`): `GET /v1/models` and `POST /v1/chat/completions` (buffered and SSE streaming).
- [x] Implement cluster management endpoints: `GET /api/cluster/topology` and `POST /api/cluster/benchmark`.
- [x] Implement telemetry WebSocket fan-out (`GET /api/telemetry`) over a tokio broadcast bus, wired to mDNS node events.
- [x] Implement GGUF download endpoint `POST /api/models/download` (202 + `.gguf` validation) fanning out `model_download_progress` over `/api/telemetry`.

## 4. Master Desktop UI (React + Tailwind CSS)
- [x] Initialize React + TypeScript app in `src/`.
- [x] Create dark obsidian glassmorphism theme (`index.css`).
- [x] Build live cluster telemetry canvas visualizer.
- [x] Build model catalog browser & Hugging Face GGUF downloader interface.
- [x] Build interactive local chat UI with streaming markdown renderer.

## 5. Mobile Worker Agent (Android & iOS)
> The Android worker app now builds toolchain-only (no device required). Running and verifying it on-LAN, the native compute engine, and the iOS agent still need hardware — see per-item notes.
- [x] Create Android Kotlin worker app in `mobile-worker/android/` — Gradle project + gRPC `WorkerService` + `NsdManager` advert; builds a debug APK locally and in the `android-worker` CI job.
- [x] Implement mDNS service registration (`_aiupcycler-worker._tcp` via `NsdManager`, publishing the TXT records the master parses) — implemented; needs a device to verify on-LAN.
- [x] Implement battery and temperature telemetry monitor — `Handshake`/`GetHealthStatus` return real RAM/battery/thermal readings; needs a device to verify sysfs values.
- [x] Verify the worker wire contract without a device — in-process gRPC test (`WorkerRpcContractTest`) runs the real `WorkerRpcService` through the generated stub, asserting the metadata replies and that the compute RPCs stay `UNIMPLEMENTED`.
- [ ] Serve the compute RPCs: cross-compile native C++ `llama.cpp` RPC worker server for ARM64 (`libllama-rpc.so`) and implement `LoadLayers`/`ExecuteForward`/`UnloadLayers` (currently `UNIMPLEMENTED`). The master-side driver that will call them is already done and mock-verified (`core/tests/distributed.rs`), so this is the remaining half of the distributed split.
- [x] Add a foreground service so the worker survives backgrounding (Android 13+ `POST_NOTIFICATIONS`) — `WorkerForegroundService` (`foregroundServiceType="dataSync"`) owns the runtime so the gRPC server + mDNS advert outlive the UI; needs a device to verify survival across backgrounding.
- [ ] Implement the worker half of mTLS/QR pairing (on-device mTLS server, QR display, trust of the master fingerprint).
- [ ] Create iOS Swift agent in `mobile-worker/ios/`.

## 6. End-to-End Testing & Optimization
> ⛔ Blocked in this environment: requires a physical 8 GB host plus 2× Android worker devices.
- [ ] Run 1000-token continuous inference test on pooled 8GB Laptop + 2x Android cluster.
- [ ] Validate host system RAM safety buffer under heavy mmap page faulting — **budgeting half covered device-free**: `never_breaches_the_eighty_percent_head_room_budget` sweeps node capacity × layer size × activation working set and asserts no assignment exceeds the 20% head-room (and that the reported RAM matches the owned range), while `plan_is_anchored_and_contiguous_for_the_split_driver` locks the plan shape the split driver requires. Both were mutation-checked. The page-faulting half still needs the physical 8 GB host.
- [ ] Perform security penetration audit on mTLS inter-node transport — **master side audited**: found and fixed the certificate role-confusion (no `extendedKeyUsage`; a `serverAuth` certificate authenticated as a client). Added `ServerAuth`/`ClientAuth` EKU pinning in `NodeCa` plus adversarial tests (`server_cert_is_not_accepted_as_a_client_cert`, `client_rejects_server_cert_from_untrusted_ca`). Remaining: the transport itself is test-only until the worker's mTLS server exists (§5), so a full audit is hardware-gated.

## 7. Known Gaps (Deferred)
- [ ] **mTLS 1.3 + QR/PIN pairing (`docs/security.md`) — master side is implemented** (`core/src/security.rs`: ephemeral CA, single-use 6-digit PIN sessions, `POST /api/pairing/session` + `/api/pairing/enroll`, `WorkerRpcClient::connect_tls`, mTLS handshake covered end-to-end in `core/tests/mtls.rs` against a CA-enforcing test server). The worker half (Android mTLS server, QR display, cert trust of the master fingerprint) is deferred to the hardware phase §5 — no Android device/toolchain is available to build or test it.
- [ ] Tauri desktop shell is on v2; remaining plugins/permissions beyond `core:default` will be added as features land.
- [ ] Full desktop builds need the Tauri v2 Linux stack (`webkit2gtk-4.1`, `libsoup-3.0`); `ai-upcycler-core` remains the fastest locally verifiable unit, while CI's `desktop` job lints the shell.
- [x] ~~mDNS discovery is best-effort: a failed browser spawn is logged and the app continues without discovery. Surface this to the UI once the cluster view is wired.~~ — fixed: `NodeRegistry` tracks browser liveness, `/api/cluster/topology` exposes `discovery.mdns: "active"|"unavailable"`, and the dashboard shows an amber chip + offline copy when the browser is down.
- [x] ~~The ADB tunnel background task has no graceful shutdown~~ — fixed: `AdbTunnelManager::start` now waits on a shutdown signal and removes every `adb forward` it created before the app exits.
- [ ] `shell.open` is intentionally not granted (unused); add a scoped `shell:allow-open` permission if external-link opening is introduced.
- [x] ~~The chat generation backend is a deterministic `StubBackend` (no inference engine loaded yet). Streaming, SQLite persistence and telemetry around it are real; swap in the real engine behind the `GenerationBackend` trait.~~ — fixed (host-only): bootstrap discovers llama.cpp (`AI_UPCYCLER_LLAMA_SERVER`/`AI_UPCYCLER_MODEL` or `engine/` + `models/qwen2.5-0.5b-instruct-q4_k_m.gguf` under the data dir), spawns `llama-server` lazily and proxies streaming chat; the stub remains the honest fallback when assets are missing. The distributed cluster RPC split across workers is still future work: the master-side driver (`LoadedPipeline`) is done and mock-verified, but the workers have no real engine loaded yet.
- [ ] `POST /api/cluster/benchmark` uses a deterministic `StubBenchmarker` (values derived from the node id hash), not on-device probes. Do not schedule on these numbers yet.
- [ ] The REST API binds to `127.0.0.1:8080` with permissive CORS for the local UI and has no authentication; acceptable under the local-only threat model, revisit if the bind address widens.
- [ ] `storage_mmap` in the topology response reports `enabled: false`: the host inference path delegates weight loading to `llama-server` (which maps the GGUF itself), so `MmapPager` is a tested primitive that is not yet on our serving path. Revisit when the in-process worker RPC path consumes it.
- [ ] Worker thermal/battery telemetry is emitted as `0.0` placeholders until the Android/iOS agents publish it (section 5).
