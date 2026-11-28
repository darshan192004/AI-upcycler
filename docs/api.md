# API Reference Specification - AI Upcycler

## 1. Overview

**AI Upcycler** exposes three API surfaces:
1. **Host Desktop External REST & WebSocket API**: Exposed on port `8080` of the Master Node (OpenAI-compatible) for consumption by frontend UIs, scripts, and local extensions. It stays bound to `127.0.0.1` and unauthenticated; see §5 for the browser-origin boundary.
2. **Inter-Node RPC Protocol**: Protocol Buffer binary protocol over TCP port `50062` between the Master Node and Worker Nodes.
3. **LAN REST API (opt-in, off by default)**: a separate bearer-authenticated listener for the phone client, on its own port and allowlist (§6).

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

Only local models appear here; virtual cloud catalog ids are excluded (see
§2.4).

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

### 2.3 Agents (tool calling)

Coding agents (opencode, Kilo Code) use the same endpoint but send `tools` and
expect `tool_calls` in the reply. Tool requests are served by the host
`llama-server` only; when no host engine is configured the API returns `400`
instead of a text answer, so the agent fails loudly rather than looping.

Small models sometimes write a call as one JSON object in `content` instead of
the native `tool_calls` field. When the request carries tools, the host backend
promotes that object only if its `name` matches a requested tool and its
`arguments` are a JSON object or a JSON string that parses. Prose, unknown
names, and multiple calls stay text. Tool requests are buffered until the turn
ends so the server can decide; requests without tools and
`tool_choice: "none"` keep streaming unchanged. A promoted turn returns
`finish_reason: "tool_calls"` and the usual `tool_calls` shape.

The supported request/response subset and the exact streaming shape are
specified in
`docs/superpowers/specs/2026-10-24-agent-api-compatibility-design.md`.

### 2.4 Provider routing on `POST /v1/chat/completions`

The request body accepts one optional field, `provider`. Its values are `auto`,
`local`, or a configured provider id: `openai`, `anthropic`, `gemini`, or
`kimi`. Omitting `provider` means `auto` (an empty string has the same effect),
so existing agent requests keep their behaviour.

- `auto`: a turn with tools goes to the host engine. Every other turn uses a
  real local backend (a ready cluster worker or the host `llama-server`), then
  a configured cloud provider. When none is available, the API returns `400`
  naming what is missing. The labelled offline stub is not a real backend, so
  `auto` never falls back to it silently; send `provider: "local"` to get that
  reply.
- `local`: explicit local routing with today's behaviour, including the
  labelled stub when no engine is loaded. This is the deterministic offline
  mode.
- A provider id: explicit cloud routing for that provider. The model may be a
  bare catalog id (`gpt-4o`) or a virtual `provider:model` id (`openai:gpt-4o`);
  the prefix must match the named provider. A tool turn is rejected with `400`
  before any session or user turn is persisted. A missing key or a model
  outside that provider's catalog is a `400` naming the problem; neither case
  writes a session.

```json
{
  "model": "openai:gpt-4o",
  "messages": [{"role": "user", "content": "Summarize AI Upcycler"}],
  "provider": "auto"
}
```

Cloud ids are virtual catalog ids: they are stored in sessions through reserved
`models` rows whose `local_file_path` starts with `virtual:`, and they are
filtered out of `GET /v1/models`. Local GGUF ids keep their existing lookup and
`404` message; a local id colliding with a required virtual id returns `409`.

### 2.5 Provider and secret endpoints

Base URL: `http://localhost:8080/api`. On loopback these endpoints are
unauthenticated (§5 describes the browser-origin boundary); the LAN mirror in §6
requires a bearer token. Every example below uses the placeholder `sk-...`; no
real key is shown or returned.

#### `GET /api/providers`

- **Response**: `200 OK` with a bare array, one entry per provider, each with
  `id`, `name`, `configured`, `models` (each with `id`, `label`,
  `supports_vision`), `supports_vision`, and `supports_tools`.
  `configured: true` means a key is available from the OS secret store; `false`
  covers both an unset key and no native store. The body never contains key
  material.

