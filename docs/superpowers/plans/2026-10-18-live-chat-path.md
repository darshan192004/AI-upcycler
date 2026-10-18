# Live Chat Request Path - Plan (2026-10-18)

Design: `docs/superpowers/specs/2026-10-18-live-chat-path-design.md` (accepted by
default; veto window until PR-1). Slices below each end in a full PR (branch,
guide comment, 6-lane CI, squash merge with resolution comment). Conventions as in
the tokenizer epic: Conventional Commits, asset-gated proofs named explicitly,
`proto/cluster_rpc.proto` stays byte-identical (epic invariant).

---

## PR-1: DistributedBackend - single-stage happy path

Status (2026-10-18, mid-implementation): code written on branch
`feat/cluster-serving-single-stage`; the asset-gated parity run below is the
acceptance gate. D4 was revised during implementation (see the spec blockquote)
- the per-session token ledger became a fresh-session-per-request rule, and the
"ledger math" hermetic tests became recovery/stop/emission tests.

- [x] `core/src/api/backend.rs`: keep `GenerationBackend`; add
      `DistributedBackend` (construction: model_id, tokenizer path, transport
      source, planner input). Constructor loads `Tokenizer::from_gguf` once.
- [x] Session handling (D4 revised, see header): fresh worker KV session per
      request+attempt; no persistent master ledger.
- [x] Sampler: greedy argmax over the last logits row + `temperature` and
      top-p (seeded LCG, llama order of operations recorded). Unit tests:
      greedy exact incl. tie-to-lower-id; temperature/top-p seeded
      determinism; top-p narrows to the peak.
- [x] Generate loop (`core/src/api/distributed.rs`): prefill once in a single
      `forward`, then one-id `forward`s; stop on the `<|im_end|>` marker in
      decoded text or `max_tokens`; decoded-diff streaming with partial-
      marker withholding; refusal pre-first-emission -> `driver.reset()` +
      one full re-prefill retry, second refusal -> honest error (D4).
      No-emission recovery only (D4 invariant: a re-prefill after emitted
      text would duplicate it). Hermetic tests against a canned
      `PipelineDriver`: greedy reply, marker stop, double refusal, refusal +
      reset recovery, marker-prefix buffering.
- [x] Selection plumbing (D1): `AdaptiveBackend` in `core/src/api/mod.rs`
      `bootstrap` - enrolled registered workers + valid plan + readable
      tokenizer -> cluster; otherwise the llama `/stub` fallback inside
      `AppState`. `session_id` added to `GenerationRequest` (llama and stub
      ignore it). Zero-node selection fallback hermetic (mock `registry` + a
      stub fallback) lives as an integration follow-up in PR-2's mock
      cluster; the in-crate unit tests cover the cluster side directly.
- [x] Asset-gated API test `core/tests/cluster_serving.rs` (PASSED 2026-10-18,
      release profile): llama-server raw `/completion` reference (this build's
      `/v1/chat/completions` prepends the rendered prompt to `content`) picks
      ids 59604 "Paris" then 151645 `<|im_end|>`; the master sampled the same
      ids id-for-id and back-to-back runs were byte-identical (determinism
      confirmed). Bumped en route, both with measured basis in the commit:
      `REQUEST_TIMEOUT` 30s -> 120s and RPC wire limits 4MB -> 256MiB (whole
      prefill logits; over-limit prompts refused master-side before dialing).
- [x] Hermetic CI additions: sampler, logits-row slicing, loop/recovery
      tests in `core/src/api/distributed.rs` (`cargo test -p ai-upcycler-core`
      green: 96 lib + 17 + 14 + 5).

## PR-2: Multi-stage splits + metrics truthing

- [x] Two-stage serving test (asset-gated, PASSED 2026-10-18, release
      profile): two local workers split the real 24-block model 12/12 (RAM
      declarations tuned; a hermetic pin test locks the planner arithmetic so
      the split cannot degenerate), streamed greedy continuation id-for-id
      equal to llama's `/completion` reference. Enabling fix: `try_cluster`
      orders nodes deterministically (ascending free RAM) - the registry's
      map order previously made multi-stage plans hash-luck.
- [x] `tokens_emitted`/telemetry: completion counting was already real (one
      emitted id per chunk); PR-2 closes the prompt side: chunks carry the
      TRUE prefill length (`GenerationChunk.prompt_tokens`), buffered
      `usage.prompt_tokens` uses it, the whitespace estimate stays the
      llama/stub fallback. Hermetic: chunks_carry_the_true_prefill_token_count.
- [x] Buffered (non-stream) path: full assembly +(asset-gated assert, passed
      as part of the single-stage test run) content == reference and
      `usage.prompt_tokens` present.
- [x] Eviction/evict-then-re-prefill hermetic test: a mid-decode refusal
      before the first emitted token triggers exactly one reset + re-prefill
      (`evicted_session_recovers_through_one_re_prefill`); post-emission
      refusal surfaces as an SSE error event with the honest message
      (`stream_failures_surface_as_sse_error_events`), second refusal keeps
      failing the request (`second_refusal_fails_the_request_honestly`).

## PR-3: Docs sweep + task tick + release notes

- [x] `docs/tasks.md`: the "wire the live chat request path" follow-up ticked with
      evidence pointers (test names, PR numbers); Android engine port moves up as
      the remaining unchecked epic.
- [x] `docs/architecture.md`: replace/extend the serving description - master
      tokenizes in-process, serves through `LoadedPipeline`, llama-server is the
      documented fallback; keep the §3 lifecycle diagram (worker contract
      unchanged). New §3.0 "Serving a live chat request" sits between the
      tokenizer note and §3.1; the worker-side diagrams are untouched.
- [x] `README.md` quickstart note: cluster serving condition + fallback behavior
      (Cluster-first serving callout in "Enable Real Inference").
- [x] Spec-plan cross-check: PR-1 and PR-2 checkboxes ticked with evidence in
      the plan; every deviating decision recorded in the spec's decision log -
      D4's fresh-session revision (blockquote + superseded-draft details) and
      the D2 gate outcome notes (the `/completion` reference, measured
      determinism, the timeout/wire-limit raises with the whole-logits bound).
      No additional deviations surfaced during PR-3 (docs-only).

---

## Non-goals (this epic)

- kv-split / per-token telemetry pane upgrades beyond metric truthing (frontend
  already consumes the same SSE shape).
- Stochastic sampling parity with llama.cpp RNG (superseeded by seeded own RNG +
  greedy gate).
- Phone-device end-to-end serving playbook (possible PR-3 addendum if trivial;
  the mTLS transport code exists and is handset-independent).
