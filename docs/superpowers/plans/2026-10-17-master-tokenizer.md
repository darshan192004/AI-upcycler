# Master-Side Tokenizer Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Remove the last service dependency of the master's inference path: tokenize
prompts to ids and detokenize output ids to text **in-process**, from the GGUF the
workers already load — proven id-exact against `llama-server`'s `/tokenize` and
`/detokenize` on the production file.

**Architecture:** The GGUF reader moves from `worker` to a new shared workspace crate
`model-io/` (partner decision Q1). `worker` re-exports it and changes nothing else.
`model-io::tokenizer` adds: KV-section reader for the `tokenizer.ggml.*` keys, the
GPT-2 byte encoder, the Qwen pre-tokenizer via `fancy-regex` (partner decision Q2),
rank-ordered BPE merges, piece-join decode with strict UTF-8, and a ChatML helper
(Qwen-specific, documented as such). The master pairs `encode` with `LoadedPipeline`'s
existing forward calls; the worker RPC proto stays byte-identical.

**Tech Stack:** Rust 2021, memmap2 0.9 (moves with the reader), `fancy-regex` 0.16,
thiserror 2 (same major both existing crates already use). The parity tests spawn the
existing `runtime/engine/llama-server.exe` on an ephemeral port exactly like
`worker/tests/parity.rs` does — asset-gated, never in CI.

**Spec:** `docs/superpowers/specs/2026-10-17-master-tokenizer-design.md` (ACCEPTED
2026-10-17: Q1 model-io extraction, Q2 fancy-regex, Q3 live-path next — all partner
calls). The plan argues from the spec; executors read both. Committed together with
Task 1.

## Global Constraints

