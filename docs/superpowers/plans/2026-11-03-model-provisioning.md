# Model Provisioning (Slice B) — Implementation Plan

Spec: `docs/superpowers/specs/2026-11-03-model-provisioning-design.md`
Branch: `feat/model-provisioning` (from `main` @ `72c4af3`)

Every task is independently committable and leaves the tree green. Tasks 1, 4, 5
and 6 are host-side and need no phone; 2 and 3 are JVM tests on the Android
module; 7 is a typecheck. The end-to-end transfer needs a phone and is listed as
unproven until one is attached.

## Status (2026-11-03)

| Task | State | Evidence |
|---|---|---|
| 1 — proto contract | todo | |
| 2 — worker receive path | todo | |
| 3 — worker RPC handlers | todo | |
| 4 — host provisioning core | todo | |
| 5 — host streaming client | todo | |
| 6 — HTTP surface | todo | |
| 7 — dashboard action | todo | |
| 8 — docs + PR | todo | |

## Task 1 — Proto contract

Files: `proto/cluster_rpc.proto`

Add to `service WorkerService`:

```proto
  rpc BeginModelPush (BeginModelPushRequest) returns (BeginModelPushResponse);
  rpc PushModelChunks (stream ModelChunk) returns (PushModelChunksResponse);
  rpc FinishModelPush (FinishModelPushRequest) returns (FinishModelPushResponse);
```

and the six messages exactly as listed in the spec §4, with field numbers
1..5 as written there.

- [ ] `cargo build -p ai-upcycler-core` regenerates `aiupcycler.rpc` with the
      three methods
- [ ] `gradlew :app:compileDebugKotlin` regenerates the Java stubs
- [ ] Commit `feat(proto): add model push RPCs`

## Task 2 — Worker receive path

Files:
- `mobile-worker/android/app/src/main/java/org/aiupcycler/worker/model/ModelStore.kt`
- `mobile-worker/android/app/src/test/java/org/aiupcycler/worker/model/ModelStoreTest.kt`

Add a receive API next to `import`:

```kotlin
/** Outcome of asking the library to accept a pushed model. */
sealed class ReceiveStart {
    data class Accepted(val resumeFrom: Long) : ReceiveStart()
    data class Refused(val reason: String) : ReceiveStart()
}

fun beginReceive(fileName: String, totalBytes: Long, sha256Hex: String): ReceiveStart
fun appendReceive(offset: Long, data: ByteArray): Long
fun finishReceive(sha256Hex: String): ModelFile
```

Rules (all from the spec):

- `beginReceive` rejects a non-`.gguf` name via the existing `safeName`, a
  non-positive `totalBytes`, and a digest that is not 64 hex chars.
- Partial state is `<target>.part` plus `<target>.part.meta` holding
  `sha256=<hex>\ntotal=<bytes>\n`. A `.part` whose sidecar digest or total
  differs is deleted, and the call starts at 0.
- Space check: `totalBytes - resumeFrom + RESERVE_BYTES > freeSpace()` →
  `Refused("not enough free space: need N bytes, have M")`.
- `appendReceive` requires `offset == part.length()`; otherwise
  `IllegalArgumentException("chunk offset X does not match Y written")`.
- `finishReceive` streams the `.part` through `MessageDigest.getInstance("SHA-256")`,
  compares hex case-insensitively, deletes both files and throws
  `IOException("sha256 mismatch: expected ..., got ...")` on a mismatch,
  otherwise moves to `uniqueTarget` and deletes the sidecar.
- `RESERVE_BYTES_RECEIVE = 256L * 1024 * 1024`; keep the existing
  `RESERVE_BYTES` for `import` untouched.

Tests in `ModelStoreTest.kt`:

- [ ] `a_fresh_receive_starts_at_zero`
- [ ] `an_interrupted_receive_resumes_at_the_partial_length`
- [ ] `a_partial_from_a_different_digest_is_discarded`
- [ ] `a_chunk_at_the_wrong_offset_is_refused`
- [ ] `a_hash_mismatch_deletes_the_partial`
- [ ] `a_torn_partial_is_not_listed_as_a_model`
- [ ] `a_receive_without_space_is_refused`
- [ ] `gradlew :app:testDebugUnitTest` green

## Task 3 — Worker RPC handlers

Files:
- `mobile-worker/.../rpc/WorkerRpcService.kt`
- `mobile-worker/.../WorkerRuntime.kt`
- `mobile-worker/android/app/src/test/java/org/aiupcycler/worker/rpc/WorkerRpcContractTest.kt`

Dependency inversion in the same shape as `EngineSource`:

```kotlin
interface ModelReceiver {
    fun beginReceive(fileName: String, totalBytes: Long, sha256Hex: String): ReceiveStart
    fun appendReceive(offset: Long, data: ByteArray): Long
    fun finishReceive(sha256Hex: String): ModelFile
}
```