```json
[
  {
    "id": "openai",
    "name": "OpenAI",
    "configured": true,
    "models": [
      {"id": "gpt-4o", "label": "GPT-4o", "supports_vision": true}
    ],
    "supports_vision": true,
    "supports_tools": false
  },
  {
    "id": "kimi",
    "name": "Kimi (Moonshot)",
    "configured": false,
    "models": [
      {"id": "moonshot-v1-8k", "label": "Moonshot v1 8K", "supports_vision": false}
    ],
    "supports_vision": false,
    "supports_tools": false
  }
]
```

#### `POST /api/providers/{id}/test`

- **Request**: a model id from that provider's catalog, for example
  `{"model": "gpt-4o"}`.
- **Response**: `200 OK` with a result-only body — the provider, the model
  used, and a short truncated preview of one completion. It carries no headers,
  upstream URL, or key material. Without a key, `400` names the provider; an
  unknown `:id` is `404`.

#### `PUT /api/settings/secrets/{provider}`

- **Request**: `{"value": "sk-..."}`. Key material is write-only.
- **Response**: `200 OK` with the provider status object only (the shape shown
  under `GET /api/providers`); the submitted `value` is never echoed. An empty
  value or malformed body is `400`, and an unknown provider path is `404`.
  Keys go to the OS keychain, never SQLite or logs; there is no plaintext
  fallback.

#### `DELETE /api/settings/secrets/{provider}`

- **Response**: `200 OK` with the same status-only object, now
  `configured: false`. Repeated deletion is idempotent; an unknown provider
  path is `404`.

### 2.6 Read-only shared session routes

Chat completions create sessions through `session_id`; these routes only let a
phone or desktop client rediscover them and replay their messages. Neither
client keeps a local session database. There are no write routes here —
rename and delete belong to a later phase.

| Route | Response |
|---|---|
| `GET /api/sessions` | `200 OK` `{"sessions":[...]}` — session rows ordered by `created_at, session_id`, with no message bodies. |
| `GET /api/sessions/:id` | `200 OK` with one session row. |
| `GET /api/sessions/:id/messages` | `200 OK` `{"messages":[...]}` — message rows with `message_id`, `role`, `content`, `token_count`, `generation_time_sec`, `created_at`. |

