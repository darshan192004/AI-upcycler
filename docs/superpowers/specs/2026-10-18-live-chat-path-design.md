# Live Chat Request Path - Design (2026-10-18)

Status: **ACCEPTED-BY-DEFAULT** (2026-10-18). The three builder-check decisions below
(D1/D2/D3) were settled autonomously under the standing "continue the task list"
instruction; each states its default and the alternative considered. The veto window
stays open until the first code PR of the epic lands. All file:line references
verified 2026-10-18 against main (`4093b9c`).

## Intent

Serve `/v1/chat/completions` from the cluster itself: the master tokenizes in-process
(model-io tokenizer, merged 2026-10-17), pushes ids through `LoadedPipeline` to real
worker engines, samples the returned logits, and streams tokens - so the host
`llama-server` is no longer *the inference engine*, only a fallback of last resort.

The gap is exact. The chat API talks only to the `GenerationBackend` trait
(`core/src/api/chat.rs:119,188`) whose sole real implementation is the llama-server
proxy (`core/src/api/llama.rs`). `LoadedPipeline` (`core/src/rpc/engine.rs`) is done
and mock-verified but wired to nothing real: its only non-test caller is a Tauri
debug command that connects per call and loads nothing (`src-tauri/src/lib.rs:37-42`).
`docs/tasks.md` records this state verbatim. Meanwhile the pieces this epic needs all
exist and are verified:

- `Tokenizer::apply_chatml/encode/decode` in `model-io`, id-exact vs llama-server
  (`model-io/tests/tokenizer_parity.rs`, ~111 strings).
- `LoadedPipeline::load/load_secure/forward/unload` with per-stage rollback and plan
  validation (`core/src/rpc/engine.rs:135,148,213,248`); activations are raw bytes,
  never ids-as-text (`engine.rs:37-60`).
- Worker engines that embed stage-0 f32 **token ids**, keep per-session KV caches
  keyed by `payload.session_id`, enforce `sequence_position` continuity, and let the
  head worker project logits `[n_tokens, n_vocab]` (`worker/src/engine.rs:404-510`).
- The planner produces anchored, gap-free plans from real nodes
  (`Planner::compute_plan`, `core/src/scheduler/planner.rs:38`) and mTLS transports
  resolve from pairing enrollments (`core/src/security.rs:359-370`).

What does NOT exist yet, and is therefore the epic's work:

1. A `GenerationBackend` implementation driven by `LoadedPipeline`.
2. Master-side state for position tracking (the "token ledger", below).
3. Master-side sampling (today `tokens_emitted` are SSE delta fragments, not ids).
4. The wiring that picks the cluster backend when the cluster is actually available.

## Decisions

### D1 - Backend selection: cluster-first, llama-server fallback (default)

When a chat request arrives, the master uses the distributed path iff (a) the
requested model exists in the SQLite store (unchanged, `chat.rs:81-84`) and (b) the
pipeline for it is loadable from **currently enrolled, verified** workers. Otherwise
the existing `LlamaCppBackend` serves unchanged. No half-fallbacks mid-stream: the
decision is made before the first byte, and a failed pipeline load falls through to
llama-server for that request (the attempt is logged, not retried inline).

Rationale: the demo must not 500 on a laptop with zero paired phones; but any real
cluster serves without the host engine. The alternative - serving only from the
cluster - makes every local dev session worse for no correctness gain. Workers
source from pairing enrollment + `NodeRegistry` state (enrolled `node_id` with a
verified transport), NOT from auto-spawned local worker processes: production never
pretends distribution. Local processes are a test vehicle only (D3).

### D2 - Sampling: master-side, greedy-first, llama-parity gate (default)

The master samples from the head worker's final logits row. Implement, minimal and
testable: greedy argmax (structural: llama.cpp greedy == argmax, ties to lower id),
then temperature/top-p on top of it. Acceptance gate: for fixed prompts with
`temperature: 0` and `max_tokens` in the small range, the **distributed path and
llama-server produce identical continuations** (asset-gated, real model + local
worker processes). That gates the whole sampling + logits plumbing without needing
llama.cpp's RNG internals; stochastic sampling is covered by unit tests (it is
deterministic given a seeded RNG we control). `temperature`/`max_tokens` request
fields already exist (`chat.rs:42-46`).

