# Cloud Provider Bridge — Implementation Plan

Date: 2026-11-27
Status: ready for execution
Spec: `docs/superpowers/specs/2026-11-27-privatelm-style-extension-design.md` §6
Branch: `feat/bundle-worker-apk`

## Goal

Add cloud-provider chat as a first-class, opt-in route behind the existing
`GenerationBackend` contract, without changing agent requests that omit
`provider`, without breaking local model ids, and without writing key material
to SQLite or logs.

This plan covers sub-project **A only**. The Android chat UI (B), multimodal
content (C), persisted settings/session management (D), workflows (E), and
image generation (F) are separate plans. No LAN listener is added here; the
security-model change in spec §5.4 belongs to sub-project B, where the phone
client needs it.

## Decisions baked into the tasks

- `Auto` follows the spec: tools → host engine; otherwise a real local backend
  (ready cluster worker or llama-server); otherwise a configured cloud
  provider; otherwise `400` naming what is missing. The stub backend is no
  longer a silent fallback under `Auto`.
- `Local` explicitly keeps today's behavior, including the labelled stub
  placeholder. This gives developers a deterministic offline mode.
- `Cloud(id)` is explicit and rejects tool turns with `400` before any session
  or user turn is persisted.
- Cloud model ids are virtual `provider:model` strings. They are persisted in
  `chat_sessions.model_id` via a reserved `models` row whose
  `local_file_path` starts with `virtual:`; `/v1/models` filters those rows out.
  This avoids a risky SQLite FK table rebuild while keeping the existing
  `chat_sessions → models` constraint intact.
- `GenerationChunk` gains `source: Option<String>`; `GenerationMetrics` gains
  `source: String`. Values are `local`, `adb:<serial>`, or a provider id.
- Secrets come from an OS keychain through `keyring`. If no native store is
  available, the provider is reported as unconfigured. There is no plaintext
  fallback.

## Files

### New

| File | Purpose |
|---|---|
| `core/src/secrets.rs` | `SecretStore`, `KeyringSecretStore`, `UnconfiguredSecretStore`, test `MemorySecretStore` |
| `core/src/providers/mod.rs` | `ProviderId`, `ProviderDescriptor`, `ProviderStatus`, `CloudProvider`, `ProviderRegistry`, `CloudRoute` |
| `core/src/providers/openai_compat.rs` | Shared SSE normalizer for OpenAI-shaped APIs (OpenAI + Kimi) |
| `core/src/providers/openai.rs` | OpenAI descriptor/adapter |
| `core/src/providers/anthropic.rs` | Anthropic Messages SSE adapter |
| `core/src/providers/gemini.rs` | Gemini `streamGenerateContent` SSE adapter |
| `core/src/providers/kimi.rs` | Kimi OpenAI-compatible adapter |
| `core/src/routing.rs` | `RoutePolicy` + request/model resolution |
| `core/src/api/providers.rs` | `/api/providers*`, `/api/settings/secrets/*` handlers |
| `core/tests/providers.rs` | Mock-HTTP adapter tests, routing table, secret-leak tests |

### Modified

| File | Change |
|---|---|
| `core/Cargo.toml` | add `keyring` dependency |
| `core/src/lib.rs` | register `providers`, `routing`, `secrets` modules |
| `core/src/api/backend.rs` | `GenerationChunk.source`, `GenerationBackend::is_real()` |
| `core/src/api/llama.rs` | emit `source: Some("local")` |
| `core/src/api/distributed.rs` | label cluster chunks with the planned worker id; `AdaptiveBackend::is_real()` |
| `core/src/telemetry.rs` | `GenerationMetrics.source` |
| `core/src/api/chat.rs` | `provider` request field, virtual model lookup, source-aware metrics |
| `core/src/api/mod.rs` | `AppState` secrets/providers, routes, CORS `PUT`, bootstrap wiring |
| `core/src/store/mod.rs` | `ensure_virtual_model`, virtual-path helper, `StoreError::Conflict` |
| `core/src/api/models.rs` | filter virtual rows out of `/v1/models` |
| `core/tests/api.rs` | adapt stub-dependent tests to the new `Auto` rule |
| `core/tests/common/mod.rs` | mock provider server helper + real-local test backend |
| `docs/api.md` | document provider routes, cloud route field, telemetry `source` |
| `docs/AGENTS.md` | note the optional `provider` field and that loopback stays unauthenticated |

