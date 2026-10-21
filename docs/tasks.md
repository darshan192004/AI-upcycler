# Master Development Tasks Checklist - AI Upcycler

## 1. Project Initialization & Documentation Setup
- [x] Create core architectural documents in `/docs/` directory (`prd.md`, `architecture.md`, `spec.md`, `api.md`, `memory.md`, `design.md`, `security.md`, `database.md`, `code_style.md`, `plan.md`, `tasks.md`).
- [x] Create project root `README.md` with overview, download badge placeholders, and architecture diagrams.
- [x] Setup repository `.gitignore` for Rust, Node, Tauri, and Android build artifacts.

## 2. CI/CD Automated Build Workflows
- [x] Create `.github/workflows/ci.yml` for pull request linting and cargo test runs.
- [x] Create `.github/workflows/release.yml` for automated cross-platform desktop installer builds (Windows `.msi`/`.exe`, macOS `.dmg`, Linux `.deb`/`.rpm`/`.AppImage`) on version tag.
- [x] Package and publish the Android `.apk` worker asset: `release.yml` gained a `publish-android-apk` job that builds the side-loadable debug APK with the same toolchain recipe as the `android-worker` CI job, publishes it as the `worker-apk` workflow artifact on every run, and attaches it to tag releases as `aiupcycler-worker-<tag>-debug.apk` (the desktop matrix owns release creation; the job waits for that release rather than racing it).