`ModelStore` already has those three methods, so it implements the interface by
declaration. `WorkerRpcService` gains two defaulted constructor parameters:

```kotlin
class WorkerRpcService(
    private val identity: WorkerIdentitySource,
    private val metrics: DeviceMetricsSource,
    private val engine: EngineSource? = null,
    private val probe: () -> HardwareProbe.Result = HardwareProbe::measure,
    private val receiver: ModelReceiver? = null,
    private val onProvisioned: ((String) -> Unit)? = null,
)
```

- `beginModelPush`: `receiver == null` → `UNIMPLEMENTED` (same honest surface as
  a worker with no engine). Otherwise map `Accepted`/`Refused`.
- `pushModelChunks`: collect the stream into the receiver, counting bytes; any
  `IllegalArgumentException`/`IOException` → `onError(Status.INVALID_ARGUMENT)`
  and no partial response. Respond with `received_bytes`.
- `finishModelPush`: `finishReceive`, then `onProvisioned(modelId)`, respond
  `success=true`, `model_path`, `size_bytes`, `engine_started = onProvisioned != null`.
  A `finishReceive` throw → `success=false` with the message (not `onError`: the
  bytes arrived, the verification failed).

`WorkerRuntime` passes `library` as `receiver` and, on provision, selects the
file and restarts serving:

```kotlin
WorkerRpcServer(identity, metrics, engine) {
    // receiver = library
    // onProvisioned = { modelId -> library.list().firstOrNull { it.displayName.startsWith(modelId) }?.let { library.select(it.file); restartServing() } }
}
```

Adapt `WorkerRpcServer`'s constructor to forward the two new parameters.

Tests in `WorkerRpcContractTest.kt` with a fake `ModelReceiver`:

- [ ] `begin_model_push_reports_the_resume_offset`
- [ ] `begin_model_push_is_unimplemented_without_a_receiver`
- [ ] `chunks_are_appended_in_order`
- [ ] `an_out_of_order_chunk_fails_the_stream`
- [ ] `finish_reports_a_verification_failure_as_unsuccessful`
- [ ] `finish_calls_back_with_the_model_id`
- [ ] `gradlew :app:testDebugUnitTest` green

## Task 4 — Host provisioning core

Files:
- `core/src/provisioning/mod.rs` (new)
- `core/src/lib.rs` (`pub mod provisioning;`)

```rust
pub const CHUNK_BYTES: usize = 1024 * 1024;
pub const MAX_PUSH_MESSAGE_BYTES: usize = 8 * 1024 * 1024;

/// A GGUF on the host, addressed by the model row's `local_file_path`.
pub struct ModelSource { path: PathBuf, file_name: String, size_bytes: u64 }

impl ModelSource {
    pub fn open(path: impl Into<PathBuf>) -> Result<Self, String>;
    pub fn file_name(&self) -> &str;
    pub fn size_bytes(&self) -> u64;
    pub fn sha256_hex(&self) -> Result<String, String>;   // streaming, no full read
}

/// Bytes to send for the chunk starting at `offset`.
pub fn chunk_len(total: u64, offset: u64, chunk: usize) -> usize;
```

`file_name` is the path's final component; `open` rejects a missing file, a
directory, an empty file and a name that does not end in `.gguf`.

Tests (`#[cfg(test)]` in the module, `tempfile` is already a dev-dependency):

- [ ] `chunk_len_covers_the_tail`
- [ ] `chunk_len_is_zero_past_the_end`
- [ ] `sha256_matches_a_known_digest`
- [ ] `a_non_gguf_file_is_refused`
- [ ] `cargo test -p ai-upcycler-core provisioning` green

## Task 5 — Host streaming client

Files:
- `core/src/rpc/client.rs`
- `core/tests/common/mod.rs` (mock worker answers the three RPCs)
- `core/tests/provisioning.rs` (new)

Client methods:

```rust
pub async fn begin_model_push(&mut self, model_id: &str, file_name: &str,
    total_bytes: u64, sha256_hex: &str) -> Result<BeginModelPushResponse, String>;
pub async fn push_model_chunks(&mut self, chunks: impl Stream<Item = ModelChunk> + Send + 'static)
    -> Result<PushModelChunksResponse, String>;
pub async fn finish_model_push(&mut self, model_id: &str, sha256_hex: &str)
    -> Result<FinishModelPushResponse, String>;

/// begin → stream from the worker's offset → finish.
pub async fn provision_model(&mut self, source: &ModelSource, model_id: &str)
    -> Result<FinishModelPushResponse, String>;
```

`provision_model`:

1. `sha256_hex()` once.
2. `begin_model_push`; if `!accepted`, return an error carrying `error_message`.
3. Open `tokio::fs::File`, `seek(SeekFrom::Start(resume_from))`.
4. `futures::stream::unfold` reading `chunk_len(total, offset, CHUNK_BYTES)` bytes
   per item; `offset` starts at `resume_from`. No full-file buffering.
