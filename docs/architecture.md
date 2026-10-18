# System Architecture Specification - AI Upcycler

## 1. High-Level Architecture Overview

**AI Upcycler** follows a **Master-Worker Pipelined RPC Distributed Topology**. The system decouples model management, user interactions, and orchestration from raw tensor computation.

The host computer acts as the **Master Node (Brain)**, managing the UI, cluster auto-discovery, model partitioning, SSD `mmap` page eviction, and initial activation processing. Secondary devices (smartphones, tablets, old PCs) act as **Worker Nodes**, maintaining designated model layer weights in their local RAM and executing layer operations via an embedded RPC worker engine.

```mermaid
graph TB
    subgraph Master Node (Host Laptop)
        UI[Desktop GUI - React + Tailwind]
        TauriCore[Tauri Rust Bridge]
        MasterEngine[Orchestrator Core]
        Scheduler[Bandwidth-Aware Layer Slicing Scheduler]
        MmapPager[SSD mmap Memory Tiering Engine]
        DB[(SQLite Local Store)]

        UI <--> TauriCore
        TauriCore <--> MasterEngine
        MasterEngine <--> Scheduler
        MasterEngine <--> MmapPager
        MasterEngine <--> DB
    end

    subgraph Network Transport Layer
        Transport[mTLS / ProtoBuf RPC over TCP (Wi-Fi 6 / USB Tunnel)]
        MasterEngine <--> Transport
    end

    subgraph Worker Cluster (Distributed Nodes)
        Transport <--> Worker1[Android Node 1 - llama.cpp RPC Worker]
        Transport <--> Worker2[Android Node 2 - llama.cpp RPC Worker]
        Transport <--> Worker3[iOS Node 3 - Swift RPC Agent]
    end

    subgraph Storage Subsystem
        MmapPager <--> NVMe[Fast NVMe SSD GGUF Layer Storage]
    end
```

---

## 2. Core System Components

### 2.1 Master Node (Host Computer)

1. **Desktop GUI (React + TypeScript)**:
   - Provides visual cluster health metrics, device discovery controls, model catalog search, and prompt interface.
2. **Master Orchestrator Core (Rust)**:
   - Manages cluster state, triggers layer streaming, initiates RPC pipeline calls, and aggregates KV-cache tensors.
3. **Bandwidth-Aware Layer Scheduler**:
   - Analyzes hardware capabilities of all registered devices and solves a constrained optimization problem to assign model layers.
4. **SSD `mmap` Memory Tiering Engine**:
   - Handles host-side memory paging when the Master Node's designated layer count exceeds available physical RAM.

### 2.2 Worker Nodes (Mobile & Secondary Devices)

1. **Agent App (Android Kotlin / iOS Swift)**:
   - Lightweight wrapper handling device auto-discovery registration, background execution prevention, and battery/thermal telemetry.
2. **Embedded Native Worker Engine (`llama.cpp` RPC C++)**:
   - Compiled for ARM64-v8a with NEON / NPU hardware acceleration.
   - Listens on TCP port `50052` for binary tensor execution payloads.
   - Allocates contiguous RAM buffers for assigned layer weights and KV-cache blocks.

---

## 3. Distributed Pipelined Inference Sequence

The following sequence diagram details the token generation lifecycle across a 32-layer LLM split between the Master Node (Laptop) and a Mobile Worker Node.

```mermaid
sequenceDiagram
    autonumber
    participant Client as User / API Client
    participant Master as Master Orchestrator (Laptop)
    participant Worker as Mobile Worker Node (Phone)

    Client->>Master: POST /v1/chat/completions ("Explain AI Upcycling")
    Master->>Master: Tokenize Prompt -> Token IDs [102, 451, 882]
    
    rect rgb(240, 248, 255)
        note over Master: Layer 0 -> Layer 15 (Master Native RAM)
        Master->>Master: Compute Embeddings & Layers 0..15 Activations
    end

    Master->>Worker: RPC ExecuteForward(Layers 16..31, ActivationTensor)
    
    rect rgb(255, 240, 245)
        note over Worker: Layer 16 -> Layer 31 (Worker Phone RAM)
        Worker->>Worker: Compute Layers 16..31 Activations
    end

    Worker-->>Master: Return Final Layer Activation Tensor
    
    Master->>Master: Compute Output Logits & Sample Token ("Upcycling")
    Master-->>Client: Stream Token "Upcycling"
    
    Master->>Master: Append Token to KV-Cache & Repeat Loop
```