- **Proto is canonical; this epic changes it ZERO times.** `proto/cluster_rpc.proto`
  stays byte-identical (`android-worker.yml`'s `checkProtoInSync` must never fire).
- **The engine keeps working from the same reader.** After the extraction,
  `worker/src/gguf.rs` is a one-line `pub use` shim; any formatting quibble in the
  moved file is fixed in a follow-up commit, never hand-mixed into the move itself
  (separating "moved" from "changed" protects future bisects).
- **`worker -> model-io -> memmap2` only.** `model-io` gains no dependency on `core`,
  `worker`, tokio, or tonic; it is a pure parse crate like the current `gguf.rs` is.
- **Corpus quality is the gate.** The parity corpus must include (the test embeds it
  as raw string literals; the following list names the classes):
  plain ASCII prose; the parity prompt ("The capital of France is"); code-like
  punctuation (`{"a": [1,2], "b": null}`); CJK ideographs; Greek/Cyrillic; accented
  Latin; an emoji with a ZWJ sequence (hair-bearer + skin-tone modifier + female
  sign); combining marks (e-acute kept as base PLUS U+0301 rather than
  precomposed); LF and multi-LF runs; trailing-space-only input; a tab directly
  before a word; a 1000-char word; digit runs (`20261017`, `3.14159`); contractions
  (`don't`, `I'll`, `we've`). 100+ strings, each compared id-exact.
- **Corpus bytes stay assertable:** the test-embedded corpus strings are compared
  against the server byte-for-byte, and the test rejects the two classic silent
  corruptions: precomposed-vs-decomposed e-acute mixups and CRLF-vs-LF mangling
  introduced by editors (each corpus entry is hex-fingerprinted once at test start).
- **Decode honesty:** any id outside `[0, n_vocab)` or a non-UTF-8 byte result is an
  `Err` (typed, message includes the offending id/byte), never
  `String::from_utf8_lossy`.
- **Commit identity:** `git -c user.name='AI Upcycler Bot' -c user.email='dev@aiupcycler.org' commit ...`.
- **Local verify before every push:** `cargo fmt --all -- --check &&
  cargo clippy --workspace --all-targets -- -D warnings && cargo test -p
  ai-upcycler-core -p ai-upcycler-worker -p ai-upcycler-model-io` (also run the two
  asset-gated parity tests locally before merge).
- **PR flow per slice:** feature branch -> `gh pr create` -> `PR Reviewer Guide`
  comment -> CI 5/5 -> `gh pr merge <N> --squash --delete-branch`. Mutation-check
  every property claim: break it, see the test fail, restore, re-confirm green.
- **Tests never use the network except the spawned local llama-server** in
  asset-gated parity tests, matching the parity.rs pattern (env
  `AI_UPCCLER_MODEL`; the server binary path comes from `runtime/engine/`;
  ergonomics follow the existing sibling, not a new env layer).

## PR slicing

| PR | Tasks | Branch |
|---|---|---|
| PR1 (design + shared reader) | 1 + spec/plan docs | `feat/model-io-extraction` |
| PR2 (tokenizer) | 2, 3, 4, 5 (hermetic mechanics) | `feat/tokenizer-bpe` |
| PR3 (parity + ChatML) | 6, 7, 8 | `feat/tokenizer-parity-chatml` |

## Review Focus

1. **KV type-matrix honesty:** `tokenizer.ggml.tokens` is a STRING array (type 9);
   treating it as anything else must be a loud `Err`, not a truncation. -> Task 2.
2. **Merge application order matters** (lowest rank first, left-to-right re-check):
   a wrong order passes most words and corrupts rare-token ids. -> Task 4, fixed by
   the parity corpus (asset-gated); any single id mismatch is a test failure, not a
   tolerance.
3. **A mid-tail id that is a partial UTF-8 byte** - decode must `Err` mid-stream, not
   lose a byte and claim success. -> Task 5, mutation-checked (truncate the last
   byte and confirm the loud error).
4. **The extraction must not change worker behavior**: split equivalence, parity and
   golden-vector tests all still pass unchanged (they get exercised every PR push by
   the workspace gate). -> Task 1.

Known disclosed gap going in: tracking the ChatML *generation loop* (stop tokens,
`<|im_end|>` handling during decode of streamed ids) belongs to the live-path epic —
the helper here only renders messages -> prompt text.

---

## File Structure

```
model-io/
  Cargo.toml        name = "ai-upcycler-model-io"
  src/lib.rs        module wiring + shared fixture helpers re-homed here
  src/gguf.rs       moved from worker (diff should read as a pure move)
  src/tokenizer.rs  new in PR2 (data loader + encode/decode)
  tests/gguf.rs     moved from worker tests (fixture-based behavioral parity)
worker/...          gguf.rs becomes a re-export shim
docs/...            this plan + the spec
```

## Tasks

### Task 1: Extract `model-io` crate (worker behavior untouched) — PR1
- [ ] New workspace member `model-io`; move `gguf.rs` verbatim (module-path tweaks
      only); `worker` re-exports (under 10 lines).
- [ ] CI gains a `model-io` lane in `ci.yml` (the lanes are strictly per-crate:
      `-p ai-upcycler-core`, `-p ai-upcycler-worker`, so a third member is invisible
      to both existing lanes). `cargo fmt --all -- --check` is already workspace-wide
      in both lanes; the new lane clippy+test `-p ai-upcycler-model-io`.
- [ ] Workspace verify (fmt / clippy `-D warnings` / core+worker+model-io tests)
      green; the whole engine test family (split equivalence, parity, golden
      vectors) unchanged. Evidence recorded here.
- [ ] `docs/tasks.md`: the tokenizer follow-up line now names `model-io` as the
      owner of the shared GGUF reader.

### Task 2: Tokenizer data reader — PR2
- [ ] `tokenizer.ggml.model`, `.pre`, `.tokens`, `.scores`, `.token_type`,
      `.merges`, `.bos_token_id`, `.eos_token_id` read with typed loud errors;
      an unknown `pre` value refuses with the value in the message.
- [ ] Unit tests on a synthetic tokenizer fixture (small vocab plus 3 merges, written
      by an extended fixture writer) covering refusals: missing merges, mismatched
      token-type length, non-STRING tokens array (same refusal style as the existing
      tensor/type gates).

### Task 3: Byte encoder — PR2
- [ ] 256-byte round-trip table (encode then decode is identity, both directions).
- [ ] Known reference values unit-gated, exactly the classic `byte_to_unicode`
      table: byte 0x00 -> U+0100 (A with macron), byte 0x0A -> U+010A (C with dot),
      byte 0x20 -> U+0120 (the well-known dotted-G piece encoding the space).

### Task 4: `encode` (pre-tokenizer + BPE) — PR2
- [ ] Hermetic hand examples on the synthetic vocab (single-token pieces,
      merge-ordered composition, contractions via the case-insensitive branch).
- [ ] Pre-tokenizer span spot-checks asserted on hand-computed splits: digit runs
      never exceed 3, punctuation-with-attached-space behaves like the server's,
      LF-only runs split separately from trailing spaces.

### Task 5: `decode` — PR2
- [ ] Strict UTF-8 refusal (typed error); mutation check with a truncated byte.
- [ ] Round-trip: `decode(encode(text)) == text` over the hermetic corpus.

### Task 6: Asset-gated /tokenize + /detokenize parity — PR3
- [ ] `model-io/tests/tokenizer_parity.rs`: spawn llama-server (ephemeral port,
      parity.rs pattern), run the 100+ corpus, `assert_eq!` on every id vector and
      detokenization, exact.
- [ ] Numbers recorded in the test header (corpus length, 100% match) and here.

### Task 7: ChatML helper — PR3
- [ ] `apply_chatml(messages)` hard-codes the Qwen template; unit tests cover
      system-first ordering and the literal `<|im_end|>` placement.
- [ ] Qwen-only enforcement: the helper refuses on a file whose
      `general.architecture` is not `qwen2` (honest, same refusal style as the
      rest of the reader).

### Task 8: Docs — PR3
- [ ] `tasks.md`: the "master-side tokenizer" follow-up checked with evidence.
- [ ] `architecture.md`: the API note becomes "master tokenizes in-process via
      model-io (tokenizer epic, PR#)".
- [ ] `plan.md` / `tasks.md` phrasing stays accurate after the reader move (the
      GGUF reader is `model-io`'s, shared with the engine).