A missing `:id` is `404` in the standard error envelope. Message `content` is
ordinary user-facing text; there is no separate field for secrets or
encrypted-looking payloads. These routes are also available on the LAN
allowlist (§6), where each request needs a bearer token.

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
  "available_pooled_ram_mb": 5184,
  "nodes": [
    {
      "node_id": "master-host-pc",
      "role": "master",
      "device_name": "Dell XPS 13 (8GB RAM)",
      "connection": "native",
      "ram_total_mb": 8192,
      "ram_used_mb": 5000,
      "ram_available_mb": 3192,
      "ram_assigned_mb": 0,
      "worker_state": "master",
      "assigned_layers": [],
      "ping_ms": 0.0
    },
    {
      "node_id": "worker-android-01",
      "role": "worker",
      "device_name": "Samsung Galaxy S10 (4GB RAM)",
      "connection": "usb_adb",
      "ram_total_mb": 4096,
      "ram_used_mb": 3000,
      "ram_available_mb": 1096,
      "ram_assigned_mb": 3200,
      "worker_state": "ready",
      "assigned_layers": [[0, 24]],
      "ping_ms": 1.2
    },
    {
      "node_id": "worker-android-02",
      "role": "worker",
      "device_name": "Pixel 4a (4GB RAM)",
      "connection": "wifi_5g",
      "ram_total_mb": 4096,
      "ram_used_mb": 3200,
      "ram_available_mb": 896,
      "ram_assigned_mb": 3200,
      "worker_state": "ready",
      "assigned_layers": [[25, 48]],
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
  },
  "thermal": [
    {
      "node_id": "worker-android-01",
      "temp_celsius": 38.5,
      "capped": false
    },
    {
      "node_id": "worker-android-02",
      "temp_celsius": 46.0,
      "capped": true
    }
  ],
  "thermal_relaxed": false
}
```
`discovery.mdns` is `"active"` when the mDNS browser daemon is running and `"unavailable"` when it failed to start (Wi-Fi discovery offline; USB/ADB workers unaffected).

`used_pooled_ram_mb` and `available_pooled_ram_mb` are **live measured sums**,
not scheduler reservations: each node contributes its current used/available
reading, and a node whose reading is unavailable is omitted rather than counted
as zero. `ram_used_mb` and `ram_available_mb` are `null` when the host or phone
cannot be read. `ram_assigned_mb` remains the scheduler's plan and is
legitimately `0` for the master, which coordinates but holds no layers.

`worker_state` distinguishes the stages a phone goes through: `usb_detected`
means ADB can see the device and its memory is listed, but the worker app has
not answered the RPC handshake yet (such a node is not dialable); `discovered`
is an unpaired mDNS node; `paired` means identity is established but no model
engine is open; `ready` means paired and serving an open engine. The dashboard
polls this endpoint every 5 seconds, so the RAM figures stay current while no
generation is running.

`assigned_layers` is the plan a real cluster request would use, not a
hypothetical one: only **paired** workers receive layers, in the scheduler's
order (least free RAM first, node id as the tie-break). The master is a
coordinator — it tokenizes and samples, so it lists its real capacity but
`assigned_layers` is always empty. A discovered-but-unpaired phone is listed
with its capacity too, but it cannot be dialed, so it also holds no layers; a
cluster with nothing paired reports no split at all and the host serves the
model locally. `active_model` names the model in the library that is being
served (locally or split), so it is set even when the plan is empty.

`thermal` reports the state the layer plan was computed against: one entry per
worker **in the plan** with a measured CPU temperature, and `capped: true` when
that worker's
share of the model was limited because it is hot (at or above 40 °C) — it is
resting while the cooler workers carry the load. `thermal_relaxed` is `true`
when the cluster was hot enough that resting was not an option and the cap had
to be relaxed to place the model. A worker with no measurement is absent from
`thermal`; an unpaired worker is never listed (it is not in the plan), and
neither is the master (it is not a polled worker).

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
  identities, plus the CA to trust. Both are pinned to one role (TLS
  certificates here are never dual-purpose):
  - `ca_pem` / `fingerprint` — the CA certificate to trust and its fingerprint;
    the master validates the worker's server certificate against this on every
    dial.
  - `server_cert_pem` / `server_key_pem` — **server** role. The worker serves
    `WorkerService` behind this; its SANs are the `addresses` above.
  - `client_cert_pem` / `client_key_pem` — **client** role, for worker→master
    links.
  - `lan_token` — the bearer credential for the opt-in LAN API (§6), returned
    only here, once. Its value is never reproduced in these docs; the host
    keeps only a SHA-256 hash in `lan-tokens.json` (§5 of `docs/security.md`).
  - `lan_port` — the LAN listener port (default `8090`; see §6).
  - `node_id` — the id this worker enrolled under, echoed back to the client.
```json
{
  "ca_pem": "-----BEGIN CERTIFICATE-----…",
  "client_cert_pem": "-----BEGIN CERTIFICATE-----…",
  "client_key_pem": "-----BEGIN PRIVATE KEY-----…",
  "server_cert_pem": "-----BEGIN CERTIFICATE-----…",
  "server_key_pem": "-----BEGIN PRIVATE KEY-----…",
  "fingerprint": "d87240d8adc360808043b42fd7e91fbe5cbbec0d21805b1b3d14b3d6ba17b4b8",
  "lan_port": 8090,
  "node_id": "Pixel-8._aiupcycler-worker._tcp.local."
}
```
  The response also contains `lan_token`; its value is deliberately omitted here.
  The enrollment and the token hash are persisted in the trust directory. When
  the stored CA is missing or unusable, the master generates a new one and
  clears both enrollments and token hashes, rotating every LAN token.
- **Errors**: `400` when `device_name` is empty or longer than 64 characters, when
  `node_id` is empty or longer than 128 characters, or when an address is neither
  an IP literal nor a DNS name (or more than 8 are given); `403` when the PIN is
  wrong or expired; `409` when the PIN was already redeemed. A rejected request
  does not consume the PIN.

---

### 3.5 Model Provisioning to a Device (spec 2026-11-03-model-provisioning)

Pushes the host's registered GGUF to one enrolled worker over the mTLS data
plane. The transfer never rides on the HTTP request — 1.9 GB over Wi-Fi takes
minutes — so `POST` starts it and answers `202`, and `GET` reports progress
(D-P5, D-P8). Progress lives in host memory only: a restart reports `idle`
rather than a stale transfer.

- **Endpoint**: `POST /api/devices/:id/provision`
- **Request**:
```json
{
  "model_id": "qwen2.5-coder-3b-instruct-q4_k_m"
}
```
  - `model_id` is an id from `/v1/models`, never a file path (D-P10): the
    server resolves the node from `:id` in its registry and the file from the
    store row, so the body cannot name a path.
  - `:id` resolves three forms — the mDNS node id, the bare ADB serial (or its
    `adb:` alias), and the `ip:port` key discovery uses before a serial exists.
- **Response**: `202 Accepted` — the push was claimed and spawned:
```json
{ "state": "running", "bytes_done": 0, "total_bytes": 2040000000 }
```
- **Endpoint**: `GET /api/devices/:id/provision`
- **Response**: `200 OK` — the current state, one of:
```json
{ "state": "idle" }
{ "state": "running", "bytes_done": 1048576, "total_bytes": 2040000000 }
{ "state": "done", "bytes": 2040000000 }
{ "state": "failed", "message": "provision tiny: ..." }
```
  `done` means the worker verified and installed the file; whether it
  reopened its engine is reported separately over RPC (`engine_started`,
  D-P7) — the worker reopens the file a moment after the transfer ends.
- **Errors**: `404` when no registry node matches `:id`, or the `model_id` is
  unknown; `409` when the device is not enrolled (D-P9 — provisioning rides
  the mTLS data plane), when the model file is missing on disk, or when a push
  to that device is already running. The checks run in that order, before a
  byte moves.

### 3.6 Task workflow runs (spec 2026-11-27, §10)

`POST /api/workflows` starts a DAG. The body requires a `model` and a non-empty
`steps` array; each step has `id`, `prompt`, `depends_on` (array of step ids),
and optional `provider` (`auto`, `local`, or a configured provider id). Unknown,
duplicate, self-, repeated, or cyclic dependencies are rejected with `400`.

```json
{
  "model": "llama-8b",
  "steps": [
    { "id": "draft", "prompt": "Draft the outline", "depends_on": [], "provider": "local" },
    { "id": "review", "prompt": "Review the outline", "depends_on": ["draft"], "provider": "local" }
  ]
}
```

Success is `202` with `{"run_id":"wf-...","status":"running"}`. Steps execute in
deterministic topological order through the same provider bridge as chat; a
failed step blocks its transitive dependents and unrelated steps still run.

`GET /api/workflows` returns `{"runs":[{"run_id","model","status","created_at","finished_at"}]}`
with statuses `running`, `completed`, `failed`.

`GET /api/workflows/{id}` returns `{"run":{...},"steps":[...]}`. Each step
carries `step_id`, `prompt`, `depends_on`, `provider`, `status`, `output`,
`source`, `error`, and `position`. Step statuses are `pending`, `running`,
`completed`, `failed`, `blocked`. An unknown id returns `404`.

---

## 4. WebSocket Real-Time Telemetry API

- **URL**: `ws://localhost:8080/api/telemetry`
- **Events Emitted**:
  - `node_connected`: Broadcast when a new mobile device connects.
  - `node_disconnected`: Broadcast when a device loses connection.
  - `generation_metrics`: Emits token speed (tokens/sec), RAM utilization, and temperatures during inference. `generation_metrics.data.source` is `local`, `adb:<serial>` (a real cluster worker), or a provider id (`openai`, `anthropic`, `gemini`, `kimi`); agent requests that omit `provider` keep their existing event shape and source.
  - `worker_health`: Emits the freshest measured health of every worker (battery, CPU temperature, free RAM) plus the master's own RAM utilization. Published after each poll cycle (every 5 s).
  - `model_download_progress`: Emits GGUF download progress (`running` / `complete` / `error`) with received and total bytes.

