# Worker Engine — Design (2026-10-16)

Status: **IMPLEMENTED** (2026-10-17). The engine ships as the pure-Rust `worker/` crate:
GGUF layer-range loading, Q4_0/Q4_K/Q5_0/Q6_K/Q8_0 dequantization, full-range /
partial-range / mid-stage forwards with KV-cached sessions, and split equivalence over
real worker processes. Evidence: PRs #41 (scaffold + contract tests), #42 (dequant + math),
#53 (forward + llama-server logits parity) and PR4 (split equivalence; this document's
status). Build-approval gates: BUILDER-CHECK COMPLETE (7 items: 6 settled in-document as
my calls, 1 — the epic edge — answered by the partner: **ends at split equivalence**).

## Intent

Make the distributed split **real**. Today the master half (`LoadedPipeline`) loads,
forwards and unloads across workers, but the only thing behind `ExecuteForward` is a mock
that appends tag bytes. This design specifies the worker engine that holds actual layer
weights and computes actual transformer math, so a prompt pushed through
`LoadedPipeline::load` → `forward` → `unload` produces real model logits — proven
device-free by running two real worker processes on loopback.

This closes the plan's #1 blocker (`docs/plan.md` §5.1, "Native llama.cpp RPC worker
bindings (host)") and delivers the contract already written in `docs/architecture.md`
§3.1.4 / §3.1.

## Goals

- A **host worker binary** implementing the full `WorkerService` compute RPCs
  (`LoadLayers`, `ExecuteForward`, `UnloadLayers`) with real weights, exactly to the
  §3.1.4 contract (`success`/`error_message`, `allocated_ram_bytes`, `freed_ram_bytes`,
  `execution_time_us`, populated output shape/dtype).
- **Real numerics** for the model family the project already ships
  (Qwen2.5-style decoder, `qwen2.5-0.5b-instruct-q4_k_m.gguf` default): RMSNorm, RoPE,
  GQA attention, SiLU FFN, in F32.
- **Split equivalence as the correctness proof**: one worker executing layers 0..N-1 and
  two workers splitting the range must produce the same logits within a numerical
  tolerance, driven through the *existing, unmodified* driver in `core/tests/`.
- **Device-free CI**: no phone, no model download in CI. Structural tests run against
  tiny GGUF fixtures generated in-test; real-model comparison tests are locally-gated.
- Wire contract stays canonical (`proto/cluster_rpc.proto`); Android can later reuse this
  engine through the same proto (its vendored copy already exists).

**Epic edge (decided with the partner):** the epic ends at **split equivalence** — slice 4
below. Tokenization and the live chat-through-cluster path are a **follow-up epic**: the
repo has no tokenizer (llama-server does it today), and anything accepting a typed prompt
needs one that matches llama.cpp exactly. The parity test uses llama-server's
`/tokenize` endpoint as the prompt oracle, so no tokenizer is needed anywhere in this
epic. Worker-side `Handshake`/`GetHealthStatus` behavior is trivially specifiable and
stays as-is (metadata RPCs, already live on Android).

## Non-goals / anti-goals

- **Not** Android/iOS execution (hardware-gated; the Rust engine is chosen partly so an
  NDK port reuses it later, but no FFI work in this epic).
- **Not** llama.cpp's own distributed RPC backend — it would bypass our `WorkerService`
  contract; considered and rejected (see Alternatives).
- **Not** quantized *compute*: Q4_K/Q6_K/… weights are dequantized to F32 **at load**.
  Correct first, fast later. `allocated_ram_bytes` honestly reports the dequantized size.
- **Not** wire-side weight push in this epic: `LoadLayersRequest.weights_chunk_data`
  stays empty; workers resolve weights from a local GGUF path (`--model`), which the
  contract already permits ("resolved worker-side for now").
- **Not** batched/prefill-optimized decode: one `ExecuteForward` = one sequence position
  (or a short prefill loop unrolled to per-position calls). Speed optimization is out.
- Anti-goal: **never claim real inference while a mock is behind it**. Tests must fail
  loudly if the engine degrades to passthrough or drifts from reference outputs.