## Task 1 — Contract and dependency groundwork (TDD)

**Files:** `core/Cargo.toml`, `core/src/lib.rs`, `core/src/api/backend.rs`,
`core/src/api/llama.rs`, `core/src/api/distributed.rs`, `core/src/telemetry.rs`.

1. Write failing tests first in `core/src/api/backend.rs` tests:

```rust
#[test]
fn stub_backend_is_not_a_real_local_engine() {
    assert!(!StubBackend::new().is_real());
}

#[test]
fn chunks_can_carry_their_execution_source() {
    let chunk = GenerationChunk { source: Some("adb:192.168.1.9:5555".into()), ..Default::default() };
    assert_eq!(chunk.source.as_deref(), Some("adb:192.168.1.9:5555"));
    assert_eq!(GenerationChunk::default().source, None);
}
```

2. Add the dependency (native stores are opt-in per target; the default `v1`
   feature already selects the platform store on desktop):

```toml
# core/Cargo.toml
keyring = { version = "3.6.3", features = [
    "windows-native",
    "apple-native",
    "linux-native-sync-persistent",
] }
```

3. Change the trait and chunk:

```rust
pub struct GenerationChunk {
    pub content: String,
    pub tokens_emitted: u32,
    pub prompt_tokens: Option<u32>,
    pub tool_calls: Option<Vec<ToolCallDelta>>,
    pub finish_reason: Option<String>,
    /// `local`, `adb:<serial>`, or a provider id. `None` is legacy/unknown.
    pub source: Option<String>,
}

pub trait GenerationBackend: Send + Sync + 'static {
    fn generate(&self, request: GenerationRequest)
        -> BoxStream<'static, Result<GenerationChunk, String>>;

    fn tool_calling_available(&self) -> bool { false }

    /// True when this backend can actually produce model output. The labelled
    /// offline stub is false so `Auto` can fall through to a cloud provider.
    fn is_real(&self) -> bool { true }
}
```

4. `StubBackend`: return `false` from `is_real()`; set
   `source: Some("local".into())` on its chunks. `LlamaCppBackend`: set
   `source: Some("local".into())` on every emitted chunk, including the
   buffered tool-call chunks from `finalize_buffered_turn`.

5. `DistributedBackend::new` gains a `source: impl Into<String>` argument and
   stores it in `Inner`; every chunk it yields carries `Some(source.clone())`.
   `AdaptiveBackend::try_cluster` passes the first planned node id from
   `serving_plan_nodes(...)` (already `adb:<serial>` in production). Add:

```rust
impl GenerationBackend for AdaptiveBackend {
    fn is_real(&self) -> bool {
        !serving_plan_nodes(&self.registry, &self.security.enrollments, &self.thermal).is_empty()
            || self.fallback.is_real()
    }
}
```

6. `GenerationMetrics` gains `pub source: String`. Update the two existing
   `chat.rs` metric tests to assert `data.source == "local"` when no chunk
   supplied a source.

7. Run `cargo test -p ai-upcycler-core --lib` before moving on. Existing
   `GenerationChunk` literals all use `..Default::default()`, so the new field
   should not break them.

## Task 2 — Secret store (TDD)

**Files:** `core/src/secrets.rs`, `core/src/lib.rs`.