```json
{
  "event": "generation_metrics",
  "timestamp": 1743457100,
  "data": {
    "source": "local",
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

`workflow_step_started` and `workflow_step_completed` carry workflow progress
for the desktop timeline and the phone's read-only view. Both use the tagged
format; `status` in a completed event is `completed`, `failed`, or `blocked`.

```json
{"event":"workflow_step_started","timestamp":1764278400,
 "data":{"run_id":"wf-...","step_id":"draft"}}
{"event":"workflow_step_completed","timestamp":1764278401,
 "data":{"run_id":"wf-...","step_id":"draft","status":"completed",
         "output":"...","source":"local","error":null}}
```

The socket does not replay old events: clients load initial state from
`GET /api/workflows/{id}` and apply WS events on top of it.

## 5. Browser Origins

The listeners bind loopback, but a browser on the same machine is not "local"
in the sense the threat model needs: without a CORS policy, any page the user
visits could read the topology or start a download. The API therefore grants
CORS only to:

- `tauri://localhost` - the bundled desktop shell on Linux and macOS;
- `http(s)://tauri.localhost` - the bundled shell on Windows and Android;
- `http(s)://localhost[:port]`, `http(s)://127.0.0.1[:port]`, `[::1]` - the
  `tauri dev` server and local tooling.

