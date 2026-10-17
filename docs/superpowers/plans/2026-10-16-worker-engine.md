# Worker Engine Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace the mock worker behind `LoadedPipeline` with a real engine that loads actual GGUF layer ranges, computes real Qwen2-family forward math, and proves a 1-worker vs 2-worker split produces equivalent logits — device-free, on loopback.

**Architecture:** A new workspace crate `worker/` (`ai-upcycler-worker`, lib + bin) parses GGUF files itself (memmap2), dequantizes owned layer weights to F32 at load, and implements the generated tonic `WorkerService` (Handshake/LoadLayers/ExecuteForward/UnloadLayers/GetHealthStatus). The binary serves gRPC on loopback and prints a machine-readable `READY <addr>` line so tests can spawn real worker processes. Core's existing driver (`LoadedPipeline`) is driven **unmodified** over those processes; slice 4's test proves 1-worker and 2-worker logits match.

**Tech Stack:** Rust 2021, tonic 0.13 + prost 0.13 + tonic-build 0.13 + protoc-bin-vendored 3 (same stack as `core`), memmap2 0.9, tokio 1.36, tokio-stream 0.1, sysinfo 0.39, thiserror 1, tempfile 3 (dev). Pure-F32 compute, no unsafe, no new proto fields.

**Spec:** `docs/superpowers/specs/2026-10-16-worker-engine-design.md` (approved 2026-10-16). The plan argues from the spec; executors read both. This plan is committed to git alongside Task 1 together with the spec.

## Global Constraints