1. Write failing tests for: set/get/delete round-trip; missing key returns
   `None`; unknown provider id is rejected; values never appear in `Debug`
   output of a status object.

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SecretScope { Provider(ProviderId), LanToken }
```

   For sub-project A only `Provider(id)` is used; `LanToken` is reserved so B
   does not have to rename the key namespace.

2. Implement:

```rust
pub trait SecretStore: Send + Sync + 'static {
    fn get(&self, scope: SecretScope) -> Option<String>;
    fn set(&self, scope: SecretScope, value: &str) -> Result<(), String>;
    fn delete(&self, scope: SecretScope) -> Result<(), String>;
}
```

   `KeyringSecretStore` uses `keyring::Entry::new("ai-upcycler", &key)` where
   `key` is `provider:openai` or `lan-token`. `get()` maps a missing entry to
   `None`, warns at `debug` without the value on other errors, and never
   returns a placeholder. `UnconfiguredSecretStore` always returns `None` and
   `Err("no OS secret store available")` for writes (tests inject
   `MemorySecretStore` instead).

   Add `#[derive(Debug)]` only on the store structs, never on values. Add a
   unit test asserting `format!("{:?}", store)` contains no `sk-` marker.

## Task 3 — Provider descriptors and registry (TDD)

**Files:** `core/src/providers/mod.rs`, `core/src/lib.rs`.

1. Write a table test covering every resolution rule before implementing:

| request `model` | request `provider` | result |
|---|---|---|
| `llama-8b` | `None` | local lookup (unchanged) |
| `openai:gpt-4o` | `None` | `Cloud(openai, gpt-4o)` |
| `gpt-4o` | `Some("openai")` | `Cloud(openai, gpt-4o)` |
| `openai:gpt-4o` | `Some("openai")` | `Cloud(openai, gpt-4o)` |
| `openai:gpt-4o` | `Some("anthropic")` | `BadRequest` mismatch |
| `openai:unknown` | `None` | `BadRequest` unknown model |
| `some-local.gguf` | `None` | local lookup (unchanged 404 if absent) |
| `gpt-4o` | `None` | local lookup (unchanged 404 if absent) |

2. Implement the types:

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ProviderId { OpenAI, Anthropic, Gemini, Kimi }

impl ProviderId {
    pub const ALL: [Self; 4] = [Self::OpenAI, Self::Anthropic, Self::Gemini, Self::Kimi];
    pub fn as_str(self) -> &'static str { /* openai | anthropic | gemini | kimi */ }
    pub fn parse(value: &str) -> Option<Self> { /* case-insensitive */ }
}

pub struct ProviderModel {
    pub id: &'static str,
    pub label: &'static str,
    pub supports_vision: bool,
}

pub struct ProviderDescriptor {
    pub id: ProviderId,
    pub name: &'static str,
    pub base_url_env: &'static str,
    pub default_base_url: &'static str,
    pub models: &'static [ProviderModel],
    pub supports_tools: bool, // false for all four in A
}