5. `push_model_chunks`; a stream error is returned with the offset it reached.
6. `finish_model_push`.

Both client constructors raise `max_decoding_message_size` /
`max_encoding_message_size` to `MAX_PUSH_MESSAGE_BYTES` (they currently use
`MAX_ACTIVATION_WIRE_BYTES`; keep the larger of the two).

`MockWorker` gains `push: Option<Arc<Mutex<PushLog>>>` with
`PushLog { model_id, sha256_hex, total_bytes, bytes: Vec<u8>, finished: bool }`,
implementing `begin_model_push` (accept, resume from `bytes.len()`),
`push_model_chunks` (append in order, reject a gap with `INVALID_ARGUMENT`) and
`finish_model_push` (compare the digest).

`core/tests/provisioning.rs`:

- [ ] `a_full_push_delivers_every_byte`
- [ ] `a_push_resumes_at_the_workers_offset` (pre-seed `PushLog.bytes`)
- [ ] `a_rejected_begin_makes_no_further_calls`
- [ ] `cargo test -p ai-upcycler-core --test provisioning` green

## Task 6 — HTTP surface

Files:
- `core/src/api/provisioning.rs` (new)
- `core/src/api/mod.rs` (module, `AppState.provisioning`, routes, CORS)
- `core/tests/devices.rs` (extend)

```rust
#[derive(Debug, Clone, PartialEq)]
pub enum ProvisionState { Idle, Running { bytes_done: u64, total_bytes: u64 }, Done { bytes: u64 }, Failed { message: String } }

pub struct ProvisioningRegistry { ... }   // Mutex<HashMap<String, ProvisionState>>
impl ProvisioningRegistry {
    pub fn status(&self, node_id: &str) -> ProvisionState;
    pub fn try_begin(&self, node_id: &str, total: u64) -> bool;   // false when already Running
    pub fn progress(&self, node_id: &str, bytes_done: u64);
    pub fn finish(&self, node_id: &str, result: Result<u64, String>);
}
```

Routes (axum 0.7 — `:id`, not `{id}`):

- `POST /api/devices/:id/provision` body `{ "model_id": "..." }` → `202`
  `{ "state": "running", "total_bytes": N }`; `409` when not enrolled, when the
  node is unknown, when the model file is missing, or when a push is running;
  `404` when the model id is unknown.
- `GET /api/devices/:id/provision` → `200` with the current `ProvisionState`
  serialized as `{ "state": "idle|running|done|failed", ... }`.

The handler spawns `tokio::spawn` around: resolve node from
`state.registry.list_nodes()`, `dial_worker(&*state.transports(), &node)`,
`provision_model`, `finish`. `AppState.provisioning: Arc<ProvisioningRegistry>`
with a `Default` in `AppState::new` and a `with_provisioning` for tests.

Tests in `core/tests/devices.rs`:

- [ ] `provisioning_an_unknown_model_is_404`
- [ ] `provisioning_a_node_that_is_not_enrolled_is_409`
- [ ] `a_second_provision_while_running_is_409`
- [ ] `provision_progress_starts_idle`
- [ ] `cargo test -p ai-upcycler-core` green

## Task 7 — Dashboard action

Files:
- `src/lib/types.ts`, `src/lib/api.ts`
- `src/components/AvailableDevices.tsx`

`ProvisionState` union mirroring the Rust enum; `startProvision(id, modelId)` and
`getProvision(id)`. The device card, when `state === "ready"`:

- shows `Provision model (<size>)` and the active model name;
- on click, `POST` then poll `GET` every 2 s while `running`;
- renders `sent / total (pct)` and the failure message;
- hides the button when `running` or `done`.

- [ ] `npx tsc --noEmit` exit 0
- [ ] `npm run build` exit 0

## Task 8 — Docs + PR

- [ ] `docs/tasks.md` §6 gains the provisioning checklist items, unticked
- [ ] `docs/api.md` documents the two routes
- [ ] Plan status table updated with commit hashes
- [ ] `cargo fmt --check`, `cargo clippy -p ai-upcycler-core --all-targets -- -D warnings`,
      `cargo test -p ai-upcycler-core` all green
- [ ] Branch pushed, PR opened, checks green, merged

## Risks

- **Regenerating protobuf stubs** touches every build; a stale `build/` on the
  Android side can hide a compile error. Run `gradlew clean :app:compileDebugKotlin`
  once in Task 3.
- **`MockWorker` is shared** by `api.rs`, `cluster_serving.rs`, `distributed.rs`
  and `mtls.rs`; add fields with defaults so those tests keep compiling.
- **1.9 GB over 2.4 GHz Wi-Fi** may take minutes. The transfer must not hold an
  HTTP request open — hence the 202 + poll design.
