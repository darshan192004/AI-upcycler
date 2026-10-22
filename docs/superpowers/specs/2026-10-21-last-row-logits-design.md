# Last-row-only logits wire contract - design

**Date:** 2026-10-21
**Status:** IMPLEMENTED (2026-10-22) - slice 1/2 #83 (`b92f8c0`), slice 2/2 #84
**Supersedes (narrowly):** `2026-10-18-live-chat-path-design.md` D6, only for the
prefill-logits response contract. Everything else in D6 stands.
**Follows:** `2026-10-21-hardware-benchmark-probes-design.md` (same additive-proto
discipline and honest-failure posture).

## Context

The distributed chat path prefills a whole prompt in one `ExecuteForward`, and the
head stage returns logits for **every** prompt token: `[n_prompt, n_vocab]`. The
caller only ever uses the **last** row - that is the next-token distribution.

On the shipped model (`qwen2.5-0.5b`-class, `n_vocab` = 151936) one row is
~608 KiB, so the 256 MiB activation cap (`MAX_ACTIVATION_WIRE_BYTES`) allows
~430 prompt tokens. Longer prompts are refused before dialing with a clear
message; the chat path is honest but short-context. The head stage also spends a
`n_prompt x hidden x n_vocab` matmul producing rows nobody reads.

`docs/tasks.md` calls this out as a known shipped limit and names the fix as "a
proto change, deliberately deferred". This spec is that change.

## Decisions

### L1 - Two additive proto fields

`proto/cluster_rpc.proto` gains:

```proto
message ExecuteForwardRequest {
  // ... existing fields 1..5 unchanged
  // Return logits for the last input row only. The head stage answers with
  // shape [1, n_vocab]; a stage that is not the head ignores it.
  bool last_logits_only = 6;
}

message HandshakeResponse {
  // ... existing fields 1..6 unchanged
  // This worker honours ExecuteForwardRequest.last_logits_only.
  bool supports_last_row_logits = 7;
}
```

Both are additive and unknown-field-tolerant in proto3, so a new master can talk
to an old worker and an old master can talk to a new worker. The Android
vendored copy is resynced under the existing `checkProtoInSync` guard - that
guard is the contract that the two generated worlds do not drift.

### L2 - Worker semantics

Only the **head stage** (the range that ends the model and owns
`output_norm`/head weights) produces logits. With `last_logits_only = true` it:

- still runs every layer for every input token, so the KV cache advances exactly
  as before (prefill semantics must not change);
- applies the output norm only to the last row;
- runs the head matmul only for the last row;
- answers shape `[1, n_vocab]` (not `[n_vocab]`), so one shape decoder handles
  both contracts.

A non-head stage ignores the flag. An old worker ignores the unknown field and
returns `[n, n_vocab]`, which the master already decodes by taking the last row -
so correctness never depends on the flag being honoured, only the wire bound
does.

`n_tokens` can never be `0` here: the worker's structural shape gate requires
`shape[1] >= 1` for a stage-0 ids tensor and `shape[0] >= 1` for a hidden
activation, and it runs before the row arithmetic. The head also uses
`saturating_sub`, so a future relaxation of the shape gate still cannot
underflow (both pinned by `an_empty_prefill_is_refused_before_the_head_stage`).

### L3 - Capability, not assumption

`HandshakeResponse.supports_last_row_logits = true` is advertised by both worker
implementations once L2 lands. The master records it per stage at load time
(`PipelineStage`), because the position of the head depends on the plan, not on
the master's own build.

Why a capability instead of "always ask and see": if the master dropped the
prompt-size pre-check unconditionally, an old worker would happily accept a
5000-token prefill and try to build a ~3 GB logits buffer, then fail while
encoding - a remote OOM, not an honest refusal. The capability keeps the old
guard exactly where it is needed and removes it only where the worker has
proven it can honour the flag.

### L4 - Master plumbing

- `TensorPayload` does **not** grow a feature flag: `execute_forward` takes the
  flag as an explicit argument so a caller cannot silently inherit it.
- `LoadedPipeline` has two entry points: `forward` (whole logits, used by the
  one-shot debug path) and `forward_with_last_row_only` (the generation path).
  The latter sets `last_logits_only = true` on the **final** stage only;
  intermediate stages carry hidden activations and are never asked for logits.
- `PipelineDriver` gains `supports_last_row_logits()` (default `false` so test
  doubles stay honest). `LoadedPipelineDriver` reports the final stage's
  advertised capability and forwards through the last-row entry point; the
  driver returns `false` when no pipeline is loaded, which is why `generate`
  asks only after `ensure_loaded`.
