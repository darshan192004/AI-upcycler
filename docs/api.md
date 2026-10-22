# API Reference Specification - AI Upcycler

## 1. Overview

**AI Upcycler** exposes two primary API surfaces:
1. **Host Desktop External REST & WebSocket API**: Exposed on port `8080` of the Master Node (OpenAI-compatible) for consumption by frontend UIs, scripts, and local extensions.
2. **Inter-Node RPC Protocol**: Protocol Buffer binary protocol over TCP port `50052` between the Master Node and Worker Nodes.

---

## 2. Host Master OpenAI-Compatible REST API

### Base URL: `http://localhost:8080/v1`

### 2.1 List Models
- **Endpoint**: `GET /v1/models`
- **Response**:
```json
{
  "object": "list",
  "data": [
    {
      "id": "Llama-3-8B-Instruct-Q4_K_M.gguf",
      "object": "model",
      "created": 1743456700,
      "owned_by": "ai-upcycler-local",
      "meta": {
        "quantization": "Q4_K_M",
        "file_size_bytes": 4920000000,
        "layers": 32,
        "recommended_min_ram_gb": 6.0
      }
    },
    {
      "id": "Qwen2.5-14B-Instruct-Q4_K_M.gguf",
      "object": "model",
      "created": 1743456800,
      "owned_by": "ai-upcycler-local",
      "meta": {
        "quantization": "Q4_K_M",
        "file_size_bytes": 8980000000,
        "layers": 48,
        "recommended_min_ram_gb": 12.0
      }
    }
  ]
}
```

### 2.2 Chat Completions (Streaming & Non-Streaming)
- **Endpoint**: `POST /v1/chat/completions`
- **Headers**: `Content-Type: application/json`
- **Request Body**:
```json
{
  "model": "Qwen2.5-14B-Instruct-Q4_K_M.gguf",
  "messages": [
    {"role": "system", "content": "You are a helpful AI assistant running on a pooled local cluster."},
    {"role": "user", "content": "What is AI Upcycler?"}
  ],
  "temperature": 0.7,
  "max_tokens": 512,
  "stream": true
}
```
- **Response (Server-Sent Events Stream)**:
```http
HTTP/1.1 200 OK
Content-Type: text/event-stream
Cache-Control: no-cache
Connection: keep-alive

data: {"id":"chatcmpl-102","object":"chat.completion.chunk","created":1743457000,"model":"Qwen2.5-14B","choices":[{"index":0,"delta":{"role":"assistant","content":"AI "},"finish_reason":null}]}

data: {"id":"chatcmpl-102","object":"chat.completion.chunk","created":1743457001,"model":"Qwen2.5-14B","choices":[{"index":0,"delta":{"content":"Upcycler "},"finish_reason":null}]}

data: [DONE]
```

---

## 3. Host Cluster Management REST API

### Base URL: `http://localhost:8080/api/cluster`

### 3.1 Get Cluster Topology & Status
- **Endpoint**: `GET /api/cluster/topology`
- **Response**:
```json
{
  "cluster_id": "cluster-local-99a",
  "status": "active",
  "active_model": "Qwen2.5-14B-Instruct-Q4_K_M.gguf",
  "total_pooled_ram_mb": 16384,
  "used_pooled_ram_mb": 11200,
  "nodes": [
    {
      "node_id": "master-host-pc",
      "role": "master",
      "device_name": "Dell XPS 13 (8GB RAM)",
      "connection": "native",
      "ram_total_mb": 8192,
      "ram_assigned_mb": 4096,
      "assigned_layers": [0, 16],
      "ping_ms": 0.0
    },
    {
      "node_id": "worker-android-01",
      "role": "worker",
      "device_name": "Samsung Galaxy S10 (4GB RAM)",
      "connection": "usb_adb",
      "ram_total_mb": 4096,
      "ram_assigned_mb": 3200,
      "assigned_layers": [17, 32],
      "ping_ms": 1.2
    },
    {
      "node_id": "worker-android-02",
      "role": "worker",
      "device_name": "Pixel 4a (4GB RAM)",
      "connection": "wifi_5g",
      "ram_total_mb": 4096,
      "ram_assigned_mb": 3200,
      "assigned_layers": [33, 48],
      "ping_ms": 8.4
    }
  ],
  "storage_mmap": {
    "enabled": false,
    "path": "",
    "mapped_layers_count": 0
  },
  "discovery": {
    "mdns": "active"
  }
}
```
`discovery.mdns` is `"active"` when the mDNS browser daemon is running and `"unavailable"` when it failed to start (Wi-Fi discovery offline; USB/ADB workers unaffected).

### 3.2 Trigger Hardware Re-Benchmarking
- **Endpoint**: `POST /api/cluster/benchmark`
- **Response**:
```json
{
  "status": "success",
  "nodes_tested": 3,
  "rebalancing_triggered": true,
  "execution_time_ms": 1420
}
```

### 3.3 Start a GGUF Model Download
- **Endpoint**: `POST /api/models/download`
- **Request**:
```json
{
  "repo_id": "bartowski/Llama-3.1-8B-Instruct-GGUF",
  "file_name": "llama-3.1-8b-instruct-q4_k_m.gguf"
}
```
- **Response**: `202 Accepted` — the download runs in the background; progress is streamed as `model_download_progress` telemetry events (section 4) and the model is registered in `/v1/models` on completion.
```json
{
  "status": "accepted",
  "file_name": "llama-3.1-8b-instruct-q4_k_m.gguf"
}
```
- **Errors**: `400` when `file_name` does not end with `.gguf`, contains path separators, or `repo_id` is not an `owner/name` Hugging Face repository; `409` when a download for the same file is already in flight.

