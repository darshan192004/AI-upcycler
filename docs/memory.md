# Memory & Storage Tiering Specification - AI Upcycler

## 1. Overview & Core Problem

LLMs are strictly **memory-bound** operations. Generating a single token requires reading the entire parameter weights array through system memory into processor caches.

- An 8GB laptop running Windows/Linux typically has only **$\approx 4.5\text{ GB}$ of usable free RAM**.
- A 14B parameter model at 4-bit precision (GGUF `Q4_K_M`) requires **$\approx 9.0\text{ GB}$ of RAM**.
- A 32B parameter model at 3-bit precision (GGUF `Q3_K_M`) requires **$\approx 15.0\text{ GB}$ of RAM**.

**AI Upcycler** bridges this gap using a **Two-Tiered Memory Architecture**:
1. **Tier 1: Pooled LPDDR4/5 RAM** (Host RAM + Mobile Device RAM over RPC).
2. **Tier 2: NVMe SSD Memory-Mapped (`mmap`) Tier** (Host OS page-file swap and zero-copy weight mapping).

---

## 2. Memory Tiering Architecture

```mermaid
graph TD
    subgraph Model Weights (e.g. 14B Q4_K_M ~ 9.0 GB)
        L1[Layers 0..15 (4.0 GB)]
        L2[Layers 16..27 (3.0 GB)]
        L3[Layers 28..35 (2.0 GB)]
    end

    subgraph Tier 1: Pooled RAM (Ultra High Bandwidth)
        L1 --> HostRAM[Laptop Native RAM - 4.5GB Free]
        L2 --> MobileRAM1[Android Phone 1 - 3.2GB Free RAM]
    end

    subgraph Tier 2: SSD mmap Tier (High Bandwidth / Paged)
        L3 --> SSDmmap[Internal/External NVMe SSD - mmap Engine]
    end

    HostRAM --> RAMSpeed["Speed: 25,000 - 50,000 MB/s | Latency: 15ns"]
    MobileRAM1 --> NetSpeed["Speed: 400 MB/s (USB) / 80 MB/s (Wi-Fi) | Latency: 1ms"]
    SSDmmap --> SSDSpeed["Speed: 3,000 - 7,000 MB/s | Latency: 30μs"]
```

---

## 3. Host System Memory Budget & Reservation Rules

To prevent Operating System Out-Of-Memory (OOM) killer crashes on the 8GB host laptop, the Master Engine enforces strict buffer reservations.

$$\text{Usable\_Host\_RAM} = \text{Total\_Physical\_RAM} - \text{OS\_Safety\_Reservation} - \text{Desktop\_GUI\_Overhead}$$

```ini
[Host Memory Reservation Policy (8GB Host Laptop)]
Total_Physical_RAM = 8192 MB
OS_Safety_Reservation = 1536 MB (Windows/Linux OS Kernel)
Desktop_GUI_Overhead = 512 MB (Tauri + App state)
Max_KV_Cache_Reservation = 1024 MB (Context length 4096 tokens)
----------------------------------------------------
Max_Allowed_Host_Model_Weights = 5120 MB
```

If assigned model layers exceed `Max_Allowed_Host_Model_Weights`, the remaining host layers are allocated to the **SSD `mmap` Tier**.

---

## 4. SSD Memory-Mapped (`mmap`) Engine Details

### 4.1 Zero-Copy File Mapping
- The Master Node uses POSIX `mmap()` (or Windows `CreateFileMappingW()`) to map GGUF model files directly into virtual memory space.
- The engine uses `madvise(MADV_WILLNEED)` to notify the OS kernel which transformer layer pages will be accessed next during forward propagation.

### 4.2 Page Fault & Eviction Strategy
- As the transformer forward pass moves from layer $l$ to layer $l+1$, the `mmap` engine pre-faults page ranges for layer $l+1$ and issues `madvise(MADV_DONTNEED)` for layer $l-1$.
- This keeps active RAM usage flat while sequentially reading weight chunks from NVMe SSDs at speeds up to **$5,000\text{ MB/s}$**.

---

## 5. Model Quantization Tiering matrix

To maximize inference performance across heterogeneous pooled devices, AI Upcycler enforces quantization standards based on available pooled cluster memory:

| Pooled Cluster RAM | Target Model Size | Quantization Format | Weight Size | Quality Loss (Perplexity) |
|---|---|---|---|---|
| **8 GB - 12 GB** | Llama-3.1-8B | `Q4_K_M` (4-bit) | $\approx 4.9\text{ GB}$ | Very Low ($+0.05$) |
| **12 GB - 16 GB** | Qwen2.5-14B | `Q4_K_M` (4-bit) | $\approx 8.9\text{ GB}$ | Negligible ($+0.03$) |
| **16 GB - 24 GB** | DeepSeek-R1-Distill-32B | `Q3_K_M` (3-bit) | $\approx 14.2\text{ GB}$ | Low ($+0.12$) |
| **24 GB - 32 GB+** | Llama-3-70B | `IQ2_XS` (2-bit K-quant) | $\approx 21.1\text{ GB}$ | Moderate ($+0.35$) |

---

## 6. Key-Value (KV) Cache Memory Management

- **Pipelined KV-Cache Isolation**: Each worker node maintains the KV-cache for its assigned layers in its local RAM. Worker nodes do **not** transmit historical KV-caches over the network; they only process current sequence position activations.
- **Quantized KV-Cache**: KV-cache data is stored in 8-bit `FP8` or 4-bit `Q4_0` format to cut worker RAM consumption by up to **$75\%$** compared to standard `FP16`.