### D3 - Verification: local subprocess workers for real-pipeline tests (default)

Asset-gated API tests spawn 1-2 REAL `ai-upcycler-worker` child processes on the
laptop (same pattern as `worker/tests/parity.rs`/`split_equivalence.rs`), point the
planner at them through the test-only `PlaintextTransports` source (loopback, no
pairing ceremony), drive `POST /v1/chat/completions` through the REAL axum router,
and compare against llama-server greedy output. Hermetic CI gets everything the
assets can't provide: ledger math, sampler determinism, re-prefill logic, backend
selection, response shaping - against in-process mock workers, which already exist
(`core/tests/distributed.rs`). mTLS end-to-end through the pairing flow stays a
manual/mixed follow-up (existing pairing tests cover the ceremony itself).

### D4 - Positions and the token ledger

The master keeps an in-memory ledger: `session_id -> tokens_already_in_context`.
Workers own KV growth; the master owns only bookkeeping:

- Turn 1: prompt = `apply_chatml(conversation)` (whole visible turns). Prefill in
  ONE `forward` whose ids span `sequence_index 0..n` (worker embeds all, head
  projects all rows). The requested continuation positions then continue `n, n+1, ...`
  one id per `forward`, worker-side KV extending each call
  (`worker/src/engine.rs:358-390` gates the contract).
- Ledger derived at request start from the chat session's active state; a master
  restart resets the ledger to 0 (and the pipeline is new then too, so worker KV and
  ledger restart together - never inconsistent).
- Refusal path: a `forward` returning `success = false` (worker KV evicted -
  `MAX_SESSIONS: 16` per worker - or session raced) means the master re-prefills the
  ENTIRE ChatML prompt from sequence 0 and continues. This must bound itself (one
  retry per request); the second refusal fails the request honestly.
- `session_id` (the chat session id) is passed through as `TensorPayload::session_id`
  verbatim; it is also the API session namespace, so KV separation is automatic.

### D5 - Stopping and shape of output

Generation stops on the `<|im_end|>` id (the ChatML turn terminator the tokenizer
already knows), at `max_tokens`, or on a decode error - the unfiltered candidates
are never surfaced. `finish_reason: "stop"` (unchanged shape,
`chat.rs:50-72`). Streaming chunks re-emitted as today's `chat.completion.chunk`
SSE with REAL `tokens_emitted` (ids now, not delta fragments); the buffered path
assembles `message.content` from decoded ids. `usage.prompt_tokens` = prefill length
(not the current whitespace word count, `chat.rs:305-307`).

### D6 - Invariants preserved across the epic

- `proto/cluster_rpc.proto`: 0 lines changed. No new RPC methods; the worker learns
  nothing about chat.
- `LoadedPipeline` and the worker engine stay text-free (D4 puts all text on the
  master, where the tokenizer lives).
- llm weights/tensors are a stage-local concern; the master handles only ids,
  logits rows, and text.
- The host llama-server binary/executable discovery code stays for the fallback
  path (no deletion), and the mock API on :8080 is untouched by all test infra.

## Request flow after the epic (distributed path)

1. `POST /v1/chat/completions` -> axum handler (unchanged entry,
   `core/src/api/chat.rs:74-111`).
2. Backend selection (D1): enrolled workers found -> build/plan/load pipeline for
   the requested model (`compute_plan` -> `LoadedPipeline::load_secure` via the
   pairing-backed transport source); else fall through to `LlamaCppBackend`.
3. Tokenize: `apply_chatml(messages)` -> ledger position check -> `encode`.
4. Prefill `forward` (all ids), take the LAST logits row, sample (D2), decode id
   -> first token.
5. Loop `max_tokens - 1`: single-id `forward` at `sequence_index = prefill_len + i`,
   sample/decode per step; stop on `<|im_end|>` / max_tokens (D5).
6. Emit SSE chunks per step (real `tokens_emitted`) or assemble the buffered JSON;
   persist the assistant turn (existing code path); `pipeline` stays cached in the
   backend for the next request; ledger advances by exactly the emitted ids.

PR slicing and per-task verification live in
`docs/superpowers/plans/2026-10-18-live-chat-path.md`.
