# Plan: PrivateLM-style Extension D — Auto-config + Persistence

> **For Claude:** REQUIRED SUB-SKILL: Use `superpowers:executing-plans` to implement this plan task-by-task. Do not dispatch subagents. Do not start D until Task 1's dependency gate is green.

**Goal:** Add deterministic settings recommendations from host memory, worker health, and model layer count; persist non-secret settings in SQLite with `GET|PUT /api/settings`; expose session list/rename/delete/messages endpoints for the existing `chat_sessions` table; make both clients consume the host-side session state; and keep Android local storage limited to UI preferences.

**Architecture:** Pure recommendation logic lives in `core/src/autoconfig.rs` behind injected reader traits, so unit tests never read the real machine. Non-secret settings are typed in `core/src/settings.rs`, validated with `serde(deny_unknown_fields)`, and persisted as JSON values in a new `settings(key, value_json, updated_at_epoch_ms)` table. API handlers are thin axum wrappers over `Store` and `AppState`. Desktop uses the same loopback REST API and generates a stable `session_id` before the first chat turn. Android writes only three boolean UI preferences to DataStore; all conversations remain on the host through the B-added LAN client.

**Tech Stack:** Rust/axum/rusqlite/serde, TypeScript/React/Tailwind, Kotlin/Jetpack Compose/DataStore, Playwright MCP, Android wireless ADB.

Date: 2026-11-27
Status: ready for execution
Spec: `docs/superpowers/specs/2026-11-27-privatelm-style-extension-design.md` §§9, 12, 13, 14
Prior plan: `docs/superpowers/plans/2026-11-27-cloud-provider-bridge.md` (sub-project A)

---

## Scope and hard constraints

- This plan covers sub-project **D only**. A (cloud provider bridge), B (mobile/desktop chat clients), C (multimodal), E (workflows), and F (image generation) are outside this plan.
- This plan is written against the **post-A/post-B** codebase. The current checkout does not yet have A's provider routes, `SecretStore`, or `AppState.secrets`/`AppState.providers`; it also does not yet have B's Compose chat/provider screens. Task 1 stops execution until those prerequisites exist.
- Worktree root: `D:\ai-upcycler-worktrees\cloud-provider-bridge`.
- Build artifacts: `D:\ai-upcycler-target`. npm cache: `D:\ai-upcycler-npm-cache`. QA screenshots: `D:\ai-upcycler-target\qa`.
- Before a large build, clear stale shell logs:

```powershell
Get-ChildItem -Path "C:\Users\darsh\.local\share\opencode\shell" -File | Remove-Item -Force
```

- Use this PowerShell environment for every Rust/desktop command in this plan:

```powershell
$env:RUSTUP_TOOLCHAIN = "stable-x86_64-pc-windows-gnu"
$env:CARGO_TARGET_DIR = "D:\ai-upcycler-target"
$env:PATH = "C:\msys64\mingw64\bin;$env:PATH"
$env:npm_config_cache = "D:\ai-upcycler-npm-cache"
```

- Secrets are **write-only** through A's `PUT|DELETE /api/settings/secrets/{provider}` and OS keychain. D's settings JSON, SQLite `settings` table, logs, telemetry, `GET /api/settings`, `GET /api/settings/recommendations`, and session bodies must never contain keys, tokens, passwords, or provider secrets.
- Android DataStore is limited to UI preferences: `compact_mode`, `show_source_badges`, and `auto_apply_recommendations`. It never stores messages, sessions, provider keys, bearer tokens, or model content.
- Every task ends with a checkpoint. Do not proceed while its checkpoint command fails.

## Dependency graph

| Dependency | Provided by | D uses it for | Gate |
|---|---|---|---|
| `SecretStore`, `KeyringSecretStore`, `PUT|DELETE /api/settings/secrets/{provider}` | A | Keeping secret material out of D's settings JSON/SQLite | Task 1 `rg` gate + Task 6 leak test |
| `AppState.providers: Arc<ProviderRegistry>`, `AppState.secrets: Arc<dyn SecretStore>` | A | No direct writes; D must not duplicate provider config | Task 1 `rg` gate |
| `StoreError::Conflict` | A | D assumes it exists and keeps it unchanged | Task 1 `rg` gate |
| `AppState.telemetry_cache: Arc<TelemetryCache>`, `AppState.host_memory: Arc<dyn HostMemorySource>` | Existing core + A ordering | Recommendation inputs | Task 2 |
| Compose chat thread, provider picker, LAN REST client with bearer token, Keystore writes | B | Android preferences attach to B's chat surface without changing transport/auth | Task 1 `rg` gate + Task 10 |
| Desktop provider switcher/source badge from B | B | D preserves it while adding session/settings controls | Task 8 Playwright checkpoint |

---

## Files

### New

| File | Purpose |
|---|---|
| `core/src/autoconfig.rs` | Pure recommendation algorithm + injected memory/worker probes |
| `core/src/settings.rs` | `AppSettings`, validation, secret-field rejection |
| `core/src/api/settings.rs` | `GET|PUT /api/settings`, `GET /api/settings/recommendations` |
| `core/src/api/sessions.rs` | Session list/rename/delete/messages handlers |
| `core/tests/settings_sessions.rs` | Router smoke tests + secret-leak assertions |
| `src/components/SettingsPanel.tsx` | Desktop settings/recommendation controls |
| `mobile-worker/android/app/src/main/java/org/aiupcycler/worker/preferences/UiPreferences.kt` | Three-field UI preference data |
| `mobile-worker/android/app/src/main/java/org/aiupcycler/worker/preferences/UiPreferencesStore.kt` | DataStore read/write for UI preferences only |
| `mobile-worker/android/app/src/test/java/org/aiupcycler/worker/preferences/UiPreferencesStoreTest.kt` | DataStore key and persistence tests |
| `mobile-worker/android/app/src/main/java/org/aiupcycler/worker/ui/PreferencesScreen.kt` | Compose preference toggles using spec tokens |

### Modified

| File | Change |
|---|---|
| `core/src/lib.rs` | Register `autoconfig`, `settings` modules |
| `core/src/store/schema.sql` | Add idempotent `settings` table |
| `core/src/store/mod.rs` | Add `SettingsJson`/`InvalidSettings` errors, settings methods, session rename/delete |
| `core/src/api/mod.rs` | Register `settings`, `sessions` modules; add routes; keep CORS `PUT` from A |
| `docs/api.md` | Document settings, recommendations, and session endpoints |
| `src/lib/types.ts` | Add settings/recommendation/session types |
| `src/lib/api.ts` | Add settings/recommendation/session client calls; handle 204/empty bodies |
| `src/components/ChatView.tsx` | Persist sessions, load messages, rename/delete, apply settings |
| `mobile-worker/android/app/build.gradle.kts` | Add DataStore + test dependencies |
| B-created `mobile-worker/android/app/src/main/java/org/aiupcycler/worker/ui/WorkerApp.kt` | Expose preferences entry point |
| B-created `mobile-worker/android/app/src/main/java/org/aiupcycler/worker/ui/ChatScreen.kt` | Consume `UiPreferences`; no local history writes |
| B-created `mobile-worker/android/app/src/main/java/org/aiupcycler/worker/api/LanClient.kt` | Add settings/recommendation calls for auto-apply, using the existing bearer token |

---

## Task 1 — Dependency gate and frozen interfaces

**Files:** none modified. This task is a hard gate.

1. Run these gates from the worktree root:

```powershell
rg -n "pub secrets: Arc<dyn SecretStore>" core/src/api/mod.rs
rg -n "pub providers: Arc<ProviderRegistry>" core/src/api/mod.rs
rg -n "Conflict" core/src/store/mod.rs
rg -n "settings/secrets" core/src/api/mod.rs core/src/api/providers.rs
rg -n "LazyColumn" mobile-worker/android/app/src/main/java/org/aiupcycler/worker
rg -n "Bearer" mobile-worker/android/app/src/main/java/org/aiupcycler/worker
rg -n "Keystore" mobile-worker/android/app/src/main/java/org/aiupcycler/worker
rg -n "fun ChatScreen|fun WorkerApp" mobile-worker/android/app/src/main/java/org/aiupcycler/worker/ui
rg -n "class LanClient|object LanClient" mobile-worker/android/app/src/main/java/org/aiupcycler/worker/api
```

2. Required results:

- A's `SecretStore` and `ProviderRegistry` fields exist in `AppState`.
- A's `StoreError::Conflict` exists.
- A's secret route is registered under `/api/settings/secrets`.
- B has a `LazyColumn` chat surface, a bearer-token LAN client, Keystore writes, the named `LanClient`, and the two Compose files named in the file table.
- If any command returns no matches, stop and execute the missing A or B plan before D. Do not create parallel versions of those surfaces.

