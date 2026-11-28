# PrivateLM-style Extension — Design

Date: 2026-11-27
Status: accepted
Supersedes: nothing
Related: `2026-10-24-agent-api-compatibility-design.md`,
`2026-11-03-wireless-device-onboarding-design.md`, `docs/api.md`,
`docs/AGENTS.md`, `docs/security.md`

## 1. Summary

Bring the user experience demonstrated by `orailnoor/cross-platform-llm-client`
(PrivateLM) and its demo video into AI Upcycler without replacing the working
Rust engine, mTLS phone cluster, or OpenAI-compatible agent endpoint.

The reference demo turns spare phones into a wireless GPU cluster that runs a
large GGUF model for AI agents. AI Upcycler already has the cluster half. What
it lacks is the *client* half: cloud-provider choice, a real phone chat client,
multimodal input, auto-configuration, task workflows, and image generation.
This spec defines those six sub-projects and the shared foundation they need.

Scope decision: extend this repository, on both surfaces (desktop and Android).
Approach decision: core-first modular extension, not a separate Flutter client
and not a frontend-only reskin. Sub-project order: **A → B → C → D → E → F**.

## 2. Goals

- Keep one source of truth for routing, credentials, sessions, and telemetry.
- Let a user choose local cluster / host engine / cloud provider per request,
  with an automatic default.
- Make the Android device a real chat client while it keeps serving as a worker.
- Accept images in chat and route them to vision-capable backends.
- Recommend context/token settings from measured hardware and persist them.
- Show multi-step tasks with live step status.
- Generate images through pluggable backends, cloud first.

## 3. Non-goals

- iOS workers and an iOS client. The reference video shows iPhone/iPad, but iOS
  remains deferred in this program.
- Replacing the loopback agent endpoint. Existing opencode/Kilo Code config
  keeps working unchanged.
- Distributed tool calling. Tool turns stay on the host engine.
- On-device image generation in the first release (feature-flagged later).
- Billing, accounts, or provider SDKs that require a cloud account owned by us.

## 4. What exists today

| Piece | Where | State |
|---|---|---|
| Streaming generation contract | `core/src/api/backend.rs` (`GenerationBackend`, `GenerationChunk`) | works |
| OpenAI-compatible chat + sessions | `core/src/api/chat.rs`, `core/src/store/mod.rs` | works |
| Host `llama-server` engine | `core/src/api/llama.rs`, `runtime/engine/` | works, verified |
| Android mTLS worker + cluster | `core/src/security.rs`, `mobile-worker/android/` | works, verified on-device |
| Live telemetry bus + WebSocket | `core/src/telemetry.rs`, `core/src/api/ws.rs` | works |
| Master RAM source + health cache | `core/src/worker_health.rs` | works |
| React dashboard + chat | `src/components/`, `src/store/cluster.ts` | works |
| Android control UI | `mobile-worker/.../MainActivity.kt` (plain Views) | works, worker-only |
| Cloud providers, secrets, multimodal, workflows, imagegen | — | **missing** |
| LAN-reachable REST API | — | **missing** (loopback only today) |

Two implementation facts constrain the design:

1. `ChatMessage.content` is a `String`, and agents rely on `null` becoming `""`.
   Any multimodal change must preserve that behavior.
2. The REST API binds to `127.0.0.1` with no authentication
   (`docs/AGENTS.md` §8). A phone client cannot reach it as-is.

## 5. Shared foundation

### 5.1 One streaming contract

`GenerationBackend` stays the only interface the HTTP layer knows. New backends
implement it and emit `GenerationChunk`, so chat handlers, persistence, and
telemetry do not branch per provider.

```
            ┌──────────────┐
 request →  │ RouterBackend│  (implements GenerationBackend)
            └──────┬───────┘
       ┌───────────┼────────────┐
       ▼           ▼            ▼
  host engine   cluster     cloud adapters
  (llama)       workers     (OpenAI/Anthropic/Gemini/Kimi)
```

### 5.2 Routing policy

New `core/src/routing.rs` defines:

```rust
enum RoutePolicy { Auto, Local, Cloud(ProviderId) }
```

- `Auto` (default): local host engine when tools are present; otherwise a ready
  cluster worker when one is assigned; otherwise a configured cloud provider;
  otherwise a clear `400` naming what is missing.
- `Local` / `Cloud(id)`: explicit overrides from a request field
  (`provider`) or a UI setting.
- Tool calls remain host-only. Cloud adapters may support tools later, but that
  is a separate decision; `Auto` never routes a tool turn to the cluster.

The selected source is attached to every `generation_metrics` event as a
`source` field (`local`, `adb:<serial>`, or `<provider>`), which is how the
dashboard proves where work actually ran.

### 5.3 Secrets