### 3.4 QR/PIN Worker Pairing (mTLS Enrollment)
- **Endpoint**: `POST /api/pairing/session`
- **Response**: `200 OK` — starts a single-use pairing session (5-minute TTL).
```json
{
  "pin": "811896",
  "qr_payload": "aiupcycler://pair?fp=d872…b4b8&token=811896",
  "expires_in_secs": 300,
  "fingerprint": "d87240d8adc360808043b42fd7e91fbe5cbbec0d21805b1b3d14b3d6ba17b4b8"
}
```
- **Endpoint**: `POST /api/pairing/enroll`
- **Request**:
```json
{
  "pin": "811896",
  "device_name": "Pixel 8",
  "node_id": "Pixel-8._aiupcycler-worker._tcp.local.",
  "addresses": ["192.168.1.50", "127.0.0.1"]
}
```
  - `node_id` is the id the master dials this worker by (`DiscoveredNode::node_id`
    for mDNS, `adb:<serial>` for USB). Required: enrollment is recorded against
    it, and the split driver resolves a worker's transport by it, so defaulting
    it would silently disagree with what discovery reports.
  - `addresses` are the addresses the master will dial this worker at — they
    become the issued server certificate's subject alternative names. Optional:
    omitted, they default to `["127.0.0.1", "localhost"]` (the `adb reverse`
    loopback tunnel). An explicitly empty list is a `400`, not a default.
- **Response**: `200 OK` — redeems the PIN and issues the worker's two mTLS
  identities, plus the CA to trust. Both are signed by the master's ephemeral CA
  and pinned to one role (TLS certificates here are never dual-purpose):
  - `server_cert_pem`/`server_key_pem` — **server** role. The worker serves
    `WorkerService` behind this; the master validates it against `fingerprint`
    on every dial, and its SANs are the `addresses` above.
  - `client_cert_pem`/`client_key_pem` — **client** role, for worker→master links.
```json
{
  "ca_pem": "-----BEGIN CERTIFICATE-----…",
  "client_cert_pem": "-----BEGIN CERTIFICATE-----…",
  "client_key_pem": "-----BEGIN PRIVATE KEY-----…",
  "server_cert_pem": "-----BEGIN CERTIFICATE-----…",
  "server_key_pem": "-----BEGIN PRIVATE KEY-----…",
  "fingerprint": "d87240d8adc360808043b42fd7e91fbe5cbbec0d21805b1b3d14b3d6ba17b4b8"
}
```
  The enrollment is recorded in-memory against `node_id`; the master's CA is
  ephemeral, so it is discarded on restart along with the certificates it signed.
- **Errors**: `400` when `device_name` is empty or longer than 64 characters, when
  `node_id` is empty or longer than 128 characters, or when an address is neither
  an IP literal nor a DNS name (or more than 8 are given); `403` when the PIN is
  wrong or expired; `409` when the PIN was already redeemed. A rejected request
  does not consume the PIN.

---

## 4. WebSocket Real-Time Telemetry API

- **URL**: `ws://localhost:8080/api/telemetry`
- **Events Emitted**:
  - `node_connected`: Broadcast when a new mobile device connects.
  - `node_disconnected`: Broadcast when a device loses connection.
  - `generation_metrics`: Emits token speed (tokens/sec), RAM utilization, and temperatures during inference.
  - `worker_health`: Emits the freshest measured health of every worker (battery, CPU temperature, free RAM) plus the master's own RAM utilization. Published after each poll cycle (every 5 s).
  - `model_download_progress`: Emits GGUF download progress (`running` / `complete` / `error`) with received and total bytes.

```json
{
  "event": "generation_metrics",
  "timestamp": 1743457100,
  "data": {
    "tokens_per_second": 8.4,
    "current_token": 142,
    "master_ram_utilization_pct": 68.2,
    "worker_nodes": [
      {"id": "worker-android-01", "temp_celsius": 38.5, "battery_pct": 92.0},
      {"id": "worker-android-02", "temp_celsius": 41.2, "battery_pct": 84.0}
    ]
  }
}
```

```json
{
  "event": "worker_health",
  "timestamp": 1743457100,
  "data": {
    "master_ram_utilization_pct": 68.2,
    "nodes": [
      {
        "id": "worker-android-01",
        "temp_celsius": 38.5,
        "battery_pct": 92.0,
        "free_ram_bytes": 2415919104,
        "observed_at_epoch_ms": 1743457098000
      },
      {
        "id": "worker-android-02",
        "temp_celsius": null,
        "battery_pct": 84.0,
        "free_ram_bytes": 1879048192,
        "observed_at_epoch_ms": 1743457098000
      }
    ]
  }
}
```

**Unknown is `null`, never `0`**: a metric the worker cannot measure — a desktop
has no battery, and Android's thermal sysfs is unreadable for unprivileged apps —
is reported as `null`. The master never substitutes a zero. A worker that has
never answered is absent from `nodes` entirely; a worker that stopped answering
keeps its last measured snapshot, with `observed_at_epoch_ms` saying how old it is.

```json
{
  "event": "model_download_progress",
  "timestamp": 1743457100,
  "data": {
    "model_id": "llama-3.1-8b-instruct-q4_k_m",
    "file_name": "llama-3.1-8b-instruct-q4_k_m.gguf",
    "received_bytes": 536870912,
    "total_bytes": 4928308224,
    "status": "running"
  }
}
```