Every other origin receives no `Access-Control-Allow-Origin` header, so the
browser drops the response. Requests without an `Origin` header (curl, the
Android worker, SDKs) are unaffected: CORS is enforced by browsers, not by the
server. The telemetry WebSocket applies the same rule to its handshake and
answers `403` to a foreign origin, because browsers do not apply CORS to
WebSocket upgrades (spec 2026-10-24).

This is not authentication. A program running as the same user can still call
the loopback API; that boundary is the OS account, and it stays documented as a
known limit in `docs/tasks.md`. The LAN listener (§6) has no CORS layer; its
routes require a bearer token instead.

---

## 6. LAN REST API (opt-in)

The LAN listener exists for the phone chat client. It is **off by default** and
is a separate listener from loopback `127.0.0.1:8080`: the loopback API stays
unauthenticated, while every LAN route below requires
`Authorization: Bearer <pairing token>`. mDNS discovery itself needs no token.
Neither listener may be published through a proxy, tunnel, or firewall rule
(`docs/AGENTS.md` §8).

| Setting | Contract |
|---|---|
| `AI_UPCYCLER_LAN_API` | Set to exactly `AI_UPCYCLER_LAN_API=1` to enable; any other value (including absence) leaves it off. |
| `AI_UPCYCLER_LAN_API_PORT` | Listener port, default `8090`; an invalid value falls back to `8090` with a warning. The effective port is returned as `lan_port` at enrollment (§3.4). |

When enabled, the listener binds a resolved LAN IPv4 address only (never
`0.0.0.0`) and advertises `_aiupcycler-master._tcp.local.` via mDNS. TXT records
carry public metadata only — `version`, `node_id`, `api_port`, `fingerprint` —
never a token. The phone also accepts a manual `host:port` when mDNS does not
cross subnets. Discovery validates the fingerprint against the one received at
pairing.