New `core/src/secrets.rs` exposes `SecretStore` with `get/set/delete` keyed by
provider id. Backends: OS keychain on desktop (Windows Credential Manager,
macOS Keychain, libsecret on Linux), Android Keystore on the phone. Values are
never written to SQLite, logs, telemetry, or any GET response. If no OS store
is available, the provider is reported as unconfigured rather than falling
back to plaintext.

### 5.4 LAN API (security-model change)

`docs/AGENTS.md` §8 currently forbids exposing port 8080. This spec changes
that deliberately:

- A new opt-in listener, enabled only by `AI_UPCYCLER_LAN_API=1` (or an explicit
  dashboard toggle), binds to the LAN interface on a configurable port.
- It requires a bearer token derived during pairing and stored alongside the
  enrollment. Loopback stays open and unauthenticated for existing agents.
- The phone finds the host through a new `_aiupcycler-master._tcp` mDNS advert,
  with a manual `host:port` field as fallback (mDNS does not cross subnets).
- The token is revoked when a device is unpaired, and rotates when the CA is
  rotated. A rejected token yields `401` with no model or session detail.
- No proxy, tunnel, or firewall rule is used to expose it. Default builds are
  unchanged.
- `docs/AGENTS.md` §8 and `docs/api.md` must be updated in the same change that
  introduces the listener.

## 6. Sub-project A — Cloud provider bridge *(first)*

New modules:

- `core/src/providers/mod.rs` — `CloudProvider` trait:
  `chat_stream(&GenerationRequest) -> BoxStream<Result<GenerationChunk, String>>`,
  `list_models()`, `supports_vision()`, `supports_tools()`.
- `core/src/providers/openai.rs`, `anthropic.rs`, `gemini.rs`, `kimi.rs` —
  SSE and JSON response normalization into `GenerationChunk`; each maps its own
  auth header and error shape.
- `core/src/api/providers.rs` — HTTP surface:
  - `GET /api/providers` — configured providers, model ids, capabilities,
    configured/not-configured status (never secrets).
  - `POST /api/providers/{id}/test` — one short completion, result only.
  - `PUT|DELETE /api/settings/secrets/{provider}` — write-only key material.
- `AppState` gains `providers: Arc<ProviderRegistry>` and
  `secrets: Arc<dyn SecretStore>`.
- Cloud model ids are virtual: the existing `store.get_model()` lookup must be
  skipped when the route is `Cloud(id)` and the id resolves in that provider's
  catalog. Convention: `provider:model` (e.g. `openai:gpt-4o`) disambiguates
  collisions with local GGUF ids. Local ids keep their current resolution and
  error messages; agent requests without a `provider` field are unchanged.

Testing: one mock HTTP server per adapter, routing policy table tests, and a
test that secrets never appear in any response body. No real keys in CI.

## 7. Sub-project B — Mobile chat client

Android (`mobile-worker/android/app/src/main/java/org/aiupcycler/worker/`):

- Add Jetpack Compose screens: session list, chat thread, provider/model picker,
  provisioning progress, and a source badge (`local` / worker / provider).
- A LAN REST client (OkHttp) uses the pairing-derived bearer token. Cloud keys
  are written through Android Keystore only.
- `WorkerRuntime`/`WorkerForegroundService` stay unchanged: the gRPC server and
  mDNS advert continue to run independently of the UI process.
- Keep the existing control/pairing screen reachable from an overflow menu.

Desktop (`src/components/ChatView.tsx`, `src/store/cluster.ts`):

- Provider switcher in the chat header; reuse the same session API.
- Show the `source` badge from telemetry so split work is visible.

Both surfaces read/write the same SQLite sessions; no client-side session copy.

## 8. Sub-project C — Multimodal chat

- `ChatMessage.content` becomes an untagged enum: `Text(String)` or
  `Parts(Vec<ContentPart>)`, with `text()` and `has_images()` helpers. A plain
  string and a missing/`null` content keep their current behavior exactly.
  This is the highest-risk change; it gets its own compatibility tests with
  real agent-shaped payloads before any UI work; the change stays
  feature-gated until those tests pass.
- New `core/src/media/` stores images content-addressed under the app data dir;
  new `attachments` table in `store/schema.sql` links them to messages.
- Cloud adapters map image parts to their native formats (base64 data URLs for
  OpenAI/Gemini/Kimi, `source.base64` for Anthropic).
- The cluster path returns an explicit error if no vision-capable model is
  loaded; it never silently drops images.
- Image attachment chips in both composers.

## 9. Sub-project D — Auto-config + persistence

- New `core/src/autoconfig.rs`: combines `host_memory`, worker health from
  `TelemetryCache`, and model layer count to recommend `context` and
  `max_tokens`. Expose `GET /api/settings/recommendations`.
- New `settings` table (key/value JSON) in `store/schema.sql`, with
  `GET|PUT /api/settings`.
