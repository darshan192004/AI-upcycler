# Master-Side Tokenizer — Design (2026-10-17)

# Master-Side Tokenizer — Design (2026-10-17)

Status: **ACCEPTED** (builder check settled 2026-10-17 — all three answered by the
partner): **Q1 = option A** (`model-io` shared crate extraction), **Q2 = `fancy-regex`**,
**Q3 = the live request path is the immediate next epic** after this one. No further
gates; implementation follows the plan doc task-by-task.

## Intent

Cut the last invisible dependency on the host `llama-server`: the master still gets
text → token ids (and ids → text) from an external process. The engine epic (#41–#54,
#55) made layer loads, forwards, KV-cached sessions and split equivalence real over
worker RPCs — but every prompt entering `ExecuteForward` arrives pre-tokenized
(`ids_payload` carries F32 values holding ids), and every output argmax id is still
interpreted via outside tooling. The follow-up tracked in `docs/tasks.md` says exactly
this: "**workers currently consume pre-tokenized ids**".

This design gives the master a GPU-free, server-free tokenizer for the model family
it already ships, reading all tokenizer state from the same GGUF file the workers
load layers from — no sidecar `tokenizer.json`, no vendored static vocab, no
`tokenizers` crate. With it, `encode("The capital of France is") → [785, 6722, 315,
9625, 374]` happens in-process, and the live chat request path (next epic) has no
remaining external inference dependency.

## Goals

- `model_io::tokenizer` (see Q1 below — settled to the new shared crate) exporting:
  - `Tokenizer::from_gguf(path: &Path) -> Result<Self, _>` — mmaps the model file and
    reads **only the header + KV list** (the tensor section is skipped, not decoded;
    the 1.1 GB file costs one `Mmap` + ~9 MB of strings).
  - `encode(&self, text: &str) -> Vec<u32>` and
    `decode(&self, ids: &[u32]) -> Result<String, _>`.
- **Correctness proven the same way every other engine claim was**: asset-gated
  parity against `llama-server` on the production file, using its `/tokenize` and
  `/detokenize` HTTP endpoints (they exist in the llama.cpp build at
  `runtime/engine/`), on a corpus that includes ASCII prose, code punctuation, CJK,
  emoji, combining characters, multi-byte whitespace, long repeated words, words that
  split across merges, and empty/whitespace-only inputs.
- **Hermetic CI for the mechanics**: the GGUF fixture writer gains a tiny tokenizer
  section (a handful of synthetic pieces + merges), so byte-encoder round-trip, BPE
  merge application, pre-tokenizer classes and decode safety run device- and asset-free.
- Wire/API non-goals preserved: the worker RPC contract (`proto/cluster_rpc.proto`)
  does **not** change — tokenization happens master-side, one place, once per request.

## The algorithm (fixed by the shipped model)

`qwen2.5-0.5b-instruct-q4_k_m.gguf` embeds the Qwen byte-level BPE
(`tokenizer.ggml.model` = `gpt2` family):

1. **Byte encoder** — the GPT-2 inversion map: every byte 0–255 is mapped to a
   printable codepoint so BPE can operate per str chunk (`bytes < 0x21` and
   `≥ 0x7F` shift up by 256 in order, e.g. byte `0x00` -> `chr(256)` U+0100
   (A+macron), byte `0x0A` -> `chr(266)` U+010A (C+dot), byte `0x20` -> `chr(288)`
   U+0120 (the well-known dotted-G piece encoding the space)), which is exactly the
   classic `byte_to_unicode` table. Gathered from llama.cpp's
   `unicode_to_byte`/`byte_to_unicode`; verified by a round-trip unit gate over all
   256 bytes.
2. **Pre-tokenizer** — the Qwen split regex (`(?i:'s|'t|'re|'ve|'m|'ll|'d)` +
   letter / short-number-run / punctuation / newline / trailing-space classes,
   including the `(?!\S)` lookahead). This is the part where an unfaithful
   implementation still passes most prompts and fails weirdly on mixed-script
   input, so its acceptance criterion is the server-parity corpus, not hand
   examples alone. Rust `fancy-regex` matches on `&str` and never splits a
   codepoint, which is the same invariant HF's Rust backend guarantees.
3. **BPE merges** — read from `tokenizer.ggml.merges` (space-joined pair per
   entry), applied lowest-rank-first inside each pre-token, matching llama.cpp's
   pairing order. `tokenizer.ggml.tokens` and `scores` land in the id table;
   `tokenizer.ggml.token_type` and the special ids (`eos`, `bos`) feed decode
   safety and the chat template.
4. **Decode** — piece strings joined through the byte encoder reversed; output is
   bytes → UTF-8. Decode must refuse producing invalid UTF-8 (mid-character
   truncation) with an honest error rather than lossy replacement — callers
   render assistant text today.

## Chat template (deliberate scope cut)

Full Jinja (`tokenizer.chat_template` is a template string in the file) is **out of
scope**. For Qwen2.5 the effective template is ChatML:

`<|im_start|>{role}\n{content}<|im_end|>\n`, system-first, and generation
ends on `<|im_end|>`. The tokenizer exposes `apply_chatml(messages) -> String`
hard-coding that shape for `architecture = qwen2`, **documented as a Qwen-specific
shortcut**, with the generic-jinja option recorded as a follow-up (it would drag a
Jinja engine into core for one template).

## Where the code lives (settled: option A)

The GGUF reader currently lives in `worker/src/gguf.rs` and moves per option A to a
new crate `model-io/` whose types are identical; `worker` takes
`ai-upcycler-model-io = { path = "../model-io" }` and re-exports so the engine and
its tests keep compiling unchanged. The tokenizer joins it as the crate's second
resident, next to the file format it reads (`memmap2` goes with it; core's
`mmap.rs` `MmapPager` is a different mechanism and stays put).

## Tasks (see the plan doc)

1. Fixture: the GGUF fixture writer stores a small tokenizer section (pieces/merges/
   scores/token_type/special ids).
2. Reader: KV-section reading + the four arrays → `Tokenizer` (hermetic unit tests).
3. Byte encoder round-trip gate (256 bytes, both directions).
4. `encode`: pre-tokenizer classes + BPE merges (hermetic hand examples) then the
   asset-gated `/tokenize` parity corpus.
5. `decode`: piece-join + UTF-8 validation, asset-gated `/detokenize` parity corpus.
6. ChatML helper (unit tests only; generation-loop behavior belongs to the live-path
   epic).
7. Docs: architecture §3.1 note, planner, `tasks.md` follow-up checked with evidence.
8. Numbers recorded in files: corpus length, exact match rate (expected **100% exact**
   — token ids are integers, no float tolerance notions apply; any mismatch is a bug).

## Non-goals

- Sub-second-startup or lazy hashing: the 9 MB string materialization is fine for a
  master that holds a 1.1 GB model pipeline.
- Streaming tokenization callbacks — `encode` on a full request string is enough.
- The Android side reusing this tokenizer: engines on phones receive ids over RPC
  (contract unchanged); Android parsing of text remains a later concern.
- `tokenizers` crate (HF) adoption: extra heavy dep + still needs a sidecar;
  GGUF-embedded is one less deployment artifact.

## Questions for the partner (builder check) — SETTLED

1. **Q1 (location)** → **A**: extract the GGUF reader to a shared `model-io` crate
   (partner decision, 2026-10-17).
2. **Q2 (dependency)** → **`fancy-regex`** (partner decision, 2026-10-17).
3. **Q3 (edge)** → the **live request path is the immediate next epic** after this
   tokenizer (partner decision, 2026-10-17).