**Exact route allowlist** (all of these require the bearer header; this is not
the loopback router):

| Method and route | Purpose |
|---|---|
| `GET /v1/models` | Registered local models (§2.1). |
| `POST /v1/chat/completions` | Streaming or non-streaming chat (§2.2, §2.4). |
| `GET /api/providers` | Provider status (§2.5). |
| `POST /api/providers/:id/test` | One short provider completion (§2.5). |
| `PUT /api/settings/secrets/:provider` | Write a provider key (§2.5). |
| `DELETE /api/settings/secrets/:provider` | Delete a provider key (§2.5). |
| `GET /api/sessions` | List sessions (§2.6). |
| `GET /api/sessions/:id` | One session (§2.6). |
| `GET /api/sessions/:id/messages` | Session messages (§2.6). |
| `GET /api/devices/:id/provision` | Provisioning status (§3.5). |

Pairing, device control, cluster management, model download, and the telemetry
WebSocket are **not** on the LAN allowlist. A missing, malformed, revoked, or
rotated token returns `401` with the fixed body
`{"error":{"message":"unauthorized","type":"unauthorized","code":401}}` — no
model, session, or provider detail. Token format, storage, rotation, and
revocation are specified in `docs/security.md` §5.

### POST /api/media
Requires AI_UPCYCLER_MULTIMODAL=1 (otherwise 404). Multipart field `file`, MIME one of
image/png, image/jpeg, image/webp, image/gif; max 10 MiB; max 4 per message.
Returns 200 {"media_hash","mime_type","file_name","byte_count"}.
Invalid input: 400 {"error":"invalid_image_attachment"}.

### ChatMessage.content union
Text-only: "content": "hello" (unchanged).
Multipart turn: "content": [{"type":"text","text":"look"},
{"type":"image","media_hash":"<64 hex>","mime_type":"image/png","file_name":"a.png","byte_count":9}].
Missing/null content is treated as "".

### Image error bodies (all 400)
- multimodal_disabled — gate off
- image_attachments_require_vision_provider — local/cluster/tool turn
- provider_lacks_vision_support — resolved model has supports_vision=false
- no_vision_capable_model_loaded — no vision-capable route
- invalid_image_attachment — MIME/magic/size/hash failure
No image bytes appear in logs or error responses.

## 2.6 Settings and recommendations

```text
GET /api/settings → 200

{
  "provider": null | "openai" | "anthropic" | "gemini" | "kimi",
  "model": null | string,
  "context_tokens": 1024..65536,
  "max_tokens": 1..32768,
  "show_source_badges": true | false,
  "compact_mode": true | false
}

PUT /api/settings ← the same object → 200 with the stored object.
Unknown fields, including api_key, token, secret, and password, return 400.

GET /api/settings/recommendations → 200

{
  "context_tokens": 2048..32768,
  "max_tokens": 256..8192,
  "reasons": [string]
}
```

Provider secrets are handled only by the write-only
`PUT /api/settings/secrets/{provider}` route and never appear in settings
responses. Settings responses carry only the non-secret fields shown above;
unknown fields such as `api_key`, `token`, `secret`, and `password` are
rejected with 400.

`GET /api/config → 200 {"multimodal_enabled": bool}` is a public
(unauthenticated loopback) endpoint added to close the desktop/Android gate
gap. It contains exactly that one boolean and no settings, paths, keys, or
tokens.

## 2.7 Sessions

```text
GET /api/sessions → 200 { "sessions": [ChatSessionRow] }
GET /api/sessions/{id}/messages → 200 { "messages": [ChatMessageRow] }
PUT /api/sessions/{id} ← { "title": "1..=120 characters" } → 200 ChatSessionRow
DELETE /api/sessions/{id} → 204
Missing session → 404 with the existing error envelope.
```

The 404 uses the API's shared error envelope
`{"error":{"message":...,"type":"not_found","code":404}}`. The loopback router
also serves `GET /api/sessions/{id}` for a single session row.