- Session list/rename/delete endpoints on the existing `chat_sessions` table;
  both clients consume them.
- Android keeps only UI preferences locally (DataStore); conversation history
  stays on the host.

## 10. Sub-project E — Task workflow view

- New `core/src/workflows/` with a small DAG executor:
  `WorkflowStep { id, prompt, depends_on, provider }`. Steps run through
  `RouterBackend`; the executor is deterministic and testable without a model.
- New `workflow_runs` and `workflow_steps` tables; WS events
  `workflow_step_started` / `workflow_step_completed`.
- REST: `POST /api/workflows`, `GET /api/workflows`, `GET /api/workflows/{id}`.
- Desktop gets an editable task timeline; the phone gets read-only progress in
  this phase.

## 11. Sub-project F — Image generation

- New `core/src/imagegen/` with `ImageBackend` and cloud-backed adapters.
- REST: `POST /api/images/generate`, `GET /api/images/{id}`; generated files
  live in the same content-addressed media store as chat attachments, with an
  `images` table for metadata.
- Desktop gallery + generator panel; phone gallery first.
- On-device generation stays behind `AI_UPCYCLER_IMAGEGEN_LOCAL=1` and is a
  follow-up: it needs a new engine (e.g. stable-diffusion.cpp) and is the
  largest unknown in this program.

## 12. UI/UX direction

Chosen with `ui-ux-pro-max` (used for every UI task in this program). Verified
matches: "Chat & Messaging App" and "Developer Tool / IDE" product profiles,
"Dark Mode (OLED)" style, developer typography, and the React / Jetpack Compose
stack guidelines. The landing-page pattern returned by the design-system search
was off-topic for an app shell and was discarded rather than persisted.

- **Visual language:** Dark Mode (OLED) + Minimalism/Swiss Style with restrained
  micro-interactions. Keep the existing Fira Code / Fira Sans pairing from
  `index.html`; do not introduce a second font family. Adopt semantic tokens:
  background `#0F172A`, card `#1B2336`, border `#475569`, muted text
  `#94A3B8`, accent `#22C55E` ("running/ready"), destructive `#EF4444`.
  Always check contrast at 4.5:1 for body text in both themes; keep dark as the
  default and never ship a light-only state.
- **Chat surface:** streaming text token by token, never a spinner for a
  multi-second generation. Use a stable skeleton/`aria-busy` for short waits.
  Sender/receiver bubbles must be distinguishable by more than color, with a
  typing grey that is still legible. Stable message ids as list keys — never
  array indexes.
- **Desktop (React):** stable keys for dynamic lists, generic list components
  with typed props, visible focus ring on every interactive control including
  modal controls, `prefers-reduced-motion` respected, hover/press transitions
  150–300 ms. Verify at 375 / 768 / 1024 / 1440 px.
- **Android (Jetpack Compose):** `LazyColumn` with stable `key` for chat history
  (never `Column.verticalScroll`), `Row`/`Column` for simple linear layouts,
  vector icons from one family, 48 dp minimum touch targets, 4/8 dp spacing
  rhythm, safe-area/status-bar clearance, and a pressed state that does not
  shift layout bounds.
- **Icons and a11y:** no emoji as structural icons; decorative icons hidden
  from the accessibility tree; icon-only controls have accessible names;
  color is never the only state indicator; disabled states are visibly
  disabled; reduced motion and large text do not break layout.
- The desktop gallery, task timeline, and phone chat screens all reuse these
  tokens. A `design-system/ai-upcycler/MASTER.md` will be generated and read
  before building each surface in sub-project B.

## 13. Verification strategy

Each sub-project follows tests-first implementation:

1. Unit tests with mock HTTP servers / injected traits (no live provider keys).
2. API smoke tests through the real axum router.
3. Desktop UI verification with Playwright MCP (the browser surface of the
   Tauri app points at `127.0.0.1:8080` for data).
4. Android verification over the existing wireless ADB tunnel
   (`192.168.1.9:5555`) with screenshots.
5. No phase is called complete without running the checks.

## 14. Risks

| Risk | Mitigation |
|---|---|
| Multimodal enum breaks agent clients | Compatibility tests first; string payloads unchanged; ship C last among the UI features if needed. |
| LAN listener widens attack surface | Opt-in, token-authenticated, loopback unchanged, docs updated in the same change. |
| Provider APIs change | Thin adapters behind one trait; mock-server tests catch drift. |
| Android Compose rewrite destabilizes worker | Screens are additive; foreground service untouched; worker readiness re-verified after each UI change. |
| Disk space on the dev machine is exhausted | Keep build artifacts on `D:\ai-upcycler-target`; clean shell output under `C:\Users\darsh\.local\share\opencode\shell` before large builds. |
