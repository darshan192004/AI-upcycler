# PrivateLM B — Mobile Chat Client Implementation Plan

Date: 2026-11-27
Status: ready for execution
Spec: `docs/superpowers/specs/2026-11-27-privatelm-style-extension-design.md` §5.4, §7, §12–§14
Prior plan: `docs/superpowers/plans/2026-11-27-cloud-provider-bridge.md` (sub-project A)
Follow-on plan: `docs/superpowers/plans/2026-11-27-privatelm-d-autoconfig-persistence.md` (sub-project D)

For agentic workers: execute tasks in order from `D:\ai-upcycler-worktrees\cloud-provider-bridge`. Each task is red → green → checkpoint. Do not start Android UI work until Task 1's prerequisite gate passes. Do not modify `WorkerRuntime` or `WorkerForegroundService`. Do not push. Commit only when the operator asks.

## Goal

Add the mobile chat client for sub-project B: Android Jetpack Compose surfaces for sessions, streaming chat, provider/model selection, provisioning progress, and execution-source badges; a bearer-authenticated LAN REST client that uses the pairing-derived token; Android Keystore-backed local secret storage; and the opt-in LAN listener, mDNS advert, and security-model change that make the phone client possible.

The desktop chat also gains the provider switcher and telemetry source badge. Both clients read and write the same host SQLite sessions through one read-only session API added here; neither client keeps a local session database.

This plan covers sub-project **B only**. Sub-project A's cloud provider bridge must land first. Sub-project D later adds settings, session rename/delete, and Android UI preferences; B must not duplicate those writes.

## Architecture

- **Host core:** The existing loopback axum API stays on `127.0.0.1` and stays unauthenticated. A second, opt-in LAN listener binds only to the resolved LAN IPv4 address when `AI_UPCYCLER_LAN_API=1`. Its router is an explicit allowlist, not the loopback router: chat completions, models, providers, secret writes, read-only sessions, and provisioning status. A per-device bearer token is generated at pairing; only its SHA-256 hash is kept on the host in the trust directory. Verification is constant-time. A missing, malformed, revoked, or rotated token returns `401` with a body containing only `{"error":{"message":"unauthorized",...}}`.
- **Discovery:** When the LAN listener is enabled, core advertises `_aiupcycler-master._tcp.local.` with public TXT records only (`version`, `node_id`, `api_port`, `fingerprint`). The token is never advertised. Android browses that type and also accepts a manual `host:port`.
- **Android:** `ChatActivity` becomes the launcher. `MainActivity` remains unchanged in behavior and is opened from ChatActivity's overflow menu. `WorkerForegroundService.runtime(context)` is not changed; ChatActivity only ensures the service is started and reads non-runtime identity/credentials. The LAN client is OkHttp-based, cancels cleanly, parses SSE deltas and the final `source`, and sends the bearer token only to the resolved master endpoint. Provider keys and the bearer token are stored only in EncryptedSharedPreferences whose master key is in Android Keystore; no plaintext secret is written to files, DataStore, SQLite, or logs.
- **Desktop:** `ChatView` uses `GET /api/providers` from A for provider/model choice, `GET /api/sessions` and `GET /api/sessions/:id/messages` from B for shared session state, and telemetry `generation_metrics.data.source` from A for the source badge. Messages in React state are a render cache only; there is no browser-storage copy.
- **Sessions:** B adds read-only `GET /api/sessions`, `GET /api/sessions/:id`, and `GET /api/sessions/:id/messages`. D later adds `PUT` rename, `DELETE`, settings, and preferences on top of the same `core/src/api/sessions.rs` module.

## Tech Stack

- Rust / axum 0.7 / tokio for core API and LAN listener.
- `mdns-sd` 0.11 for the host advert; Android `NsdManager` for browsing.
- `sha2` 0.10 plus `subtle` 2.6 for token hashing and constant-time verification; `rand` 0.8 for token generation.
- Kotlin 1.9.24, Android Gradle plugin 8.5.2, Gradle wrapper 8.7, `minSdk = 26`, `targetSdk = 34`.
- Jetpack Compose compiler extension `1.5.14` (pinned to Kotlin 1.9.24), Compose BOM `2024.09.02`, Material 3, lifecycle-compose, OkHttp 4.12.0, `androidx.security:security-crypto:1.1.0-alpha06`.
- React 19, TypeScript, Zustand, Tailwind CSS, Motion, Fira Sans / Fira Code.
- JUnit4 JVM unit tests for Android reducers and request builders; Playwright MCP for desktop; wireless ADB at `192.168.1.9:5555` for Android screenshots.

## Spec

- §5.4: opt-in LAN listener, pairing-derived bearer token, mDNS advert plus manual `host:port`, revocation on unpair, rotation on CA change, rejected token `401` with no detail, loopback unchanged, no proxy/tunnel/firewall rule, docs updated in the same change.
- §7: Android Compose session list, chat thread, provider/model picker, provisioning progress, source badge; OkHttp LAN client; Android Keystore cloud keys; worker runtime/service untouched; control screen in overflow; desktop provider switcher and source badge; shared SQLite sessions.
- §12: exact dark tokens and typography, streaming text without multi-second spinners, stable keys, `LazyColumn`, 48 dp touch targets, safe-area clearance, pressed states that do not shift bounds, visible focus rings, reduced motion, no emoji icons, dark default.
- §13: tests-first, mock HTTP/injected traits, real axum router smoke tests, Playwright against `127.0.0.1:8080`, wireless ADB with screenshots at `192.168.1.9:5555`.
- §14: LAN attack surface and Android rewrite are mitigated by opt-in, token auth, additive screens, runtime re-verification, and D: build artifacts.

## Global Constraints

