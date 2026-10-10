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

    Master->>Worker: RPC ExecuteLayers(Layers 16..31, ActivationTensor)
    
    rect rgb(255, 240, 245)
        note over Worker: Layer 16 -> Layer 31 (Worker Phone RAM)
        Worker->>Worker: Compute Layers 16..31 Activations
    end

    Worker-->>Master: Return Final Layer Activation Tensor
    
    Master->>Master: Compute Output Logits & Sample Token ("Upcycling")
    Master-->>Client: Stream Token "Upcycling"
    
    Master->>Master: Append Token to KV-Cache & Repeat Loop
```

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