#[derive(Debug, Clone, Serialize)]
#[derive(Debug, Clone, Serialize)]
pub struct ProviderModelWire {
    pub id: String,
    pub label: String,
    pub supports_vision: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct ProviderStatus {
    pub id: ProviderId,
    pub name: &'static str,
    pub configured: bool,
    pub models: Vec<ProviderModelWire>,
    pub supports_vision: bool,
    pub supports_tools: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CloudRoute { pub provider: ProviderId, pub model: String }

pub trait CloudProvider: Send + Sync + 'static {
    fn id(&self) -> ProviderId;
    fn descriptor(&self) -> &'static ProviderDescriptor;
    fn chat_stream(&self, request: GenerationRequest, api_key: &str)
        -> BoxStream<'static, Result<GenerationChunk, String>>;
}
```

   Static catalogs (kept in code, no network calls):
   - OpenAI: `gpt-4o`, `gpt-4o-mini`, `gpt-4.1`, `gpt-4.1-mini` (vision true).
   - Anthropic: `claude-3-5-sonnet-latest`, `claude-3-5-haiku-latest`,
     `claude-3-7-sonnet-latest` (vision true).
   - Gemini: `gemini-2.0-flash`, `gemini-2.5-flash`, `gemini-2.5-pro`
     (vision true).
   - Kimi: `moonshot-v1-8k`, `kimi-k2-turbo-preview` (vision false in A).

   `ProviderRegistry::new(secrets)` builds all four adapters with base urls
   from `<ID>_BASE_URL` env vars falling back to descriptor defaults. Add
   `with_base_url(id, url)` for tests. `statuses()` reads `secrets.get()` only
   to set `configured`; it never includes the value.

   `resolve()` returns `Ok(None)` for local ids and `Ok(Some(route))` for
   cloud ids. Parse a prefix only when it matches a known provider id, so a
   local GGUF id containing a colon is still looked up locally.

3. `ProviderRegistry::stream(&self, route, request)` fetches the secret,
   returns an error stream stating `<provider> is not configured` when absent,
   and otherwise calls the adapter. `ProviderRegistry::test(id, model)` runs a
   one-token stream with a fixed prompt and returns only a truncated preview.

## Task 4 — OpenAI and Kimi adapters (TDD)

**Files:** `core/src/providers/openai_compat.rs`, `openai.rs`, `kimi.rs`,
`core/tests/providers.rs`.

1. Add `spawn_mock_provider(app: Router) -> (String, JoinHandle<()>)` to
   `core/tests/common/mod.rs` using `tokio::net::TcpListener::bind("127.0.0.1:0")`,
   `axum::serve`, and a broadcast shutdown channel. Reuse the pattern in
   `serve_mock_identity`.

2. Write failing tests that assert:
   - `Authorization: Bearer sk-test-openai` is sent.
   - Body contains `stream: true` and the bare model `gpt-4o`, not
     `openai:gpt-4o`.
   - Two SSE `delta.content` fragments reassemble to `Hello from OpenAI`.
   - A final chunk carries `finish_reason: Some("stop")` and
     `source: Some("openai")`.
   - A non-200 response becomes a stream error whose message contains the
     provider id and HTTP status but never the request body/key.
   - Kimi uses the same shape with `ProviderId::Kimi` and its default base url.

3. Implement the shared helper:

```rust
pub(crate) fn openai_compat_stream(
    client: reqwest::Client,
    base_url: &str,
    provider: ProviderId,
    api_key: &str,
    request: GenerationRequest,
) -> BoxStream<'static, Result<GenerationChunk, String>> {
    let url = format!("{}/chat/completions", base_url.trim_end_matches('/'));
    // POST JSON: model (bare id), messages, temperature, max_tokens,
    // stream: true, plus stop/top_p/seed when present. Never send tools.
    // Parse each `data:` line as JSON; ignore `[DONE]`; forward
    // choices[0].delta.content and choices[0].finish_reason.
    // Emit one chunk per content fragment with tokens_emitted: 1.
}
```

   Use `reqwest::Client::builder().no_proxy()` in tests only; production keeps
   normal proxy behavior. SSE parsing must handle multiple `data:` lines in one
   byte chunk and split lines that span byte chunks.

## Task 5 — Anthropic adapter (TDD)

**Files:** `core/src/providers/anthropic.rs`, `core/tests/providers.rs`.

1. Mock server asserts headers `x-api-key: sk-test-anthropic` and
   `anthropic-version: 2023-06-01`; body has top-level `system` (concatenated
   system messages), `messages` with roles limited to `user`/`assistant`, and
   `stream: true`.

2. Mock SSE events:

```text
event: message_start
data: {"type":"message_start","message":{"usage":{"input_tokens":9}}}

event: content_block_delta
data: {"type":"content_block_delta","delta":{"type":"text_delta","text":"Hello "}}

event: content_block_delta
data: {"type":"content_block_delta","delta":{"type":"text_delta","text":"from Anthropic"}}

event: message_delta
data: {"type":"message_delta","delta":{"stop_reason":"end_turn"},"usage":{"output_tokens":4}}