3. Checkpoint: all nine `rg` commands have at least one match. Record the outputs in the execution log; do not modify source in this task.

---

## Task 2 — Pure autoconfig recommendation (TDD)

**Files:** create `core/src/autoconfig.rs`; modify `core/src/lib.rs`.

1. Add `pub mod autoconfig;` to `core/src/lib.rs`.

2. Create `core/src/autoconfig.rs` with only these tests first:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::telemetry::WorkerHealth;
    use crate::worker_health::HostMemoryUsage;

    struct FixedMemory(Option<HostMemoryUsage>);
    impl HostMemoryProbe for FixedMemory {
        fn usage(&self) -> Option<HostMemoryUsage> {
            self.0
        }
    }

    struct FixedWorkers(Vec<WorkerHealth>);
    impl WorkerHealthProbe for FixedWorkers {
        fn workers(&self) -> Vec<WorkerHealth> {
            self.0.clone()
        }
    }

    fn worker(id: &str, free_ram_bytes: u64, supports_compute: bool) -> WorkerHealth {
        WorkerHealth {
            id: id.into(),
            temp_celsius: None,
            battery_pct: None,
            free_ram_bytes,
            observed_at_epoch_ms: 1_700_000_000_000,
            supports_compute,
        }
    }

    #[test]
    fn recommends_upper_bounds_when_host_and_compute_workers_have_memory() {
        let memory = FixedMemory(Some(HostMemoryUsage {
            total_bytes: 16 * 1024 * 1024 * 1024,
            used_bytes: 0,
            available_bytes: 16 * 1024 * 1024 * 1024,
        }));
        let workers = FixedWorkers(vec![
            worker("w1", 8 * 1024 * 1024 * 1024, true),
            worker("w2", 8 * 1024 * 1024 * 1024, true),
        ]);
        let models = [ModelLayerInfo {
            model_id: "llama-8b".into(),
            total_layers: 32,
        }];

        let result = recommend_settings(&memory, &workers, &models);

        assert_eq!(result.context_tokens, MAX_CONTEXT_TOKENS);
        assert_eq!(result.max_tokens, MAX_OUTPUT_TOKENS);
        assert_eq!(result.reasons.len(), 3);
        assert!(result.reasons[0].starts_with("host available "));
        assert!(result.reasons[1].starts_with("compute worker RAM "));
        assert!(result.reasons[2].starts_with("largest model "));
    }

    #[test]
    fn ignores_non_compute_workers_and_clamps_to_the_floor() {
        let memory = FixedMemory(Some(HostMemoryUsage {
            total_bytes: 4 * 1024 * 1024 * 1024,
            used_bytes: 1024 * 1024 * 1024,
            available_bytes: 3 * 1024 * 1024 * 1024,
        }));
        let workers = FixedWorkers(vec![worker("w1", 100 * 1024 * 1024 * 1024, false)]);
        let models = [ModelLayerInfo {
            model_id: "llama-8b".into(),
            total_layers: 8,
        }];

        let result = recommend_settings(&memory, &workers, &models);

        assert_eq!(result.context_tokens, MIN_CONTEXT_TOKENS);
        assert_eq!(result.max_tokens, 512);
        assert_eq!(result.reasons.len(), 3);
        assert!(result.reasons[1].starts_with("no compute workers reported"));
    }

    #[test]
    fn falls_back_without_real_memory_readings() {
        let memory = FixedMemory(None);
        let workers = FixedWorkers(vec![]);
        let models = [];

        let result = recommend_settings(&memory, &workers, &models);

        assert_eq!(result.context_tokens, MIN_CONTEXT_TOKENS);
        assert_eq!(result.max_tokens, 512);
        assert_eq!(
            result.reasons,
            vec!["no usable memory reading; using conservative defaults".to_string()]
        );
    }
}
```

3. Run the red checkpoint. It must fail because the production items do not exist:

```powershell
$env:RUSTUP_TOOLCHAIN = "stable-x86_64-pc-windows-gnu"
$env:CARGO_TARGET_DIR = "D:\ai-upcycler-target"
$env:PATH = "C:\msys64\mingw64\bin;$env:PATH"
cargo test -p ai-upcycler-core autoconfig::tests -- --nocapture
```

4. Implement the exact contract in `core/src/autoconfig.rs`:

```rust
use crate::telemetry::WorkerHealth;
use crate::worker_health::{HostMemorySource, HostMemoryUsage, TelemetryCache};
use serde::Serialize;
use std::sync::Arc;

