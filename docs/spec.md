# Technical System Specification - AI Upcycler

## 1. Overview & Formal Definitions

This specification outlines the mathematical formulations, network packet structures, and layer partitioning algorithms used by **AI Upcycler**.

---

## 2. Mathematical Model for Layer Partitioning

Let a Large Language Model $\mathcal{M}$ consist of $L$ transformer layers with total weight size $W_{\text{bytes}}$ and hidden state dimensions $d_{\text{model}}$.

The cluster $\mathcal{C}$ consists of $N$ nodes:
- Master Node: $S_0$
- Worker Nodes: $S_1, S_2, \dots, S_{N-1}$

Each node $S_i$ has the following measured parameters:
- $M_i$: Available physical RAM (in Bytes) after reserving system overhead.
- $B_i$: Memory bandwidth of native RAM (in Bytes/sec).
- $T_i$: Compute capability (in FLOPS or tok/sec per layer).
- $N_{bw, i}$: Network bandwidth between Master $S_0$ and Worker $S_i$ (Bytes/sec).
- $\tau_i$: One-way network latency between Master $S_0$ and Worker $S_i$ (seconds).

### 2.1 Layer Allocation Optimization Problem

We seek an assignment vector $\boldsymbol{l} = [l_0, l_1, \dots, l_{N-1}]$ where $l_i$ is the contiguous number of layers assigned to node $S_i$, satisfying:

$$\sum_{i=0}^{N-1} l_i = L$$

Subject to the memory budget constraint for each node:

$$l_i \cdot \left( \frac{W_{\text{bytes}}}{L} \right) + \text{KV\_Cache}(l_i) \le M_i \quad \forall i \in \{0, \dots, N-1\}$$

Where the Key-Value (KV) Cache size per layer for context length $C$ and batch size $b=1$ is defined as:

$$\text{KV\_Cache}(l_i) = 2 \cdot l_i \cdot C \cdot n_{\text{heads\_kv}} \cdot d_{\text{head}} \cdot \text{sizeof}(\text{fp16})$$

### 2.2 Execution Time Minimization Objective

The total generation latency per token $t_{\text{token}}$ is the sum of compute times and inter-node transfer times:

$$t_{\text{token}} = \sum_{i=0}^{N-1} \left( \frac{l_i \cdot \text{FLOPs\_per\_layer}}{T_i} \right) + \sum_{i=1}^{N-1} \left( 2 \cdot \tau_i + \frac{2 \cdot b \cdot d_{\text{model}} \cdot \text{sizeof}(\text{fp16})}{N_{bw, i}} \right)$$

The scheduler uses greedy dynamic programming to select $l_i$ minimizing $t_{\text{token}}$.

---

## 3. Protocol Buffer RPC Specification (`cluster_rpc.proto`)

```protobuf
syntax = "proto3";

package aiupcycler.rpc;

// Service definition for Worker Nodes
service WorkerService {
  rpc Handshake (HandshakeRequest) returns (HandshakeResponse);
  rpc LoadLayers (LoadLayersRequest) returns (LoadLayersResponse);
  rpc ExecuteForward (ExecuteForwardRequest) returns (ExecuteForwardResponse);
  rpc UnloadLayers (UnloadLayersRequest) returns (UnloadLayersResponse);
  rpc GetHealthStatus (HealthRequest) returns (HealthResponse);
}

message HandshakeRequest {
  string master_id = 1;
  string protocol_version = 2;
  int64 timestamp = 3;
}

message HandshakeResponse {
  string worker_id = 1;
  string device_name = 2;
  int64 total_ram_bytes = 3;
  int64 free_ram_bytes = 4;
  int32 cpu_cores = 5;
  bool has_npu_acceleration = 6;
}

message LoadLayersRequest {
  string model_id = 1;
  int32 start_layer = 2;
  int32 end_layer = 3;
  bytes weights_chunk_data = 4; // Compressed layer weights
}

message LoadLayersResponse {
  bool success = 1;
  string error_message = 2;
  int64 allocated_ram_bytes = 3;
}

message TensorBuffer {
  repeated int64 shape = 1;
  enum DataType {
    FP32 = 0;
    FP16 = 1;
    BF16 = 2;
  }
  DataType data_type = 2;
  bytes raw_data = 3;
}

message ExecuteForwardRequest {
  string session_id = 1;
  int32 start_layer = 2;
  int32 end_layer = 3;
  TensorBuffer activation_tensor = 4;
  int32 sequence_position = 5;
}

message ExecuteForwardResponse {
  bool success = 1;
  TensorBuffer output_activation_tensor = 2;
  int64 execution_time_us = 3;
}

message HealthRequest {}

message HealthResponse {
  float battery_percentage = 1;
  float cpu_temp_celsius = 2;
  int64 free_ram_bytes = 3;
}
```

---

## 4. Hardware Connectivity & Bus Bandwidth Benchmarks

| Bus / Connection Type | Theoretical Bandwidth | Real-World Bandwidth | Latency | Viability for AI Upcycler |
|---|---|---|---|---|
| LPDDR4x Internal Phone RAM | $25.6\text{ GB/s} - 34.1\text{ GB/s}$ | $\approx 20\text{ GB/s}$ | $\approx 20\text{ ns}$ | **Primary Worker Memory** |
| USB 3.2 Gen 2x2 (Phone to PC) | $20\text{ Gbps} \quad (2.5\text{ GB/s})$ | $\approx 1.8\text{ GB/s}$ | $\approx 1\text{ ms}$ | **Optimal Low-Latency Connection** |
| USB 3.0 / ADB Forward | $5\text{ Gbps} \quad (625\text{ MB/s})$ | $\approx 400\text{ MB/s}$ | $\approx 1.5\text{ ms}$ | **Recommended Wired Connection** |
| Wi-Fi 6 (802.11ax 5GHz) | $1.2\text{ Gbps} \quad (150\text{ MB/s})$ | $\approx 80\text{ MB/s}$ | $\approx 5-12\text{ ms}$ | **Supported Wireless Connection** |
| USB 2.0 (Legacy Phone Cable) | $480\text{ Mbps} \quad (60\text{ MB/s})$ | $\approx 35\text{ MB/s}$ | $\approx 10\text{ ms}$ | Supported (Slowest Wireless alternative) |
| Internal NVMe PCIe Gen 4 SSD | $7,000\text{ MB/s}$ | $\approx 5,000\text{ MB/s}$ | $\approx 30\text{ }\mu\text{s}$ | **Primary Host `mmap` Tier** |
| External Mechanical HDD | $150\text{ MB/s}$ | $\approx 80\text{ MB/s}$ | $\approx 15\text{ ms}$ | Static Weight Storage Only |