> **Master-side tokenization (the `model-io` tokenizer).** The master owns
> the tokenizer. `model-io/src/tokenizer.rs` reads the `tokenizer.ggml.*` KVs
> from the same `.gguf` the workers load and runs the qwen2-family pipeline
> (`Tokenizer::encode`): the GPT-2 byte map, the qwen2 pre-tokenizer split -
> one digit per pre-token, one arbitrary non-CR/LF prefix before letter runs -
> rank-ordered BPE merges, and special markers such as `<|im_end|>` matched
> leftmost-longest BEFORE BPE (llama.cpp's parse-special step). Prompts are
> rendered through the hard-coded Qwen2.5 ChatML template
> (`Tokenizer::apply_chatml`); decode refuses non-UTF-8 byte runs (`decode`).
> The gates are **id-exact**: 111 corpus strings must produce byte-identical
> id lists to llama.cpp's `POST /tokenize`, and the server's `/detokenize`
> must string-equal our decode - see `model-io/tests/tokenizer_parity.rs`
> (asset-gated like the engine parity; the proof run happened against
> qwen2.5-0.5b-instruct-q4_k_m.gguf + llama.cpp build 11429). Workers keep
> consuming pre-tokenized ids over `ExecuteForward` - they never see text.

### 3.0 Serving a live chat request (2026-10-18)

Chat requests no longer delegate to the host `llama-server` when the cluster
exists. `/v1/chat/completions` (`core/src/api/chat.rs`) runs on
`AdaptiveBackend` (`core/src/api/distributed.rs`), which decides **once, before
the first streamed byte**: cluster serving iff at least one enrolled, currently
registered worker + a validated `compute_plan` + a readable tokenizer for the
model's store row; otherwise the pre-existing `llama-server` backend is the
fallback of last resort (the stub backend remains the final fallback and
desktop debug path). No mid-stream fallbacks - a request either ends on the
cluster or on the fallback.

On the cluster path the master:

1. Renders ChatML (`Tokenizer::apply_chatml`) + encodes in-process to `{ids}`
   (§ above; the wire stays id-only, D6).
2. Prefills in ONE `ExecuteForward` (the head stage returns WHOLE prefill
   logits `[n_tokens, n_vocab]`), then drives one-id decode `forward`s through
   `PipelineDriver`/`LoadedPipelineDriver` (fresh worker KV session per
   request+attempt; `UnloadLayers`-backed reset recovery, bounded: exactly one
   transparent re-prefill while nothing has been emitted, otherwise the
   request fails honestly with the worker's error message).
3. Samples master-side from the final logits row: greedy argmax (ties to the
   lowest id, llama.cpp order) or temperature+top-p with a seeded LCG (our
   determinism, not llama's RNG), and stops on `<|im_end|>` in decoded text -
   the marker is never emitted, trailing marker-prefix bytes are withheld from
   the stream.
4. Streams decoded-diff chunks as SSE `chat.completion.chunk` with
   `tokens_emitted` = real ids per chunk and carries the TRUE prefill length
   for `usage.prompt_tokens`; the buffered (non-stream) shape assembles the
   same content.

Acceptance gates are asset-gated parity runs in `core/tests/cluster_serving.rs`
(real router + real worker subprocesses): greedy continuations match
llama-server's raw `/completion` id-for-id for both single-stage and a planned
12/12 two-worker split; hermetic suites cover the sampler, marker handling,
recovery windows, metric truthing and the SSE failure shape. Shipped
limitation: the whole-logits prefill contract bounds prompts at roughly 430
tokens on the shipped model (the 256MiB RPC message limits in
`core/src/rpc/client.rs`); serving long contexts needs a last-row-only wire
contract - a proto change, deliberately deferred.

### 3.1 Distributed Split Lifecycle: Load → Forward → Unload

The diagram above traces a single *token*. A **session** loads the cluster once and then
pushes many tokens through it. That is what `LoadedPipeline` (`core/src/rpc/engine.rs`)
implements, and it is verified device-free against two in-process mock workers in
`core/tests/distributed.rs` — each mock appends a unique tag byte to the activation it
receives, so the resulting byte order is proof the stages ran in ascending layer order.

```mermaid
sequenceDiagram
    autonumber
    participant Sched as LayerScheduler
    participant Driver as LoadedPipeline (Master)
    participant Wa as Worker A (layers 0..15)
    participant Wb as Worker B (layers 16..31)

    Sched->>Driver: PartitioningPlan (list of LayerAssignment)

    rect rgb(240, 255, 240)
        note over Driver: load() - sort by start_layer, then validate the plan
        note over Driver: an unsound plan is rejected here, before any TCP connection
        Driver->>Wa: Handshake(master_id, protocol_version, timestamp)
        Wa-->>Driver: worker_id, free_ram_bytes, cpu_cores, has_npu_acceleration
        Driver->>Wa: LoadLayers(model_id, 0..15)
        Wa-->>Driver: success, allocated_ram_bytes
        Driver->>Wb: Handshake(master_id, protocol_version, timestamp)
        Wb-->>Driver: worker_id, free_ram_bytes, cpu_cores, has_npu_acceleration
        Driver->>Wb: LoadLayers(model_id, 16..31)
        Wb-->>Driver: success, allocated_ram_bytes
        note over Driver: a failure here rolls back the stages already loaded
    end

    loop one activation per generated token
        Driver->>Wa: ExecuteForward(session_id, 0..15, activation_tensor, sequence_position)
        Wa-->>Driver: output_activation_tensor (shape, data_type, raw_data)
        Driver->>Wb: ExecuteForward(session_id, 16..31, A output tensor)
        Wb-->>Driver: output_activation_tensor
        note over Driver: execution_time_us is summed across the stages
    end

    rect rgb(255, 245, 240)
        note over Driver: unload() consumes the pipeline - use-after-unload is a compile error
        Driver->>Wa: UnloadLayers(model_id)
        Driver->>Wb: UnloadLayers(model_id)
    end
```

The phases, precisely:

1. **Plan** — `LayerScheduler::compute_plan` produces the `LayerAssignment`s. Ranges are
   **inclusive** of both `start_layer` and `end_layer`, and the scheduler reserves 20% of
   each node's free RAM as head-room before assigning weights.
2. **Load** — `LoadedPipeline::load(model_id, &assignments)` sorts the assignments by
   `start_layer` and validates the plan (§3.1.1), then for each stage in order: connect
   (plaintext, or mTLS when the transport source says so) → `Handshake` → `LoadLayers`. If a
   worker reports `success = false` the load aborts with
   that worker's `error_message`, and the stages already loaded are released via
   `UnloadLayers` so a partial load does not pin RAM on the cluster.
3. **Forward** — `LoadedPipeline::forward(payload)` threads **one** activation through
   every stage in ascending layer order via `ExecuteForward`, feeding each stage's output
   tensor into the next (§3.1.2), and sums each stage's `execution_time_us`. A worker
   returning `success = false` fails the whole forward.
4. **Unload** — `LoadedPipeline::unload(self)` **consumes** the pipeline, so using it after
   its workers have dropped the weights is a compile error. Every stage is released even if
   an earlier release failed; the first error is returned after the loop.

A session is deliberately distinct from `RpcPipelineEngine::dispatch_forward_step`, the
connectionless one-shot used by the desktop debug command: that connects, handshakes and
runs exactly one step, whereas `LoadedPipeline` keeps the workers loaded and connected
across many forward passes.

#### 3.1.1 Plan validity — the split contract

`load` validates the plan **before dialing any worker**, so an unsound split fails fast
(the tests assert the mock event log is still empty) instead of silently producing wrong
activations. A plan is rejected when it is:

| Rejected | Why it is unsound |
|---|---|
| empty | a pipeline with no stages would pass activations through unmodified |
| inverted (`start > end`) | the range is meaningless |
| not anchored at layer 0 | a plan partitions the model, so starting later silently skips the model head |
| overlapping (`next.start <= prev.end`) | those layers would execute twice |
| gapped (`next.start > prev.end + 1`) | those layers would be skipped |

The **tail is deliberately not checked**: the driver is not told how many layers the model
has, and `LayerScheduler::compute_plan` may legitimately return a plan that the available
cluster is too small to finish — it always starts at layer 0, but may stop short of the
last layer. `LoadedPipeline` is intentionally not `Debug`, so a load result cannot be
`expect_err`ed into a panic on the error path.

#### 3.1.2 Activation handoff

One `TensorBuffer` threads through the stages: `shape`, `data_type` and `raw_data`. The
driver carries each stage's **output** shape and dtype into the next stage rather than
assuming they are unchanged, so a stage may legally reshape or requantise the tensor.

Two fallbacks keep a silent worker from corrupting the pipeline: an empty output shape
falls back to the request's shape, and an unknown `data_type` falls back to the request's
dtype (`ActivationType`: `FP32 = 0`, `FP16 = 1`, `BF16 = 2`, matching
`TensorBuffer.DataType`). Both fallbacks are only correct for a shape- and
dtype-preserving stage, so a real engine must populate both fields rather than rely on
them.

#### 3.1.3 Wire contract

`proto/cluster_rpc.proto` (`aiupcycler.rpc.WorkerService`) is canonical; the Android worker
vendors a copy and its `checkProtoInSync` task fails the build on drift.

| RPC | Kind | Master side | Worker side |
|---|---|---|---|
| `Handshake` | metadata | used by `load` | live |
| `GetHealthStatus` | metadata | `WorkerRpcClient::health` | live |
| `LoadLayers` | compute | used by `load` | `UNIMPLEMENTED` |
| `ExecuteForward` | compute | used by `forward` | `UNIMPLEMENTED` |
| `UnloadLayers` | compute | used by `unload` and load rollback | `UNIMPLEMENTED` |

`LoadLayersRequest.weights_chunk_data` is left empty: weights are resolved worker-side for
now, and pushing a contiguous GGUF chunk is future work.

#### 3.1.4 What the worker engine must implement

> **Implemented.** The host worker engine is the pure-Rust `worker/` crate
> (`ai-upcycler-worker`): it satisfies this whole contract and is verified by
> `worker/tests/*` (fixture tiers in CI; `load_contract.rs`, `split_equivalence.rs`
> drive real processes through `LoadedPipeline`). The contract text below is kept
> verbatim as the spec record; design + implementation notes live in
> `docs/superpowers/specs/2026-10-16-worker-engine-design.md` and
> `docs/superpowers/plans/2026-10-16-worker-engine.md` (PRs #41/#42/#53, PR4).

The master half is done and mock-verified, so the remaining half is the engine. This is the
contract it must satisfy:

- **`LoadLayers`** — retain the weights for `[start_layer, end_layer]` of `model_id` (both
  bounds inclusive); return `success = false` with a human-readable `error_message` when it
  cannot, e.g. insufficient free RAM; report `allocated_ram_bytes`.
- **`ExecuteForward`** — run the owned range over the incoming activation and return the
  output tensor with **shape and dtype populated** (§3.1.2); return `success = false` to
  reject; report `execution_time_us`.
- **`UnloadLayers`** — free the RAM held for `model_id` and report `freed_ram_bytes`. It
  must tolerate being called for a model it never loaded: the driver unloads after a failed
  forward, and after a partial load it rolls back only the stages that succeeded.

Limits the driver applies to every call: 5 s to establish the connection
(`CONNECT_TIMEOUT`) and 30 s per RPC (`REQUEST_TIMEOUT`).

> **Transport.** The driver dials each worker with the transport a `WorkerTransportSource`
> resolves for it. `LoadedPipeline::load_secure` takes that source, so the caller — backed by
> the pairing store; `EnrolledWorkerSource` is the implementation today — decides per worker; `LoadedPipeline::load` is the plaintext shorthand,
> acceptable only on a loopback or ADB-tunneled link. A worker reported as
> `WorkerTransport::MutualTls` is reached over mTLS with its server certificate **pinned to
> the cluster CA** (`WorkerRpcClient::connect_tls`; pinning in `core/src/security.rs`,
> adversarial TLS tests in `core/tests/mtls.rs`). Certificate verification uses the address
> actually dialed (`LayerAssignment::ip_address`), which is the SAN the worker was issued
> during pairing — a `WorkerTransportSource` resolves from the node id alone and so cannot
> know that name. The address is normalized before it is used for either the URL or the
> verification (an IPv6 literal's brackets are stripped, via `host`), so `[::1]` verifies
> against the bare `::1` SAN rather than a name no worker was ever issued. There is
> deliberately **no fallback to plaintext**, and every transport is
> resolved *before* the first dial, so a misconfigured or incomplete credential store fails
> the load without contacting any worker. `EnrolledWorkerSource` is the production source: it
> resolves a transport only for a worker that completed pairing, and errors otherwise. Still
> hardware-gated: the worker-side mTLS server (§5.1 of the plan).

---

## 4. Communication & Transport Layer

### 4.1 Discovery Protocol (mDNS / Zeroconf)
- Worker nodes broadcast service type `_aiupcycler-worker._tcp.local.` on port `50052`.
- TXT records include: `ram_free_mb`, `cpu_cores`, `os_type`, `api_version`.

### 4.2 USB Transport Adapter (ADB / USB Tethering)
- When Wi-Fi latency exceeds $15\text{ ms}$, the Master app prompts the user to connect via USB cable.
- Automatically executes `adb forward tcp:50052 tcp:50052` to achieve low latency ($\approx 1\text{ ms}$) and higher bandwidth ($480\text{ Mbps} - 5\text{ Gbps}$).

### 4.3 RPC Serialization Format
- High-efficiency binary serialization using Protocol Buffers (`cluster_rpc.proto`) with custom flat buffer tensor views to eliminate serialization copying overhead.

---

## 5. Fault Tolerance & Fallback Strategy

1. **Worker Disconnection during Inference**:
   - If a worker drops out (e.g. Wi-Fi packet loss or thermal throttling), the Master detects RPC timeout within $500\text{ ms}$.
   - The Master dynamically re-allocates the missing layers to host SSD `mmap` storage and resumes generation without resetting the session.
2. **Thermal & Battery Protection**:
   - Mobile workers transmit battery state and temperature telemetry.
   - If phone battery drops below $15\%$ or CPU temperature reaches $45^\circ\text{C}$, the scheduler migrates layers off the affected phone to prevent device degradation.