## 3. Master Orchestrator Engine (Rust + Tauri)
- [x] Initialize Tauri 2.0 Rust app in `src-tauri/` (config v2 schema, capability-based permissions).
- [x] **Implement the host worker engine — the pure-Rust `worker/` crate (`ai-upcycler-worker`, engine half of the distributed split, `docs/superpowers/specs/2026-10-16-worker-engine-design.md`):**
  - [x] Worker crate scaffold + GGUF-v3 reader + `LoadLayers`/`UnloadLayers` layer-range contract (PR #41; `worker/tests/process_smoke.rs`, `gguf_reader.rs`, `load_contract.rs`).
  - [x] Dequantize the real model's ggml types (F32/F16/Q4_0/Q4_K/Q5_0/Q6_K/Q8_0) + decoder math units (RMS norm, linear, rope, softmax, SiLU) with f64 reference gates (PR #42; `dequant.rs`, `math_units.rs`).
  - [x] `ExecuteForward`: KV-cached sessions, stage roles (token ids ↔ hidden handoff), full-range/partial/mid-stage forwards over the real 24-block Qwen2.5-0.5B file (PR #53; `forward.rs` — 13 tests incl. an f64-reference block gate).
  - [x] **Logits parity vs `llama-server`** on the real quantized GGUF (asset-gated `worker/tests/parity.rs`): greedy argmax exact on "The capital of France is" (server −0.8261 vs ours −0.8355), all 25 server-listed logprobs shared, worst |Δ| 0.165 nats — bounded by llama.cpp's 8-bit activation quantization in its quantized-weight CPU kernels (an F32-converted twin of the same model agrees to ≤ 4e-4). The gate itself caught two real engine bugs: missing `attn_{q,k,v}.bias` support and GPT-J-style rope pairing instead of llama.cpp's NeoX `(i, i + head_dim/2)`.
  - [x] **Split equivalence over real worker processes** (`worker/tests/split_equivalence.rs`, PR4): two-worker `[0..1, 2..3]` split through the master-side `LoadedPipeline` produces the same logits as the single-worker `[0..3]` run (fixture gate ≤ 1e-6; real-model 12/12 split gate ≤ 1e-4 + identical argmax). A tied model's tail stage carries `token_embd` as its output head — the ownership rule now reflects that.
- [x] Master-side tokenizer **done**: spec accepted, spec
  `docs/superpowers/specs/2026-10-17-master-tokenizer-design.md` + plan
  `docs/superpowers/plans/2026-10-17-master-tokenizer.md`; the GGUF reader owns
  the shared crate `model-io` (worker re-exports, PR #58), the tokenizer reads
  `tokenizer.ggml.*` KVs of the same model file (PR #59: byte encoder, Qwen2
  pre-tokenizer, rank-ordered BPE, specials-before-BPE, strict decode), and
  the id-exact parity vs llama-server `/tokenize` (`/detokenize` agreeing with
  decode) gates on a 111-string corpus (`model-io/tests/tokenizer_parity.rs`).
- [x] Wire the live chat request path through `LoadedPipeline` + worker engines
  — **done (PRs #62/#63)**. The master renders ChatML + encodes in-process
  (`model-io` tokenizer), drives the plan through `PipelineDriver`/`LoadedPipeline`
  (prefill once, one-id decode steps), samples master-side (greedy argmax /
  seeded temperature+top-p), stops on `<|im_end|>` with marker withholding, and
  streams `chat.completion.chunk` SSE; `AdaptiveBackend` picks the cluster when a
  plan + enrolled registered workers + a readable tokenizer exist and falls back
  to `llama-server` otherwise (decided before the first byte). Evidence: the
  asset-gated acceptance gates `core/tests/cluster_serving.rs` — the streamed
  temperature-0 continuation matches llama-server's raw `/completion` reference
  id-for-id (llama picks 59604 "Paris" then `<|im_end|>`) BOTH single-stage and
  through a real 12/12 two-worker split, with byte-identical back-to-back runs;
  hermetic suites cover the sampler, marker withholding, fresh-session
  recovery windows, SSE error events and metric truthing (true
  `usage.prompt_tokens` = prefill length). Design +
  slice record: `docs/superpowers/specs/2026-10-18-live-chat-path-design.md`,
  `docs/superpowers/plans/2026-10-18-live-chat-path.md`. Known shipped limits:
  whole-logits prefill bounds prompts at ~430 tokens on the shipped model
  (last-row-only wire contract is a proto change, deliberately deferred), and
  `proto/cluster_rpc.proto` stayed byte-identical through the epic (D6).
- [x] Port the engine to the Android worker (mTLS server + compute RPCs so
  phones join the split) — **device-free complete**: pure-Kotlin GGUF parser +
  dequant (#66), forward-parity vectors at the D-A2 bar (#68), the refusal
  matrix and a full split lifecycle over in-process gRPC (#67, #70 —
  `WorkerRpcContractTest`), and the worker serves its RPC behind mTLS once
  paired (#76, `WorkerTls.kt`). Compute answers when an engine is loaded and
  honestly surfaces `UNIMPLEMENTED` when it is not (spec D-A4). Remaining:
  verify on physical hardware (§6 checklist).
- [x] Commit K-quant dequant golden vectors: `worker/tests/dequant_golden.rs` parses committed fixtures (`worker/tests/goldens/q{5_0,8_0,4_K,6_K}.txt` — 4 legacy + 6 K-quant records each cut from real file blocks) generated by the committed `worker/examples/golden_dump.rs`, compared with exact bit equality; the corruption check (a single flipped raw byte) fails loudly with a per-element diagnostic (PR5).
- [x] Implement mDNS Service Browser (`mdns-sd` crate) to discover mobile workers.
- [x] Implement ADB USB Tunnel Manager to trigger `adb forward tcp:50052 tcp:50052`.
- [x] Build RPC Client module connecting to `cluster_rpc.proto` endpoints.
- [x] Implement SSD `mmap` zero-copy weight pager for Windows/Linux file mapping (`core::mmap::MmapPager`, memmap2-backed, bounds-checked, unit-tested). Note: the host inference path shells out to `llama-server`, which maps the GGUF itself; `MmapPager` remains the tested primitive for the future in-process worker RPC path.
- [x] Implement greedy dynamic programming layer scheduler.
- [x] Implement the host-side distributed split driver — `LoadedPipeline` (`core/src/rpc/engine.rs`) opens a gRPC client per assignment, handshakes, issues `LoadLayers`, threads one activation through every stage in ascending layer order via `ExecuteForward` (feeding each stage's output tensor to the next), and releases the cluster with `UnloadLayers`; a partial load failure rolls back the stages already loaded. Covered device-free by `core/tests/distributed.rs` against two in-process mock workers. The plan is validated before any worker is dialed — inverted, overlapping or gapped layer ranges, or a plan that does not begin at layer 0, are rejected instead of silently duplicating or skipping layers. It dials each worker with the transport a per-worker `WorkerTransportSource` resolves — mTLS, with the worker's certificate pinned to the cluster CA, and deliberately no plaintext fallback — and pairing provisions both directions of that link (§7). Not yet wired to a live request path — the real GGUF slicing/weight push and the worker's compute RPCs are still missing (§5). The session lifecycle — plan validity, activation handoff, the wire contract and the worker-side obligations — is written up in `docs/architecture.md` §3.1.
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
- [x] Serve the compute RPCs: the Android worker runs a pure-Kotlin engine port
      mirroring `worker/src` (GGUF reader, dequant, qwen2 block math,
      load/forward/unload state machine; spec
      `docs/superpowers/specs/2026-10-19-android-compute-rpcs-design.md` — the
      earlier "cross-compile llama.cpp rpc-server" idea was rejected there in
      favour of a device-free-testable port). `WorkerRpcService` delegates
      `LoadLayers`/`ExecuteForward`/`UnloadLayers` through `EngineSource`
      (honest `UNIMPLEMENTED` while no engine is wired, spec D-A4; FP32-only
      wire guard at the wrapper, D-A2b). Device-free proof on the
      `android-worker.yml` lane: dequant goldens, forward parity against
      Rust-cut vectors at <= 1e-5 relative (`ForwardParityTest`), the refusal
      matrix (`WorkerRpcEngineTest`), and a full split lifecycle over
      in-process gRPC (`WorkerRpcContractTest`, D-A6). The master-side driver
      it serves is already done and mock-verified (`core/tests/distributed.rs`).
      PRs: #65 (spec), #66 (parser/dequant), #67 (refusal matrix), #68
      (forward parity), #70 (RPC wiring).
- [x] Add a foreground service so the worker survives backgrounding (Android 13+ `POST_NOTIFICATIONS`) — `WorkerForegroundService` (`foregroundServiceType="dataSync"`) owns the runtime so the gRPC server + mDNS advert outlive the UI; needs a device to verify survival across backgrounding.
- [x] Implement the worker half of mTLS/QR pairing (on-device mTLS server, QR display, trust of the master fingerprint). The master side is complete: enrollment now issues the worker a CA-signed **server** certificate (SANs = the addresses it declared) alongside its client certificate, and the master presents its own client identity, so the on-device server has a usable certificate to serve behind. It must declare the same `node_id` the master discovers it as, or the driver refuses to dial it (`EnrolledWorkerSource`), and it must present a CA-signed server certificate and enforce client auth — `LoadedPipeline::load_secure` verifies that certificate against the address it dials (`core/tests/distributed.rs`, `core/tests/api.rs`). **All worker-half slices have since landed device-free**: pairing-only listener + `adb reverse` (#73), enrollment client + credential store (#74), worker mTLS serving (#76), phone pairing screen with the two distinct entry paths (#77, D-B3), and the dashboard `prepare_pairing` + `PairingPanel` — QR, PIN, copy-URI — with the reverse tunnel retired when its session completes or expires (D-B8). The live on-device run is hardware-gated (§6 checklist).
- [ ] Create iOS Swift agent in `mobile-worker/ios/`.

## 6. End-to-End Testing & Optimization
> ⛔ Blocked in this environment: requires a physical 8 GB host plus 2× Android worker devices.
- [ ] Run 1000-token continuous inference test on pooled 8GB Laptop + 2x Android cluster.
- [ ] Validate host system RAM safety buffer under heavy mmap page faulting — **budgeting half covered device-free**: `never_breaches_the_eighty_percent_head_room_budget` sweeps node capacity × layer size × activation working set and asserts no assignment exceeds the 20% head-room (and that the reported RAM matches the owned range), while `plan_is_anchored_and_contiguous_for_the_split_driver` locks the plan shape the split driver requires. Both were mutation-checked. The page-faulting half still needs the physical 8 GB host.
- [ ] Perform security penetration audit on mTLS inter-node transport — **master side audited and complete**: found and fixed the certificate role-confusion (no `extendedKeyUsage`; a `serverAuth` certificate authenticated as a client). `NodeCa` now signs four identities — the master's server and client certificates and each worker's server and client certificates — each pinned to exactly one role, with adversarial tests (`server_cert_is_not_accepted_as_a_client_cert`, `client_rejects_server_cert_from_untrusted_ca`). The transport is no longer test-only: enrollment provisions both link directions and `EnrolledWorkerSource` is the production transport source, so `LoadedPipeline::load_secure` is reachable outside tests (covered in `core/tests/api.rs`, and the master never issues its own server identity to a worker). Remaining: the live chat path's `AdaptiveBackend` now passes `EnrolledWorkerSource` in production (`core/src/api/distributed.rs`) and the worker's mTLS server exists (#76); what remains is running the pair → serve → drive loop on physical hardware (§6 checklist), so a full audit of a running cluster stays hardware-gated.
- [ ] **On-device pairing demo (spec 2026-10-20, D-B10 — ⛔ hardware-gated: a physical Android device on USB)** — everything below is already proven device-free; this is the run that closes the pairing epic's edge:
  1. **Plug in**: USB debugging on, `adb devices` shows the phone as `device`.
  2. **Pair from the dashboard**: Dashboard → *Pair a phone worker* → enter the serial → *Create session*; the panel shows the QR, the 6-digit PIN, the CA fingerprint and the copyable `aiupcycler://pair?…` URI.
  3. **Enroll from the phone**: paste the URI into the worker app's pairing field → enroll; the master logs the enrollment, and within one cleanup tick `adb reverse --list` no longer shows `tcp:8081` for the device (session completed → tunnel retired).
  4. **Verify TLS-only serving**: the worker's RPC port refuses a plaintext dial; a master handshake succeeds against the issued credentials (`WorkerTls.kt`, client-auth enforced).
  5. **Master `load_secure` dial**: the paired worker resolves a transport (`EnrolledWorkerSource`, certificate pinned to the cluster CA, no plaintext fallback) and a distributed chat request runs across host + phone — the chat-path epic's milestone, not a claim of this epic.
- [ ] **On-device benchmark values (spec 2026-10-21, D-C9/D-C10 — ⛔ hardware-gated: a physical Android device on USB)** — everything about the probe is proven device-free; this is the run that records real phone numbers:
  1. Build and install the worker APK, side-load a GGUF, and start serving.
  2. `POST /api/cluster/benchmark` against the phone (paired → mTLS, or plaintext over ADB) and confirm `status: "success"`, `nodes_failed: 0`, and one `node_benchmark` row.
  3. Record the reported `memory_bandwidth_mbs`, `throughput_flops` and `probe_duration_ms`. Assert only that they are positive and finite — the *values* are device facts, not CI assertions.
  4. Sanity-check `probe_duration_ms` against the ~0.5 s bound (spec D-C2); a wildly different number means the probe did not run as designed.
  5. `POST /api/cluster/benchmark` again with the phone's worker binary replaced by a pre-probe build and confirm `nodes_failed: 1`, no new row, `status: "partial"` — the graceful-degradation path on real hardware.

## 7. Known Gaps (Deferred)
- [ ] **mTLS 1.3 + QR/PIN pairing (`docs/security.md`) — master side is implemented** (`core/src/security.rs`: ephemeral CA, single-use 6-digit PIN sessions, `POST /api/pairing/session` + `/api/pairing/enroll`, `WorkerRpcClient::connect_tls`, mTLS handshake covered end-to-end in `core/tests/mtls.rs` against a CA-enforcing test server). **Pairing now provisions both link directions**: enroll issues the worker a `ServerAuth` certificate (SANs = the addresses it declared) plus the `ClientAuth` one, and the master generates its own `ClientAuth` identity, so the master-as-client links the split driver opens can complete. The driver dials over it — `LoadedPipeline::load_secure` takes a per-worker `WorkerTransportSource`, verifies the certificate against the address actually dialed, resolves every transport before the first dial, and never falls back to plaintext (4 tests in `core/tests/distributed.rs`); the production `EnrolledWorkerSource` resolves a transport only for a worker that finished pairing, and the full loop — enroll over HTTP → serve the issued certificate → drive it over mTLS — is covered in `core/tests/api.rs`. ~~The worker half (Android mTLS server, QR display, cert trust of the master fingerprint) is deferred to the hardware phase §5 — no Android device/toolchain is available to build or test it.~~ **Worker half landed device-free**: enrollment client + credential store (#74), worker mTLS serving (#76), phone pairing screen (#77), dashboard `prepare_pairing`/`PairingPanel` (D-B8); only the on-device run remains (§6 checklist, hardware-gated).
- [ ] Tauri desktop shell is on v2; remaining plugins/permissions beyond `core:default` will be added as features land.
- [ ] Full desktop builds need the Tauri v2 Linux stack (`webkit2gtk-4.1`, `libsoup-3.0`); `ai-upcycler-core` remains the fastest locally verifiable unit, while CI's `desktop` job lints the shell.
- [x] ~~mDNS discovery is best-effort: a failed browser spawn is logged and the app continues without discovery. Surface this to the UI once the cluster view is wired.~~ — fixed: `NodeRegistry` tracks browser liveness, `/api/cluster/topology` exposes `discovery.mdns: "active"|"unavailable"`, and the dashboard shows an amber chip + offline copy when the browser is down.
- [x] ~~The ADB tunnel background task has no graceful shutdown~~ — fixed: `AdbTunnelManager::start` now waits on a shutdown signal and removes every `adb forward` it created before the app exits.
- [ ] `shell.open` is intentionally not granted (unused); add a scoped `shell:allow-open` permission if external-link opening is introduced.
- [x] ~~The chat generation backend is a deterministic `StubBackend` (no inference engine loaded yet). Streaming, SQLite persistence and telemetry around it are real; swap in the real engine behind the `GenerationBackend` trait.~~ — fixed (host-only): bootstrap discovers llama.cpp (`AI_UPCYCLER_LLAMA_SERVER`/`AI_UPCYCLER_MODEL` or `engine/` + `models/qwen2.5-0.5b-instruct-q4_k_m.gguf` under the data dir), spawns `llama-server` lazily and proxies streaming chat; the stub remains the honest fallback when assets are missing. The distributed cluster RPC split across workers is still future work: the master-side driver (`LoadedPipeline`) is done and mock-verified, but the workers have no real engine loaded yet.
- [x] ~~`POST /api/cluster/benchmark` uses a deterministic `StubBenchmarker` (values derived from the node id hash), not on-device probes.~~ — fixed (2026-10-21, spec `docs/superpowers/specs/2026-10-21-hardware-benchmark-probes-design.md`): the endpoint probes each discovered node over the transport it serves on (mTLS when enrolled, plaintext otherwise, D-C3) against a new additive `Benchmark` RPC; the Rust worker and the Kotlin worker both measure their own memory bandwidth and single-threaded FMA rate with bounded, pure probes (32 MiB / fixed passes, D-C2/D-C9). A node that cannot answer — dial failure, 2 s timeout, or a pre-probe worker's gRPC `UNIMPLEMENTED` — is counted in the new `nodes_failed` field and gets **no** row: `status` is `success` / `partial` / `no_nodes` (D-C6). `StubBenchmarker` is now `#[cfg(test)]`-only, so a fabricated number is unconstructible in production (D-C7). The Kotlin probe performs a true fused multiply-add (`Math.fma`, the same operation as the Rust twin's `mul_add`) and answers `RESOURCE_EXHAUSTED` if it cannot allocate its 32 MiB buffer, so an exhausted phone is counted as failed instead of crashing the worker. Verified device-free end-to-end against the real Rust worker binary with real host measurements (`worker/tests/process_smoke.rs`), plus mock-worker tests for the measured / unimplemented / unreachable / mixed paths. **Still true:** nothing schedules on these numbers yet, so `node_benchmark` has no consumer; such scheduling is a separate change. Physical-phone values remain hardware-gated.
- [ ] The REST API binds to `127.0.0.1:8080` with permissive CORS for the local UI and has no authentication; acceptable under the local-only threat model, revisit if the bind address widens.
- [ ] `storage_mmap` in the topology response reports `enabled: false`: the host inference path delegates weight loading to `llama-server` (which maps the GGUF itself), so `MmapPager` is a tested primitive that is not yet on our serving path. Revisit when the in-process worker RPC path consumes it.
- [ ] Worker thermal/battery telemetry is emitted as `0.0` placeholders until the Android/iOS agents publish it (section 5).