pub const MIN_CONTEXT_TOKENS: u32 = 2_048;
pub const MAX_CONTEXT_TOKENS: u32 = 32_768;
pub const MIN_OUTPUT_TOKENS: u32 = 256;
pub const MAX_OUTPUT_TOKENS: u32 = 8_192;
pub const CONTEXT_STEP_TOKENS: u32 = 1_024;
pub const BYTES_PER_CONTEXT_TOKEN: u64 = 2_048;
pub const BYTES_PER_MODEL_LAYER: u64 = 64 * 1024 * 1024;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SettingsRecommendation {
    pub context_tokens: u32,
    pub max_tokens: u32,
    pub reasons: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModelLayerInfo {
    pub model_id: String,
    pub total_layers: u32,
}

pub trait HostMemoryProbe: Send + Sync {
    fn usage(&self) -> Option<HostMemoryUsage>;
}

pub trait WorkerHealthProbe: Send + Sync {
    fn workers(&self) -> Vec<WorkerHealth>;
}

pub struct AppMemoryProbe {
    inner: Arc<dyn HostMemorySource>,
}

impl AppMemoryProbe {
    pub fn new(inner: Arc<dyn HostMemorySource>) -> Self {
        Self { inner }
    }
}

impl HostMemoryProbe for AppMemoryProbe {
    fn usage(&self) -> Option<HostMemoryUsage> {
        self.inner.usage()
    }
}

pub struct TelemetryCacheProbe {
    inner: Arc<TelemetryCache>,
}

impl TelemetryCacheProbe {
    pub fn new(inner: Arc<TelemetryCache>) -> Self {
        Self { inner }
    }
}

impl WorkerHealthProbe for TelemetryCacheProbe {
    fn workers(&self) -> Vec<WorkerHealth> {
        self.inner.workers()
    }
}

pub fn recommend_settings(
    memory: &dyn HostMemoryProbe,
    workers: &dyn WorkerHealthProbe,
    models: &[ModelLayerInfo],
) -> SettingsRecommendation {
    let host_usage = memory.usage();
    let compute_workers: Vec<WorkerHealth> = workers
        .workers()
        .into_iter()
        .filter(|worker| worker.supports_compute)
        .collect();
    let compute_worker_ram: u64 = compute_workers
        .iter()
        .map(|worker| worker.free_ram_bytes)
        .sum();

    let mut limits: Vec<u64> = Vec::new();
    if let Some(usage) = host_usage {
        limits.push(usage.available_bytes);
    }
    if !compute_workers.is_empty() {
        limits.push(compute_worker_ram);
    }

    let Some(usable_bytes) = limits.into_iter().min() else {
        return SettingsRecommendation {
            context_tokens: MIN_CONTEXT_TOKENS,
            max_tokens: 512,
            reasons: vec!["no usable memory reading; using conservative defaults".into()],
        };
    };

    let max_layers = models
        .iter()
        .map(|model| model.total_layers)
        .max()
        .unwrap_or(0) as u64;
    let reserve_bytes = max_layers.saturating_mul(BYTES_PER_MODEL_LAYER);
    let usable_for_context = usable_bytes.saturating_sub(reserve_bytes);
    let raw_context = (usable_for_context / BYTES_PER_CONTEXT_TOKEN) as u32;
    let rounded_context = (raw_context / CONTEXT_STEP_TOKENS) * CONTEXT_STEP_TOKENS;
    let context_tokens = rounded_context.clamp(MIN_CONTEXT_TOKENS, MAX_CONTEXT_TOKENS);
    let raw_output = context_tokens / 4;
    let rounded_output = (raw_output / MIN_OUTPUT_TOKENS) * MIN_OUTPUT_TOKENS;
    let output_cap = context_tokens / 2;
    let max_tokens = rounded_output
        .clamp(MIN_OUTPUT_TOKENS, MAX_OUTPUT_TOKENS)
        .min(output_cap)
        .max(MIN_OUTPUT_TOKENS);

    let mut reasons = Vec::new();
    if let Some(usage) = host_usage {
        reasons.push(format!(
            "host available {} MiB",
            usage.available_bytes / (1024 * 1024)
        ));
    }
    if compute_workers.is_empty() {
        reasons.push("no compute workers reported; using host memory only".into());
    } else {
        reasons.push(format!(
            "compute worker RAM {} MiB across {} worker(s)",
            compute_worker_ram / (1024 * 1024),
            compute_workers.len()
        ));
    }
    reasons.push(format!(
        "largest model {} layer(s); reserved {} MiB",
        max_layers,
        reserve_bytes / (1024 * 1024)
    ));

    SettingsRecommendation {
        context_tokens,
        max_tokens,
        reasons,
    }
}
```

5. Run the green checkpoint:

```powershell
$env:RUSTUP_TOOLCHAIN = "stable-x86_64-pc-windows-gnu"
$env:CARGO_TARGET_DIR = "D:\ai-upcycler-target"
$env:PATH = "C:\msys64\mingw64\bin;$env:PATH"
cargo test -p ai-upcycler-core autoconfig::tests -- --nocapture
```

6. Checkpoint: three tests pass; no real host memory or worker network call occurs in the test path.

---

## Task 3 — Typed settings contract and secret-field rejection (TDD)

**Files:** create `core/src/settings.rs`; modify `core/src/lib.rs`.

1. Add `pub mod settings;` to `core/src/lib.rs`.

2. Create `core/src/settings.rs` with these failing tests first:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_settings_are_deterministic() {
        assert_eq!(
            AppSettings::default(),
            AppSettings {
                provider: None,
                model: None,
                context_tokens: 4_096,
                max_tokens: 1_024,
                show_source_badges: true,
                compact_mode: false,
            }
        );
    }

    #[test]
    fn rejects_ranges_without_echoing_values() {
        let invalid = AppSettings {
            context_tokens: 512,
            ..AppSettings::default()
        };
        assert_eq!(
            invalid.validate(),
            Err("context_tokens must be between 1024 and 65536")
        );

        let invalid = AppSettings {
            context_tokens: 2_048,
            max_tokens: 4_096,
            ..AppSettings::default()
        };
        assert_eq!(
            invalid.validate(),
            Err("max_tokens must not exceed context_tokens")
        );
    }

    #[test]
    fn json_with_secret_fields_is_rejected() {
        let json = r#"{"provider":"openai","model":"gpt-4o","context_tokens":4096,"max_tokens":1024,"show_source_badges":true,"compact_mode":false,"api_key":"sk-shared-never"}"#;
        let result: Result<AppSettings, _> = serde_json::from_str(json);
        assert!(result.is_err(), "unknown secret-bearing fields must not deserialize");
    }
}
```

3. Run the red checkpoint:

```powershell
$env:RUSTUP_TOOLCHAIN = "stable-x86_64-pc-windows-gnu"
$env:CARGO_TARGET_DIR = "D:\ai-upcycler-target"
$env:PATH = "C:\msys64\mingw64\bin;$env:PATH"
cargo test -p ai-upcycler-core settings::tests -- --nocapture
```

4. Implement the exact contract:

```rust
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AppSettings {
    pub provider: Option<String>,
    pub model: Option<String>,
    pub context_tokens: u32,
    pub max_tokens: u32,
    pub show_source_badges: bool,
    pub compact_mode: bool,
}

impl Default for AppSettings {
    fn default() -> Self {
        Self {
            provider: None,
            model: None,
            context_tokens: 4_096,
            max_tokens: 1_024,
            show_source_badges: true,
            compact_mode: false,
        }
    }
}

impl AppSettings {
    pub fn validate(&self) -> Result<(), &'static str> {
        if !(1_024..=65_536).contains(&self.context_tokens) {
            return Err("context_tokens must be between 1024 and 65536");
        }
        if !(1..=32_768).contains(&self.max_tokens) {
            return Err("max_tokens must be between 1 and 32768");
        }
        if self.max_tokens > self.context_tokens {
            return Err("max_tokens must not exceed context_tokens");
        }
        if !valid_optional_string(&self.provider) {
            return Err("provider must be omitted or a non-empty string of at most 200 characters");
        }
        if !valid_optional_string(&self.model) {
            return Err("model must be omitted or a non-empty string of at most 200 characters");
        }
        Ok(())
    }
}

fn valid_optional_string(value: &Option<String>) -> bool {
    match value.as_deref() {
        None => true,
        Some(text) => !text.trim().is_empty() && text.chars().count() <= 200,
    }
}
```

5. Run the green checkpoint:

```powershell
$env:RUSTUP_TOOLCHAIN = "stable-x86_64-pc-windows-gnu"
$env:CARGO_TARGET_DIR = "D:\ai-upcycler-target"
$env:PATH = "C:\msys64\mingw64\bin;$env:PATH"
cargo test -p ai-upcycler-core settings::tests -- --nocapture
```

6. Checkpoint: three tests pass; `AppSettings` has no field capable of carrying secret material.

---

## Task 4 — SQLite settings table and store methods (TDD)

**Files:** modify `core/src/store/schema.sql`, `core/src/store/mod.rs`.

1. Append this idempotent table to `core/src/store/schema.sql` after `chat_messages`:

```sql
CREATE TABLE IF NOT EXISTS settings (
    key TEXT PRIMARY KEY NOT NULL,
    value_json TEXT NOT NULL,
    updated_at_epoch_ms INTEGER NOT NULL
);
```

2. Add these failing tests to `core/src/store/mod.rs` tests:

```rust
#[test]
fn settings_round_trip_uses_json_values() {
    let store = store();
    let settings = crate::settings::AppSettings {
        provider: Some("openai".into()),
        model: Some("gpt-4o".into()),
        context_tokens: 16_384,
        max_tokens: 2_048,
        show_source_badges: false,
        compact_mode: true,
    };

    store.put_settings(&settings).unwrap();

    assert_eq!(store.get_settings().unwrap(), settings);
    assert_eq!(store.setting_json("context_tokens").unwrap().unwrap(), "16384");
    assert_eq!(store.setting_json("provider").unwrap().unwrap(), "\"openai\"");
}

#[test]
fn settings_default_when_table_is_empty() {
    let store = store();
    assert_eq!(store.get_settings().unwrap(), crate::settings::AppSettings::default());
}

#[test]
fn put_settings_rejects_invalid_values_before_writing() {
    let store = store();
    let invalid = crate::settings::AppSettings {
        context_tokens: 512,
        ..crate::settings::AppSettings::default()
    };

    let error = store.put_settings(&invalid).unwrap_err();

    assert_eq!(
        error.to_string(),
        "invalid setting: context_tokens must be between 1024 and 65536"
    );
    assert_eq!(store.get_settings().unwrap(), crate::settings::AppSettings::default());
}
```

3. Run the red checkpoint:

```powershell
$env:RUSTUP_TOOLCHAIN = "stable-x86_64-pc-windows-gnu"
$env:CARGO_TARGET_DIR = "D:\ai-upcycler-target"
$env:PATH = "C:\msys64\mingw64\bin;$env:PATH"
cargo test -p ai-upcycler-core store::tests::settings -- --nocapture
```

4. In `core/src/store/mod.rs`, add imports and error variants. Keep A's `Conflict(String)` variant unchanged:

```rust
use crate::settings::AppSettings;
use serde::de::DeserializeOwned;

#[derive(Debug, thiserror::Error)]
pub enum StoreError {
    #[error("sqlite error: {0}")]
    Sqlite(#[from] rusqlite::Error),
    #[error("store mutex poisoned")]
    Poisoned,
    #[error("conflict: {0}")]
    Conflict(String),
    #[error("invalid settings json: {0}")]
    SettingsJson(#[from] serde_json::Error),
    #[error("invalid setting: {0}")]
    InvalidSettings(String),
}
```

If A's `Conflict(String)` variant is already present, do not add a duplicate.

5. Add these methods inside `impl Store` before the closing brace:

```rust
pub fn get_settings(&self) -> Result<AppSettings, StoreError> {
    Ok(AppSettings {
        provider: self.setting("provider")?,
        model: self.setting("model")?,
        context_tokens: self.setting("context_tokens")?,
        max_tokens: self.setting("max_tokens")?,
        show_source_badges: self.setting("show_source_badges")?,
        compact_mode: self.setting("compact_mode")?,
    })
}

pub fn put_settings(&self, settings: &AppSettings) -> Result<(), StoreError> {
    settings
        .validate()
        .map_err(|message| StoreError::InvalidSettings(message.into()))?;
    self.put_setting_json("provider", &settings.provider)?;
    self.put_setting_json("model", &settings.model)?;
    self.put_setting_json("context_tokens", &settings.context_tokens)?;
    self.put_setting_json("max_tokens", &settings.max_tokens)?;
    self.put_setting_json("show_source_badges", &settings.show_source_badges)?;
    self.put_setting_json("compact_mode", &settings.compact_mode)?;
    Ok(())
}

pub fn setting_json(&self, key: &str) -> Result<Option<String>, StoreError> {
    let conn = self.lock()?;
    conn.query_row(
        "SELECT value_json FROM settings WHERE key = ?1",
        params![key],
        |row| row.get(0),
    )
    .optional()
    .map_err(Into::into)
}

fn setting<T: DeserializeOwned + Default>(&self, key: &str) -> Result<T, StoreError> {
    match self.setting_json(key)? {
        Some(text) => serde_json::from_str(&text).map_err(StoreError::SettingsJson),
        None => Ok(T::default()),
    }
}

fn put_setting_json<T: Serialize>(&self, key: &str, value: &T) -> Result<(), StoreError> {
    let text = serde_json::to_string(value).map_err(StoreError::SettingsJson)?;
    let conn = self.lock()?;
    conn.execute(
        "INSERT INTO settings (key, value_json, updated_at_epoch_ms)
         VALUES (?1, ?2, ?3)
         ON CONFLICT(key) DO UPDATE SET
             value_json = excluded.value_json,
             updated_at_epoch_ms = excluded.updated_at_epoch_ms",
        params![key, text, now_epoch_ms()],
    )?;
    Ok(())
}

fn now_epoch_ms() -> i64 {
    i64::try_from(
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis(),
    )
    .unwrap_or(0)
}
```

6. Run the green checkpoint:

```powershell
$env:RUSTUP_TOOLCHAIN = "stable-x86_64-pc-windows-gnu"
$env:CARGO_TARGET_DIR = "D:\ai-upcycler-target"
$env:PATH = "C:\msys64\mingw64\bin;$env:PATH"
cargo test -p ai-upcycler-core store::tests::settings -- --nocapture
```

7. Checkpoint: three tests pass; only the six named non-secret keys can be written through `put_settings`.

---

## Task 5 — Session rename/delete/cascade store methods (TDD)

**Files:** modify `core/src/store/mod.rs`.

1. Add these failing tests to the store test module:

```rust
#[test]
fn session_rename_updates_title_and_returns_row() {
    let store = store();
    store
        .upsert_model(&NewModel {
            model_id: "llama-8b".into(),
            file_name: "llama.gguf".into(),
            quantization_format: "Q4_K_M".into(),
            total_layers: 32,
            weight_size_bytes: 1_024,
            local_file_path: "D:\\ai-upcycler-target\\models\\llama.gguf".into(),
        })
        .unwrap();
    store.create_session("s1", "Old title", "llama-8b").unwrap();

    let renamed = store.rename_session("s1", "New title").unwrap().unwrap();

    assert_eq!(renamed.title, "New title");
    assert_eq!(store.get_session("s1").unwrap().unwrap().title, "New title");
}

#[test]
fn session_rename_missing_returns_none() {
    let store = store();
    assert!(store.rename_session("missing", "Title").unwrap().is_none());
}

#[test]
fn session_delete_cascades_messages() {
    let store = store();
    store
        .upsert_model(&NewModel {
            model_id: "llama-8b".into(),
            file_name: "llama.gguf".into(),
            quantization_format: "Q4_K_M".into(),
            total_layers: 32,
            weight_size_bytes: 1_024,
            local_file_path: "D:\\ai-upcycler-target\\models\\llama.gguf".into(),
        })
        .unwrap();
    store.create_session("s1", "Title", "llama-8b").unwrap();
    store
        .append_message(&NewChatMessage {
            session_id: "s1".into(),
            role: "user".into(),
            content: "keep me out after delete".into(),
            token_count: 3,
            generation_time_sec: 0.1,
        })
        .unwrap();

    assert!(store.delete_session("s1").unwrap());
    assert!(store.get_session("s1").unwrap().is_none());
    assert!(store.list_messages("s1").unwrap().is_empty());
    assert!(!store.delete_session("s1").unwrap());
}
```

2. Run the red checkpoint:

```powershell
$env:RUSTUP_TOOLCHAIN = "stable-x86_64-pc-windows-gnu"
$env:CARGO_TARGET_DIR = "D:\ai-upcycler-target"
$env:PATH = "C:\msys64\mingw64\bin;$env:PATH"
cargo test -p ai-upcycler-core store::tests::session -- --nocapture
```

3. Implement inside `impl Store`:

```rust
pub fn rename_session(
    &self,
    session_id: &str,
    title: &str,
) -> Result<Option<ChatSessionRow>, StoreError> {
    let changed = {
        let conn = self.lock()?;
        conn.execute(
            "UPDATE chat_sessions SET title = ?1 WHERE session_id = ?2",
            params![title, session_id],
        )?
    };
    if changed == 0 {
        return Ok(None);
    }
    self.get_session(session_id)
}

pub fn delete_session(&self, session_id: &str) -> Result<bool, StoreError> {
    let conn = self.lock()?;
    let deleted = conn.execute(
        "DELETE FROM chat_sessions WHERE session_id = ?1",
        params![session_id],
    )?;
    Ok(deleted > 0)
}
```

4. Run the green checkpoint:

```powershell
$env:RUSTUP_TOOLCHAIN = "stable-x86_64-pc-windows-gnu"
$env:CARGO_TARGET_DIR = "D:\ai-upcycler-target"
$env:PATH = "C:\msys64\mingw64\bin;$env:PATH"
cargo test -p ai-upcycler-core store::tests::session -- --nocapture
```

5. Checkpoint: three tests pass; `PRAGMA foreign_keys = ON` from `Store::from_connection` is still enabled and the cascade is proven.

---

## Task 6 — Settings and recommendations HTTP surface (TDD)

**Files:** create `core/src/api/settings.rs`; modify `core/src/api/mod.rs`, `core/tests/settings_sessions.rs`.

1. Add `pub mod settings;` to `core/src/api/mod.rs`. Add `settings_sessions` imports to `core/tests/settings_sessions.rs`:

```rust
use ai_upcycler_core::api::backend::StubBackend;
use ai_upcycler_core::api::downloads::StubFetcher;
use ai_upcycler_core::api::{router, AppState, HostInfo};
use ai_upcycler_core::discovery::mdns::NodeRegistry;
use ai_upcycler_core::store::{NewModel, Store};
use ai_upcycler_core::telemetry::{TelemetryBus, WorkerHealth};
use ai_upcycler_core::worker_health::{HostMemorySource, HostMemoryUsage, TelemetryCache};
use axum::body::Body;
use axum::http::{header, Request, StatusCode};
use axum::Router;
use serde_json::{json, Value};
use std::path::PathBuf;
use std::sync::Arc;
use tower::ServiceExt;

struct FixedMemory(Option<HostMemoryUsage>);
impl HostMemorySource for FixedMemory {
    fn utilization_pct(&self) -> Option<f64> {
        self.0.map(|usage| {
            (usage.used_bytes as f64 / usage.total_bytes as f64) * 100.0
        })
    }
    fn usage(&self) -> Option<HostMemoryUsage> {
        self.0
    }
}

fn state() -> AppState {
    let store = Arc::new(Store::open_in_memory().expect("store"));
    store
        .upsert_model(&NewModel {
            model_id: "llama-8b".into(),
            file_name: "llama.gguf".into(),
            quantization_format: "Q4_K_M".into(),
            total_layers: 32,
            weight_size_bytes: 1_024,
            local_file_path: "D:\\ai-upcycler-target\\models\\llama.gguf".into(),
        })
        .unwrap();
    let cache = Arc::new(TelemetryCache::new());
    cache.record_worker(WorkerHealth {
        id: "worker-1".into(),
        temp_celsius: None,
        battery_pct: None,
        free_ram_bytes: 8 * 1024 * 1024 * 1024,
        observed_at_epoch_ms: 1_700_000_000_000,
        supports_compute: true,
    });
    AppState::new(
        "cluster-1".into(),
        Arc::new(NodeRegistry::new()),
        store,
        Arc::new(StubBackend::new()),
        TelemetryBus::new(16),
        HostInfo::default(),
    )
    .with_fetcher(Arc::new(StubFetcher::new()))
    .with_models_dir(PathBuf::from(r"D:\ai-upcycler-target\test-models"))
    .with_host_memory(Arc::new(FixedMemory(Some(HostMemoryUsage {
        total_bytes: 16 * 1024 * 1024 * 1024,
        used_bytes: 0,
        available_bytes: 16 * 1024 * 1024 * 1024,
    }))))
    .with_telemetry_cache(cache)
}

async fn send(
    app: Router,
    method: &str,
    uri: &str,
    body: Option<Value>,
) -> (StatusCode, Value) {
    let mut builder = Request::builder().method(method).uri(uri);
    let request = match body {
        Some(value) => builder
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(serde_json::to_vec(&value).unwrap()))
            .unwrap(),
        None => builder.body(Body::empty()).unwrap(),
    };
    let response = app.oneshot(request).await.unwrap();
    let status = response.status();
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let value = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
    (status, value)
}
```

2. Add these failing tests:

```rust
#[tokio::test]
async fn settings_get_put_round_trip() {
    let state = state();
    let app = router(state.clone());
    let body = json!({
        "provider": "openai",
        "model": "gpt-4o",
        "context_tokens": 16384,
        "max_tokens": 2048,
        "show_source_badges": false,
        "compact_mode": true
    });

    let (status, saved) = send(app.clone(), "PUT", "/api/settings", Some(body.clone())).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(saved, body);

    let (status, loaded) = send(app, "GET", "/api/settings", None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(loaded, body);
}

#[tokio::test]
async fn settings_recommendations_use_injected_sources() {
    let (status, body) = send(
        router(state()),
        "GET",
        "/api/settings/recommendations",
        None,
    )
    .await;

    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["context_tokens"], 32768);
    assert_eq!(body["max_tokens"], 8192);
    assert!(body["reasons"].as_array().unwrap().len() >= 3);
}

#[tokio::test]
async fn settings_put_rejects_secret_fields_without_persisting_them() {
    let state = state();
    let app = router(state.clone());
    let body = json!({
        "provider": "openai",
        "model": "gpt-4o",
        "context_tokens": 4096,
        "max_tokens": 1024,
        "show_source_badges": true,
        "compact_mode": false,
        "api_key": "sk-never-persist"
    });

    let (status, error) = send(app.clone(), "PUT", "/api/settings", Some(body)).await;

    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(error["error"]["message"], "unknown field `api_key`, expected one of `provider`, `model`, `context_tokens`, `max_tokens`, `show_source_badges`, `compact_mode`");
    let (_, loaded) = send(app, "GET", "/api/settings", None).await;
    assert!(loaded.get("api_key").is_none());
    assert_eq!(state.store.setting_json("api_key").unwrap(), None);
}
```

3. Run the red checkpoint:

```powershell
$env:RUSTUP_TOOLCHAIN = "stable-x86_64-pc-windows-gnu"
$env:CARGO_TARGET_DIR = "D:\ai-upcycler-target"
$env:PATH = "C:\msys64\mingw64\bin;$env:PATH"
cargo test -p ai-upcycler-core --test settings_sessions settings_ -- --nocapture
```

4. Implement `core/src/api/settings.rs`:

```rust
use crate::api::{ApiError, ApiResult, AppState};
use crate::autoconfig::{
    recommend_settings, AppMemoryProbe, ModelLayerInfo, SettingsRecommendation,
    TelemetryCacheProbe,
};
use crate::settings::AppSettings;
use axum::{extract::State, Json};

pub async fn get(State(state): State<AppState>) -> ApiResult<Json<AppSettings>> {
    Ok(Json(state.store.get_settings()?))
}

pub async fn put(
    State(state): State<AppState>,
    Json(update): Json<AppSettings>,
) -> ApiResult<Json<AppSettings>> {
    update
        .validate()
        .map_err(|message| ApiError::BadRequest(message.into()))?;
    state.store.put_settings(&update)?;
    Ok(Json(update))
}

pub async fn recommendations(
    State(state): State<AppState>,
) -> ApiResult<Json<SettingsRecommendation>> {
    let models: Vec<ModelLayerInfo> = state
        .store
        .list_models()?
        .into_iter()
        .filter(|model| !model.local_file_path.starts_with("virtual:"))
        .map(|model| ModelLayerInfo {
            model_id: model.model_id,
            total_layers: u32::try_from(model.total_layers).unwrap_or(0),
        })
        .collect();
    let memory = AppMemoryProbe::new(state.host_memory.clone());
    let workers = TelemetryCacheProbe::new(state.telemetry_cache.clone());
    Ok(Json(recommend_settings(&memory, &workers, &models)))
}
```

5. Wire routes in `core/src/api/mod.rs`:

```rust
pub mod sessions;
pub mod settings;
```

```rust
use axum::routing::{delete, get, post, put};
```

Add inside `router(state)`:

```rust
.route("/api/settings", get(settings::get).put(settings::put))
.route("/api/settings/recommendations", get(settings::recommendations))
```

Keep the A-provided CORS method list as:

```rust
.allow_methods([Method::GET, Method::POST, Method::PUT, Method::DELETE])
```

6. Run the green checkpoint:

```powershell
$env:RUSTUP_TOOLCHAIN = "stable-x86_64-pc-windows-gnu"
$env:CARGO_TARGET_DIR = "D:\ai-upcycler-target"
$env:PATH = "C:\msys64\mingw64\bin;$env:PATH"
cargo test -p ai-upcycler-core --test settings_sessions settings_ -- --nocapture
```

7. Checkpoint: three tests pass; no secret-bearing JSON field deserializes; CORS permits `PUT` only for allowed localhost/Tauri origins.

---

## Task 7 — Session HTTP surface (TDD)

**Files:** create `core/src/api/sessions.rs`; modify `core/src/api/mod.rs`, `core/tests/settings_sessions.rs`.

1. Add `pub mod sessions;` to `core/src/api/mod.rs` if it was not added in Task 6.

2. Append these failing tests to `core/tests/settings_sessions.rs`:

```rust
#[tokio::test]
async fn session_list_rename_and_delete() {
    let state = state();
    let app = router(state.clone());
    state
        .store
        .create_session("s1", "First prompt", "llama-8b")
        .unwrap();

    let (status, listed) = send(app.clone(), "GET", "/api/sessions", None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(listed["sessions"][0]["session_id"], "s1");
    assert_eq!(listed["sessions"][0]["title"], "First prompt");

    let (status, renamed) = send(
        app.clone(),
        "PUT",
        "/api/sessions/s1",
        Some(json!({ "title": "Renamed in D" })),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(renamed["title"], "Renamed in D");

    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("DELETE")
                .uri("/api/sessions/s1")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::NO_CONTENT);

    let (_, empty) = send(app, "GET", "/api/sessions", None).await;
    assert_eq!(empty["sessions"].as_array().unwrap().len(), 0);
}

#[tokio::test]
async fn session_messages_are_read_from_the_host_store() {
    let state = state();
    state
        .store
        .create_session("s1", "First prompt", "llama-8b")
        .unwrap();
    state
        .store
        .append_message(&ai_upcycler_core::store::NewChatMessage {
            session_id: "s1".into(),
            role: "user".into(),
            content: "persisted message".into(),
            token_count: 2,
            generation_time_sec: 0.0,
        })
        .unwrap();

    let (status, body) = send(
        router(state),
        "GET",
        "/api/sessions/s1/messages",
        None,
    )
    .await;

    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["messages"][0]["content"], "persisted message");
    assert_eq!(body["messages"][0]["message_id"], 1);
}

#[tokio::test]
async fn rename_rejects_empty_and_secret_fields() {
    let (status, body) = send(
        router(state()),
        "PUT",
        "/api/sessions/missing",
        Some(json!({ "title": "   " })),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(body["error"]["message"], "title must be 1..=120 characters");

    let (status, body) = send(
        router(state()),
        "PUT",
        "/api/sessions/missing",
        Some(json!({ "title": "ok", "api_key": "sk-never" })),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert!(body["error"]["message"]
        .as_str()
        .unwrap()
        .starts_with("unknown field"));
}
```

3. Run the red checkpoint:

```powershell
$env:RUSTUP_TOOLCHAIN = "stable-x86_64-pc-windows-gnu"
$env:CARGO_TARGET_DIR = "D:\ai-upcycler-target"
$env:PATH = "C:\msys64\mingw64\bin;$env:PATH"
cargo test -p ai-upcycler-core --test settings_sessions session_ -- --nocapture
```

4. Implement `core/src/api/sessions.rs`:

```rust
use crate::api::{ApiError, ApiResult, AppState};
use crate::store::{ChatMessageRow, ChatSessionRow};
use axum::{
    extract::{Path, State},
    http::StatusCode,
    Json,
};
use serde::{Deserialize, Serialize};

#[derive(Debug, Serialize)]
pub struct SessionList {
    pub sessions: Vec<ChatSessionRow>,
}

#[derive(Debug, Serialize)]
pub struct MessageList {
    pub messages: Vec<ChatMessageRow>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RenameRequest {
    pub title: String,
}

pub async fn list(State(state): State<AppState>) -> ApiResult<Json<SessionList>> {
    Ok(Json(SessionList {
        sessions: state.store.list_sessions()?,
    }))
}

pub async fn messages(
    Path(session_id): Path<String>,
    State(state): State<AppState>,
) -> ApiResult<Json<MessageList>> {
    if state.store.get_session(&session_id)?.is_none() {
        return Err(ApiError::NotFound(format!(
            "session {session_id} not found"
        )));
    }
    Ok(Json(MessageList {
        messages: state.store.list_messages(&session_id)?,
    }))
}

pub async fn rename(
    Path(session_id): Path<String>,
    State(state): State<AppState>,
    Json(request): Json<RenameRequest>,
) -> ApiResult<Json<ChatSessionRow>> {
    let length = request.title.chars().count();
    if length == 0 || length > 120 || request.title.trim().is_empty() {
        return Err(ApiError::BadRequest(
            "title must be 1..=120 characters".into(),
        ));
    }
    state
        .store
        .rename_session(&session_id, request.title.trim())?
        .ok_or_else(|| ApiError::NotFound(format!("session {session_id} not found")))
        .map(Json)
}

pub async fn delete(
    Path(session_id): Path<String>,
    State(state): State<AppState>,
) -> ApiResult<StatusCode> {
    if !state.store.delete_session(&session_id)? {
        return Err(ApiError::NotFound(format!(
            "session {session_id} not found"
        )));
    }
    Ok(StatusCode::NO_CONTENT)
}
```

5. Add these routes inside `router(state)` immediately after the settings routes:

```rust
.route("/api/sessions", get(sessions::list))
.route("/api/sessions/:id/messages", get(sessions::messages))
.route(
    "/api/sessions/:id",
    put(sessions::rename).delete(sessions::delete),
)
```

6. Run the green checkpoint:

```powershell
$env:RUSTUP_TOOLCHAIN = "stable-x86_64-pc-windows-gnu"
$env:CARGO_TARGET_DIR = "D:\ai-upcycler-target"
$env:PATH = "C:\msys64\mingw64\bin;$env:PATH"
cargo test -p ai-upcycler-core --test settings_sessions session_ -- --nocapture
```

7. Checkpoint: three tests pass; session response bodies contain only `ChatSessionRow`/`ChatMessageRow` fields.

---

## Task 8 — Desktop client and persisted chat sessions (TDD)

**Files:** modify `src/lib/types.ts`, `src/lib/api.ts`, `src/components/ChatView.tsx`; create `src/components/SettingsPanel.tsx`.

1. Add these types to `src/lib/types.ts`:

```ts
export interface AppSettings {
  provider: string | null;
  model: string | null;
  context_tokens: number;
  max_tokens: number;
  show_source_badges: boolean;
  compact_mode: boolean;
}

export interface SettingsRecommendation {
  context_tokens: number;
  max_tokens: number;
  reasons: string[];
}

export interface ChatSession {
  session_id: string;
  title: string;
  model_id: string;
  created_at: string;
}

export interface SessionList {
  sessions: ChatSession[];
}

export interface PersistedChatMessage {
  message_id: number;
  session_id: string;
  role: 'system' | 'user' | 'assistant';
  content: string;
  token_count: number;
  generation_time_sec: number;
  created_at: string;
}

export interface SessionMessages {
  messages: PersistedChatMessage[];
}
```

2. Modify `request<T>` in `src/lib/api.ts` so a 204/empty body returns `undefined` instead of throwing JSON parse errors:

```ts
const text = await resp.text();
if (text.length === 0) return undefined as T;
return JSON.parse(text) as T;
```

Keep the existing error-body parsing branch unchanged.

3. Add these exports to `src/lib/api.ts`:

```ts
export function getSettings(): Promise<AppSettings> {
  return request<AppSettings>('/api/settings');
}

export function putSettings(settings: AppSettings): Promise<AppSettings> {
  return request<AppSettings>('/api/settings', {
    method: 'PUT',
    body: JSON.stringify(settings),
  });
}

export function getSettingsRecommendations(): Promise<SettingsRecommendation> {
  return request<SettingsRecommendation>('/api/settings/recommendations');
}

export function listSessions(): Promise<SessionList> {
  return request<SessionList>('/api/sessions');
}

export function listSessionMessages(sessionId: string): Promise<SessionMessages> {
  return request<SessionMessages>(
    `/api/sessions/${encodeURIComponent(sessionId)}/messages`,
  );
}

export function renameSession(sessionId: string, title: string): Promise<ChatSession> {
  return request<ChatSession>(
    `/api/sessions/${encodeURIComponent(sessionId)}`,
    { method: 'PUT', body: JSON.stringify({ title }) },
  );
}

export function deleteSession(sessionId: string): Promise<void> {
  return request<void>(`/api/sessions/${encodeURIComponent(sessionId)}`, {
    method: 'DELETE',
  });
}
```

Update the import list at the top of `src/lib/api.ts` to include `AppSettings`, `ChatSession`, `SessionList`, `SessionMessages`, and `SettingsRecommendation`.

4. Create `src/components/SettingsPanel.tsx` with this contract:

```tsx
import { useEffect, useState } from 'react';
import { getSettingsRecommendations, putSettings } from '../lib/api';
import type { AppSettings, SettingsRecommendation } from '../lib/types';

interface SettingsPanelProps {
  settings: AppSettings | null;
  onSettingsChange: (settings: AppSettings) => void;
}

export function SettingsPanel({ settings, onSettingsChange }: SettingsPanelProps) {
  const [recommendation, setRecommendation] = useState<SettingsRecommendation | null>(null);
  const [draft, setDraft] = useState<AppSettings | null>(settings);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    void getSettingsRecommendations()
      .then(setRecommendation)
      .catch((err: unknown) => setError(err instanceof Error ? err.message : String(err)));
  }, []);

  useEffect(() => setDraft(settings), [settings]);

  if (!draft) return null;

  async function save() {
    try {
      setError(null);
      const saved = await putSettings(draft);
      onSettingsChange(saved);
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
    }
  }

  function applyRecommendation() {
    if (!recommendation) return;
    const next: AppSettings = {
      ...draft,
      context_tokens: recommendation.context_tokens,
      max_tokens: recommendation.max_tokens,
    };
    setDraft(next);
    void putSettings(next).then(onSettingsChange);
  }

  return (
    <section aria-label="Generation settings" className="rounded-xl border border-[#475569] bg-[#1B2336] p-3 text-[#94A3B8]">
      <h3 className="mb-2 text-xs font-semibold text-slate-100">Generation settings</h3>
      {recommendation && (
        <button
          type="button"
          onClick={applyRecommendation}
          className="min-h-11 rounded-lg border border-[#22C55E] px-3 text-xs text-[#22C55E] focus-visible:outline focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-[#22C55E]"
          aria-label="Apply recommended context and output limits"
        >
          Apply recommended: {recommendation.context_tokens}/{recommendation.max_tokens}
        </button>
      )}
      <label className="mt-3 block text-xs">
        Context tokens
        <input
          aria-label="Context tokens"
          type="number"
          min={1024}
          max={65536}
          value={draft.context_tokens}
          onChange={(event) =>
            setDraft({ ...draft, context_tokens: Number(event.target.value) })
          }
          className="mt-1 min-h-11 w-full rounded-lg border border-[#475569] bg-[#0F172A] px-2 py-1 text-[#E2E8F0] focus-visible:outline focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-[#22C55E]"
        />
      </label>
      <label className="mt-2 block text-xs">
        Max tokens
        <input
          aria-label="Max tokens"
          type="number"
          min={1}
          max={32768}
          value={draft.max_tokens}
          onChange={(event) => setDraft({ ...draft, max_tokens: Number(event.target.value) })}
          className="mt-1 min-h-11 w-full rounded-lg border border-[#475569] bg-[#0F172A] px-2 py-1 text-[#E2E8F0] focus-visible:outline focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-[#22C55E]"
        />
      </label>
      <button
        type="button"
        onClick={() => void save()}
        className="mt-3 min-h-11 rounded-lg bg-[#22C55E] px-3 text-xs font-semibold text-[#0F172A] focus-visible:outline focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-[#22C55E]"
        aria-label="Save generation settings"
      >
        Save settings
      </button>
      {error && <p className="mt-2 text-xs text-[#EF4444]">{error}</p>}
    </section>
  );
}
```

5. Modify `src/components/ChatView.tsx`:

- Change `DisplayMessage.id` from `number` to `string`; remove `let nextId = 1` and use `crypto.randomUUID()`.
- Add state: `sessions: ChatSession[]`, `activeSessionId: string` initialized with `crypto.randomUUID()`, `settings: AppSettings | null`, and `settingsOpen: boolean`.
- On mount, call `getSettings()`, `listSessions()`, then `listSessionMessages(activeSessionId)` when a saved session is selected. Do not copy session or message state into localStorage.
- Add a session `<select aria-label="Chat session">` with `key={session.session_id}` options and handlers:
  - selecting a session loads messages from `GET /api/sessions/:id/messages`;
  - "Rename session" uses an inline `<input aria-label="Session title">` and `renameSession`;
  - "Delete session" uses `deleteSession`, then selects a fresh `crypto.randomUUID()` and clears messages.
- Pass `session_id: activeSessionId` and `max_tokens: settings?.max_tokens ?? 512` to `streamChat`.
- After the first successful turn per session, call `listSessions()` so the new persisted title appears.
- Render `<SettingsPanel settings={settings} onSettingsChange={setSettings} />` behind a button with `aria-label="Open generation settings"`.
- Preserve B's provider switcher and source badge; do not remove or rename them.

6. Run the red checkpoint before changing TS files: `npx tsc --noEmit` must fail because `SettingsPanel.tsx` and the new client exports do not exist. Then implement steps 1–5 and run the green checkpoint:

```powershell
$env:npm_config_cache = "D:\ai-upcycler-npm-cache"
npx tsc --noEmit
```

7. Playwright MCP checkpoint against `http://127.0.0.1:8080`:

- Send one message; reload the page; select the persisted session; assert the message returns from `/api/sessions/:id/messages`.
- Rename the session through the UI; reload; assert the new title.
- Delete the session; reload; assert it is gone.
- Open settings; apply recommendation; assert the visible values equal `GET /api/settings`.
- Repeat at 375, 768, 1024, and 1440 px; assert focus rings are visible and no control is clipped.

8. Checkpoint: `npx tsc --noEmit` passes and all five Playwright assertions pass. No browser storage key contains message text or session records.

---

## Task 9 — Android DataStore UI preferences (TDD)

**Files:** modify `mobile-worker/android/app/build.gradle.kts`; create `UiPreferences.kt`, `UiPreferencesStore.kt`, `UiPreferencesStoreTest.kt`.

1. Add these dependencies to `mobile-worker/android/app/build.gradle.kts`:

```kotlin
implementation("androidx.datastore:datastore-preferences:1.1.1")
implementation("org.jetbrains.kotlinx:kotlinx-coroutines-android:1.8.1")
testImplementation("androidx.test:core:1.6.1")
testImplementation("org.jetbrains.kotlinx:kotlinx-coroutines-test:1.8.1")
testImplementation("org.robolectric:robolectric:4.12.2")
```

Add inside `android { ... }`:

```kotlin
testOptions {
    unitTests.isIncludeAndroidResources = true
}
```

2. Create the failing JUnit test `UiPreferencesStoreTest.kt`:

```kotlin
package org.aiupcycler.worker.preferences

import android.content.Context
import androidx.test.core.app.ApplicationProvider
import kotlinx.coroutines.flow.first
import kotlinx.coroutines.test.runTest
import org.junit.Assert.assertEquals
import org.junit.Test
import org.junit.runner.RunWith
import org.robolectric.RobolectricTestRunner

@RunWith(RobolectricTestRunner::class)
class UiPreferencesStoreTest {
    private val context: Context = ApplicationProvider.getApplicationContext()

    @Test
    fun defaults_are_stable() = runTest {
        val store = UiPreferencesStore(context)

        assertEquals(UiPreferences(), store.flow.first())
    }

    @Test
    fun writes_only_ui_preference_keys() = runTest {
        val store = UiPreferencesStore(context)

        store.setCompactMode(true)
        store.setShowSourceBadges(false)
        store.setAutoApplyRecommendations(true)

        assertEquals(
            UiPreferences(
                compactMode = true,
                showSourceBadges = false,
                autoApplyRecommendations = true,
            ),
            store.flow.first(),
        )
        assertEquals(
            setOf("compact_mode", "show_source_badges", "auto_apply_recommendations"),
            store.writtenKeys(),
        )
    }
}
```

3. Run the red checkpoint from `mobile-worker/android`:

```powershell
.\gradlew.bat :app:testDebugUnitTest --tests "org.aiupcycler.worker.preferences.UiPreferencesStoreTest"
```

4. Implement `UiPreferences.kt`:

```kotlin
package org.aiupcycler.worker.preferences

data class UiPreferences(
    val compactMode: Boolean = false,
    val showSourceBadges: Boolean = true,
    val autoApplyRecommendations: Boolean = false,
)
```

5. Implement `UiPreferencesStore.kt`:

```kotlin
package org.aiupcycler.worker.preferences

import android.content.Context
import androidx.datastore.core.DataStore
import androidx.datastore.preferences.core.Preferences
import androidx.datastore.preferences.core.booleanPreferencesKey
import androidx.datastore.preferences.core.edit
import androidx.datastore.preferences.preferencesDataStore
import kotlinx.coroutines.flow.Flow
import kotlinx.coroutines.flow.first
import kotlinx.coroutines.flow.map

private val Context.uiPreferencesDataStore: DataStore<Preferences> by preferencesDataStore(
    name = "ui_preferences",
)

class UiPreferencesStore(context: Context) {
    private val dataStore = context.applicationContext.uiPreferencesDataStore

    val flow: Flow<UiPreferences> = dataStore.data.map { prefs ->
        UiPreferences(
            compactMode = prefs[COMPACT_MODE] ?: false,
            showSourceBadges = prefs[SHOW_SOURCE_BADGES] ?: true,
            autoApplyRecommendations = prefs[AUTO_APPLY_RECOMMENDATIONS] ?: false,
        )
    }

    suspend fun setCompactMode(value: Boolean) {
        dataStore.edit { it[COMPACT_MODE] = value }
    }

    suspend fun setShowSourceBadges(value: Boolean) {
        dataStore.edit { it[SHOW_SOURCE_BADGES] = value }
    }

    suspend fun setAutoApplyRecommendations(value: Boolean) {
        dataStore.edit { it[AUTO_APPLY_RECOMMENDATIONS] = value }
    }

    suspend fun writtenKeys(): Set<String> =
        dataStore.data.first().asMap().keys.map { it.name }.toSet()

    companion object {
        private val COMPACT_MODE = booleanPreferencesKey("compact_mode")
        private val SHOW_SOURCE_BADGES = booleanPreferencesKey("show_source_badges")
        private val AUTO_APPLY_RECOMMENDATIONS = booleanPreferencesKey("auto_apply_recommendations")
    }
}
```

6. Run the green checkpoint from `mobile-worker/android`:

```powershell
.\gradlew.bat :app:testDebugUnitTest --tests "org.aiupcycler.worker.preferences.UiPreferencesStoreTest"
```

7. Checkpoint: two tests pass; the DataStore file name is `ui_preferences`; only the three named keys appear in `writtenKeys()`.

---

## Task 10 — Android preferences UI and B-surface integration

**Files:** create `ui/PreferencesScreen.kt`; modify B-created `ui/WorkerApp.kt`, `ui/ChatScreen.kt`, and `api/LanClient.kt`.

1. Verify the B files exist and contain the expected composables:

```powershell
rg -n "fun ChatScreen|fun WorkerApp" mobile-worker\android\app\src\main\java\org\aiupcycler\worker\ui
rg -n "class LanClient|object LanClient" mobile-worker\android\app\src\main\java\org\aiupcycler\worker\api
```

2. Add a failing compile-time expectation by adding this call to B's `WorkerApp.kt` before implementing `PreferencesScreen`:

```kotlin
PreferencesScreen(store = preferencesStore, onClose = { /* navigate back */ })
```

Run the red checkpoint from `mobile-worker/android`:

```powershell
.\gradlew.bat :app:compileDebugKotlin
```

It must fail because `PreferencesScreen` is unresolved.

3. Implement `ui/PreferencesScreen.kt` with spec tokens and 48 dp targets:

```kotlin
package org.aiupcycler.worker.ui

import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.padding
import androidx.compose.material3.Switch
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.collectAsState
import androidx.compose.runtime.getValue
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.unit.dp
import kotlinx.coroutines.launch
import org.aiupcycler.worker.preferences.UiPreferencesStore

private val Background = Color(0xFF0F172A)
private val Card = Color(0xFF1B2336)
private val Muted = Color(0xFF94A3B8)
private val Accent = Color(0xFF22C55E)

@Composable
fun PreferencesScreen(store: UiPreferencesStore, onClose: () -> Unit) {
    val prefs by store.flow.collectAsState(
        initial = org.aiupcycler.worker.preferences.UiPreferences(),
    )
    val scope = rememberCoroutineScope()
    Column(
        Modifier
            .fillMaxSize()
            .background(Background)
            .padding(16.dp),
    ) {
        Text("Preferences", color = Color(0xFFE2E8F0))
        preferenceToggle(
            label = "Compact mode",
            checked = prefs.compactMode,
            onChange = { value -> scope.launch { store.setCompactMode(value) } },
        )
        preferenceToggle(
            label = "Show source badges",
            checked = prefs.showSourceBadges,
            onChange = { value -> scope.launch { store.setShowSourceBadges(value) } },
        )
        preferenceToggle(
            label = "Auto-apply recommended limits",
            checked = prefs.autoApplyRecommendations,
            onChange = { value -> scope.launch { store.setAutoApplyRecommendations(value) } },
        )
        Text(
            "Close",
            color = Accent,
            modifier = Modifier
                .heightIn(min = 48.dp)
                .clickable(onClick = onClose)
                .semantics { contentDescription = "Close preferences" },
        )
    }
}

@Composable
private fun preferenceToggle(
    label: String,
    checked: Boolean,
    onChange: (Boolean) -> Unit,
) {
    Row(
        modifier = Modifier
            .fillMaxWidth()
            .heightIn(min = 48.dp)
            .padding(vertical = 4.dp)
            .background(Card)
            .clickable { onChange(!checked) },
        verticalAlignment = Alignment.CenterVertically,
    ) {
        Text(
            label,
            color = Muted,
            modifier = Modifier
                .weight(1f)
                .padding(start = 8.dp),
        )
        Switch(
            checked = checked,
            onCheckedChange = onChange,
            modifier = Modifier
                .padding(end = 8.dp)
                .semantics { contentDescription = label },
        )
    }
}
```

4. In B's `WorkerApp.kt`, construct `UiPreferencesStore(LocalContext.current)` once and pass it to both `PreferencesScreen` and `ChatScreen`.

5. In B's `ChatScreen.kt`, add a `preferencesStore: UiPreferencesStore` parameter and consume `preferencesStore.flow.collectAsState(...)`:

- when `showSourceBadges == false`, hide B's source badge;
- when `compactMode == true`, reduce message bubble vertical padding from B's current value to `4.dp` while keeping 48 dp minimum touch targets on controls;
- when `autoApplyRecommendations == true`, apply `GET /api/settings/recommendations` through B's LAN client before the first generation of a new session;
- keep chat history reads/writes through the B LAN client, never through DataStore.

In B's `api/LanClient.kt`, add these calls using the existing bearer-token header and the same JSON serializer B already uses for chat requests:

```kotlin
data class SettingsDto(
    val provider: String?,
    val model: String?,
    val context_tokens: Int,
    val max_tokens: Int,
    val show_source_badges: Boolean,
    val compact_mode: Boolean,
)

data class SettingsRecommendationDto(
    val context_tokens: Int,
    val max_tokens: Int,
    val reasons: List<String>,
)

suspend fun getSettings(): SettingsDto                         // GET /api/settings
suspend fun putSettings(settings: SettingsDto): SettingsDto    // PUT /api/settings
suspend fun getSettingsRecommendations(): SettingsRecommendationDto // GET /api/settings/recommendations
```

`autoApplyRecommendations` calls `getSettings()`, then `getSettingsRecommendations()`, then `putSettings()` with `context_tokens`/`max_tokens` replaced; it never constructs a secret field.

6. Run the compile and unit-test checkpoint from `mobile-worker/android`:

```powershell
.\gradlew.bat :app:compileDebugKotlin
.\gradlew.bat :app:testDebugUnitTest
```

7. Device checkpoint using the existing tunnel:

```powershell
adb connect 192.168.1.9:5555
.\gradlew.bat :app:assembleDebug
adb -s 192.168.1.9:5555 install -r app\build\outputs\apk\debug\app-debug.apk
adb -s 192.168.1.9:5555 shell am start -n org.aiupcycler.worker/.MainActivity
New-Item -ItemType Directory -Force -Path "D:\ai-upcycler-target\qa" | Out-Null
adb -s 192.168.1.9:5555 exec-out screencap -p > D:\ai-upcycler-target\qa\android-preferences.png
```

8. On-device assertions from the screenshot and live UI: all three toggles are visible, each row is at least 48 dp tall, labels remain legible on `#0F172A`/`#1B2336`, compact mode changes chat padding without shifting button bounds, and the paired/control screen remains reachable from B's overflow menu.

9. Checkpoint: Kotlin compiles, all Android unit tests pass, and the screenshot exists at `D:\ai-upcycler-target\qa\android-preferences.png`.

---

## Task 11 — API documentation and secret-boundary audit

**Files:** modify `docs/api.md`.

1. Add `§2.6 Settings and recommendations` with these exact contracts:

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

State explicitly that provider secrets are handled only by A's write-only `/api/settings/secrets/{provider}` endpoint and never appear in settings responses.

2. Add `§2.7 Sessions` with:

```text
GET /api/sessions → 200 { "sessions": [ChatSessionRow] }
GET /api/sessions/{id}/messages → 200 { "messages": [ChatMessageRow] }
PUT /api/sessions/{id} ← { "title": "1..=120 characters" } → 200 ChatSessionRow
DELETE /api/sessions/{id} → 204
Missing session → 404 with the existing error envelope.
```

3. Audit the changed source with these commands:

```powershell
rg -n "api_key|secret|token|password" core/src/settings.rs core/src/autoconfig.rs core/src/api/settings.rs core/src/api/sessions.rs src/lib/types.ts src/lib/api.ts src/components/SettingsPanel.tsx
rg -n "chat_messages|conversation|history|session" mobile-worker\android\app\src\main\java\org\aiupcycler\worker\preferences
```

4. Required audit result:

- The first command matches only rejection strings/docs, never a serialized field or stored key.
- The second command returns no matches; Android preferences contain no chat history surface.

5. Checkpoint: documentation added and both audit commands satisfy the required results.

---

## Task 12 — Full verification and completion gate

**Files:** none modified.

1. Clear stale shell logs and run the full Rust/desktop gate from the worktree root:

```powershell
Get-ChildItem -Path "C:\Users\darsh\.local\share\opencode\shell" -File | Remove-Item -Force
$env:RUSTUP_TOOLCHAIN = "stable-x86_64-pc-windows-gnu"
$env:CARGO_TARGET_DIR = "D:\ai-upcycler-target"
$env:PATH = "C:\msys64\mingw64\bin;$env:PATH"
$env:npm_config_cache = "D:\ai-upcycler-npm-cache"

cargo fmt --all -- --check
cargo test -p ai-upcycler-core
cargo clippy -p ai-upcycler-core --all-targets -- -D warnings
cargo build --release -p ai-upcycler-desktop
npx tsc --noEmit
```

2. Run the full Android gate from `mobile-worker/android`:

```powershell
.\gradlew.bat :app:testDebugUnitTest
.\gradlew.bat :app:assembleDebug
```

3. Run the spec §13 verification matrix:

- Unit tests: `autoconfig`, `settings`, store settings/session methods, API router tests, DataStore tests — all green from step 1/2.
- API smoke: `GET /api/settings`, `PUT /api/settings`, `GET /api/settings/recommendations`, `GET /api/sessions`, `GET /api/sessions/:id/messages`, `PUT /api/sessions/:id`, `DELETE /api/sessions/:id` exercised through the real axum router in `core/tests/settings_sessions.rs`.
- Desktop Playwright: repeat Task 8's five assertions on the release-built browser surface served from `127.0.0.1:8080`.
- Android ADB: repeat Task 10's screenshot check on `192.168.1.9:5555`.
- No phase is called complete if any command fails or any screenshot contradicts the acceptance list.

4. Final secret-boundary proof:

```powershell
rg -n "sk-|api_key|password" core/tests/settings_sessions.rs
```

Expected: matches only inside rejected request bodies and assertion strings; no stored setting key or response field contains those values.

5. Completion checkpoint: every command in steps 1–4 passes, `D:\ai-upcycler-target\qa` contains the D QA screenshots, and no file outside the declared Files tables was modified by D.

## Self-review

- Spec §9 covered: `core/src/autoconfig.rs` with host memory + `TelemetryCache` workers + model layer count; `GET /api/settings/recommendations`; `settings` key/value JSON table; `GET|PUT /api/settings`; session list/rename/delete plus message reads; Android DataStore limited to UI preferences.
- Spec §12 covered: desktop/Android tokens, stable keys, accessible names, visible focus rings, reduced-motion-compatible controls, 48 dp Android targets, `LazyColumn`/Compose requirement preserved from B.
- Spec §13 covered: injected-trait unit tests, axum router smoke tests, Playwright desktop checks, wireless ADB screenshots at `192.168.1.9:5555`, no phase complete without checks.
- Spec §14 covered: artifacts on `D:\ai-upcycler-target`, npm cache on `D:\ai-upcycler-npm-cache`, shell log cleanup before large builds.
- A dependency preserved: `SecretStore`, provider/secret routes, `StoreError::Conflict`, and post-A `AppState` fields are gates, not reimplemented.
- B dependency preserved: Compose chat/provider UI, LAN REST client, bearer token, and Keystore secret writes are gates; D only adds preferences and consumes host sessions.
- Secret boundary proven by `deny_unknown_fields`, exact error strings, router tests, SQLite key assertions, and source audits.
- Data boundary proven by the DataStore key assertion and the absence of chat/session keys in Android preference code.