event: message_stop
data: {"type":"message_stop"}
```

3. Assert the collected chunks give `prompt_tokens == Some(9)`, content
   `Hello from Anthropic`, `finish_reason == Some("end_turn")`, and
   `source == Some("anthropic")`. Implement the adapter behind those tests;
   payload shape errors become stream errors without echoing the API key.

## Task 6 — Gemini adapter (TDD)

**Files:** `core/src/providers/gemini.rs`, `core/tests/providers.rs`.

1. Mock server asserts `x-goog-api-key`, URL suffix
   `/v1beta/models/gemini-2.5-flash:streamGenerateContent?alt=sse`, and body
   `contents` with `role: "user"` / `"model"` and `parts[].text`. System
   messages are prefixed to the first user part.

2. Mock chunks with `candidates[0].content.parts[0].text` and a final
   `usageMetadata` (`promptTokenCount`, `candidatesTokenCount`). Assert
   reassembled content, token counts, `finish_reason: Some("stop")`, and
   `source: Some("gemini")`.

3. Implement behind the tests. Do not use API keys in the query string;
   `x-goog-api-key` keeps it out of URLs and logs.

## Task 7 — Routing policy and app wiring (TDD)

**Files:** `core/src/routing.rs`, `core/src/api/mod.rs`, `core/src/store/mod.rs`,
`core/src/api/models.rs`, `core/tests/providers.rs`.

1. Add `RoutePolicy` in `core/src/routing.rs`:

```rust
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RoutePolicy { Auto, Local, Cloud(ProviderId) }

impl RoutePolicy {
    pub fn parse(provider: Option<&str>) -> Result<Self, String> {
        match provider.map(str::trim) {
            None | Some("") | Some("auto") => Ok(Self::Auto),
            Some("local") => Ok(Self::Local),
            Some(value) => ProviderId::parse(value)
                .map(Self::Cloud)
                .ok_or_else(|| format!("unknown provider '{value}'")),
        }
    }
}
```

   `chat.rs` combines this with `ProviderRegistry::resolve(&model, provider)`;
   model-prefix inference is used only when `provider` is absent.

2. Add to `AppState`:

```rust
pub secrets: Arc<dyn crate::secrets::SecretStore>,
pub providers: Arc<crate::providers::ProviderRegistry>,
```

   `AppState::new` initializes both with `UnconfiguredSecretStore`. Add
   `with_secrets(secrets)` (rebuilds the registry) and `with_providers(registry)`
   builders. `bootstrap` uses `KeyringSecretStore::default()` and calls
   `.with_secrets(...)`.

3. Add `StoreError::Conflict(String)` and:

```rust
pub const VIRTUAL_MODEL_PATH_PREFIX: &str = "virtual:";

pub fn ensure_virtual_model(&self, model_id: &str, provider: &str)
    -> Result<(), StoreError>
{
    if let Some(existing) = self.get_model(model_id)? {
        if !existing.local_file_path.starts_with(VIRTUAL_MODEL_PATH_PREFIX) {
            return Err(StoreError::Conflict(format!(
                "model id '{model_id}' is already a local GGUF"
            )));
        }
    }
    self.upsert_model(&NewModel {
        model_id: model_id.into(),
        file_name: model_id.into(),
        quantization_format: "cloud".into(),
        total_layers: 0,
        weight_size_bytes: 0,
        local_file_path: format!("{VIRTUAL_MODEL_PATH_PREFIX}{provider}"),
    })
}
```

   `models::list_models` filters out rows whose `local_file_path` starts with
   the virtual prefix. Add tests: virtual row is created on a cloud request;
   `/v1/models` does not list it; a local model colliding with a virtual id is
   rejected with `409`.

4. Write routing tests through the in-process router for:
   - `provider: "local"` with the stub backend → stub reply (offline mode kept).
   - `provider: "openai"` with key set → mock cloud reply; `source` telemetry
     is `openai`.
   - `provider: "openai"` with no key → `400` naming the provider, no session
     written.
   - `provider: "openai"` with `tools` → `400` naming the host engine, no
     session written.
   - `Auto` + stub backend + no cloud → `400` naming local engine and cloud
     providers.
   - `Auto` + configured cloud + stub backend → cloud reply.
   - `Auto` + `is_real()` local backend → local reply, cloud not called.
   - local id `llama-8b` still uses `store.get_model()` and keeps the existing
     `404` message when absent.

## Task 8 — Chat handler integration (TDD)

**Files:** `core/src/api/chat.rs`, `core/tests/api.rs`,
`core/tests/providers.rs`.

1. Add `#[serde(default)] pub provider: Option<String>` to
   `ChatCompletionRequest`. Update the local `request()` test helper and every
   struct literal in `core/tests/api.rs` with `provider: None`.