- Worktree root is `D:\ai-upcycler-worktrees\cloud-provider-bridge`.
- Build artifacts stay on D:. Set these in every PowerShell verification shell:
  ```powershell
  $env:RUSTUP_TOOLCHAIN = "stable-x86_64-pc-windows-gnu"
  $env:CARGO_TARGET_DIR = "D:\ai-upcycler-target"
  $env:PATH = "C:\msys64\mingw64\bin;$env:PATH"
  $env:npm_config_cache = "D:\ai-upcycler-npm-cache"
  $env:GRADLE_USER_HOME = "D:\ai-upcycler-gradle-home"
  ```
  Gradle project cache uses `--project-cache-dir D:\ai-upcycler-gradle-project-cache`. Screenshots go under `D:\ai-upcycler-artifacts\b-mobile\`.
- Before large builds, clean old shell output under `C:\Users\darsh\.local\share\opencode\shell` without deleting the directory.
- No code, build, test, commit, or push is performed while creating this plan.
- No real provider key, bearer token, CA key, or PIN is allowed in tests, fixtures, logs, screenshots, or documentation.
- No client-side session store: no Room, DataStore message keys, `localStorage`, or copied `chat_messages` on Android/desktop. In-memory render state is allowed.
- `WorkerRuntime`, `WorkerForegroundService`, `WorkerProtocol.RPC_PORT`, and the worker mDNS advert are unchanged.
- `MainActivity.handleLaunch`, auto-enroll semantics, and the pairing-only listener on `127.0.0.1:8081` are unchanged.
- No proxy, tunnel, firewall rule, or `usesCleartextTraffic` shortcut for pairing. The only cleartext LAN traffic is the opt-in, bearer-authenticated LAN API; `EnrollmentClient` continues to refuse plain HTTP except loopback.
- All tests are first written to fail for the intended reason, then made green. "Handle errors" is never acceptable: every failure has a named type, state, user message, and verification assertion.

## Dependencies on sub-project A

B does not reimplement these A contracts; Task 1 stops execution if any gate is missing.

| A contract | B uses it for | Gate |
|---|---|---|
| `SecretScope::LanToken` reserved in `core/src/secrets.rs` | Keeps host token naming stable; B's per-device hash lives in the trust store, not in the single-scope keyring | `rg -n "LanToken" core/src/secrets.rs` |
| `GenerationChunk.source`, `GenerationMetrics.source` | Android SSE source badge; desktop telemetry source badge | `rg -n "pub source" core/src/api/backend.rs core/src/telemetry.rs` |
| `GET /api/providers`, `POST /api/providers/:id/test`, `PUT|DELETE /api/settings/secrets/:provider` | Provider/model picker and write-only key entry | `rg -n "settings/secrets" core/src/api/mod.rs` |
| Virtual `provider:model` ids | Desktop and Android model picker | `rg -n "virtual:" core/src/store/mod.rs` |
| CORS `PUT` | Desktop browser can call secret endpoint | `rg -n "Method::PUT" core/src/api/mod.rs` |

## Files

### New

| File | Purpose |
|---|---|
| `core/src/api/sessions.rs` | Read-only session/message handlers |
| `core/src/api/lan.rs` | LAN allowlist router, bearer middleware, config parsing |
| `core/tests/session_reads.rs` | axum router tests for B's read-only session API |
| `core/tests/lan_api.rs` | token generation/revocation/CA-rotation/auth allowlist tests |
| `design-system/ai-upcycler/MASTER.md` | Source of truth for B's tokens, type scale, spacing, and a11y rules |
| `assets/fonts/FiraSans-Regular.ttf`, `assets/fonts/FiraSans-Medium.ttf`, `assets/fonts/FiraCode-Regular.ttf`, `assets/fonts/OFL.txt` | Shared OFL font assets vendored once |
| `mobile-worker/android/app/src/main/java/org/aiupcycler/worker/chat/ChatActivity.kt` | Compose launcher chat surface |
| `mobile-worker/android/app/src/main/java/org/aiupcycler/worker/chat/ChatViewModel.kt` | UI state owner and LAN client orchestration |
| `mobile-worker/android/app/src/main/java/org/aiupcycler/worker/chat/ChatUiState.kt` | Pure state types and reducer |
| `mobile-worker/android/app/src/main/java/org/aiupcycler/worker/chat/LanClient.kt` | OkHttp LAN REST + SSE client |
| `mobile-worker/android/app/src/main/java/org/aiupcycler/worker/chat/LanEndpoint.kt` | Endpoint parsing and storage contract |
| `mobile-worker/android/app/src/main/java/org/aiupcycler/worker/chat/MasterDiscovery.kt` | `_aiupcycler-master._tcp` NSD browser + fingerprint check |
| `mobile-worker/android/app/src/main/java/org/aiupcycler/worker/chat/secrets/KeystoreSecretStore.kt` | EncryptedSharedPreferences secret store |
| `mobile-worker/android/app/src/main/java/org/aiupcycler/worker/chat/secrets/SecretStore.kt` | JVM-testable secret-store interface and memory fake |
| `mobile-worker/android/app/src/main/java/org/aiupcycler/worker/chat/ui/SessionListScreen.kt` | `LazyColumn` session list |
| `mobile-worker/android/app/src/main/java/org/aiupcycler/worker/chat/ui/ChatThreadScreen.kt` | `LazyColumn` streaming thread |
| `mobile-worker/android/app/src/main/java/org/aiupcycler/worker/chat/ui/ProviderPicker.kt` | Local / cloud provider and model selection |
| `mobile-worker/android/app/src/main/java/org/aiupcycler/worker/chat/ui/ProvisioningProgressScreen.kt` | Polls and renders model push progress |
| `mobile-worker/android/app/src/main/java/org/aiupcycler/worker/chat/ui/SourceBadge.kt` | `local` / worker / provider badge |
| `mobile-worker/android/app/src/main/java/org/aiupcycler/worker/chat/ui/ChatTheme.kt` | Dark-first Material 3 theme and token mapping |
| `mobile-worker/android/app/src/main/res/font/*` | Android copies of the Fira assets |
| `mobile-worker/android/app/src/test/java/org/aiupcycler/worker/chat/ChatReducerTest.kt` | Reducer tests |
| `mobile-worker/android/app/src/test/java/org/aiupcycler/worker/chat/LanRequestBuilderTest.kt` | JSON/auth/SSE parsing tests |
| `mobile-worker/android/app/src/test/java/org/aiupcycler/worker/chat/MasterDiscoveryTest.kt` | Endpoint/fingerprint parsing tests |
| `mobile-worker/android/app/src/test/java/org/aiupcycler/worker/chat/SecretStoreContractTest.kt` | Key naming and no-secret-in-debug tests |
| `mobile-worker/android/app/src/androidTest/java/org/aiupcycler/worker/chat/ChatActivitySmokeTest.kt` | On-device Compose smoke checks |
| `public/fonts/*` | Desktop copies of the Fira assets |
| `tests/e2e/chat-b.spec.ts` | Playwright MCP-driven B acceptance script |

### Modified

| File | Change |
|---|---|
| `core/Cargo.toml` | add `rand`, `subtle` |
| `core/src/api/error.rs` | add `Unauthorized` → 401 with fixed message |
| `core/src/api/mod.rs` | register `sessions`/`lan`, add read-only routes, LAN startup with mDNS advert |
| `core/src/api/pairing.rs` | return `lan_token`, `lan_port`, `node_id`; store token hash for all enrollment ids |
| `core/src/api/devices.rs` | revoke LAN token with enrollment |
| `core/src/security.rs` | load/store/revoke/verify LAN token hashes; clear on CA replacement |
| `core/src/trust.rs` | persist token hashes atomically in `lan-tokens.json` |
| `core/src/discovery/mdns.rs` | add master service type, advert builder, LAN IP resolver |
| `mobile-worker/android/app/build.gradle.kts` | Compose, OkHttp, security-crypto, androidTest deps; `composeOptions` |
| `mobile-worker/android/app/src/main/AndroidManifest.xml` | `ChatActivity` launcher; `MainActivity` explicit-intent reachable; cleartext policy change |
| `mobile-worker/android/app/src/main/res/xml/network_security_config.xml` | allow LAN cleartext only for the B LAN API; pairing guard stays in code |
| `mobile-worker/android/app/src/main/res/values/themes.xml` | dark theme parent |
| `mobile-worker/android/app/src/main/res/values/colors.xml` | exact §12 tokens |
| `mobile-worker/android/app/src/main/res/values/strings.xml` | chat/provider/provisioning/source labels |
| `mobile-worker/android/app/src/main/java/org/aiupcycler/worker/pairing/WorkerCredentials.kt` | add `nodeId`, `lanToken`, `lanPort` |
| `mobile-worker/android/app/src/main/java/org/aiupcycler/worker/pairing/CredentialStore.kt` | persist non-secret `node_id`/`lan_port`; route token to Keystore |
| `mobile-worker/android/app/src/main/java/org/aiupcycler/worker/pairing/EnrollmentClient.kt` | parse new enrollment fields; keep transport guard unchanged |
| `src/lib/types.ts` | provider/status types, source field, session types, `ChatRequest.provider` |
| `src/lib/api.ts` | provider, secret, session list/messages calls; SSE source passthrough |
| `src/store/cluster.ts` | keep `latestMetrics.source`; expose provider list state |
| `src/components/ChatView.tsx` | provider switcher, source badge, shared session id/reads |
| `src/index.css` | local Fira `@font-face`, exact §12 CSS variables, focus-ring token |
| `docs/api.md` | LAN API, session reads, new enrollment fields, provider/source fields |
| `docs/security.md` | LAN threat model, token lifecycle, no tunnel/proxy/firewall rule |
| `docs/AGENTS.md` | §8 deliberately narrowed: loopback still forbidden to expose; opt-in LAN API described |

## Task 1 — Dependency gate and frozen design contract

**Files:** read-only gates plus new `design-system/ai-upcycler/MASTER.md`, `src/index.css`, `mobile-worker/android/app/src/main/res/values/colors.xml`, `mobile-worker/android/app/src/main/res/values/themes.xml`, `assets/fonts/*`, `public/fonts/*`.

**Consumes:** A plan and spec §12.
**Produces:** a checked prerequisite gate and one design-token source of truth.

1. Before changing any file, run and record these gates:
   ```powershell
   git status --short
   rg -n "LanToken" core/src/secrets.rs
   rg -n "pub source" core/src/api/backend.rs core/src/telemetry.rs
   rg -n "settings/secrets" core/src/api/mod.rs core/src/api/providers.rs
   rg -n "virtual:" core/src/store/mod.rs
   rg -n "Method::PUT" core/src/api/mod.rs
   rg -n "pub const PAIRING_PORT" core/src/api/mod.rs
   ```
   Every gate must have at least one match. If A is absent, stop and do not continue with B.

2. Write the failing token-gate assertions first as `rg` expectations: exact hex strings `#0F172A`, `#1B2336`, `#475569`, `#94A3B8`, `#22C55E`, `#EF4444`, and the font names `Fira Sans` / `Fira Code` must be absent from the repo's B surfaces until this task adds them.

3. Create `design-system/ai-upcycler/MASTER.md` with:
   - dark background `#0F172A`, card `#1B2336`, border `#475569`, muted text `#94A3B8`, accent `#22C55E`, destructive `#EF4444`;
   - body text contrast rule `>= 4.5:1` in dark and light states, dark default;
   - Fira Sans for UI, Fira Code for model ids/tokens/fingerprints, no other families;
   - 4/8 dp spacing rhythm, 48 dp Android touch targets, desktop focus ring `2px #22C55E` with `2px` offset;
   - motion `150–300 ms`, `prefers-reduced-motion` / Compose reduced-motion off switch;
   - state indicators: icon plus text, never color alone; decorative icons hidden from a11y trees.

4. Vendor the OFL fonts into `assets/fonts/`, mirror them into `mobile-worker/android/app/src/main/res/font/` and `public/fonts/`, and include `assets/fonts/OFL.txt`. Replace the Google Fonts link in `index.html` with local `@font-face` declarations in `src/index.css`. Replace CSS variables with the exact §12 hex values while keeping the existing class names working.

5. Change `themes.xml` from `android:Theme.Material.Light.NoActionBar` to a dark `Theme.Material3.Dark.NoActionBar` parent and add `colors.xml` with the same six tokens.

6. **Verification:**
   ```powershell
   rg -n "#0F172A|#1B2336|#475569|#94A3B8|#22C55E|#EF4444" design-system/ai-upcycler/MASTER.md src/index.css mobile-worker/android/app/src/main/res/values/colors.xml
   rg -n "fonts.googleapis.com" index.html
   ```
   The first command must match all six values in all three files; the second must produce no output. Then run `npx tsc --noEmit` with `$env:npm_config_cache` set to the D: cache.

7. **Checkpoint:** A prerequisites are present, fonts are local, tokens match exactly, and dark is the default. Do not open a UI task until this passes.

## Task 2 — Read-only shared session API (TDD)

**Files:** new `core/src/api/sessions.rs`, `core/tests/session_reads.rs`; modified `core/src/api/mod.rs`.

**Consumes:** `Store::list_sessions`, `Store::get_session`, `Store::list_messages`, `ChatSessionRow`, `ChatMessageRow`.
**Produces:** the read-only session surface D will extend.

1. Write failing axum tests in `core/tests/session_reads.rs`:
   - `GET /api/sessions` → 200 `{"sessions":[...]}` ordered by `created_at, session_id`;
   - `GET /api/sessions/s1` → 200 `ChatSessionRow`;
   - `GET /api/sessions/missing` → 404 with the existing error envelope and no message bodies;
   - `GET /api/sessions/s1/messages` → 200 `{"messages":[...]}` with `message_id`, `role`, `content`, `token_count`, `generation_time_sec`, `created_at`;
   - encrypted or secret-looking strings inserted into message content are returned only as user content, never as a separate field.

2. Implement:
   ```rust
   #[derive(Serialize)]
   pub struct SessionList { pub sessions: Vec<ChatSessionRow> }

   #[derive(Serialize)]
   pub struct SessionMessages { pub messages: Vec<ChatMessageRow> }

   pub async fn list(State(state): State<AppState>) -> ApiResult<Json<SessionList>>;
   pub async fn get(State(state): State<AppState>, Path(id): Path<String>) -> ApiResult<Json<ChatSessionRow>>;
   pub async fn messages(State(state): State<AppState>, Path(id): Path<String>) -> ApiResult<Json<SessionMessages>>;
   ```
   Missing session → `ApiError::NotFound(format!("session {id} not found"))`. Do not create, rename, or delete sessions here; chat completions already create them via `session_id`.

3. Register only these routes:
   ```rust
   .route("/api/sessions", get(sessions::list))
   .route("/api/sessions/:id", get(sessions::get))
   .route("/api/sessions/:id/messages", get(sessions::messages))
   ```

4. **Verification:**
   ```powershell
   $env:RUSTUP_TOOLCHAIN = "stable-x86_64-pc-windows-gnu"; $env:CARGO_TARGET_DIR = "D:\ai-upcycler-target"; $env:PATH = "C:\msys64\mingw64\bin;$env:PATH"
   cargo test -p ai-upcycler-core --test session_reads -- --nocapture
   cargo clippy -p ai-upcycler-core --all-targets -- -D warnings
   ```

5. **Checkpoint:** four router tests pass; no write route is present; D can later add `PUT`/`DELETE` to the same module without renaming B's types.

## Task 3 — Pairing-derived LAN token lifecycle (TDD)

**Files:** modified `core/Cargo.toml`, `core/src/trust.rs`, `core/src/security.rs`, `core/src/api/error.rs`, `core/src/api/pairing.rs`, `core/src/api/devices.rs`.

**Consumes:** pairing PIN redemption, enrollment ids, trust store, `devices::revoke`.
**Produces:** one token per enrollment, hash-only host persistence, revocation and rotation.

1. Add dependencies:
   ```toml
   rand = "0.8"
   subtle = "2.6"
   ```

2. Write failing tests first:
   - token is `aiu1_` + 43 base64url characters (32 random bytes, no padding);
   - `verify_lan_token` accepts the exact token and rejects one-character mutation;
   - `lan-tokens.json` contains a 64-character lowercase hex SHA-256 and never the token string;
   - `devices::revoke` makes the old token fail;
   - `SecurityState::load_or_generate` on an unusable/missing CA clears `lan-tokens.json` with `enrollments.json`;
   - re-enrolling the same device replaces the token hash, so the previous token fails.

3. In `core/src/trust.rs`, add:
   ```rust
   const LAN_TOKENS_FILE: &str = "lan-tokens.json";

   pub fn load_lan_tokens(&self) -> HashMap<String, String>; // node_id -> sha256 hex
   pub fn save_lan_tokens(&self, entries: &HashMap<String, String>) -> Result<(), TrustError>;
   pub fn delete_lan_token(&self, node_id: &str);
   pub fn clear_lan_tokens(&self);
   ```
   Use the same atomic temp-file/rename helper as `save_ca`. Never log file contents.

4. In `core/src/security.rs`, extend `EnrollmentRegistry` with `set_lan_token`, `get_lan_token`, `verify_lan_token`, `revoke_lan_token`, and `load_lan_tokens`. Store parsed hashes as `[u8; 32]` in memory and compare with `subtle::ConstantTimeEq`. `SecurityState::load_or_generate` must call `clear_lan_tokens()` in both branches that currently call `store.clear_enrollments()`, then load the remaining token hashes.

5. In `core/src/api/pairing.rs`, generate the token only after the PIN is redeemed and certificates are issued:
   ```rust
   pub struct EnrollResponse {
       pub ca_pem: String,
       pub client_cert_pem: String,
       pub client_key_pem: String,
       pub server_cert_pem: String,
       pub server_key_pem: String,
       pub fingerprint: String,
       pub lan_token: String,
       pub lan_port: u16,
       pub node_id: String,
   }
   ```
   Store one hash for every id returned by `enrollment_ids(node_id, serial)`. `lan_port` comes from Task 5's `LanApiConfig::port()` default 8090; until that task lands this task may use the constant `8090`.

6. In `core/src/api/error.rs`, add `Unauthorized(String)` mapped to `StatusCode::UNAUTHORIZED`. In `core/src/api/devices.rs`, call `security.enrollments.revoke_lan_token(&id)` in the same `spawn_blocking` closure as `revoke(&id)`.

7. **Verification:**
   ```powershell
   $env:RUSTUP_TOOLCHAIN = "stable-x86_64-pc-windows-gnu"; $env:CARGO_TARGET_DIR = "D:\ai-upcycler-target"; $env:PATH = "C:\msys64\mingw64\bin;$env:PATH"
   cargo test -p ai-upcycler-core --lib security:: -- --nocapture
   cargo test -p ai-upcycler-core --test lan_api token -- --nocapture
   rg -n "aiu1_[A-Za-z0-9_-]{43}" core/tests core/src | Measure-Object
   ```
   The final `rg` must find the token only in generated-test assertions, never in committed fixtures or docs.

8. **Checkpoint:** token generation, verification, revocation, replacement, and CA rotation are proven; no plaintext token exists on the host disk.

## Task 4 — LAN allowlist router and bearer middleware (TDD)

**Files:** new `core/src/api/lan.rs`; modified `core/src/api/mod.rs`, `core/tests/lan_api.rs`.

**Consumes:** `AppState`, A's provider/secret routes, B's session routes, LAN token registry.
**Produces:** `lan_router(state)` with auth on every route.

1. Write failing tests in `core/tests/lan_api.rs`:
   - no `Authorization` header → `401`, body is exactly the fixed envelope with `message == "unauthorized"` and contains no `model`, `session`, or `provider` text;
   - malformed `Authorization: Bearer` → `401`;
   - valid token → chat/models/providers/sessions/provisioning routes answer;
   - revoked token → `401`;
   - `POST /api/pairing/enroll` and `DELETE /api/devices/:id` are absent from the LAN router (`404` or `405`, never 200);
   - loopback `router(state)` still answers `/api/pairing/session` without a bearer token.

2. Implement config:
   ```rust
   pub const LAN_API_ENV: &str = "AI_UPCYCLER_LAN_API";
   pub const LAN_PORT_ENV: &str = "AI_UPCYCLER_LAN_API_PORT";
   pub const DEFAULT_LAN_PORT: u16 = 8090;

   pub struct LanApiConfig { pub enabled: bool, pub port: u16 }
   impl LanApiConfig { pub fn from_env() -> Self; }
   ```
   `enabled` is true only for exact value `1`. Invalid ports log `ignoring invalid AI_UPCYCLER_LAN_API_PORT=<value>; using 8090` without a token and fall back.

3. Implement middleware:
   ```rust
   pub async fn require_lan_token(
       State(state): State<AppState>,
       request: Request<Body>,
       next: Next,
   ) -> Response;
   ```
   Extract `Authorization`, require exactly `Bearer <token>`, hash the presented token, verify against `state.security.enrollments`, and on failure return `ApiError::Unauthorized("unauthorized".into())`. Configure `TraceLayer` so the `authorization` header is never recorded. Add no CORS layer to the LAN router.

4. Allowlist exactly:
   ```rust
   .route("/v1/models", get(models::list_models))
   .route("/v1/chat/completions", post(chat::chat_completions))
   .route("/api/providers", get(providers::list))
   .route("/api/providers/:id/test", post(providers::test))
   .route("/api/settings/secrets/:provider", put(providers::put_secret).delete(providers::delete_secret))
   .route("/api/sessions", get(sessions::list))
   .route("/api/sessions/:id", get(sessions::get))
   .route("/api/sessions/:id/messages", get(sessions::messages))
   .route("/api/devices/:id/provision", get(provisioning::status))
   .layer(from_fn_with_state(state.clone(), require_lan_token))
   ```
   Do not use `router(state)` on the LAN listener.

5. **Verification:**
   ```powershell
   $env:RUSTUP_TOOLCHAIN = "stable-x86_64-pc-windows-gnu"; $env:CARGO_TARGET_DIR = "D:\ai-upcycler-target"; $env:PATH = "C:\msys64\mingw64\bin;$env:PATH"
   cargo test -p ai-upcycler-core --test lan_api allowlist -- --nocapture
   cargo test -p ai-upcycler-core --test lan_api unauthorized -- --nocapture
   ```

6. **Checkpoint:** every LAN route requires a valid token; loopback behavior is unchanged; pairing and device control are not reachable from LAN.

## Task 5 — LAN listener, config, and master mDNS advert (TDD)

**Files:** modified `core/Cargo.toml` (none if deps already present), `core/src/discovery/mdns.rs`, `core/src/api/mod.rs`, `core/tests/lan_api.rs`.

**Consumes:** `LanApiConfig`, `lan_router`, `mdns_sd::ServiceDaemon`.
**Produces:** an opt-in listener and advert started from `serve`.

1. Write failing tests in `core/tests/lan_api.rs`:
   - `parse_api_port` helper shape: env `"1"` enables, `"0"`/`"true"`/`absent` disables; port `"8099"` parses; `"70000"` and `"0"` fall back with the exact warning text;
   - `MasterServiceInfo::txt_records()` contains `version=1`, `node_id`, `api_port`, and `fingerprint`, and does not contain `token`;
   - `resolve_lan_bind_addr()` with an injected interface list ignores loopback and link-local addresses, chooses the first private IPv4, and returns `None` when only loopback exists;
   - advert service type is exactly `_aiupcycler-master._tcp.local.`.

2. Add to `core/src/discovery/mdns.rs`:
   ```rust
   pub const MASTER_SERVICE_TYPE: &str = "_aiupcycler-master._tcp.local.";

   pub struct MasterServiceInfo {
       pub instance_name: String,
       pub node_id: String,
       pub fingerprint: String,
       pub port: u16,
   }

   impl MasterServiceInfo {
       pub fn txt_records(&self) -> Vec<(String, String)>;
       pub fn service_info(&self, host: std::net::Ipv4Addr) -> mdns_sd::ServiceInfo;
   }
   ```
   Add a pure `resolve_lan_bind_addr(candidates: &[Ipv4Addr]) -> Option<Ipv4Addr>` and a production wrapper that enumerates local interfaces. Never advertise on `0.0.0.0`.

3. Add `serve_lan(addr: SocketAddr, state: AppState)` in `core/src/api/mod.rs`. It binds `addr`, starts the advert with a daemon that is dropped with the future, and serves `lan_router(state)`. `serve` calls it only when `LanApiConfig::from_env().enabled`; when no LAN IPv4 exists, log `LAN API enabled but no LAN IPv4 address was found; manual host:port is unavailable` and continue serving loopback.

4. **Verification:**
   ```powershell
   $env:RUSTUP_TOOLCHAIN = "stable-x86_64-pc-windows-gnu"; $env:CARGO_TARGET_DIR = "D:\ai-upcycler-target"; $env:PATH = "C:\msys64\mingw64\bin;$env:PATH"
   cargo test -p ai-upcycler-core --test lan_api config -- --nocapture
   cargo test -p ai-upcycler-core --test lan_api advert -- --nocapture
   ```

5. **Checkpoint:** LAN listener is off by default; with `AI_UPCYCLER_LAN_API=1` it binds only a LAN IPv4 and advertises public metadata only.

## Task 6 — LAN security documentation in the same slice

**Files:** modified `docs/api.md`, `docs/security.md`, `docs/AGENTS.md`.

**Consumes:** Tasks 3–5 behavior and exact env names.
**Produces:** user-visible security model matching the code.

1. Write documentation assertions before editing: gather the exact strings that must exist and must disappear:
   - must exist: `AI_UPCYCLER_LAN_API=1`, `AI_UPCYCLER_LAN_API_PORT`, `_aiupcycler-master._tcp`, `lan-tokens.json`, `401`, `lan_token`;
   - must be corrected in `docs/AGENTS.md` §8: the current blanket "do not expose port 8080" still applies to loopback 8080; the opt-in LAN listener is a different port and is not a proxy/tunnel/firewall rule.

2. `docs/api.md`: document new enroll response fields, read-only session routes, LAN route allowlist, and `generation_metrics.data.source` values `local`, `adb:<serial>`, or provider id. State that all LAN routes except mDNS discovery require `Authorization: Bearer <pairing token>` and that `401` bodies carry no detail.

3. `docs/security.md`: document token format, hash-only host storage, Keystore-only phone storage, revocation on unpair, rotation on CA replacement, constant-time verification, no token in mDNS TXT/logs, and that loopback remains unauthenticated.

4. `docs/AGENTS.md` §8: replace the absolute prohibition with two explicit rules: never expose loopback 8080; LAN API is off unless the env opt-in is set and must not be published through a proxy/tunnel/firewall rule.

5. **Verification:**
   ```powershell
   rg -n "AI_UPCYCLER_LAN_API=1|AI_UPCYCLER_LAN_API_PORT|_aiupcycler-master._tcp|lan-tokens.json|lan_token" docs/api.md docs/security.md docs/AGENTS.md
   ```

6. **Checkpoint:** Tasks 3–6 are one indivisible LAN security change; do not proceed to Android before docs agree with code.

## Task 7 — Android credentials and Keystore secret store (TDD)

**Files:** modified `mobile-worker/android/app/build.gradle.kts`, `.../pairing/WorkerCredentials.kt`, `.../pairing/CredentialStore.kt`, `.../pairing/EnrollmentClient.kt`; new `.../chat/secrets/SecretStore.kt`, `.../chat/secrets/KeystoreSecretStore.kt`; tests in `.../pairing/CredentialStoreTest.kt`, `.../pairing/EnrollmentClientTest.kt`, new `.../chat/SecretStoreContractTest.kt`.

**Consumes:** Task 3 enrollment response fields.
**Produces:** token + node id available to chat; provider keys and bearer token in Keystore-backed storage only.

1. Write failing JVM tests first:
   - `WorkerCredentials` round-trips `nodeId`, `lanPort`, but the token is never written by `CredentialStore` to any file;
   - `CredentialStore.FILES` gains `node_id.txt` and `lan_port.txt` but no `lan_token.txt`;
   - `EnrollmentClient` parses `lan_token`, `lan_port`, `node_id` and rejects an enrollment response missing any of them with `EnrollmentResult.Failed("Master response is missing <field>")`;
   - `EnrollmentClient` still refuses plain HTTP to a non-loopback host with the existing exact message;
   - `SecretStore.keyFor(provider)` returns exactly `provider:openai|anthropic|gemini|kimi` and `lan_token`; `MemorySecretStore` debug output contains no stored value.

2. Extend `WorkerCredentials` with `nodeId: String`, `lanToken: String`, `lanPort: Int`. Add defaults only in test fixtures; production construction must supply all values.

3. Extend `CredentialStore`: write/read `node_id.txt` and `lan_port.txt` with the existing staging-then-publish behavior; do not add a token file. `clear()` removes all eight files and calls the injected secret-store interface to delete `lan_token` and all provider keys.

4. Add:
   ```kotlin
   interface SecretStore {
       fun read(key: String): String?
       fun write(key: String, value: String)
       fun delete(key: String)
   }

   class KeystoreSecretStore(context: Context) : SecretStore
   ```
   Implement with `MasterKey` (`AES256_GCM`, alias `ai-upcycler-chat`) and `EncryptedSharedPreferences` (`AES256_SIV` values, `AES256_GCM` keys), file `ai-upcycler-chat-secrets`. Validate provider ids against the four A ids before writing; invalid id throws `IllegalArgumentException("unknown provider: <id>")`. Never log values.

5. In `EnrollmentClient.parseEnrollment`, require and return the three new fields. On success `WorkerRuntime.pair` saves credentials generically without modification; CredentialStore routes `lanToken` into `SecretStore.write("lan_token", value)`.

6. Add Gradle dependencies:
   ```kotlin
   implementation("androidx.security:security-crypto:1.1.0-alpha06")
   ```

7. **Verification:**
   ```powershell
   $env:GRADLE_USER_HOME = "D:\ai-upcycler-gradle-home"; cd D:\ai-upcycler-worktrees\cloud-provider-bridge\mobile-worker\android
   .\gradlew --project-cache-dir D:\ai-upcycler-gradle-project-cache :app:testDebugUnitTest --tests "org.aiupcycler.worker.pairing.*" --tests "org.aiupcycler.worker.chat.SecretStoreContractTest"
   rg -n "lan_token" mobile-worker\android\app\src\main\java\org\aiupcycler\worker\pairing\CredentialStore.kt
   ```
   The `rg` command must find no match in `CredentialStore.kt`.

8. **Checkpoint:** pairing still works over ADB loopback; token is available to chat but never stored in plaintext files; provider keys have a Keystore-only local path.

## Task 8 — Android LAN endpoint discovery and OkHttp client (TDD)

**Files:** new `.../chat/LanEndpoint.kt`, `.../chat/MasterDiscovery.kt`, `.../chat/LanClient.kt`; tests `.../chat/LanRequestBuilderTest.kt`, `.../chat/MasterDiscoveryTest.kt`.

**Consumes:** `WorkerCredentials.nodeId/lanPort`, `SecretStore`, A provider/secret routes, B session routes.
**Produces:** typed LAN API client usable by ViewModel.

1. Write failing tests first:
   - `LanEndpoint.parse("192.168.1.20:8090")` → host `192.168.1.20`, port `8090`; reject empty host, port `0`, port `>65535`, and userinfo with exact messages;
   - `MasterDiscovery` ignores services whose TXT `version` is not `1` and rejects a fingerprint that differs from the stored credential fingerprint;
   - `LanRequestBuilder.chatBody(sessionId, model, provider, history)` emits `stream: true`, `session_id`, and omits `provider` when the picker is `Auto`;
   - `LanRequestBuilder.authHeader("token")` → `Bearer token`, and an empty token throws `IllegalStateException("LAN token missing; pair again")`;
   - SSE parser splits CRLF, ignores heartbeat lines, emits content deltas, and captures final `source` into `Source.Local`, `Source.Worker(serial)`, or `Source.Provider(id)`;
   - `401` maps to `LanError.Unauthorized("unauthorized")`, `404` to `LanError.NotFound`, socket timeout to `LanError.Offline`.

2. Implement `LanEndpoint` with `baseUrl(): String` and `SharedPreferences` storage keys `lan_host`, `lan_port` in the existing `ai-upcycler-worker` prefs (non-secret only).

3. Implement `MasterDiscovery` with Android `NsdManager.discoverServices("_aiupcycler-master._tcp.", PROTOCOL_DNS_SD, listener)`. On resolve, require TXT `version == "1"`, `fingerprint == credentials.fingerprint`, and valid `api_port`; emit `MasterEndpoint(host, port)`; otherwise emit `DiscoveryFailure(reason)` with exact reason strings. Manual `host:port` bypasses NSD but still goes through `LanEndpoint.parse`.

4. Implement `LanClient` with injected OkHttp `Call.Factory` for JVM tests:
   ```kotlin
   fun listSessions(): List<ChatSession>
   fun listMessages(sessionId: String): List<ChatMessage>
   fun listModels(): List<ModelEntry>
   fun listProviders(): List<ProviderStatus>
   fun putProviderKey(id: String, value: String)
   fun deleteProviderKey(id: String)
   fun provisionState(deviceId: String): ProvisionState
   fun streamChat(request: ChatRequest, onDelta: (String) -> Unit, onDone: (Source) -> Unit)
   ```
   Every request adds `Authorization: Bearer <token>` from `SecretStore.read("lan_token")`. Connect timeout 5 s, read timeout 0 for streaming, call timeout disabled. Cancel via returned `Call`. Response bodies that are invalid JSON map to `LanError.MalformedResponse`.

5. Update `network_security_config.xml` to permit cleartext LAN traffic for the B LAN API with `base-config cleartextTrafficPermitted="true"`, and add a comment that `EnrollmentClient` continues to reject non-loopback plain HTTP so pairing keys are never sent in cleartext. Do not set `android:usesCleartextTraffic` in the manifest.

6. **Verification:**
   ```powershell
   $env:GRADLE_USER_HOME = "D:\ai-upcycler-gradle-home"; cd D:\ai-upcycler-worktrees\cloud-provider-bridge\mobile-worker\android
   .\gradlew --project-cache-dir D:\ai-upcycler-gradle-project-cache :app:testDebugUnitTest --tests "org.aiupcycler.worker.chat.LanRequestBuilderTest" --tests "org.aiupcycler.worker.chat.MasterDiscoveryTest"
   ```

7. **Checkpoint:** endpoint parsing, discovery validation, auth header, SSE source parsing, and explicit error mapping are green; no pairing transport guard was weakened.

## Task 9 — Android chat state and catalog reducers (TDD)

**Files:** new `.../chat/ChatUiState.kt`, tests `.../chat/ChatReducerTest.kt`.

**Consumes:** `LanClient` data types.
**Produces:** pure reducers that UI composables render without business logic.

1. Write failing tests first:
   - `SessionListLoaded` preserves `session_id` strings as keys; adding a session never rekeys existing entries;
   - `Delta(text)` appends to the assistant message id, never to the last index;
   - `StreamDone(source)` sets `source` and clears `busy`;
   - `ProvisioningUpdated(state)` with `running` stores `bytesDone/totalBytes`, with `failed` stores exact `message`;
   - `Unauthorized` sets a blocking state with the user message `LAN credentials rejected; pair again`;
   - `ModelCatalogLoaded(models, providers)` keeps local ids unchanged and adds `provider:model` ids exactly once, sorted by provider id then model id;
   - reduced-motion and compact flags are preserved across all events.

2. Implement data classes:
   ```kotlin
   data class ChatUiState(
       val endpoint: LanEndpoint?,
       val sessions: List<ChatSession>,
       val activeSessionId: String,
       val messages: List<ChatMessage>,
       val draft: String,
       val busy: Boolean,
       val source: Source?,
       val models: List<ModelEntry>,
       val providers: List<ProviderStatus>,
       val selectedProvider: ProviderChoice,
       val selectedModel: String,
       val provisioning: ProvisionState,
       val banner: Banner?,
   )
   ```
   `ProviderChoice` is `Auto`, `Local`, or `Cloud(providerId)`. `Banner` is a sealed type with `Unauthorized`, `Offline`, `NotFound`, `MalformedResponse`, and `PairingRequired`.

3. Implement `reduce(state, event): ChatUiState` as a pure top-level function. No Android or OkHttp imports in this file.

4. **Verification:**
   ```powershell
   $env:GRADLE_USER_HOME = "D:\ai-upcycler-gradle-home"; cd D:\ai-upcycler-worktrees\cloud-provider-bridge\mobile-worker\android
   .\gradlew --project-cache-dir D:\ai-upcycler-gradle-project-cache :app:testDebugUnitTest --tests "org.aiupcycler.worker.chat.ChatReducerTest"
   ```

5. **Checkpoint:** reducer tests pass; all list identity is by stable id; no reducer mutates its input.

## Task 10 — Android Compose session list and streaming chat thread

**Files:** new `.../chat/ui/SessionListScreen.kt`, `.../chat/ui/ChatThreadScreen.kt`, `.../chat/ui/ChatTheme.kt`; modified `strings.xml`, `colors.xml`.

**Consumes:** `ChatUiState`, `reduce`, `ChatViewModel` events.
**Produces:** dark-first session list and token-streaming thread.

1. Write failing Compose-independent expectations first as assertions in `ChatReducerTest.kt`: every visible message has a non-empty stable id; optimistic user messages use `UUID.randomUUID()` and are never replaced by an index; the assistant placeholder keeps its own id through all deltas.

2. Implement `SessionListScreen`:
   - `LazyColumn` with `key = { it.sessionId }`;
   - "New session" button with 48 dp height and no layout shift on press;
   - each row shows title in Fira Sans, model id in Fira Code, and a provider-local icon plus text; decorative icons `clearAndSetSemantics {}`;
   - empty state text `No sessions yet. Start a chat.`;
   - `contentPadding = WindowInsets.statusBars.only(WindowInsetsSides.Top).asPaddingValues()`.

3. Implement `ChatThreadScreen`:
   - `LazyColumn` with `key = { it.stableId }`, never `Column.verticalScroll`;
   - user and assistant bubbles differ by alignment, border shape, and an a11y label (`"You"` / `"Assistant"`), not by color alone;
   - assistant content renders streamed text directly while `busy`; no spinner for generation; a stable skeleton with `Modifier.semantics { stateDescription = "Generating" }` is shown only before the first delta;
   - send button disabled visibly while `busy`/empty draft; stop button labeled `Stop generation`;
   - 4/8 dp spacing rhythm and 48 dp minimum touch targets.

4. Implement `ChatTheme` with `darkTheme = true` default, exact token colors, Fira families, and reduced-motion flag from `LocalContext`.

5. **Verification:** install and capture over wireless ADB after Task 12 wires the activity:
   ```powershell
   adb connect 192.168.1.9:5555
   adb -s 192.168.1.9:5555 shell am start -n org.aiupcycler.worker/.chat.ChatActivity
   adb -s 192.168.1.9:5555 exec-out screencap -p > D:\ai-upcycler-artifacts\b-mobile\01-session-list.png
   adb -s 192.168.1.9:5555 exec-out screencap -p > D:\ai-upcycler-artifacts\b-mobile\02-chat-thread.png
   ```
   Screenshots must show dark background, legible muted text, and no clipped status bar.

6. **Checkpoint:** stable-key tests pass; ADB screenshots exist at the exact paths and show both screens.

## Task 11 — Android provider picker, source badge, provisioning progress

**Files:** new `.../chat/ui/ProviderPicker.kt`, `.../chat/ui/SourceBadge.kt`, `.../chat/ui/ProvisioningProgressScreen.kt`; modified `.../chat/ChatUiState.kt`, `.../chat/ChatReducerTest.kt`.

**Consumes:** `GET /api/providers`, A virtual model ids, provisioning status route, SSE final `source`.
**Produces:** visible provider/model choice, source transparency, and model push progress.

1. Write failing tests first:
   - `ProviderChoice.Auto` omits `provider` from the request body; `Local` sends `"local"`; `Cloud(id)` sends the id;
   - `SourceBadge.text(Source.Local)` → `Local engine`; `Source.Worker("adb:serial")` → `Worker serial`; `Source.Provider("openai")` → `Cloud OpenAI`;
   - progress state with `running` renders `bytesDone/totalBytes`, not a percentage derived from an index;
   - changing provider resets selected model only if the current model is not in the new catalog.

2. `ProviderPicker`: a `Row` + `DropdownMenu` with entries `Auto`, `Local`, `OpenAI`, `Anthropic`, `Gemini`, `Kimi`; each entry has `Modifier.heightIn(min = 48.dp)`, icon + text, and a pressed background that does not change bounds. Model dropdown lists local ids from `/v1/models` and virtual ids from `/api/providers`, using Fira Code.

3. `SourceBadge`: shows icon + text; local uses the accent token, worker uses border/muted tokens, provider uses card/border tokens; disabled/unknown source reads `Source unknown`. Decorative icon is hidden from a11y; the text is the accessible name.

4. `ProvisioningProgressScreen`: polls `LanClient.provisionState(credentials.nodeId)` once per second only while `running`; shows determinate progress with byte counts, exact failure message for `failed`, and `Model ready` for `done`. Stop polling when the screen leaves composition.

5. **Verification:** with the host running and LAN API enabled, capture:
   ```powershell
   adb -s 192.168.1.9:5555 exec-out screencap -p > D:\ai-upcycler-artifacts\b-mobile\03-provider-picker.png
   adb -s 192.168.1.9:5555 exec-out screencap -p > D:\ai-upcycler-artifacts\b-mobile\04-source-badge.png
   adb -s 192.168.1.9:5555 exec-out screencap -p > D:\ai-upcycler-artifacts\b-mobile\05-provisioning-progress.png
   ```

6. **Checkpoint:** provider switcher changes the outgoing body; source text is visible; provisioning state maps exactly; all three screenshots exist.

## Task 12 — ChatActivity wiring, overflow menu, manifest, and Gradle

**Files:** modified `mobile-worker/android/app/build.gradle.kts`, `AndroidManifest.xml`, `network_security_config.xml`, `themes.xml`; new `.../chat/ChatActivity.kt`, `.../chat/ChatViewModel.kt`, `.../androidTest/.../ChatActivitySmokeTest.kt`.

**Consumes:** all Android tasks above.
**Produces:** launcher activity with control/pairing screen in overflow and unchanged service/runtime.

1. Add Gradle config and dependencies:
   ```kotlin
   composeOptions { kotlinCompilerExtensionVersion = "1.5.14" }

   val composeBom = platform("androidx.compose:compose-bom:2024.09.02")
   implementation(composeBom)
   androidTestImplementation(composeBom)
   implementation("androidx.activity:activity-compose:1.9.2")
   implementation("androidx.compose.ui:ui")
   implementation("androidx.compose.ui:ui-tooling-preview")
   implementation("androidx.compose.material3:material3")
   implementation("androidx.lifecycle:lifecycle-viewmodel-compose:2.8.6")
   implementation("androidx.lifecycle:lifecycle-runtime-compose:2.8.6")
   implementation("com.squareup.okhttp3:okhttp:4.12.0")
   implementation("org.jetbrains.kotlinx:kotlinx-coroutines-android:1.8.1")
   androidTestImplementation("androidx.compose.ui:ui-test-junit4")
   debugImplementation("androidx.compose.ui:ui-test-manifest")
   ```
   Set `android.testOptions.unitTests.isIncludeAndroidResources = true`.

2. Implement `ChatActivity` as `ComponentActivity`:
   - calls `WorkerForegroundService.start(applicationContext)` in `onCreate`; it does not call or modify `WorkerRuntime.pair`;
   - hosts `ChatTheme { ChatScreen(viewModel) }`;
   - overflow `DropdownMenu` contains exactly `Control & pairing` and `About`; `Control & pairing` starts `Intent(this, MainActivity::class.java)`;
   - on unpaired state, shows a `PairingRequired` banner and the overflow action is still available;
   - handles `onNewIntent` only for LAN discovery callback data, leaving auto-enroll intents to MainActivity.

3. Implement `ChatViewModel`:
   - loads credentials from `CredentialStore`, token from `SecretStore`, endpoint from `LanEndpoint`/`MasterDiscovery`;
   - dispatches `SessionListLoaded`, `ModelCatalogLoaded`, `ProvisioningUpdated`, `Delta`, `StreamDone`, and error events into `reduce`;
   - generates a new `session_id` with `UUID.randomUUID()` for "New session"; passes it on the first chat request;
   - refreshes session list after the first successful turn;
   - exposes `send()`, `stop()`, `selectProvider()`, `selectModel()`, and `openProvisioning()`.

4. Manifest:
   - `ChatActivity` gets `MAIN`/`LAUNCHER` and `android:exported="true"`;
   - `MainActivity` remains `android:exported="true"` with no launcher filter, so explicit desktop `am start` continues to work;
   - service block is unchanged.

5. **Verification:**
   ```powershell
   $env:GRADLE_USER_HOME = "D:\ai-upcycler-gradle-home"; cd D:\ai-upcycler-worktrees\cloud-provider-bridge\mobile-worker\android
   .\gradlew --project-cache-dir D:\ai-upcycler-gradle-project-cache :app:testDebugUnitTest :app:assembleDebug
   adb -s 192.168.1.9:5555 install -r app\build\outputs\apk\debug\app-debug.apk
   adb -s 192.168.1.9:5555 shell am start -n org.aiupcycler.worker/.MainActivity
   adb -s 192.168.1.9:5555 exec-out screencap -p > D:\ai-upcycler-artifacts\b-mobile\06-control-screen-overflow.png
   adb -s 192.168.1.9:5555 shell am start -n org.aiupcycler.worker/.chat.ChatActivity
   adb -s 192.168.1.9:5555 shell am start -n org.aiupcycler.worker/.MainActivity --ez auto_enroll false
   ```
   The last command must not spend a PIN or render a pairing failure; auto-enroll remains unchanged.

6. **Checkpoint:** APK installs, chat is launcher, control/pairing is reachable via overflow and explicit intent, worker readiness is unchanged after restart.

## Task 13 — Desktop API client and types (TDD)

**Files:** modified `src/lib/types.ts`, `src/lib/api.ts`, `src/store/cluster.ts`; new `tests/e2e/chat-b.spec.ts`.

**Consumes:** A provider routes and B session routes.
**Produces:** typed client calls and a Playwright acceptance script.

1. Write failing TypeScript expectations in `tests/e2e/chat-b.spec.ts` before implementation:
   - provider switcher exists with accessible name `Provider`;
   - selecting `openai` makes the next chat request body contain `provider: "openai"`;
   - source badge reads `Local engine` when telemetry reports `source: "local"`;
   - session `<select>` contains a session created by a previous turn after page reload;
   - no `localStorage` key contains message text.

2. Add types:
   ```ts
   export interface ProviderModelWire { id: string; label: string; supports_vision: boolean }
   export interface ProviderStatus {
     id: 'openai' | 'anthropic' | 'gemini' | 'kimi';
     name: string;
     configured: boolean;
     models: ProviderModelWire[];
     supports_vision: boolean;
     supports_tools: boolean;
   }
   export interface ProviderList { providers: ProviderStatus[] }
   export interface ChatSession { session_id: string; title: string; model_id: string; created_at: string }
   export interface SessionList { sessions: ChatSession[] }
   export interface SessionMessages { messages: Array<ChatMessage & { message_id: number; created_at: string }> }
   ```
   Add `source: string` to `GenerationMetrics` and `provider?: string` to `ChatRequest`.

3. Add `listProviders()`, `putProviderSecret(id, value)`, `deleteProviderSecret(id)`, `listSessions()`, `listSessionMessages(id)` to `src/lib/api.ts` using the existing `request<T>` helper and `API_BASE = 'http://127.0.0.1:8080'`.

4. Extend `src/store/cluster.ts` with `providers: ProviderStatus[]` and `refreshProviders()`. Keep `latestMetrics` as the single source for the source badge; do not copy it into component state.

5. **Verification:**
   ```powershell
   $env:npm_config_cache = "D:\ai-upcycler-npm-cache"; npx tsc --noEmit
   ```

6. **Checkpoint:** TypeScript compiles; Playwright spec exists and fails only on missing UI implementation.

## Task 14 — Desktop provider switcher, source badge, and shared sessions (TDD)

**Files:** modified `src/components/ChatView.tsx`, `src/index.css`; `tests/e2e/chat-b.spec.ts`.

**Consumes:** Task 13 client, telemetry source, A virtual model ids.
**Produces:** desktop UI matching spec §7 and §12.

1. In `ChatView`, add state `sessionId = crypto.randomUUID()`, `providers`, `selectedProvider: '' | 'local' | providerId`, and `source` read from `useClusterStore((s) => s.latestMetrics?.source ?? null)`.

2. Header:
   - `<select aria-label="Provider">` with `Auto`, `Local`, and configured/unconfigured provider entries keyed by `provider.id`;
   - model `<select aria-label="Model">` lists local models and virtual `provider:model` ids with `key={m.id}`;
   - `SourceBadge` shows icon + text from `source`, mapping `local` → `Local engine`, `adb:<serial>` → `Worker <serial>`, provider id → `Cloud <Name>`; unknown reads `Source unknown`;
   - all controls have visible `focus-visible` ring from the CSS token.

3. Send path:
   - call `listSessions()` on mount and after the first successful turn;
   - add session `<select aria-label="Chat session">` with `key={session.session_id}`;
   - pass `session_id: sessionId` and `provider` only when `selectedProvider !== ''` to `streamChat`;
   - load `GET /api/sessions/:id/messages` when a different session is selected; do not write messages to `localStorage`;
   - keep streaming deltas in React state and keep the existing stop button.

4. Verify with Playwright MCP against `http://127.0.0.1:8080` at 375, 768, 1024, and 1440 px:
   - no console errors;
   - provider switcher changes outgoing body to `provider: "openai"` when selected;
   - source badge text matches the latest telemetry event;
   - session persists across reload through `/api/sessions/:id/messages`;
   - keyboard Tab reaches every control with a visible ring; `prefers-reduced-motion` disables transitions; screenshots saved to `D:\ai-upcycler-artifacts\b-mobile\desktop-375.png`, `desktop-768.png`, `desktop-1024.png`, `desktop-1440.png`.

5. **Checkpoint:** all five Playwright assertions pass at four widths; no browser-storage key contains chat content.

## Task 15 — Full verification and handoff

**Files:** no new source files; verification log only.

**Consumes:** all tasks.
**Produces:** a single pass/fail report with commands and observed results.

1. Clean old shell output, then run:
   ```powershell
   $env:RUSTUP_TOOLCHAIN = "stable-x86_64-pc-windows-gnu"
   $env:CARGO_TARGET_DIR = "D:\ai-upcycler-target"
   $env:PATH = "C:\msys64\mingw64\bin;$env:PATH"
   $env:npm_config_cache = "D:\ai-upcycler-npm-cache"
   $env:GRADLE_USER_HOME = "D:\ai-upcycler-gradle-home"

   cargo fmt --all -- --check
   cargo test -p ai-upcycler-core
   cargo clippy -p ai-upcycler-core --all-targets -- -D warnings
   npx tsc --noEmit

   cd D:\ai-upcycler-worktrees\cloud-provider-bridge\mobile-worker\android
   .\gradlew --project-cache-dir D:\ai-upcycler-gradle-project-cache :app:testDebugUnitTest :app:assembleDebug
   ```

2. API smoke through the real axum router:
   - loopback `GET /api/sessions` without bearer → 200;
   - LAN `GET /api/sessions` without bearer → 401 with no detail;
   - LAN with token → 200 and one session after a chat turn;
   - `POST /api/pairing/enroll` on LAN → not reachable;
   - unpair → old token 401;
   - CA replacement → old token 401.

3. Desktop Playwright: rerun all five Task 14 assertions at all four widths and confirm screenshots exist under `D:\ai-upcycler-artifacts\b-mobile\`.

4. Android ADB at `192.168.1.9:5555`: rerun session list, chat thread, provider picker, source badge, provisioning progress, and control-screen screenshots; confirm no console errors with `adb -s 192.168.1.9:5555 logcat -d *:E`.

5. Secret-boundary audit:
   ```powershell
   rg -n "sk-|api_key|password|lan_token|Bearer" docs core mobile-worker\android\app\src\main\java src | Select-String -NotMatch "test|Test"
   rg -n "localStorage|sessionStorage|chat_messages|Room|DataStore" src mobile-worker\android\app\src\main\java\org\aiupcycler\worker\chat
   ```
   Every match in the first command must be a documented field name or a fixed error string, never a value. The second command must show no client-side session storage.

6. Checkpoint: every command passes, screenshots exist, and the report lists each spec section covered. Do not push. Commit only on explicit operator instruction.

## Self-review

- Spec §5.4 covered: opt-in env listener, configurable port, LAN-only bind, pairing-derived token, hash-only host storage, Keystore phone storage, mDNS advert plus manual endpoint, revocation, CA rotation, fixed `401`, loopback unchanged, no tunnel/proxy/firewall rule, docs in Task 6.
- Spec §7 covered: Android session list, chat thread, provider/model picker, provisioning progress, source badge, OkHttp bearer client, Keystore secrets, untouched runtime/service, control screen in overflow, desktop switcher/source badge, shared SQLite sessions.
- Spec §12 covered: exact tokens and fonts, dark default, streaming text, stable ids, `LazyColumn`, 48 dp touch targets, safe-area padding, no color-only state, reduced motion, focus rings, four desktop breakpoints.
- Spec §13 covered: red-first tests, mock/injected HTTP, axum router smoke, Playwright at `127.0.0.1:8080`, wireless ADB screenshots at `192.168.1.9:5555`.
- Spec §14 covered: LAN opt-in and auth, additive Android screens, worker readiness re-check, D: artifacts and shell cleanup.
- A boundary preserved: no provider adapter, routing, secret store, CORS, or telemetry code is reimplemented; B consumes those contracts and stops if the Task 1 gates fail.
- D boundary preserved: B adds only read session endpoints and no settings/rename/delete/DataStore preferences; D extends the same module and surfaces.
- Secrets and sessions: no plaintext token on host disk, no token/provider key in logs or mDNS TXT, no messages or sessions in Android DataStore or browser storage.
