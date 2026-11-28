# Model Provisioning (Slice B) — Design

Status: accepted, executing 2026-11-03
Supersedes: nothing. Extends
`docs/superpowers/specs/2026-11-03-wireless-device-onboarding-design.md`.

## 1. Problem

Slice A gets a phone discovered, enrolled and trusted. The worker app then
serves metadata only, because `WorkerEngine.open` needs a `.gguf` that a human
copied onto the device with a USB cable and a file manager. `ModelStore` can
import from a document picker, but nothing on the host can reach that picker.

So the cluster can discover a phone and still not use it. Issue #123.

## 2. Goal

A provisioned phone serves compute with no cable and no manual file copy:

1. The host has a model registered in `Store` (`models.local_file_path`).
2. The user presses **Provision** on the device card.
3. The host streams that file to the worker over the transport the node already
   uses (mTLS once enrolled).
4. The worker verifies the bytes, installs the model, selects it and restarts
   serving, so the next handshake reports `supports_compute = true`.

Non-goals: multi-device model placement (#124), layer-range choice, delta
updates, resumable transfer across a host restart, and provisioning to a device
that has not enrolled.

## 3. Decisions

### D-P1 — Three RPCs, not one streaming call

`BeginModelPush` (unary) → `PushModelChunks` (client-streaming) →
`FinishModelPush` (unary).

A client-streaming call cannot return a value mid-stream, so the worker's
"bytes I already have" offset has to come back *before* the stream starts.
Sending from zero and letting the server discard the prefix would re-send a
1.9 GB prefix after every interruption.

### D-P2 — 1 MiB chunks, 8 MiB message ceiling

The existing activation path raises `max_*_message_size` to
`MAX_ACTIVATION_WIRE_BYTES`. Provisioning sets its own 8 MiB ceiling and sends
1 MiB chunks: a phone with ~2.3 GB free should not need an 8 MiB buffer per
message, and 1 MiB keeps the resumable unit small enough that a Wi-Fi drop
costs seconds.

### D-P3 — SHA-256 is the resume contract

The host hashes the file before the first byte moves and sends the lowercase
hex digest in `BeginModelPush`. The worker:

- resumes a `.part` only when the digest matches the digest it recorded when
  that `.part` was created;
- discards a `.part` whose recorded digest differs (a different build of the
  same file name must never be stitched together);
- re-hashes on `FinishModelPush` and refuses a mismatch.

The digest of a partial file is not checkable, so the `.part` carries a
sidecar `<name>.gguf.part.meta` with `sha256=<hex>\ntotal=<bytes>\n`. The
sidecar is deleted on success or failure.

### D-P4 — `.part` is invisible to the library

`ModelStore.list()` only accepts `*.gguf`, so `<name>.gguf.part` never appears
in the UI or in `selected()`. `finishReceive` moves the `.part` to a unique
`*.gguf` name, so a torn transfer cannot be opened as a model.

### D-P5 — Provisioning is an explicit action

Nothing pushes 1.9 GB over the user's Wi-Fi and battery on its own. The device
card shows the size and a **Provision model** button once the device is
`Ready`. This mirrors the Slice A rule that Install is always an explicit
action.

### D-P6 — Free-space gate with headroom

The worker refuses a push when
`total - resume_from + RESERVE_BYTES > usableSpace`, with
`RESERVE_BYTES = 256 MiB`. The existing import path keeps 64 MiB; a model is
larger and the engine maps it, so the reserve is bigger here. The refusal
carries both numbers so the UI can say what to delete.

### D-P7 — Success means "verified and installed", not "serving"

`FinishModelPushResponse` separates the two: `success` is the transfer,
`engine_started` is whether the worker reopened its engine on the new file. A
verified file with a failed engine start is a success with a warning, not a
failure — the bytes are on the phone and a restart may fix it.

### D-P8 — Progress lives in host memory

`POST /api/devices/:id/provision` returns `202` with the current state and
spawns a task. `GET /api/devices/:id/provision` reports
`idle | running | done | failed` plus `bytes_done`, `total_bytes` and an error
string. A second `POST` while one is running returns `409`. No telemetry
events, consistent with Slice A's deferral; the dashboard polls.

### D-P9 — Provisioning requires enrollment

The endpoint refuses a node that is not in the enrollment registry (409). The
whole point is the mTLS data plane; a plaintext 1.9 GB push to an
unauthenticated peer is not something to make convenient.

### D-P10 — The endpoint resolves names, not addresses

The request body is `{ "model_id": "..." }`. The node comes from
`NodeRegistry::list_nodes()` by node id; the file comes from
`Store::get_model(model_id).local_file_path`. Missing node → 404, missing model
→ 404, model file absent on disk → 409 with the path. No client-supplied paths.

## 4. Protocol

```proto
message BeginModelPushRequest {
  string model_id = 1;
  string file_name = 2;     // bare file name, must end .gguf
  int64 total_bytes = 3;
  string sha256_hex = 4;    // lowercase hex, 64 chars
}

message BeginModelPushResponse {
  bool accepted = 1;
  string error_message = 2;
  int64 resume_from_bytes = 3;
}

message ModelChunk { int64 offset = 1; bytes data = 2; }

message PushModelChunksResponse {
  bool success = 1;
  string error_message = 2;
  int64 received_bytes = 3;
}

message FinishModelPushRequest { string model_id = 1; string sha256_hex = 2; }

message FinishModelPushResponse {
  bool success = 1;
  string error_message = 2;
  string model_path = 3;
  int64 size_bytes = 4;
  bool engine_started = 5;
}
```

`PushModelChunks` requires `chunk.offset == bytes already written`, so a
reordered or duplicated chunk fails loudly instead of corrupting the file.

## 5. Failure modes

| Failure | Behaviour |
|---|---|
| Phone drops mid-stream | Stream errors; `.part` + sidecar survive; the next push resumes at the worker's offset |
| Host restarts mid-stream | Same as above; the registry is in memory, so the UI shows `idle` and the next push resumes |
| Disk full on phone | `BeginModelPush` refuses before any bytes move |
| Different file, same name | Sidecar digest mismatch → worker deletes the `.part` and starts at 0 |
| Corrupt transfer | `FinishModelPush` hash mismatch → `.part` deleted, `success=false` |
| Engine cannot open the model | `success=true`, `engine_started=false`, reason in `error_message` |
| Device not enrolled | `409` from the HTTP endpoint, no RPC dial |
| Second concurrent push | `409` from the HTTP endpoint |

## 6. Security

- The transfer rides the existing enrollment-aware transport (`dial_worker`),
  so an enrolled phone gets the CA-verified mTLS channel from Slice A.
- The worker writes only inside its app-private `models/` directory and only
  to names that pass `ModelStore.safeName`.
- No path, port or address crosses the API boundary; only ids.

## 7. Verification

Device-free (this slice's automated bar):

- JVM: `ModelStoreReceiveTest` (resume, digest mismatch, offset mismatch, hash
  mismatch, free-space refusal, torn `.part` invisible to `list()`).
- JVM: `WorkerRpcServiceTest` contract tests for the three RPCs.
- Rust: `provisioning` unit tests for chunk arithmetic and digests.
- Rust: `core/tests/provisioning.rs` router tests — 202/409/404, progress
  transitions, and a mock worker that receives the bytes.
- Rust: full suite, `cargo fmt --check`, clippy `-D warnings`.
- Frontend: `tsc --noEmit`.

Device-gated (recorded as unproven until a phone is attached):

- 1.9 GB transfer over Wi-Fi to `16284c49` with a mid-transfer drop and resume.
- The worker opens the provisioned 3B model and answers a forward pass.
- Measured throughput and battery cost for the transfer.