2. Replace the unconditional model lookup in `chat_completions` with:

```rust
let policy = RoutePolicy::parse(request.provider.as_deref())
    .map_err(ApiError::BadRequest)?;
let cloud = state.providers.resolve(&request.model, request.provider.as_deref())
    .map_err(ApiError::BadRequest)?;

if request.wants_tools() && !matches!(policy, RoutePolicy::Auto | RoutePolicy::Local) {
    return Err(ApiError::BadRequest(
        "tool calls require a local host engine (llama-server); cloud providers \
         cannot call tools in this phase".into(),
    ));
}
if request.wants_tools() && !state.backend.tool_calling_available() {
    return Err(ApiError::BadRequest(/* existing host-engine message */));
}

let cloud = match (policy, cloud) {
    (RoutePolicy::Cloud(_), Some(route)) => Some(route),
    (RoutePolicy::Cloud(id), None) => return Err(ApiError::BadRequest(/* unknown model for provider */)),
    (RoutePolicy::Local, _) => None,
    (RoutePolicy::Auto, Some(route)) if !state.backend.is_real() => Some(route),
    (RoutePolicy::Auto, _) if !state.backend.is_real() => {
        return Err(ApiError::BadRequest(/* no local engine and no cloud configured */));
    }
    (RoutePolicy::Auto, _) => None,
};

let (generation_model, response_model) = if let Some(route) = &cloud {
    state.store.ensure_virtual_model(&request.model, route.provider.as_str())
        .map_err(|e| match e {
            crate::store::StoreError::Conflict(msg) => ApiError::Conflict(msg),
            other => other.into(),
        })?;
    (route.model.clone(), request.model.clone())
} else {
    let model = state.store.get_model(&request.model)?
        .ok_or_else(|| ApiError::NotFound(format!("model '{}'", request.model)))?;
    (model.model_id.clone(), model.model_id)
};
```

   Note: `resolve()` already returns `None` for local ids; with an explicit
   `provider`, a local id is a `BadRequest`, not a silent local fallback.

3. `resolve_session` keeps using `request.model` as the session model id so
   history shows `openai:gpt-4o`. Pass `response_model` into
   `buffered_response` / `stream_response` for the OpenAI envelope; pass
   `generation_model` into `GenerationRequest.model` for the adapter.

4. `publish_metrics` takes `source: &str`. In both response modes track the
   first non-empty `chunk.source` (default `"local"`) and pass it to every
   metrics event. Add tests asserting a cloud stream emits
   `data.source == "openai"` and a cluster stream emits `adb:<serial>`.

5. Update `core/tests/api.rs`: tests that called the stub under default `Auto`
   now set `provider: Some("local".into())` to express offline mode. Keep one
   test asserting `Auto` + stub + no cloud returns `400` and writes no session.

## Task 9 — HTTP provider and secret endpoints (TDD)

**Files:** `core/src/api/providers.rs`, `core/src/api/mod.rs`,
`core/tests/providers.rs`.

1. Write failing router tests first:
   - `GET /api/providers` → 200, four entries, `configured: false` with
     `UnconfiguredSecretStore`; body never contains `sk-`.
   - After `PUT /api/settings/secrets/openai` with
     `{"value":"sk-super-secret"}`, `GET /api/providers` shows
     `configured: true` but the body still does not contain the key.
   - `DELETE` flips it back; repeated delete is idempotent.
   - Unknown provider path → `404`; malformed body → `400`.
   - `POST /api/providers/openai/test` with a mock base url → 200 result only;
     without a key → `400` naming the provider.

2. Implement handlers:

```rust
pub async fn list(State(state): State<AppState>) -> ApiResult<Json<ProviderList>>;
pub async fn test(State(state): State<AppState>, Path(id): Path<String>,
                  Json(body): Json<ProviderTestRequest>) -> ApiResult<Json<ProviderTestResult>>;
pub async fn put_secret(State(state): State<AppState>, Path(id): Path<String>,
                        Json(body): Json<SecretValue>) -> ApiResult<Json<ProviderStatus>>;
pub async fn delete_secret(State(state): State<AppState>, Path(id): Path<String>)
    -> ApiResult<Json<ProviderStatus>>;
```

   `SecretValue` is `{ value: String }`. Reject empty values with `400`.
   Responses contain only provider id/name/configured/capabilities and a
   truncated test preview. Never log `body.value`.

3. Register routes in `router()`:

```rust
.route("/api/providers", get(providers::list))
.route("/api/providers/:id/test", post(providers::test))
.route("/api/settings/secrets/:provider",
       put(providers::put_secret).delete(providers::delete_secret))
```

   Import `put` from `axum::routing` and add `Method::PUT` to `cors_layer()`'s
   `allow_methods` or the dashboard cannot save a key from the browser.

## Task 10 — Documentation and final verification

**Files:** `docs/api.md`, `docs/AGENTS.md`.

1. `docs/api.md`: add §2.4 `provider` field and virtual model ids, §2.5
   provider endpoints with sample responses using `sk-...` placeholders, and
   a sentence in the telemetry section that `generation_metrics.data.source`
   is `local`, `adb:<serial>`, or a provider id.
2. `docs/AGENTS.md`: add one short note that agent requests should omit
   `provider`; loopback remains unauthenticated in this phase and the LAN
   listener is still out of scope. Do **not** weaken §8.
3. Full verification, with build artifacts off the full C drive:

```powershell
$env:RUSTUP_TOOLCHAIN = "stable-x86_64-pc-windows-gnu"
$env:CARGO_TARGET_DIR = "D:\ai-upcycler-target"
$env:PATH = "C:\msys64\mingw64\bin;$env:PATH"
$env:npm_config_cache = "D:\ai-upcycler-npm-cache" # only for npx skills

cargo fmt --all -- --check
cargo test -p ai-upcycler-core
cargo clippy -p ai-upcycler-core --all-targets -- -D warnings
cargo build --release -p ai-upcycler-desktop
```

4. Manual smoke test with the real host engine (optional but recommended):
   start the desktop app with the established env vars, set a fake provider
   key through `PUT /api/settings/secrets/openai`, then confirm
   `provider: "local"` returns the real 3B answer and `provider: "openai"`
   returns a clear unconfigured/upstream error without touching the engine.
   Playwright UI verification belongs to sub-project B, which adds the
   provider switcher.

5. Commit:

```bash
git add core docs/api.md docs/AGENTS.md
git commit -m "feat: add cloud provider bridge and routing"
```

## Self-review

- Spec §6 covered: trait, four adapters, three endpoint groups, `AppState`
  fields, virtual model ids, mock-server tests, no real keys in CI.
- Spec §5 covered: one `GenerationBackend` contract, `RoutePolicy`, tool turns
  host-only, `source` telemetry, keychain secrets. No LAN listener here by
  design; it belongs to B.
- Agent compatibility: requests without `provider` use `Auto`. With a real
  local backend they are unchanged; with no engine they now fail loudly rather
  than returning a stub, which follows spec §5.2. Explicit `provider: "local"`
  preserves the old placeholder behavior.
- Persistence: virtual rows satisfy the existing FK without a table rebuild;
  `/v1/models` remains local-only; unknown local ids keep the same `404`.
- Security: keys are write-only, never in SQLite/logs/GET bodies; CORS gains
  `PUT` only for same-origin desktop origins; loopback stays unauthenticated.
- DRY: OpenAI and Kimi share `openai_compat`; all adapters emit the same
  `GenerationChunk`. No new frontend work in this plan.
- Risks: `keyring` needs a native desktop store; unconfigured providers fail
  closed. SSE edge cases are covered by split-line mock tests. `Auto`'s
  `is_real()` change requires updating stub-based tests, listed in tasks 7–8.