## Constraints

- Driver timeouts: 5 s connect, **30 s per RPC** (`core/src/rpc/client.rs`). F32 CPU
  decode for a 0.5B model is tens of ms per token — headroom exists; large prefills are
  the risk to watch.
- Plan validity (§3.1.1): plans must anchor at layer 0 and tile contiguously, and the
  driver does not know the layer count — a worker must reject an out-of-range
  `LoadLayers` with `success = false` rather than partial-load.
- `TensorBuffer.DataType` is FP32/FP16/BF16 only; output shape/dtype must be populated
  (§3.1.2 fallbacks are only correct if they are identity).
- Repo workflow: every slice lands as branch → PR → PR-Agent review → squash merge.
- No secrets, no network in tests, repo stays public.

## The decided shape (calls made in brainstorming; veto-able)

### 1. Execution: a purpose-built Rust decoder, not llama.cpp layer control

llama.cpp's public API (`llama_decode`) always runs the full graph from input tokens or
input embeddings to output — there is no "run layers s..e over this hidden state" entry
point, and the epic's contract is exactly that. A partial-GGUF trick (rewriting hparams
so llama.cpp loads a layer subset as a whole model) breaks input/output semantics and
lives outside any supported API. Therefore the worker engine implements the decoder
blocks directly in Rust:

- **Architecture support: Qwen2-family only** (matches `DEFAULT_MODEL_FILE`). Wrong-shape
  tensors from another family fail loudly at `LoadLayers`.
- **Activation convention**: F32 `[tokens, hidden]` between stages.
  - Stage 0 owns `token_embd`; its input is token ids carried as an F32 `[1, n]`
    activation (the only wire representation that fits `TensorBuffer`), converted to
    embeddings worker-side.
  - The stage owning the last layer also owns `output_norm` + lm_head and returns logits
    `[1, vocab]` — the same field every intermediate stage returns as hidden states. The
    driver needs no knowledge of who is last. **Tie fallback**: when the GGUF ties word
    embeddings (as `qwen2.5-0.5b-instruct-q4_k_m` does — no separate `output` tensor),
    lm_head *is* `token_embd`, used only by the stage that also owns `output_norm`.
  - **Short plans stay allowed at `load` (§3.1.1 untouched) but can never be sampled.**
    The follow-up live-path epic preflights: it reads `n_layers` from the model's GGUF header
    master-side and refuses to open a session whose plan stops before the final layer —
    loud failure, no proto change, driver validation untouched. Workers stay honest:
    only the final-layer owner computes head tensors; earlier stages return hidden
    states, which the contract already permits.
- **KV cache is per `session_id`**, held on the worker for its owned range;
  `sequence_position` gates RoPE and cache append; `UnloadLayers` frees weights **and**
  session state for that `model_id`. Sessions beyond a bounded count are rejected
  (`success = false`) instead of growing unboundedly.
  Live-path pattern: a **new `session_id` per generation**; the master abandons a session
  after a failed or aborted forward rather than resuming a half-appended cache; workers
  GC idle sessions. **No cross-turn cache continuity** in this epic — each generation
  feeds its full context (correct, simpler; continuity would be additive proto work later).

### 2. Weights: resolved worker-side from a local GGUF path

Worker process takes `--model <path>`. `LoadLayers` parses the GGUF, keeps only tensors
belonging to `[start_layer, end_layer]` plus the head/embedding tensors it owns by the
rule above, dequantizes them to F32, and reports `allocated_ram_bytes`. Missing file,
unsupported tensor type, out-of-range layer, or insufficient memory ⇒
`success = false` + message. GGUF parsing: hand-rolled (v3 header + kv + tensor table is
small and dependency-free) or the `gguf` crate — builder's call, criteria: must read the
tensor offsets/types correctly for the default model and stay license-compatible.

