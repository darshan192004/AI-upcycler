# AI Upcycler ♻️🤖

> **Transform your e-waste into a distributed AI supercomputer.** Run 14B–70B parameter LLMs locally on modest laptops by pooling old smartphones, tablets, and external SSDs/HDDs.

---

[![License: MIT](https://img.shields.io/badge/License-MIT-blue.svg)](LICENSE)
[![Status: Concept & Architecture](https://img.shields.io/badge/Status-Architectural_Design-orange.svg)]()
[![Platform: Windows | macOS | Linux | Android | iOS](https://img.shields.io/badge/Platform-Desktop_%26_Mobile-green.svg)]()

## 🌟 Overview

**AI Upcycler** is an open-source, local-first distributed inference orchestration framework. High hardware costs and high RAM requirements create a barrier to running powerful local Large Language Models (LLMs). At the same time, millions of old smartphones and storage drives sit unused in drawers as e-waste.

AI Upcycler solves both problems by turning your old electronics into active compute and memory worker nodes. Instead of trying to connect external devices as raw physical RAM (which fails due to hardware bus latency), AI Upcycler uses **Pipelined Layer-wise RPC Inference** and **Tiered Memory-Mapped (mmap) SSD Caching**.

With an 8GB laptop and two 4GB Android phones connected over Wi-Fi or USB, you can run a 14B or 70B quantized LLM entirely offline on your personal cluster.

---

## ⚡ Key Features

- 📱 **Mobile Hardware Pooling**: Turn Android and iOS devices into RPC compute nodes. Mobile CPUs and NPUs execute assigned LLM layers using their local RAM.
- 💾 **SSD/HDD mmap Layer Tiering**: Offload remaining model layers directly from fast NVMe SSDs into system memory using zero-copy `mmap` with pre-faulting and page eviction.
- ⚡ **USB & Wi-Fi Dual Transport**: Auto-discover workers over Wi-Fi (mDNS) or plug them in via USB (ADB port forwarding / tethering) for ultra-low latency ($\approx 1\text{ ms}$) and up to 480 Mbps–5 Gbps bandwidth.
- 🧠 **Dynamic Bandwidth-Aware Scheduler**: Auto-benchmarks memory bandwidth, FLOPS, and network ping upon pairing to calculate the optimal layer distribution matrix.
- 🔒 **Local-First & Private**: Zero cloud dependency. All prompt computations stay on your local physical devices protected by mTLS inter-device encryption.
- 🎯 **1-Click Desktop GUI**: Master control app (built with Tauri & React) for one-click cluster pooling, model downloading (GGUF / Hugging Face), and chat interface.

---

## 🏗️ Architecture at a Glance

```mermaid
graph TD
    subgraph Laptop / PC (Master Node - 8GB RAM)
        GUI[Desktop Dashboard & Chat UI] --> MasterCore[Rust Master Orchestrator]
        MasterCore --> Scheduler[Bandwidth-Aware Layer Scheduler]
        MasterCore --> MmapEngine[SSD Layer Pager & mmap]
        Scheduler --> MasterLayers[Master Executed Layers e.g., 0-12]
    end

    subgraph High-Speed Network / USB
        Scheduler -- "mTLS RPC over Wi-Fi / ADB USB Tunnel" --> Interconnect[Inter-Node Protocol]
    end

    subgraph Worker Cluster (Pooled E-Waste)
        Interconnect --> Node1[Android Phone 1 (4GB RAM - Layers 13-20)]
        Interconnect --> Node2[Android Phone 2 (3GB RAM - Layers 21-26)]
        Interconnect --> Node3[Old iPad/Phone 3 (4GB RAM - Layers 27-32)]
    end
```

---

## 📊 Performance & Hardware Matrix

| Hardware Setup | Model Size & Quantization | Effective Pooled Memory | Est. Inference Speed | Transport |
|---|---|---|---|---|
| 8GB Laptop | Llama-3-8B (Q4_K_M ~ 4.9 GB) | 8 GB Native RAM | 12 - 18 tok/s | Local Native |
| 8GB Laptop + 2x 4GB Androids | Qwen2.5-14B (Q4_K_M ~ 9.0 GB) | 16 GB Pooled RAM | 6 - 10 tok/s | USB 3.0 / Wi-Fi 6 |
| 8GB Laptop + 4x 4GB Androids + NVMe SSD | DeepSeek-R1-Distill-Qwen-32B (Q3_K_M ~ 14.5 GB) | 24 GB Pooled RAM + mmap | 3 - 6 tok/s | USB 3.0 Tethering |

---

## 📂 Documentation Directory

Comprehensive architectural specifications, design guidelines, and API docs are located in the [`/docs`](./docs) directory:

- 📄 [**Product Requirements Document (PRD)**](./docs/prd.md) — Product vision, goals, and user stories.
- 🏗️ [**System Architecture**](./docs/architecture.md) — Topology, sequence diagrams, and module specs.
- 🔬 [**Technical Specification**](./docs/spec.md) — Layer slicing math, RPC protocols, and network packet structures.
- 🔌 [**API Reference**](./docs/api.md) — Inter-device RPC protocol & REST/WebSocket Desktop endpoints.
- 🧠 [**Memory & Storage Management**](./docs/memory.md) — SSD mmap strategy, RAM budget allocation, and GGUF quantization tiering.
- 🎨 [**UI/UX & Design System**](./docs/design.md) — Desktop & mobile interfaces, dark theme, and micro-interactions.
- 🛡️ [**Security & Threat Model**](./docs/security.md) — Local security, mTLS authentication, and mobile sandboxing.
- 🗄️ [**Database & Persistence**](./docs/database.md) — SQLite schema for node registry, model catalog, and benchmark logs.
- 📏 [**Code Style & Guidelines**](./docs/code_style.md) — Coding conventions for Rust, C++, TypeScript, and Kotlin.
- 🗺️ [**Implementation Plan**](./docs/plan.md) — 5-Phase development and testing roadmap.
- 📋 [**Tasks Checklist**](./docs/tasks.md) — Actionable task tracker for development.

---

## 🚀 Quickstart (Development Setup)

### Prerequisites
- **Rust Toolchain**: `rustc` 1.75+ and `cargo`
- **Node.js**: v18+ and `pnpm`
- **C++ Compiler**: GCC/Clang with CMake (for building embedded `llama.cpp` RPC server)
- **Android Studio / NDK** (for building Android Worker Agent)

### 1. Clone Repository
```bash
git clone https://github.com/darshan192004/AI-upcycler.git
cd AI-upcycler
```

### 2. Build Master Orchestrator & UI
```bash
# Install frontend dependencies
pnpm install

# Run Desktop App in Development Mode
pnpm tauri dev
```

### 3. Enable Real Inference (optional)

Chat runs on a clearly-labelled stub backend until a llama.cpp engine is present. For real model output:

1. Download a CPU build of [`llama.cpp`](https://github.com/ggml-org/llama.cpp/releases) (e.g. `llama-b*-bin-win-cpu-x64.zip`) and extract it so `llama-server[.exe]` sits in `<app-data>/engine/`, or set `AI_UPCYCLER_LLAMA_SERVER` to the binary path.
2. Place a small GGUF such as [`qwen2.5-0.5b-instruct-q4_k_m.gguf`](https://huggingface.co/Qwen/Qwen2.5-0.5B-Instruct-GGUF) in `<app-data>/models/`, or set `AI_UPCYCLER_MODEL`.

When both files exist, the desktop app spawns `llama-server` on a loopback port at the first chat request and streams real tokens through the OpenAI-compatible API. With either file missing, behaviour is unchanged (stub backend + warning log).

---

## 🤝 Contributing

We welcome contributions from developers, hardware enthusiasts, and open-source AI advocates! Please check our [Code Style Guidelines](./docs/code_style.md) and open issues/PRs against the `main` branch.

---

## 📜 License

This project is licensed under the [MIT License](LICENSE).
