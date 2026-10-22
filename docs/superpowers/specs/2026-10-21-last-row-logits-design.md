# Last-row-only logits wire contract - design

**Date:** 2026-10-21
**Status:** PLANNED
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
- `LoadedPipeline::forward` sets `last_logits_only = true` on the **final**
  stage only. Intermediate stages carry hidden activations and never asked for
  logits.
- `PipelineDriver` gains `supports_last_row_logits()` (async, loads the pipeline
  if needed; default `false` so test doubles stay honest). `LoadedPipelineDriver`
  reports the final stage's advertised capability.
- `DistributedBackend::generate` keeps the current honest refusal **only when
  the head stage does not advertise the capability**; otherwise it skips the
  size check and prefills the whole prompt. The refusal message stays the same
  and names the reason, so an old worker still produces an actionable error.
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
| `last_row_only_matches_the_tail_of_whole_logits` | `worker/tests/forward.rs` | same fixture, same session: `[1, VOCAB]` bytes equal the last row of the `[n, VOCAB]` answer, and no other row is computed |
| `last_row_only_still_advances_the_kv_cache` | `worker/tests/forward.rs` | a following 1-token decode is identical whether the prefill asked for all rows or one |
| `a_non_head_stage_ignores_last_row_only` | `worker/tests/forward.rs` | hidden activation shape is unchanged |
| `handshake_advertises_last_row_logits` | `worker/tests/forward.rs` | capability bit set by the Rust worker |
| `the_pipeline_asks_only_the_final_stage_for_the_last_row` | `core/tests/...` mock gRPC | flag is `true` on the head and `false` on every earlier stage |
| `a_capable_head_accepts_a_prompt_over_the_old_wire_limit` | `core/tests/distributed.rs` | no refusal when capability is advertised; prefill still sends every id |
| `an_incapable_head_still_refuses_before_dialing` | `core/tests/distributed.rs` | old behaviour and message preserved |
| Kotlin head/flag test | `mobile-worker/.../EngineTest` or contract test | `[1, n_vocab]`, equal to the last row, capability bit |

Mutation checks: remove the flag from the final-stage request and the
"only final stage" test must fail; make the worker ignore the flag and the
equality test must fail; make the capability always report `true` and the
incapable-head refusal test must fail.

## Slices

**Slice 1/2 - proto + both workers (behaviour-neutral for the chat path).**
Proto fields, Rust worker honours the flag and advertises the capability, Kotlin
worker does the same, vendored proto resynced, worker + JVM tests. The master
does not send the flag yet, so the chat path is unchanged apart from a new
handshake bit.

**Slice 2/2 - master uses the contract.** `PipelineStage` capability, final-stage
flag, conditional prompt guard, capability-aware tests, and docs: `tasks.md`
known-limit paragraph rewritten, chat-path spec D6 annotated as superseded for
this one point, this spec marked IMPLEMENTED.

## Acceptance

- A prompt longer than ~430 tokens prefills and streams on a capable worker
  (device-free: tiny fixture + a mock assertion that no size refusal fires).
- An old/incapable head still refuses with the current message and dials nothing.
- Whole-logits and last-row responses decode identically at the master.
- No fabricated capability: the bit is derived from the implementation, never
  from configuration.