- `DistributedBackend::generate` keeps the honest refusal **only when the head
  stage does not advertise the capability**; otherwise it skips the size check
  and prefills the whole prompt. The refusal names the missing capability.
  Capability is a runtime fact, so the cached, idempotent load now runs before
  the check: a doomed long prompt against an old worker costs one load, never a
  prefill (it used to refuse before dialing at all).
- `decode_logits` keeps slicing the last row and asserting the shape, so
  `[1, n_vocab]` and `[n, n_vocab]` both decode. No caller reads `rows`.

### L5 - Kotlin parity

`Engine.execute` gains `lastLogitsOnly: Boolean` (default `false` at the RPC
boundary is not enough - the flag is passed explicitly from the request), and
the Kotlin head path applies the same "last row only" rule. The capability bit
is advertised by `WorkerRpcService.handshake`. JVM tests use the existing tiny
GGUF fixture so no phone is required.

### L6 - What is deliberately not in scope

- KV-cache growth, chunked prefill, or sliding-window context. This change
  removes the *response* bound; the head stage still computes every layer for
  every prompt token, so prompt length is now limited by worker RAM and attention
  cost, not by the wire.
- FP16/BF16 logits on the wire. FP32 stays the only logits dtype (parity gates
  depend on it).
- Sampling changes, streaming changes, or `usage` accounting.
- Any change to the benchmark RPC or its contract.

## Tests

| Test | Where | Proves |
|---|---|---|
| `last_row_only_matches_the_tail_of_whole_logits` | `worker/tests/forward.rs` | same fixture, separate sessions: `[1, VOCAB]` bytes equal the last row of the `[n, VOCAB]` answer |
| `last_row_only_still_advances_the_kv_cache` | `worker/tests/forward.rs` | a following 1-token decode is identical whether the prefill asked for all rows or one |
| `a_non_head_stage_ignores_last_row_only` | `worker/tests/forward.rs` | hidden activation shape is unchanged |
| `an_empty_prefill_is_refused_before_the_head_stage` | `worker/tests/forward.rs` (added in #83) | the structural shape gate (`shape[1] >= 1` / `shape[0] >= 1`) runs before the row arithmetic, and the head uses `saturating_sub`, so there is no underflow path |
| `handshake_advertises_last_row_logits` | `worker/tests/forward.rs` | capability bit set by the Rust worker |
| `asks_only_the_final_stage_for_the_last_logits_row` | `core/tests/distributed.rs` (mock gRPC) | flag is `true` on the head and absent on every earlier stage |
| `a_whole_logits_forward_still_asks_every_stage_for_every_row` | `core/tests/distributed.rs` | `forward` does not inherit the generation path's hint |
| `an_old_head_is_not_reported_as_capable` | `core/tests/distributed.rs` | only the final stage's handshake decides the capability |
| `a_capable_head_prefills_a_prompt_over_the_old_wire_limit` | `core/src/api/distributed.rs` | a prompt over the whole-logits bound prefills and emits, no refusal |
| `an_incapable_head_still_refuses_a_prompt_over_the_wire_limit` | `core/src/api/distributed.rs` | old head: actionable refusal, zero forwards |
| Kotlin head/flag tests | `WorkerRpcEngineTest`, `WorkerMtlsServerTest` | `[1, n_vocab]`, equal to the last row, capability bit over mTLS |

Mutation checks: force the worker to ignore the flag and the equality test
fails (verified for both the Rust and Kotlin engines); drop the flag from the
final-stage request and the "only final stage" assertion fails; make the
capability always report `true` and the incapable-head refusal test fails.

## Slices

**Slice 1/2 - proto + both workers (behaviour-neutral for the chat path).**
MERGED as #83 (`b92f8c0`): proto fields, Rust worker honours the flag and
advertises the capability, Kotlin worker does the same, vendored proto resynced,
worker + JVM tests. The master did not send the flag yet, so the chat path was
unchanged apart from a new handshake bit.

**Slice 2/2 - master uses the contract.** #84: `PipelineStage` capability,
final-stage flag, conditional prompt guard, capability-aware tests, and docs:
`tasks.md` bound rewritten, chat-path spec D6 annotated as superseded for this
one point, this spec marked IMPLEMENTED.

## Acceptance

- A prompt longer than ~430 tokens prefills and streams on a capable worker
  (device-free: tiny fixture + a mock assertion that no size refusal fires).
- An old/incapable head still refuses with the current message and dials nothing.
  *As built*: capability is a runtime fact, so the refusal happens after the
  cached, idempotent load and before any prefill; the message names the missing
  capability. Dialling nothing is no longer achievable without assuming the
  answer, and the cost is one load, never a forward.
- Whole-logits and last-row responses decode identically at the master.
- No fabricated capability: the bit is derived from the implementation, never
  from configuration.