**Model identity (builder-check #5):** `LoadLayers.model_id` must equal the GGUF's
`general.name` KV, checked worker-side; a mismatch is a loud `success = false`. This
convention (no proto change, no `checkProtoInSync` impact) makes "two split stages
holding different weights" a hard error instead of confidently-wrong logits, and the
slice-4 equivalence test additionally asserts both spawned workers resolved the same
`--model` path before comparing outputs.

### 3. Shape of the epic — slices, each an independent PR

1. **GGUF reader + LoadLayers/UnloadLayers** in a new workspace crate `worker/`
   (`ai-upcycler-worker`): parse fixtures, retain/free layer tensors, honest RAM
   reporting, contract-negative tests (bad range, missing file, unknown type).
2. **Decoder block math**: RMSNorm, RoPE, GQA attention, SiLU FFN as testable units with
   hand-computed / fixed golden vectors; tensor-type dequantization tests.
3. **`ExecuteForward` end-to-end**: KV cache + session lifecycle; single worker owning
   0..N-1 producing logits. Locally-gated test: prompt tokenized **by `llama-server`'s
   `/tokenize` endpoint (the oracle — no tokenizer in this repo)**, those exact ids fed
   to our engine, compared against the server's next-token output → identical argmax and
   top-k logprobs within tolerance.
4. **Split equivalence**: spawn two worker processes on loopback, drive the **existing**
   `core/tests` driver over them; 1-worker vs 2-worker logits match; RAM-based
   `LayerScheduler` plan feeds it.
**Not in this epic — follow-up epic (partner decision, 2026-10-16):** the *live request
path* — desktop chat running through the cluster (master-side tokenizer matching
llama.cpp exactly, master spawning workers, sampling from driver logits, `n_layers`
preflight against short plans, llama-server kept as the honest fallback). It needs a
tokenizer the repo does not have, so this epic stops at slice 4.

### 4. Verification tiers

| Tier | Assets | Where it runs |
|---|---|---|
| Structural + contract tests | tiny GGUF fixtures (random weights, ~4 layers) generated in-test | every CI run |
| Golden math vectors | fixed arrays in test source | every CI run |
| Reference parity | real 0.5B model + `llama-server` binary | local only (`#[ignore]`/asset-gated) |
| Split equivalence (1 vs 2 workers) | tiny fixture GGUF | every CI run |

Reference-parity mechanics: `llama-server`'s HTTP surface cannot dump a full `[1, vocab]`
logit vector, so the locally-gated test compares the **argmax token (must be identical) and
top-k logprobs (within tolerance)** from the server's completion response against our full
logits reduced the same way. If the engine's weights or math were wrong, top-k would
diverge — the comparison is strong where it matters without linking llama.cpp anywhere.

Load budget: `LoadLayers` for the real model is a single synchronous read + dequant
(≈400 MB q4 file → ≈1.9 GB F32 resident for 0.5B) inside the 30 s RPC timeout. Slice 1
includes a benchmark of exactly this on a dev machine; if it approaches the timeout the
fallback is chunked load via `weights_chunk_data` (already on the wire), not a timeout
increase. F32 weight storage is deliberate — the simplest correct compute; halving it to
F16 storage is a later optimization with a parity test to protect it.

## Alternatives considered

- **llama.cpp in-process, partial layer control**: no public API for layer-range
  execution (verified during recon; re-verify in source-driven phase). Rejected.
- **llama.cpp distributed RPC (`rpc-server`)**: graph-split tensor offload; replaces our
  wire contract entirely. Rejected — the project's contract and reviews are built around
  `cluster_rpc.proto`.
- **Shell out to `llama-server` per worker**: full-model only; no intermediate
  activations. Rejected.
- **Defer engine; finish `.apk` release asset first**: smaller, but doesn't move the
  plan's #1 blocker; partner chose the epic explicitly.

## What's left to the builder

- Crate/module layout, the GGUF-reading choice, error-message wording, exact golden
  values, tolerance numbers for parity, and how worker processes are spawned in tests
  (CLI vs library spawn).
- Any additive proto change must be justified; `checkProtoInSync` keeps Android in sync.
- Re-verify llama.cpp API claims against official docs before coding slice 2–3
  (source-driven).
