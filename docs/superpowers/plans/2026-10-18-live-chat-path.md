# Live Chat Request Path - Plan (2026-10-18)

Design: `docs/superpowers/specs/2026-10-18-live-chat-path-design.md` (accepted by
default; veto window until PR-1). Slices below each end in a full PR (branch,
guide comment, 6-lane CI, squash merge with resolution comment). Conventions as in
the tokenizer epic: Conventional Commits, asset-gated proofs named explicitly,
`proto/cluster_rpc.proto` stays byte-identical (epic invariant).

---

## PR-1: DistributedBackend - single-stage happy path

- [ ] `core/src/api/backend.rs`: keep `GenerationBackend`; add
      `DistributedBackend` (construction: model_id, tokenizer path, transport
      source, planner input). Constructor loads `Tokenizer::from_gguf` once.
- [ ] Token ledger: `session_id -> in_context_tokens` map owned by the backend
      (Mutex-protected). Turn-1 prefill length == ChatML prompt length; ledger has
      no persistence by design (D4: resets together with a fresh pipeline).
- [ ] Sampler: greedy argmax over the last logits row + `temperature`/top-p
      (seeded, llama-order-of-operations notes recorded). Unit tests: greedy exact
      on a hand-made row incl. tie -> lower id; temperature flips a known ordering;
      seeded determinism.
- [ ] Generate loop: prefill once, then one-id `forward`s; stop on the
      `<|im_end|>` id or `max_tokens`; decode per step; re-emission through the
      existing `GenerationChunk` stream with real ids counted; refusal -> one
      full re-prefill retry, second refusal -> honest error (D4).
- [ ] Selection plumbing (D1): `AppState` learns a cluster-aware factory;
      request-level fall-through to `LlamaCppBackend` when no enrolled verified
      worker or a load/refusal failure occurs before the first byte. Unit test:
      zero enrolled workers -> llama backend chosen (with mock discovery objects).
- [ ] Asset-gated API test `core/tests/chat_serving.rs`: spawn ONE local
      `ai-upcycler-worker` subprocess (full model, single stage), drive the REAL
      router with a fixed prompt + `temperature: 0`, assert the streamed
      continuation is token-equal to llama-server greedy on the same prompt
      (llama-server spawned by the test, both reaped). Metrics events still
      published per chunk. Skips without `AI_UPCCLER_MODEL` +
      `AI_UPCCLER_LLAMA_SERVER` (worker binary path = `runtime/engine/`
      sibling of llama-server, discovered the same way).
- [ ] Hermetic CI additions: mock-worker loop test for the generate loop (1
      worker, fake logits row chosen to force a stop id), ledger math, selection
      fall-through. `cargo test -p ai-upcycler-core` green; workspace lanes green.

## PR-2: Multi-stage splits + metrics truthing

- [ ] Two-stage serving test (asset-gated): two local workers, planner splits the
      real 24-layer model, same greedy parity assert through the real router.
- [ ] `tokens_emitted`/telemetry: publish_metrics counts REAL ids; add a hermetic
      assert that a streamed response's `completion_tokens` equals the emitted id
      count (fixes today's "fragments==tokens" fudge, `chat.rs:122-131`).
- [ ] Buffered (non-stream) path: full assembly + `usage.prompt_tokens == prefill
      len`; asset-gated parity assert on the buffered shape too (same prompt).
- [ ] Eviction/evict-then-re-prefill hermetic test: mock worker destroys the
      session mid-decode; assert one transparent re-prefill; second refusal ->
      documented failure surfaced as a 5xx-style SSE error event.

## PR-3: Docs sweep + task tick + release notes

- [ ] `docs/tasks.md`: the "wire the live chat request path" follow-up ticked with
      evidence pointers (test names, PR numbers); Android engine port moves up as
      the remaining unchecked epic.
- [ ] `docs/architecture.md`: replace/extend the serving description - master
      tokenizes in-process, serves through `LoadedPipeline`, llama-server is the
      documented fallback; keep the §3 lifecycle diagram (worker contract
      unchanged).
- [ ] `README.md` quickstart note: cluster serving condition + fallback behavior.
- [ ] Spec-plan cross-check: every checkbox above ticked with evidence; any
      deviating decision recorded in the spec's decision log with the reason.

---

## Non-goals (this epic)

- kv-split / per-token telemetry pane upgrades beyond metric truthing (frontend
  already consumes the same SSE shape).
- Stochastic sampling parity with llama.cpp RNG (superseeded by seeded own RNG +
  greedy gate).
- Phone-device end-to-end serving playbook (possible PR-3 addendum if trivial;
  the mTLS transport code exists and is handset-independent).