- **Proto is canonical; this epic changes it ZERO times.** `proto/cluster_rpc.proto` stays byte-identical, so `android-worker.yml`'s `checkProtoInSync` never triggers. Any additive field needs explicit justification in its PR plus the Android vendored copy regenerated.
- **Driver is read-only.** No behavior changes in `core/src/rpc/**` or `core/src/scheduler/**`. Finding a genuine driver bug is a PR-level event: call it out, do not silently patch.
- **Timeouts (verbatim from `core/src/rpc/client.rs`):** `CONNECT_TIMEOUT` 5 s, `REQUEST_TIMEOUT` 30 s. Every `LoadLayers` must finish well inside 30 s; benchmark target for the real 0.5B model is < 25 s (leaves headroom; exceeding it means chunked `weights_chunk_data`, never a timeout bump).
- **Activation conventions (verbatim from spec):** `data_type` must be FP32 (proto value 0) — anything else is `success = false`. Shapes: stage-0 input = token ids `[1, n]`; intermediate = `[n, hidden]`; final-stage output = logits `[n, vocab]`; intermediate output = `[n, hidden]`. Row `i` has absolute position `sequence_position + i`. Output tensor shape AND dtype are always populated on success.
- **`model_id` must equal the GGUF's `general.name` KV.** One model per worker process (`--model`); one loaded range per process at a time. `weights_chunk_data` stays empty on the wire — workers resolve weights from their local `--model` file (spec §2); wire-side weight push is out of this epic.
- **Session rules:** `MAX_SESSIONS = 16` (17th concurrent session → `success = false`); forward requires `sequence_position ==` rows already in that session's KV cache (first call: 0); `sequence_position + n > hparams.context_length` → `success = false`.
- **Unload tolerance (spec §3.1.4):** `UnloadLayers` for a model the process never loaded returns `success = true, freed_ram_bytes = 0`. This is what makes the driver's rollback path work.
- **Supported tensor types:** F32 (0), F16 (1), Q4_0 (2), Q8_0 (8), Q4_K (12), Q6_K (14). Any other ggml type id → `LoadLayers` `success = false` with the type id in `error_message` (loud, never partial). *(Task 4 amendment: Q5_0 (6) joins the supported set — the real-model gate found Q5_0 attention tensors inside Qwen's official `qwen2.5-0.5b-instruct-q4_k_m.gguf` (byte-size verified against the HuggingFace listing), so "q4_k_m" files are unloadable without it; reference `dequantize_row_q5_0`, ggml-quants.c:500. The loud-refusal fixture marker moved to Q5_1 (7).)*
- **Commit identity:** `git -c user.name='AI Upcycler Bot' -c user.email='dev@aiupcycler.org' commit ...` on every commit.
- **Local verify before every push:**
  `export PATH="$HOME/.local/bin:$HOME/.cargo/bin:$PATH"` then
  `cargo fmt --all -- --check && cargo clippy --workspace --all-targets -- -D warnings && cargo test -p ai-upcycler-core -p ai-upcycler-worker`
- **PR flow per slice:** feature branch → `gh pr create` → confirm the **`PR Reviewer Guide 🔍` comment** exists (a green check is NOT a review; rerun the `opened` workflow with `gh run rerun <id> --failed` if quota starved, and keep working on the next slice meanwhile) → `gh pr merge <N> --squash --delete-branch`. Never use the `localhost:3000` Gitea credential. Token only via `export GH_TOKEN=$(sed -n 's#https://[^:]*:\([^@]*\)@github.com#\1#p' ~/.git-credentials | head -1)`; never print secrets. Repo stays PUBLIC.
- **Mutation-check** every security/property claim before docs or PR text assert it: back the file up to `/tmp/opencode/`, break the property, confirm the owning test fails, restore, re-confirm green.
- **Tests never use the network.** Asset-dependent tests are `#[ignore]` **and** gated on `AI_UPCYCLER_MODEL` / `AI_UPCYCLER_LLAMA_SERVER`; CI runs fixtures only.
- **Renderer lesson:** tool-call output may garble; re-read every edited file region from disk before trusting it.

## PR slicing

| PR | Tasks | Branch |
|---|---|---|
| PR1 (design slice 1) | 1, 2, 3 + spec/plan docs | `feat/worker-engine-gguf-load` |
| PR2 (slice 2) | 4, 5 | `feat/worker-engine-quant-math` |
| PR3 (slice 3) | 6, 8 | `feat/worker-engine-forward` |
| PR4 (slice 4) | 7, 9 | `feat/worker-engine-split-equivalence` |

## Review Focus

Failure modes the spec implies but no single task's happy-path test exercises — each is pinned to the task that owns the code:

1. **A caller sends FP16/BF16 or wrong-rank activation to a stage** — expected: loud `success = false`, bytes never reinterpreted as F32. → Task 6, test `mid_stage_consumes_hidden_and_rejects_roles` (covers both the wrong-rank and the Fp16-dtype rejection).
2. **`sequence_position` arrives out of order or repeated** (caller retry / bug) — expected: reject and leave the KV cache untouched; a subsequent correct forward still works. → Task 6, test `forward_rejects_out_of_order_position_then_accepts_corrected`.
3. **`model_id` mismatch or double `LoadLayers`** — expected: refuse before retaining any bytes; a failed load leaves zero residue (next correct load allocates the exact expected figure). → Task 3, tests `wrong_model_id_is_refused_and_leaves_no_residue` and `second_load_without_unload_is_refused`.
4. **The default model ties word embeddings (no `output` tensor)** — expected: full-range load succeeds with lm_head = `token_embd`; an untied fixture's allocation includes `output.weight`. → Task 3 (`tied_fixture_loads_full_range`, allocation differential) and Task 6 (`tied_full_range_forward_returns_logits`).
5. **Driver rollback after a partial load** — expected: `UnloadLayers` on a never-loaded range = `success/freed 0`, and a reload afterwards succeeds (no stuck "already loaded" state). → Task 3, test `unload_of_never_loaded_model_succeeds_with_zero_then_reload_works`.

Known CI gap, disclosed rather than hidden: **K-quant (Q4_K/Q6_K) correctness has no committed golden vectors** (generating them needs the real model). CI gets structural + invariant tests (Task 4); the real gate is the asset-gated real-model load (Task 4) and reference parity (Task 8), both run locally before PR3/PR4 merge. PR descriptions must say this.

---

## File Structure

```
Cargo.toml                  members += "worker"
.github/workflows/ci.yml    new "worker" job (clippy + test), because ci.yml only
                            runs -p ai-upcycler-core/-p ai-upcycler-desktop today
worker/Cargo.toml           package ai-upcycler-worker, lib + bin (bin = default)
worker/build.rs             tonic-build ../proto/cluster_rpc.proto (protoc-bin-vendored)
worker/src/lib.rs           pub mod proto, gguf, quant, math, layers, engine
worker/src/proto.rs         tonic::include_proto!("aiupcycler.rpc")  (mirrors core)
worker/src/gguf.rs          GgufFile: header/KV/tensor-table parse, raw tensor reads
worker/src/quant.rs         GgmlType + dequantize (F32/F16/Q4_0/Q8_0/Q4_K/Q6_K)
worker/src/math.rs          rms_norm, linear, rope_row, softmax_inplace, silu_inplace
worker/src/layers.rs        BlockWeights, KvLayer, run_block, embed/head helpers
worker/src/engine.rs        WorkerEngine: WorkerService impl, state machine
worker/src/main.rs          CLI args, bind, READY line, tonic serve
worker/tests/common/mod.rs  write_tiny_model fixture writer, serve_engine, spawn_worker
worker/tests/process_smoke.rs     Task 1
worker/tests/gguf_reader.rs       Task 2
worker/tests/load_contract.rs     Task 3
worker/tests/dequant.rs           Task 4 (plus tests/real_model.rs, #[ignore])
worker/tests/math_units.rs        Task 5
worker/tests/forward.rs           Task 6
worker/tests/split_equivalence.rs Task 7 (plus #[ignore] real-model variant)
worker/tests/parity.rs            Task 8 (#[ignore])
```

Boundaries: `gguf` knows bytes and metadata, `quant` knows bit layouts, `math` is pure numerics with no engine knowledge, `layers` assembles one transformer block, `engine` owns RPC semantics/state. Files that change together live together; tasks only edit their listed files.

**The tiny fixture model (used by every CI test), exact spec** — writer and tests both derive from this table:

| KV | value |
|---|---|
| general.name | `tiny-qwen2-f32` (configurable) |
| general.architecture | `qwen2` (any other → `GgufFile::open` error) |
| qwen2.block_count | 4 (configurable; reference test uses 1) |
| qwen2.context_length | 64 |
| qwen2.embedding_length | 64 |
| qwen2.feed_forward_length | 128 |
| qwen2.attention.head_count | 4 |
| qwen2.attention.head_count_kv | 2 |
| qwen2.attention.key_length / value_length | 16 (head_dim) |
| qwen2.attention.layer_norm_rms_epsilon | 1e-5 (f32; reader also accepts f16 KV) |
| qwen2.rope.freq_base | 10000 (absent → default 10000) |
| vocab | derived from `token_embd.weight` rows = 256 |

Tensors (ggml dims: `dims[0] = n_in` contiguous, `dims[1] = n_out`; `y[o] = Σᵢ w[o·n_in + i]·x[i]`):
`token_embd.weight` [64,256]; per layer `i`: `blk.{i}.{attn_norm,ffn_norm}.weight` [64], `blk.{i}.{attn_q_norm,attn_k_norm}.weight` [16], `blk.{i}.attn_q.weight` [64,64], `blk.{i}.attn_{k,v}.weight` [64,32], `blk.{i}.attn_output.weight` [64,64], `blk.{i}.ffn_{gate,up}.weight` [64,128], `blk.{i}.ffn_down.weight` [128,64]; `output_norm.weight` [64]; `output.weight` [64,256] only when `tied = false`. Tied config omits `output.weight` (this is the default-model shape — Review Focus 4).

Weights: deterministic xorshift32 in `[-0.05, 0.05]` (same seed → byte-identical fixtures across runs; no `rand` dependency). Writer encodes GGUF v3: magic `GGUF`, version 3, n_tensors, n_kv, KV list (u64 len + key, u32 type + value — both type tags are 4-byte enums on disk, corrected after PR1 review against gguf.md and llama.cpp's reference reader — ), tensor infos (name, u32 n_dims, u64 dims[], u32 type — `ggml_type` is `uint32_t` per the official spec, corrected after PR1 review — , u64 offset), data section at 32-byte alignment.

---

### Task 1: Workspace crate scaffold, proto codegen, Handshake/Health, CI wiring

**Files:**
- Modify: `Cargo.toml:2` (members), `.github/workflows/ci.yml:9-34`
- Create: `worker/Cargo.toml`, `worker/build.rs`, `worker/src/{lib.rs,proto.rs,engine.rs,main.rs}`, `worker/tests/common/mod.rs`, `worker/tests/process_smoke.rs`

**Interfaces:**
- Consumes: nothing from `core` yet.
- Produces:
  - `pub struct WorkerEngine` in `worker::engine` with `pub fn new(worker_id: &str, device_name: &str, model_path: PathBuf) -> Result<Self, String>` (returns `Ok` until Task 2 adds parsing), implementing `worker::proto::aiupcycler::rpc::worker_service_server::WorkerService`.
  - CLI: `ai-upcycler-worker --model <path> [--bind 127.0.0.1:0] [--worker-id host-worker]`; on bind prints exactly `READY <ip:port>\n` to stdout and flushes. `--model` missing → exit 2 with usage on stderr.
  - `worker/tests/common`: `pub fn write_tiny_model(dir: &Path, cfg: TinyModelConfig) -> Fixture` where `TinyModelConfig { n_layers: usize, tied: bool, quant: FixtureQuant }`, `FixtureQuant::{F32, Q4_0, UnsupportedQ5_1}` *(Task 4 amendment: was `UnsupportedQ5_0` — Q5_0 (6) joined the supported set after the real-model gate, so the loud-refusal marker moved to Q5_1 (7)),* and `Fixture { path: PathBuf, model_id: String, n_layers: usize, hidden: usize, vocab: usize, ... , fn expected_f32_bytes(&self, start: usize, end: usize) -> u64 }` implementing the ownership rule (includes `token_embd` iff `start == 0`, head tensors iff `end == n_layers - 1`) from the fixture's declared dims — independent of anything the engine returns.
  - Handshake response: `worker_id`/`device_name` as configured (device defaults to `sysinfo::System::host_name()` or `"host"`), real `total_ram_bytes`/`free_ram_bytes` via sysinfo, `cpu_cores` from `std::thread::available_parallelism()`, `has_npu_acceleration = false`. Health: real `free_ram_bytes`, `battery_percentage = 0.0`, `cpu_temp_celsius = 0.0`.
  - Compute RPCs in this task honestly refuse: `LoadLayers`/`UnloadLayers` → `success = false` + `error_message` containing `not implemented`; `ExecuteForward` → `success = false`, no tensor (its response type has **no** message field — reason goes to `eprintln!`).

- [ ] **Step 1: Write the fixture writer and the failing smoke test**

`worker/tests/common/mod.rs`: the GGUF-v3 byte writer per the spec table above (this is test-support, written with the test). `worker/tests/process_smoke.rs`:

```rust
#[test]
fn worker_binary_serves_handshake_and_health() {
    let dir = tempfile::tempdir().unwrap();
    let fx = common::write_tiny_model(dir.path(), common::TinyModelConfig::default());
    let mut child = Command::new(env!("CARGO_BIN_EXE_ai-upcycler-worker"))
        .args(["--model", fx.path.to_str().unwrap(), "--bind", "127.0.0.1:0",
               "--worker-id", "smoke-1"])
        .stdout(Stdio::piped()).spawn().unwrap();
    let addr = common::read_ready_line(&mut child.stdout); // "READY ip:port" -> String
    // connect tonic client: handshake returns worker_id "smoke-1"; health free_ram_bytes > 0
    child.kill().unwrap();
}
```

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test -p ai-upcycler-worker process_smoke`
Expected: FAIL — package `ai-upcycler-worker` does not exist (workspace member missing).

- [ ] **Step 3: Implement the scaffold**

`Cargo.toml` members → `["core", "src-tauri", "worker"]`. `worker/Cargo.toml`: name `ai-upcycler-worker`, version `0.1.0`, edition `2021`, license `MIT`; deps `tonic = { version = "0.13", features = ["transport"] }`, `prost = "0.13"`, `tokio = { version = "1.36", features = ["rt-multi-thread", "macros", "net"] }`, `tokio-stream = "0.1"`, `memmap2 = "0.9"`, `sysinfo = "0.39"`, `thiserror = "1"`; build-deps `tonic-build = "0.13"`, `protoc-bin-vendored = "3"`; dev-deps `tempfile = "3"`, `tokio = { features = ["full"] }`. `build.rs` copies `core/build.rs` verbatim (path `../proto/cluster_rpc.proto`). `engine.rs`: state behind `Arc<Mutex<...>>` (tonic methods take `&self`); lock poisoning → refuse RPC. `main.rs`: hand-rolled arg parse, `tokio::net::TcpListener::bind` → print `READY {local_addr}` → `Server::builder().add_service(WorkerServiceServer::new(engine)).serve_with_incoming(TcpListenerStream::new(listener))`. CI: new `worker` job in `ci.yml` mirroring the `core` job skeleton (checkout, rust-toolchain with rustfmt+clippy, rust-cache, then `cargo clippy -p ai-upcycler-worker --all-targets -- -D warnings` and `cargo test -p ai-upcycler-worker`).

- [ ] **Step 4: Run to verify it passes**

Run: `cargo test -p ai-upcycler-worker process_smoke` → PASS.

- [ ] **Step 5: Full verify, then commit (spec + plan travel with PR1)**

Run: `cargo fmt --all -- --check && cargo clippy --workspace --all-targets -- -D warnings && cargo test -p ai-upcycler-core -p ai-upcycler-worker` → all green.

```bash
git checkout -b feat/worker-engine-gguf-load
git add Cargo.toml worker .github/workflows/ci.yml docs/superpowers
git -c user.name='AI Upcycler Bot' -c user.email='dev@aiupcycler.org' commit \
  -m "feat(worker): scaffold worker crate with proto codegen, handshake, health"
```

---

### Task 2: GGUF reader

**Files:**
- Modify: `worker/src/engine.rs` (constructor now parses), `worker/src/lib.rs`
- Create: `worker/src/gguf.rs`, `worker/tests/gguf_reader.rs`

**Interfaces:**
- Consumes: `common::write_tiny_model` / `Fixture` from Task 1.
- Produces (all pub in `worker::gguf`):
  - `GgufFile::open(path: &Path) -> Result<GgufFile, GgufError>` (`thiserror`) — accepts GGUF version 3 only; little-endian reads throughout; honors `general.alignment` (default 32); fails on bad magic, truncated table, `general.architecture != "qwen2"`, or missing required KV.
  - `fn model_name(&self) -> &str` (from `general.name`).
  - `fn hparams(&self) -> &ModelHparams` with `pub struct ModelHparams { block_count: u32, embedding_length: u32, feed_forward_length: u32, head_count: u32, head_count_kv: u32, head_dim: u32, rms_eps: f32, rope_theta: f32, context_length: u32 }` (`head_dim` = `key_length` KV if present, else `embedding_length / head_count`; `rope_theta` = `<arch>.rope.freq_base` KV else 10000.0; `vocab_size` is NOT here — vocab comes from tensor rows in Task 3).
  - `fn tensor(&self, name: &str) -> Option<TensorDesc>` with `pub struct TensorDesc { ggml_type: u32, ne: Vec<u64>, nbytes: u64 }`.
  - `fn read_raw(&self, name: &str) -> Result<Vec<u8>, GgufError>` — exact `nbytes` from the mmap'd data section.
- `WorkerEngine::new(...)` now parses at construction; `main.rs` exits 1 printing the `GgufError` when `--model` is bad.

- [ ] **Step 1: Write the failing reader tests**

`worker/tests/gguf_reader.rs`: parse the default fixture and assert `model_name()`, `hparams()` against the spec-table constants (4/64/128/4/2/16/1e-5/10000/64), tensor table contains exactly the declared names with `ne` as specified, `read_raw("blk.0.attn_q.weight")` length = `64*64*4` and bytes equal what the writer produced; negative cases: corrupted magic → `Err`, `general.architecture = "llama"` fixture → `Err`, file truncated mid-tensor-info → `Err`.

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test -p ai-upcycler-worker gguf_reader`
Expected: FAIL — `gguf` module not found / unresolved import.

- [x] **Step 3: Implement `gguf.rs` and constructor parsing**

Parse order: magic/version → n_tensors/n_kv → KV (generic skip by type, typed getters for the required keys) → tensor infos → data-section bounds check (offset + nbytes ≤ file len). `WorkerEngine::new` stores `GgufFile` (bad file → `Err`, process exits before serving).

- [x] **Step 4: Run to verify it passes (plus Task 1's smoke)**

Run: `cargo test -p ai-upcycler-worker` → all PASS (smoke still green — it already passes a real fixture).

- [x] **Step 5: Commit**

```bash
git add worker && git -c user.name='AI Upcycler Bot' -c user.email='dev@aiupcycler.org' \
  commit -m "feat(worker): gguf v3 reader with hparams and tensor table"
```

---

### Task 3: LoadLayers /UnloadLayers contract with real weight retention

**Files:**
- Modify: `worker/src/engine.rs`
- Create: `worker/tests/load_contract.rs`

**Interfaces:**
- Consumes: `GgufFile` (Task 2), `Fixture::expected_f32_bytes` (Task 1), tonic server types.
- Produces:
  - `tests/common::serve_engine(engine: WorkerEngine) -> (u16, tokio::task::JoinHandle<()>)` — binds `127.0.0.1:0` and serves in-process, mirroring `core/tests/common/mod.rs::serve_mock`.
  - `LoadLayers` semantics (in order): `model_id != model_name` → false+msg; `start > end || end >= block_count` → false+msg; already loaded → false+msg; collect tensors = the 11 `blk.{i}.*` names present for each layer in range, `token_embd.weight` iff `start == 0`, `output_norm.weight` + `output.weight` iff `end == block_count-1` and it exists (tied: skip) → any missing tensor or unsupported/non-F32 type → false+msg; else materialize F32, store range, return `success=true, allocated_ram_bytes = materialized.len() * 4`. *(Task 4 amendment: `attn_q_norm`/`attn_k_norm` become optional-if-present — real qwen2 files have no QK-norm tensors, while the fixture (Qwen3-style shape) does; the other 9 per-block tensors stay strict-refuse-if-missing.)*
  - `UnloadLayers`: wrong `model_id` → false+msg; never loaded → `true, 0`; loaded → free everything, `true, freed_ram_bytes = bytes freed`.
  - Loaded state also records `(start, end)` — Task 6's forward reads it.

- [ ] **Step 1: Write the failing contract tests**

`worker/tests/load_contract.rs` (each fn one behavior; direct tonic `WorkerServiceClient` against `serve_engine`):

- `load_full_range_allocates_expected_and_unload_reports_same` — fixture default (4 layers, tied): load `0..3` → success; allocated equals `Fixture::expected_f32_bytes(0,3)` **and** an inline recomputation from the table `(256·64 + 4·(64+16+16+64·64+2·64+2·64+64·64+64+128·64+128·64+64·128) + 64) · 4` (two independent expressions must agree); unload → freed == allocated.
- `partial_load_includes_embd_but_not_head` — load `0..1` → allocated == `expected_f32_bytes(0,1)` (contains token_embd, excludes output_norm); distinguishes ownership rule.
- `wrong_model_id_is_refused_and_leaves_no_residue` — load with `"other-model"` → `success=false`, `error_message` non-empty; then correct load → success with exact expected allocation (no bytes retained by the refusal).
- `out_of_range_and_inverted_ranges_are_refused` — `0..4` and `2..1` → false with message.
- `second_load_without_unload_is_refused` — load ok, load again → false; allocated state untouched (subsequent unload still reports the original figure).
- `unload_of_never_loaded_model_succeeds_with_zero_then_reload_works` — (rollback scenario, Review Focus 5) load `0..1`, unload, then load `0..3` → success with full allocation; also fresh-engine unload → `true, freed = 0`.
- `tied_fixture_loads_full_range` — tied default fixture loads `0..3` (no `output.weight` exists; guards tie rule at load).
- `unsupported_tensor_type_is_refused_loudly` — `FixtureQuant::UnsupportedQ5_1` fixture → false, message contains the type id *(Task 4 amendment: was `UnsupportedQ5_0`; see the supported-types amendment above)*.

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test -p ai-upcycler-worker load_contract`
Expected: FAIL — LoadLayers still returns "not implemented".

- [ ] **Step 3: Implement `load_layers` / `unload_layers` in `worker/src/engine.rs`**

Follow the Produces contract exactly; validation order as listed (cheap checks before any mmap read).

- [ ] **Step 4: Run to verify it passes**

Run: `cargo test -p ai-upcycler-worker load_contract` → PASS.

- [ ] **Step 5: Mutation check the model_id gate**

Back up `worker/src/engine.rs` to `/tmp/opencode/`; replace the `model_id != model_name` comparison with `false`; run `cargo test -p ai-upcycler-worker wrong_model_id` → must FAIL; restore; re-run → PASS. Record the check for the PR description.

- [ ] **Step 6: Commit**

```bash
git add worker && git -c user.name='AI Upcycler Bot' -c user.email='dev@aiupcycler.org' \
  commit -m "feat(worker): LoadLayers/UnloadLayers retain real gguf layer weights"
```

End of PR1: local verify (Global Constraints), `gh pr create` (describe contract tests + mutation check + note that ExecuteForward still refuses honestly), confirm `PR Reviewer Guide 🔍`, squash-merge.

---

### Task 4: Dequantization (F16/Q4_0/Q8_0/Q4_K/Q6_K) + real-model load gate

**Files:**
- Modify: `worker/src/lib.rs`, `worker/src/engine.rs` (materialization path)
- Create: `worker/src/quant.rs`, `worker/tests/dequant.rs`, `worker/tests/real_model.rs`

**Interfaces:**
- Consumes: `TensorDesc.ggml_type`, fixture writer's `FixtureQuant` variants (Task 1).
- Produces (pub in `worker::quant`):
  - `pub enum GgmlType { F32, F16, Q4_0, Q5_0, Q8_0, Q4_K, Q6_K }` with `pub fn from_id(id: u32) -> Option<GgmlType>` (ids 0/1/2/6/8/12/14 — *(Task 4 amendment: Q5_0 (6) added after the real-model gate found it in Qwen's official q4_k_m asset; see the supported-types amendment at the top of this task)* anything else → `None` → Task 3's loud refusal).
  - `pub fn dequantize(ty: GgmlType, raw: &[u8], ne: &[u64]) -> Result<Vec<f32>, String>` — whole tensor; row length = `ne[0]`, rows = product of `ne[1..]`; row length must satisfy the type's block divisibility (32 for Q4_0/Q8_0, 256 for Q4_K/Q6_K, else error); output length = product(`ne`).
- Engine: `LoadLayers` now accepts any `GgmlType` in the supported set (still materializes F32; `allocated_ram_bytes` unchanged semantics = F32 bytes).

- [ ] **Step 1: Write the failing dequant tests**

`worker/tests/dequant.rs`:
- `q4_0_block_matches_hand_computation` — hand-construct one 32-element block (`d = 0.5` exactly representable in f16, chosen nibble bytes), expected values pinned in the test as literals with the arithmetic shown in a comment.
- `q8_0_block_matches_hand_computation` — same style (d + 32 i8 scales).
- `f16_round_trip_for_exactly_representable_values` — `[0.5, -1.0, 2.0, 0.25, 1.0009765625]` → bit-exact f32 (hand-rolled f16→f32 conversion, no new dep).
- `k_quants_produce_finite_full_length_output` — synthetic Q4_K/Q6_K super-block bytes (hand-assembled per the layout in llama.cpp `ggml-common.h`, cited): output length correct, all finite, values within `±(|d|·sum(scales)…)` bounded range — structural, not golden (see Known CI gap).
- `q4_0_fixture_allocates_same_as_f32_fixture` — engine-level differential: `Q4_0` variant of the default fixture loads `0..3` with allocated **equal** to the F32 fixture's (dequantized storage claim).
- `unsupported_type_refused_with_type_id` — promotes the Task 3 placeholder to a real assertion against `UnsupportedQ5_1` *(Task 4 amendment: was `UnsupportedQ5_0`; see above)*.

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test -p ai-upcycler-worker dequant` → FAIL — `quant` module not found.

- [ ] **Step 3: Implement `quant.rs` and wire it into load**

Q4_K/Q6_K implemented from llama.cpp's `dequant_row_q4_K`/`dequant_row_q6_K` (source-driven step: fetch official `ggml.c`/`ggml-quants.c` before coding; cite file+function in comments). F16→f32 via the standard bit-twiddling algorithm.

- [ ] **Step 4: Run to verify it passes**

Run: `cargo test -p ai-upcycler-worker dequant && cargo test -p ai-upcycler-worker` → PASS (load_contract untouched, still green).

- [ ] **Step 5: Asset-gated real-model gate (`#[ignore]`, local)**

`worker/tests/real_model.rs::real_model_header_and_full_load` — skips with a printed message unless `AI_UPCYCLER_MODEL` is set. When set, assert: `GgufFile` hparams match Qwen2.5-0.5B (block_count 24, embedding 896, heads 14, kv_heads 2, ff 4864, context ≥ 8192 — cross-check against Qwen's `config.json` in the step and fix constants if they differ, they are the independent oracle), **every** tensor's type ∈ supported set, full-range materialization completes in < 25 s (print elapsed + GB allocated — this is the spec's load-budget benchmark; *(Task 4 amendment: the budget asserts only in release profiles — debug builds dequantize ~6× slower and the binary ships in release)*), and all values finite. If assets are absent, follow the spec's asset step first: download `llama-server` linux-x64 release from `ggml-org/llama.cpp` releases and `qwen2.5-0.5b-instruct-q4_k_m.gguf` from `huggingface.co/Qwen/Qwen2.5-0.5B-Instruct-GGUF` (matches `DEFAULT_MODEL_FILE`), store outside the repo, export both env vars.

- [ ] **Step 6: Commit (include real-model results in PR2 text)**

```bash
git add worker && git -c user.name='AI Upcycler Bot' -c user.email='dev@aiupcycler.org' \
  commit -m "feat(worker): dequantize gguf weight types at load"
```

---

### Task 5: Decoder math units (pure F32)

**Files:**
- Modify: `worker/src/lib.rs`
- Create: `worker/src/math.rs`, `worker/tests/math_units.rs`

**Interfaces:**
- Consumes: nothing (pure functions).
- Produces (pub in `worker::math`):
  - `pub fn rms_norm(x: &mut [f32], weight: &[f32], eps: f32)` — in-place, `weight.len() == x.len()`.
  - `pub fn linear(out: &mut [f32], weight: &[f32], n_in: usize, x: &[f32])` — `out.len() = weight.len() / n_in`, ggml row-major `w[o·n_in + i]`, overwrites `out`.
  - `pub fn rope_row(head: &mut [f32], pos: usize, theta: f32)` — rotates pairs in place; pair `j` frequency `theta^(-2j/head_dim)`; `head.len()` even.
  - `pub fn softmax_inplace(v: &mut [f32])` — max-subtracted.
  - `pub fn silu_inplace(v: &mut [f32])` — `x · σ(x)`.

- [ ] **Step 1: Write the failing unit tests**

`worker/tests/math_units.rs` — test-side f64 reference implementations of each formula, asserting `|f32 − f64| ≤ 1e-5` on inputs including negatives/zeros, PLUS one hand-computed literal case each:
- `rms_norm_matches_f64_reference` and literal: `x=[3,4], w=[1,1], eps=1e-12` → `[3,4]/sqrt(12.5)` to 1e-6.
- `linear_2x3_known_matrix` — literal 2×3 weight, 3-vector in → 2 outputs computed by hand.
- `rope_pos0_is_identity`, `rope_preserves_pair_norm` (property), `rope_matches_f64_reference` for `pos=1..4`.
- `softmax_matches_f64_reference` on `[1.0, 2.0, 0.5, -3.0]`; literal case on `[0.0, ln2]` → `[1/3, 2/3]`.
- `silu_matches_f64_reference` on `[-2.0, -0.5, 0.0, 1.5]`.

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test -p ai-upcycler-worker math_units` → FAIL — module not found.

- [ ] **Step 3: Implement `math.rs`**

- [ ] **Step 4: Run to verify it passes**

Run: `cargo test -p ai-upcycler-worker math_units` → PASS.

- [ ] **Step 5: Commit**

```bash
git add worker && git -c user.name='AI Upcycler Bot' -c user.email='dev@aiupcycler.org' \
  commit -m "feat(worker): f32 decoder math primitives (rmsnorm, rope, softmax, silu)"
```

End of PR2: local verify, PR, guide-comment check, squash-merge.

---

### Task 6: ExecuteForward — sessions, KV cache, stage roles, logits

**Files:**
- Modify: `worker/src/engine.rs`
- Create: `worker/src/layers.rs`, `worker/tests/forward.rs`

**Interfaces:**
- Consumes: `math::*` (Task 5), loaded state `(start, end)` + F32 tensors (Task 3/4), `ModelHparams`.
- Produces:
  - `worker::layers`:
    - `pub struct KvLayer` — per-layer K/V rows, layout `[seq][kv_heads·head_dim]` row-major, grows dynamically (double); **cap = `hparams.context_length` rows** (never pre-allocates full context — the real model's 32k context × 24 layers would be gigabytes).
    - `pub struct BlockWeights` — the 11 (or tied-reduced) F32 tensors for one layer.
    - `pub fn run_block(w: &BlockWeights, h: &ModelHparams, kv: &mut KvLayer, x: &mut [f32], n_tokens: usize, positions: std::ops::Range<usize>)` — qwen2 block order: `rms_norm(attn_norm)` → q/k/v `linear` → per-head `q_norm`/`k_norm` → `rope_row` on q and k at each row's position → append **rotated** k/v to `kv` → causal attention over the whole cache (row `i` sees keys `0..prev_len+i+1`; GQA: kv head `g` serves heads `g·(head_count/head_count_kv)..`) with `1/sqrt(head_dim)` scaling and f32 softmax → `linear(attn_output)` → residual → `rms_norm(ffn_norm)` → `silu(gate)·up` → `linear(ffn_down)` → residual.
    - `pub fn embed_ids(ids: &[u32], token_embd: &[f32], hidden: usize, out: &mut [f32])`, `pub fn lm_head(hidden_row: &[f32], head: &[f32], hidden: usize, out: &mut [f32])`.
  - `WorkerEngine` sessions: `HashMap<String, Session>` (session = `Vec<KvLayer>` indexed by `abs_layer − start`), `MAX_SESSIONS = 16`.
  - `execute_forward` contract: not loaded → false; new session at cap → false; `data_type != FP32` → false; role shape check — `start == 0` requires `[1, n]`, `start > 0` requires `[n, embedding_length]`, `n ≥ 1` — else false; token ids (stage 0) must satisfy `id < vocab` → else false; `sequence_position < 0` or `!= kv_len` → false; `position + n > context_length` → false. Success: run `start..=end`; if `end == block_count-1` apply `output_norm` + head → logits `[n, vocab]`, else output `[n, hidden]`; populate shape AND `data_type = FP32`; `execution_time_us` = elapsed micros (> 0 for any real block pass). Failures: `success=false`, tensor `None`, time 0, reason via `eprintln!("[worker] forward refused: …")` (the proto has no error message on this response — noted for reviewers).
  - Unload frees sessions too; forward after unload → false; reload → fresh sessions at position 0.

- [x] **Step 1: Write the failing tests**

`worker/tests/forward.rs` (in-process `serve_engine` + direct tonic client; default 4-layer fixture unless noted):
- `stage0_full_range_returns_logits` — fixture config `n_layers: 1` (writer's `TinyModelConfig`), ids `[1,2]` as `[1,2]` F32 → `success`, shape `[2, 256]`, dtype FP32, `execution_time_us > 0`.
- `partial_stage0_returns_hidden` — `0..1` load, ids `[1,3,2]` → shape `[3, 64]`. *(Amendment, PR3: the plan listed "ids `[1,3]`" against shape `[3, 64]` — inconsistent token count; resolved to three ids `[1,3,2]`.)*
- `mid_stage_consumes_hidden_and_rejects_roles` — `2..2` load: F32 `[3,64]` hidden → success `[3,64]`; then `[1,3]` ids → false; `[3,64]` Fp16 dtype → false (Review Focus 1).
- `forward_rejects_out_of_order_position_then_accepts_corrected` — fresh session: `sequence_position=1` → false; `=0` → success; repeat `=0` → false (cache intact); `= n` → success (Review Focus 2).
- `forward_rejects_token_id_at_or_above_vocab` and `forward_rejects_beyond_context_length`.
- `session_cap_refuses_the_seventeenth_concurrent_session` (16 succeed, 17th false), `sessions_are_isolated_interleaved` (same ids/pos in two sessions → byte-identical to solo runs), `unload_clears_sessions_and_reload_starts_fresh`.
- `tied_full_range_forward_returns_logits` (Review Focus 4 tie at forward).
- `single_block_matches_f64_reference` — fixture `n_layers: 1`, ids `[1,2]`: test-side f64 reference runs embed → qwen2 block (rms/qknorm/rope/causal-GQA-softmax/silu) → tied head; assert every logit within `1e-4` of the engine's F32 output. This is the CI gate for block assembly (attention indexing, GQA broadcast, residual order).

- [x] **Step 2: Run to verify it fails**

Run: `cargo test -p ai-upcycler-worker forward` → FAIL — ExecuteForward still refuses.

- [x] **Step 3: Implement `layers.rs` + `execute_forward`**

- [x] **Step 4: Run to verify it passes**

Run: `cargo test -p ai-upcycler-worker` → all green (load/dequant/reader/smoke untouched).

- [x] **Step 5: Mutation check the position gate**

Back up `engine.rs`; weaken the `sequence_position != kv_len` check to `sequence_position < kv_len` → `forward_rejects_out_of_order_position_then_accepts_corrected` must FAIL; restore; re-run green. Record for PR3.

- [x] **Step 6: Commit**

```bash
git add worker && git -c user.name='AI Upcycler Bot' -c user.email='dev@aiupcycler.org' \
  commit -m "feat(worker): ExecuteForward with kv sessions, stage roles, real block math"
```

---

### Task 8: Reference parity vs llama-server (asset-gated) — lands in PR3

**Files:**
- Create: `worker/tests/parity.rs`

**Interfaces:**
- Consumes: `GgufFile`, engine + `serve_engine` (real model via `AI_UPCYCLER_MODEL`), `llama-server` binary via `AI_UPCYCLER_LLAMA_SERVER`.
- Produces: `#[ignore]` test `next_token_matches_llama_server`; prompt `"The capital of France is"`, `sequence_position = 0`.

- [x] **Step 1: Source-driven discovery (local, before writing asserts)**

With the assets from Task 4's Step 5 present, run `"$AI_UPCYCLER_LLAMA_SERVER" --help` and the official llama.cpp HTTP docs: confirm `POST /tokenize` (returns token ids for `{"content": ...}`), `GET /health`, and whether `/v1/completions` or `/completion` returns `logprobs` in this release. Record findings in the test's header comment — they decide which assert branch below is active.

- [x] **Step 2: Write the failing parity test**

Flow: start `llama-server --model $AI_UPCYCLER_MODEL --port <free> --ctx 512` (stdout piped, killed at end), poll `/health` to ready → `POST /tokenize` → ids → feed `[1, n]` F32 to a full-range in-process engine → `argmax(logits)`. Required assert: greedy server next token (`temperature 0, n_predict 1` on the same prompt text) equals our argmax. Secondary assert if logprobs exist (per Step 1): server top-5 logprob values vs our `log_softmax` top-5 — same token set ≥ 4/5 and shared-token logprob diff ≤ 0.05. Test returns early (printing "skipped: env unset") when either env var is missing, so CI is never affected even without `#[ignore]`.

- [x] **Step 3: Run locally with assets**

Run: `AI_UPCYCLER_MODEL=... AI_UPCYCLER_LLAMA_SERVER=... cargo test -p ai-upcycler-worker parity -- --ignored --nocapture`
Expected: PASS; paste the argmax pair + logprob table into PR3's description. This plus Task 6's mutation check is PR3's evidence.
*(See the test's header comment for the numeric-regime amendment: greedy-argmax stays exact, the shared-token bound is 0.6 on the quantized file — llama.cpp's CPU kernels quantize activations to 8-bit inside quantized-weight matmuls (block-bisect evidence in the PR); an F32-converted twin of the same model agrees with this engine to ≤ 4e-4 nats. Two real engine bugs were found and fixed by this gate: real Qwen2.5 files ship `attn_{q,k,v}.bias` tensors, and `rope_row` used GPT-J-style pairing instead of llama.cpp's NeoX `(i, i + head_dim/2)`.)*

- [x] **Step 4: Commit**

```bash
# two commits — the gate uncovered real engine bugs:
git add worker/src/math.rs worker/src/layers.rs worker/src/engine.rs worker/tests/forward.rs worker/tests/common worker/tests/math_units.rs
git -c user.name='AI Upcycler Bot' -c user.email='dev@aiupcycler.org' \
  commit -F <file>   # "fix(worker): qkv attention biases + neox rope pairing for llama parity"
git add worker/tests/parity.rs worker/examples
git -c user.name='AI Upcycler Bot' -c user.email='dev@aiupcycler.org' \
  commit -F <file>   # "test(worker): reference parity of full-range logits vs llama-server"
```

End of PR3: local verify (fixture tests all green without assets), PR with parity numbers + mutation check + K-quant disclosure, guide-comment check, squash-merge.

---

### Task 7: Split equivalence over two real worker processes

**Files:**
- Modify: `worker/Cargo.toml` (dev-dep `ai-upcycler-core = { path = "../core" }`), `worker/tests/common/mod.rs`
- Create: `worker/tests/split_equivalence.rs`

**Interfaces:**
- Consumes: `ai_upcycler_core::rpc::engine::{LoadedPipeline, TensorPayload, ActivationType}`, `ai_upcycler_core::scheduler::planner::LayerAssignment`, fixture + spawn helpers.
- Produces: `common::spawn_worker(model_path: &Path, worker_id: &str) -> (u16, std::process::Child)` — spawns `env!("CARGO_BIN_EXE_ai-upcycler-worker")` with `--bind 127.0.0.1:0 --model … --worker-id …`, reads stdout until `READY ` and parses the port; `common::ids_payload(session: &str, ids: &[u32]) -> TensorPayload`.

- [ ] **Step 1: Write the failing test**

`worker/tests/split_equivalence.rs::two_worker_split_matches_single_worker`:

```rust
// single: one process, plan [0..3]; dual: two processes, plan [0..1, 2..3];
// same fixture (default 4-layer, tied, F32), same ids [3,1,2,1], session "split-1".
// LoadedPipeline::load(&fixture.model_id, &plan) -> forward -> logits_a / logits_b
// -> unload both, kill children.
assert_eq!(logits_a.shape, logits_b.shape, "[4, 256]");
let max_diff = zip f32 pairs -> max abs diff;
assert!(max_diff <= 1e-6, "split changed the math: {max_diff}");
```

Plus `#[ignore]` sibling `real_model_two_worker_split_matches_single_worker` for `AI_UPCYCLER_MODEL` (same flow, 24 layers split at 12, `max_diff ≤ 1e-4`, argmax equality) — local evidence for PR4.

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test -p ai-upcycler-worker split_equivalence` → FAIL — `spawn_worker` not defined.

- [ ] **Step 3: Implement `spawn_worker` + `ids_payload` in `tests/common/mod.rs`**

- [ ] **Step 4: Run to verify it passes**

Run: `cargo test -p ai-upcycler-worker` → PASS. If `max_diff > 0`: debug before relaxing — identical F32 ops in identical order should be near bit-exact; a real gap means stage-boundary conversion crept in.

- [ ] **Step 5: Local real-model variant**

Run with `AI_UPCYCLER_MODEL` set (`-- --ignored --nocapture`), record numbers for PR4.

- [ ] **Step 6: Commit**

```bash
git add worker && git -c user.name='AI Upcycler Bot' -c user.email='dev@aiupcycler.org' \
  commit -m "test(worker): prove two-worker split equals single worker over real processes"
```

---

### Task 9: Docs and tracker catch-up (end of PR4)

**Files:**
- Modify: `docs/plan.md` (§5.1 first-blocker row → resolved, pointing at the worker crate + PRs), `docs/tasks.md` (mark engine items done; add follow-up items: master-side tokenizer, live request-path epic, committed K-quant golden vectors once assets allow, worker mTLS + Android port), `docs/architecture.md` §3.1.4 (note "implemented by `worker/` — see spec/plan links", keep the contract text), `docs/superpowers/specs/2026-10-16-worker-engine-design.md` (Status → IMPLEMENTED with PR numbers)

- [ ] **Step 1: Update the four docs to match what actually shipped**

Re-read each edited region from disk (renderer garble guard). No claims beyond test evidence: cite the fixture CI tiers, the two `#[ignore]` gates, and the mutation checks by name.

- [ ] **Step 2: Verify docs and full suite**

Run: `cargo fmt --all -- --check && cargo clippy --workspace --all-targets -- -D warnings && cargo test -p ai-upcycler-core -p ai-upcycler-worker` → green; `git diff` review of doc edits.

- [ ] **Step 3: Commit into PR4, merge**

```bash
git add docs && git -c user.name='AI Upcycler Bot' -c user.email='dev@aiupcycler.org' \
  commit -m "docs: worker engine landed — plan, tasks, architecture updated"
```

Then: guide-comment check → `gh pr merge <N> --squash --delete-branch` → update `/tmp/opencode/todos-remaining-work.md` (epic done; next: `.apk` release asset, PR-Agent hardening, follow-up tokenizer/live-path epic).
