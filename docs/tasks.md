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
- [ ] Implement SSD `mmap` zero-copy weight pager for Windows/Linux file mapping.
- [x] Implement greedy dynamic programming layer scheduler.
- [x] Implement embedded SQLite store (`rusqlite`) for nodes, models, benchmarks and chat history (`docs/database.md`).
- [x] Implement OpenAI-compatible REST API (`axum`): `GET /v1/models` and `POST /v1/chat/completions` (buffered and SSE streaming).
- [x] Implement cluster management endpoints: `GET /api/cluster/topology` and `POST /api/cluster/benchmark`.
- [x] Implement telemetry WebSocket fan-out (`GET /api/telemetry`) over a tokio broadcast bus, wired to mDNS node events.
- [ ] Implement GGUF download endpoint `POST /api/models/download` (202 + `.gguf` validation) fanning out `model_download_progress` over `/api/telemetry`.

## 4. Master Desktop UI (React + Tailwind CSS)
- [x] Initialize React + TypeScript app in `src/`.
- [x] Create dark obsidian glassmorphism theme (`index.css`).
- [x] Build live cluster telemetry canvas visualizer.
- [x] Build model catalog browser & Hugging Face GGUF downloader interface.
- [x] Build interactive local chat UI with streaming markdown renderer.

## 5. Mobile Worker Agent (Android & iOS)
- [ ] Create Android Kotlin worker app in `mobile-worker/android/`.
- [ ] Cross-compile native C++ `llama.cpp` RPC worker server for ARM64 (`libllama-rpc.so`).
- [ ] Implement mDNS service registration (`_aiupcycler-worker._tcp.local.`).
- [ ] Implement battery and temperature telemetry monitor.
- [ ] Create iOS Swift agent in `mobile-worker/ios/`.

## 6. End-to-End Testing & Optimization
- [ ] Run 1000-token continuous inference test on pooled 8GB Laptop + 2x Android cluster.
- [ ] Validate host system RAM safety buffer under heavy mmap page faulting.
- [ ] Perform security penetration audit on mTLS inter-node transport.

## 7. Known Gaps (Deferred)
- [ ] **mTLS 1.3 + QR/PIN pairing (`docs/security.md`) is not implemented.** Inter-node RPC is currently plaintext local transport. Deferred to a later hardware phase; tracked here so the gap is explicit rather than implicit.
- [ ] Tauri desktop shell is on v2; remaining plugins/permissions beyond `core:default` will be added as features land.
- [ ] Full desktop builds need the Tauri v2 Linux stack (`webkit2gtk-4.1`, `libsoup-3.0`); `ai-upcycler-core` remains the fastest locally verifiable unit, while CI's `desktop` job lints the shell.
- [ ] mDNS discovery is best-effort: a failed browser spawn is logged and the app continues without discovery. Surface this to the UI once the cluster view is wired.
- [ ] The ADB tunnel background task has no graceful shutdown; add a shutdown signal so `adb forward` state is torn down cleanly on exit.
- [ ] `shell.open` is intentionally not granted (unused); add a scoped `shell:allow-open` permission if external-link opening is introduced.
- [ ] The chat generation backend is a deterministic `StubBackend` (no inference engine loaded yet). Streaming, SQLite persistence and telemetry around it are real; swap in the real engine behind the `GenerationBackend` trait.
- [ ] `POST /api/cluster/benchmark` uses a deterministic `StubBenchmarker` (values derived from the node id hash), not on-device probes. Do not schedule on these numbers yet.
- [ ] The REST API binds to `127.0.0.1:8080` with permissive CORS for the local UI and has no authentication; acceptable under the local-only threat model, revisit if the bind address widens.
- [ ] `storage_mmap` in the topology response reports `enabled: false` until the SSD mmap pager (section 3) lands.
- [ ] Worker thermal/battery telemetry is emitted as `0.0` placeholders until the Android/iOS agents publish it (section 5).
