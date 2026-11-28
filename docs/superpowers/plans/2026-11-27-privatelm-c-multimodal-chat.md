# Sub-project C: Multimodal Chat Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Let desktop and Android chat turns carry image attachments without breaking text-only clients, with images stored content-addressed and sent only to vision-capable cloud providers.

**Architecture:** Extend `ChatMessage.content` from `String` to an untagged `MessageContent` union, add a `MediaStore` and `attachments` table, and keep all image turns behind the `AI_UPCYCLER_MULTIMODAL=1` gate. Local, cluster, and tool turns fail closed with explicit 400/stream errors; cloud adapters map normalized image parts to each provider's native payload using A's `CloudProvider::chat_stream` contract. Desktop and Android composers render attachment chips from spec §12 tokens after the compatibility suite is green.

**Tech Stack:** Rust/axum/tokio/serde/SQLite with `sha2` + `base64`; TypeScript/React/Tauri desktop; Jetpack Compose + LAN REST Android client; Playwright MCP and wireless ADB verification.

**Spec:** `docs/superpowers/specs/2026-11-27-privatelm-style-extension-design.md` §§8, 12–14.

## Global Constraints

- Work only in `D:\ai-upcycler-worktrees\cloud-provider-bridge`; modify only files listed in a task's **Files** block.
- Do not commit or push during plan authoring. Commit steps below are for the future executor only.
- Do not dispatch subagents to write code. Verification commands are run by the executor at the stated task step, not during plan authoring.
- Rust commands always use:

  ```powershell
  $env:RUSTUP_TOOLCHAIN = "stable-x86_64-pc-windows-gnu"
  $env:CARGO_TARGET_DIR = "D:\ai-upcycler-target"
  $env:PATH = "C:\msys64\mingw64\bin;$env:PATH"
  ```

- npm commands always use `$env:npm_config_cache = "D:\ai-upcycler-npm-cache"`.
- Browser artifacts and Android screenshots go to `D:\ai-upcycler-temp\`; create it with `New-Item -ItemType Directory -Force -Path D:\ai-upcycler-temp` if absent. Never use `/tmp`.
- Browser verification uses Playwright MCP at desktop viewport `1280x800`; Android verification uses `adb connect 192.168.1.9:5555`.
- Dependency A: `docs/superpowers/plans/2026-11-27-cloud-provider-bridge.md` must land before Tasks 6–9. Dependency B: spec §7 Android Compose chat surface and `design-system/ai-upcycler/MASTER.md` must land before Task 11. If a dependency is absent, stop that task and report it; do not invent a substitute.
- Text-only compatibility invariant: `{"role":"user","content":"hello"}` serializes to exactly the same JSON. Missing or `null` content becomes `MessageContent::Text(String::new())`.
- Agent compatibility invariants: requests omitting `provider` keep using `RoutePolicy`; tool turns stay host-only; loopback stays unauthenticated.
- The OpenAI-compatible `/v1` agent endpoint stays text-only per `docs/AGENTS.md` §8. Image parts sent to that surface return `400 {"error":"image_attachments_require_vision_provider"}` before any host-engine or tool call; no session or user turn is stored on that path.
- Feature gate: `AI_UPCYCLER_MULTIMODAL=1`. `AppState.multimodal_enabled` defaults to `false`; `with_multimodal(bool)` sets it. Gate-off rejects image parts with `400 {"error":"multimodal_disabled"}` before session writes and returns `404` from `/api/media`.
- Fail-closed error bodies (all HTTP 400): `image_attachments_require_vision_provider` (local/cluster/tool), `provider_lacks_vision_support` (resolved model has `supports_vision=false`), `no_vision_capable_model_loaded` (no vision route), `invalid_image_attachment` (MIME/magic/size/hash failure).
- Streaming image failure emits exactly one `GenerationChunk` with `delta = ""`, `error = Some("<code>")`, then closes the stream.
- Allowed images: `image/png` (`89 50 4E 47 0D 0A 1A 0A`), `image/jpeg` (`FF D8 FF`), `image/webp` (`RIFF` + `WEBP` at offset 8), `image/gif` (`GIF87a` or `GIF89a`). Max 10 MiB per file, max 4 per message.
- Media path: `root/hash[0..2]/hash[2..]` under `<app data dir>/media`; hash is lowercase hex SHA-256 of original bytes. No image bytes in logs, errors, or `GenerationChunk`s.
- §12 tokens: background `#0F172A`, chip surface `#1B2336`, border `#475569`, muted text `#94A3B8`, accent `#22C55E`, destructive `#EF4444`, transition 150–300 ms with reduced-motion override, visible 2 px accent focus ring, 48 dp Android minimum touch target.
- No placeholders of any kind. Every step contains exact code, exact commands, exact expected output, or exact observable assertions.

---

## Files Map

| File | Responsibility |
|---|---|
| `core/src/api/backend.rs` | Owns `MessageContent`, `ContentPart`, `ChatMessage`, `null_as_empty`, `GenerationRequest`, `StubBackend` |
| `core/src/api/chat.rs` | Gate checks, validation order, persistence, token/session-title paths, stream dispatch |
| `core/src/api/mod.rs` | `AppState` gate/media fields, `/api/media` route, router wiring |
| `core/src/api/distributed.rs` | ChatML rendering and cluster image refusal |
| `core/src/api/llama.rs` | Host-engine body rendering for text/parts |
| `core/src/media/mod.rs` | **Create** `MediaStore`, `MediaRef`, `MediaError` |
| `core/src/lib.rs` | Register `pub mod media;` |
| `core/src/store/schema.sql` | `attachments` table + index |
| `core/src/store/mod.rs` | Content union in rows, atomic message+attachment insert |
| `core/tests/compat_content.rs` | **Create** plain-string and missing/`null` compatibility tests |
| `core/tests/multimodal_router.rs` | **Create** real axum router smoke tests |
| `core/src/providers/messages.rs` | **Create** pure OpenAI/Gemini/Anthropic mappers (A's provider dir) |
| A's adapter files under `core/src/providers/` | `chat_stream` body uses `MediaStore` and mappers |
| `src/lib/types.ts` | `ChatContent`, `ContentPart`, `MediaRef` |
| `src/lib/api.ts` | `uploadImage`, `sendChat` union |
| `src/components/ChatView.tsx` | Composer chips and gate-off hiding |
| `src-tauri/src/lib.rs` | Derive `<app data dir>/media`, set gate from env |
| B's Chat files under `mobile-worker/android/app/src/main/java/org/aiupcycler/worker/chat/` | Add attachment chip composable and upload call |
| `docs/api.md` | Document `/api/media`, content union, error bodies |

`core/Cargo.toml` already has `sha2` and `base64`; do not add dependencies. `design-system/ai-upcycler/MASTER.md` is B-produced and read-only for C.

## Task Order and Dependencies

```
Task 1 ──► Task 2 ──► Task 3 ──► Task 4 ──► Task 5 ──► Task 6 ──► Task 7 ──► Task 8 ──► Task 9 ──► Task 10 ──► Task 11 ──► Task 12
```

- Task 1 before all UI (spec §14).
- Tasks 6–9 depend on A.
- Task 11 depends on B.
- Task 12 depends on all above.

Each task ends with a Checkpoint. Do not begin a task whose upstream Checkpoint is not green.

---

### Task 1: Compatibility tests and feature-gate skeleton

**Files:**
- Create: `core/tests/compat_content.rs`
- Modify: `core/src/api/backend.rs`, `core/src/api/mod.rs`
- Test: `core/tests/compat_content.rs`

**Interfaces:**
- Consumes: existing `ChatMessage`, `GenerationRequest`, `AppState`.
- Produces: `MessageContent::{Text, Parts}`, `ContentPart::{Text, Image}`, `AppState.multimodal_enabled`, `AppState::with_multimodal(bool)`; later tasks rely on these exact names.

**Dependencies:** None. Must complete before any UI file is touched.

**Acceptance criteria:** A plain string round-trips byte-for-byte; missing/`null` content becomes empty text; an agent request without `provider` deserializes; gate setter compiles and defaults off.

- [ ] **Step 1: Write the failing tests**

Create `core/tests/compat_content.rs` with exactly:

```rust
use ai_upcycler_core::api::backend::{ChatMessage, GenerationRequest, MessageContent};
use serde_json::json;

#[test]
fn plain_string_content_round_trips_unchanged() {
    let msg: ChatMessage = serde_json::from_value(json!({"role":"user","content":"hello"})).unwrap();
    assert_eq!(msg.content, MessageContent::Text("hello".into()));
    assert_eq!(
        serde_json::to_value(&msg).unwrap(),
        json!({"role":"user","content":"hello"})
    );
}

#[test]
fn missing_and_null_content_become_empty_text() {
    for value in [json!({"role":"user"}), json!({"role":"user","content":null})] {
        let msg: ChatMessage = serde_json::from_value(value).unwrap();
        assert_eq!(msg.content, MessageContent::Text(String::new()));
    }
}

#[test]
fn agent_request_omitting_provider_still_deserializes() {
    let req: GenerationRequest = serde_json::from_value(json!({
        "messages":[{"role":"user","content":"hello"}],
        "max_tokens":64
    })).unwrap();
    assert!(req.provider.is_none());
}
```

- [ ] **Step 2: Run tests to verify they fail**

```powershell
cargo test -p ai-upcycler-core --test compat_content -- --exact plain_string_content_round_trips_unchanged
```

Expected: compile error `cannot find type MessageContent in module backend`.

- [ ] **Step 3: Write minimal implementation**

In `core/src/api/mod.rs`, add to `AppState`:

```rust
pub multimodal_enabled: bool,
```

and:

```rust
impl AppState {
    pub fn with_multimodal(mut self, enabled: bool) -> Self {
        self.multimodal_enabled = enabled;
        self
    }
}
```

Initialize the new field to `false` in every existing `AppState` constructor.

In `core/src/api/backend.rs`, add exactly:

```rust
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum MessageContent {
    Parts(Vec<ContentPart>),
    Text(String),
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ContentPart {
    Text { text: String },
    Image {
        media_hash: String,
        mime_type: String,
        file_name: String,
        byte_count: u64,
    },
}
```

Change `ChatMessage.content` to:

```rust
#[serde(default, deserialize_with = "null_as_empty")]
pub content: MessageContent,
```

Rewrite `null_as_empty` to deserialize through `Option<String>` and return `MessageContent::Text(String::new())` for `None`, otherwise `MessageContent::Text(s)`. Variant order `Parts` before `Text` is required.

- [ ] **Step 4: Run tests to verify they pass**

```powershell
cargo fmt --all -- --check
cargo test -p ai-upcycler-core --test compat_content
```

Expected: all three tests PASS; `cargo fmt --check` exits 0.

- [ ] **Step 5: Commit**

```powershell
git add core/tests/compat_content.rs core/src/api/backend.rs core/src/api/mod.rs
git commit -m "test: lock text-only chat compatibility and multimodal gate"
```

**Checkpoint 1:** `compat_content` green; no UI file touched; gate defaults off. Continue only then.

---

### Task 2: Migrate all content call sites

**Files:**
- Modify: `core/src/api/backend.rs`, `core/src/api/chat.rs`, `core/src/api/distributed.rs`, `core/src/api/llama.rs`, `core/src/store/mod.rs`
- Test: inline `#[cfg(test)]` modules in `backend.rs` and `llama.rs`

**Interfaces:**
- Consumes: `MessageContent`, `ContentPart` from Task 1.
- Produces: `content_text(&MessageContent) -> &str` helper in `chat.rs`; updated `NewChatMessage.content` and `ChatMessageRow.content`; `StubBackend` image refusal string `image_attachments_require_vision_provider`.

**Dependencies:** Task 1.

**Acceptance criteria:** Every former `String` content field is `MessageContent`; stub echoes text and rejects images; host-engine body contains no base64; text-only sessions still persist identical JSON.

- [ ] **Step 1: Write the failing tests**

Add to `core/src/api/backend.rs` inline tests:

```rust
fn req_from_content(content: serde_json::Value) -> GenerationRequest {
    serde_json::from_value(serde_json::json!({
        "messages":[{"role":"user","content":content}],
        "max_tokens":64
    })).unwrap()
}

#[test]
fn stub_backend_echoes_text_content() {
    let out = StubBackend.reply(&req_from_content(serde_json::json!("ping"))).unwrap();
    assert_eq!(out.content, MessageContent::Text("ping".into()));
}

#[test]
fn stub_backend_rejects_image_parts() {
    let req = req_from_content(serde_json::json!([{
        "type":"image",
        "media_hash":"ab".repeat(32),
        "mime_type":"image/png",
        "file_name":"a.png",
        "byte_count":4
    }]));
    assert_eq!(
        StubBackend.reply(&req),
        Err("image_attachments_require_vision_provider".into())
    );
}
```

In `core/src/api/llama.rs` inline tests, replace every `content: "hi".into()` literal with `content: MessageContent::Text("hi".into())` and add:

```rust
#[test]
fn host_engine_body_serializes_parts_without_raw_bytes() {
    let body = build_host_body(&req_from_content(serde_json::json!([{
        "type":"image",
        "media_hash":"ab".repeat(32),
        "mime_type":"image/png",
        "file_name":"a.png",
        "byte_count":4
    }])));
    let s = serde_json::to_string(&body).unwrap();
    assert!(s.contains("\"type\":\"image\""));
    assert!(!s.contains("base64"));
}
```

Define the same `req_from_content` helper in `llama.rs` inline tests.

- [ ] **Step 2: Run tests to verify they fail**

```powershell
cargo test -p ai-upcycler-core --lib -- --exact api::backend::tests::stub_backend_rejects_image_parts
cargo test -p ai-upcycler-core --lib -- --exact api::llama::tests::host_engine_body_serializes_parts_without_raw_bytes
```

Expected: type errors on `String` assignments (red).

- [ ] **Step 3: Write minimal implementation**

- `StubBackend::reply`: match `&MessageContent`; `Text(t)` echoes exactly as before; `Parts(_)` returns `Err("image_attachments_require_vision_provider".into())`.
- In `core/src/api/chat.rs`, add:

```rust
fn content_text(c: &MessageContent) -> &str {
    match c {
        MessageContent::Text(t) => t,
        MessageContent::Parts(parts) => parts.iter().find_map(|p| match p {
            ContentPart::Text { text } => Some(text.as_str()),
            _ => None,
        }).unwrap_or(""),
    }
}
```

Use `content_text(&msg.content)` for token counting, session-title extraction, metrics, and stub response text. Persist the full `MessageContent` unchanged.
- In `core/src/api/distributed.rs`, render `Text(t)` as `{"role":..,"content":t}` and `Parts(ps)` as `{"role":..,"content":[{"type":"text","text":t},...]}`. On any `ContentPart::Image`, return `Err("image_attachments_require_vision_provider")` before constructing the cluster body.
- In `core/src/store/mod.rs`, set `NewChatMessage.content: MessageContent` and `ChatMessageRow.content: MessageContent`; bind SQLite values with `serde_json::to_string(&content)` and parse with `serde_json::from_str`, preserving the old text string shape.
- In `core/src/api/llama.rs`, render image parts as `{"type":"image","media_hash":"<hash>"}` with no bytes.

- [ ] **Step 4: Run tests to verify they pass**

```powershell
cargo fmt --all -- --check
cargo test -p ai-upcycler-core --test compat_content
cargo test -p ai-upcycler-core --lib -- --exact api::backend::tests::stub_backend_echoes_text_content
cargo test -p ai-upcycler-core --lib -- --exact api::backend::tests::stub_backend_rejects_image_parts
cargo test -p ai-upcycler-core --lib -- --exact api::llama::tests::host_engine_body_serializes_parts_without_raw_bytes
```

Expected: all PASS; compat suite still green.

- [ ] **Step 5: Commit**

```powershell
git add core/src/api/backend.rs core/src/api/chat.rs core/src/api/distributed.rs core/src/api/llama.rs core/src/store/mod.rs
git commit -m "refactor: migrate chat content to MessageContent union"
```

**Checkpoint 2:** All named tests green; no `String` remains as the `ChatMessage.content` type. Continue only then.

---

### Task 3: Content-addressed MediaStore

**Files:**
- Create: `core/src/media/mod.rs`
- Modify: `core/src/lib.rs`
- Test: inline `#[cfg(test)]` in `core/src/media/mod.rs`

**Interfaces:**
- Consumes: `sha2::Sha256`, `base64::engine::general_purpose::STANDARD` already in `core/Cargo.toml`.
- Produces: `MediaStore::{new, put, get, data_url}`, `MediaRef { media_hash, mime_type, file_name, byte_count }`, `MediaError::{InvalidMime, InvalidMagic, TooLarge, BadHash, Io}`.

**Dependencies:** Task 1 (types compile). No new crates.

**Acceptance criteria:** Identical bytes yield one hash-derived file; wrong magic and oversize are rejected; `data_url` contains standard base64; invalid hash paths are rejected.

- [ ] **Step 1: Write the failing tests**

Add to the new `core/src/media/mod.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn temp_root(tag: &str) -> PathBuf {
        let p = std::env::temp_dir().join(format!("ai-upcycler-media-{}-{}", tag, std::process::id()));
        let _ = std::fs::remove_dir_all(&p);
        p
    }

    #[tokio::test]
    async fn put_is_idempotent_and_path_is_hash_derived() {
        let root = temp_root("idempotent");
        let store = MediaStore::new(root.clone());
        let bytes = b"\x89PNG\r\n\x1a\nrest";
        let a = store.put(bytes, "image/png", "a.png").await.unwrap();
        let b = store.put(bytes, "image/png", "copy.png").await.unwrap();
        assert_eq!(a.media_hash, b.media_hash);
        let expected = root.join(&a.media_hash[..2]).join(&a.media_hash[2..]);
        assert!(expected.exists());
        assert_eq!(store.get(&a.media_hash).await.unwrap(), bytes);
    }

    #[tokio::test]
    async fn rejects_wrong_magic_and_oversize() {
        let store = MediaStore::new(temp_root("reject"));
        assert!(matches!(
            store.put(b"not a png", "image/png", "x.png").await,
            Err(MediaError::InvalidMagic)
        ));
        let big = vec![0u8; 10 * 1024 * 1024 + 1];
        assert!(matches!(
            store.put(&big, "image/png", "big.png").await,
            Err(MediaError::TooLarge)
        ));
    }

    #[tokio::test]
    async fn data_url_contains_base64_payload() {
        let store = MediaStore::new(temp_root("dataurl"));
        let bytes = b"\x89PNG\r\n\x1a\nabc";
        let r = store.put(bytes, "image/png", "a.png").await.unwrap();
        let url = store.data_url(&r.media_hash, "image/png").await.unwrap();
        assert!(url.starts_with("data:image/png;base64,"));
        assert_eq!(
            url,
            format!("data:image/png;base64,{}", base64::engine::general_purpose::STANDARD.encode(bytes))
        );
    }
}
```

Add `pub mod media;` to `core/src/lib.rs` before the tests compile.

- [ ] **Step 2: Run tests to verify they fail**

```powershell
cargo test -p ai-upcycler-core --lib -- --exact media::tests::put_is_idempotent_and_path_is_hash_derived
```

Expected: compile error `cannot find struct MediaStore` (red).

- [ ] **Step 3: Write minimal implementation**

In `core/src/media/mod.rs`:

```rust
pub struct MediaStore {
    root: std::path::PathBuf,
}

#[derive(Debug, Clone, PartialEq)]
pub struct MediaRef {
    pub media_hash: String,
    pub mime_type: String,
    pub file_name: String,
    pub byte_count: u64,
}

#[derive(Debug, PartialEq)]
pub enum MediaError {
    InvalidMime,
    InvalidMagic,
    TooLarge,
    BadHash,
    Io,
}
```

Implement `std::fmt::Display` for `MediaError` with exact messages `invalid_mime`, `invalid_magic`, `too_large`, `bad_hash`, `io_error`.

- `new(root)` stores root.
- `put`: check MIME against the four allowed values (`InvalidMime`); check `bytes.len() <= 10 * 1024 * 1024` (`TooLarge`); check magic bytes per Global Constraints (`InvalidMagic`); compute lowercase hex SHA-256; build `root/hash[0..2]/hash[2..]`; `tokio::fs::create_dir_all` parent; `tokio::fs::write` bytes; return `MediaRef`.
- `get`: require `media_hash.len() == 64` and all chars `[0-9a-f]` (`BadHash`); build path; `tokio::fs::read`; re-hash bytes and compare (`BadHash` on mismatch); map I/O errors to `Io`.
- `data_url`: `format!("data:{};base64,{}", mime_type, STANDARD.encode(bytes))`.

- [ ] **Step 4: Run tests to verify they pass**

```powershell
cargo fmt --all -- --check
cargo test -p ai-upcycler-core --lib -- --exact media::tests::put_is_idempotent_and_path_is_hash_derived
cargo test -p ai-upcycler-core --lib -- --exact media::tests::rejects_wrong_magic_and_oversize
cargo test -p ai-upcycler-core --lib -- --exact media::tests::data_url_contains_base64_payload
```

Expected: all PASS.

- [ ] **Step 5: Commit**

```powershell
git add core/src/media/mod.rs core/src/lib.rs
git commit -m "feat: add content-addressed image media store"
```

**Checkpoint 3:** Three media tests green; no raw bytes in logs or chunks. Continue only then.

---

### Task 4: Attachments table and atomic insertion

**Files:**
- Modify: `core/src/store/schema.sql`, `core/src/store/mod.rs`
- Test: inline `#[cfg(test)]` in `core/src/store/mod.rs`

**Interfaces:**
- Consumes: `MessageContent`, `ContentPart` from Tasks 1–2; existing `test_db()` helper used by current store tests.
- Produces: `AttachmentRow { message_id, media_hash, mime_type, file_name, byte_count, created_at }`, `Store::insert_message_with_attachments`, `Store::attachments_for_message`.

**Dependencies:** Task 2 (content types in store).

**Acceptance criteria:** DDL creates table/index; message and attachment rows commit together; invalid hash rolls back the message; existing `insert_message` still works with no attachments.

- [ ] **Step 1: Write the failing tests**

Add to `core/src/store/mod.rs` inline tests:

```rust
#[tokio::test]
async fn insert_message_with_attachments_is_atomic() {
    let db = test_db();
    let msg = NewChatMessage {
        session_id: 1,
        role: "user".into(),
        content: MessageContent::Parts(vec![
            ContentPart::Text { text: "look".into() },
            ContentPart::Image {
                media_hash: "cd".repeat(32),
                mime_type: "image/png".into(),
                file_name: "a.png".into(),
                byte_count: 4,
            },
        ]),
    };
    db.insert_message_with_attachments(
        &msg,
        &[("cd".repeat(32), "image/png".into(), "a.png".into(), 4)],
    ).await.unwrap();
    let rows = db.attachments_for_message(1).await.unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].mime_type, "image/png");
}

#[tokio::test]
async fn invalid_attachment_rolls_back_message() {
    let db = test_db();
    let msg = NewChatMessage {
        session_id: 2,
        role: "user".into(),
        content: MessageContent::Text("x".into()),
    };
    let res = db.insert_message_with_attachments(
        &msg,
        &[("bad-hash".into(), "image/png".into(), "a.png".into(), 4)],
    ).await;
    assert!(res.is_err());
    assert!(db.session_messages(2).await.unwrap().is_empty());
}
```

- [ ] **Step 2: Run tests to verify they fail**

```powershell
cargo test -p ai-upcycler-core --lib -- --exact store::tests::insert_message_with_attachments_is_atomic
```

Expected: `no method named insert_message_with_attachments` (red).

- [ ] **Step 3: Write minimal implementation**

Append exactly to `core/src/store/schema.sql`:

```sql
CREATE TABLE IF NOT EXISTS attachments (
  message_id INTEGER NOT NULL REFERENCES chat_messages(id) ON DELETE CASCADE,
  media_hash TEXT NOT NULL,
  mime_type TEXT NOT NULL,
  file_name TEXT NOT NULL,
  byte_count INTEGER NOT NULL,
  created_at TEXT NOT NULL,
  PRIMARY KEY (message_id, media_hash)
);
CREATE INDEX IF NOT EXISTS idx_attachments_hash ON attachments(media_hash);
```

In `core/src/store/mod.rs` add `AttachmentRow` and:

```rust
pub async fn insert_message_with_attachments(
    &self,
    msg: &NewChatMessage,
    attachments: &[(String, String, String, u64)],
) -> Result<i64, StoreError>;

pub async fn attachments_for_message(
    &self,
    message_id: i64,
) -> Result<Vec<AttachmentRow>, StoreError>;
```

Implementation rules:
- Begin transaction.
- Insert into `chat_messages` with `content = serde_json::to_string(&msg.content)`.
- For each attachment require `media_hash.len() == 64` with lowercase hex chars and `byte_count > 0`; otherwise `ROLLBACK` and return `StoreError::InvalidAttachment`.
- Insert `attachments` row using `chrono::Utc::now().to_rfc3339()` for `created_at`.
- `COMMIT`; any error triggers `ROLLBACK`.
- Keep existing `insert_message` delegating to the new method with `&[]`.

- [ ] **Step 4: Run tests to verify they pass**

```powershell
cargo fmt --all -- --check
cargo test -p ai-upcycler-core --lib -- --exact store::tests::insert_message_with_attachments_is_atomic
cargo test -p ai-upcycler-core --lib -- --exact store::tests::invalid_attachment_rolls_back_message
```

Expected: all PASS.

- [ ] **Step 5: Commit**

```powershell
git add core/src/store/schema.sql core/src/store/mod.rs
git commit -m "feat: store image attachments atomically with messages"
```

**Checkpoint 4:** Atomicity proven; `CREATE TABLE IF NOT EXISTS` migrates existing DBs. Continue only then.

---

### Task 5: Upload endpoint and AppState wiring

**Files:**
- Modify: `core/src/api/mod.rs`, `core/src/api/chat.rs`, `src-tauri/src/lib.rs`
- Create: `core/tests/multimodal_router.rs`
- Test: `core/tests/multimodal_router.rs`

**Interfaces:**
- Consumes: `MediaStore` from Task 3, `AppState::with_multimodal` from Task 1.
- Produces: `AppState::with_media(MediaStore) -> Self`, `AppState::with_media_root(PathBuf) -> Self`, `POST /api/media`, test helpers `test_app(bool)`, `post_chat(&Router, serde_json::Value)`, `json_body(Response)`, `image_chat_body()` defined in Step 1.

**Dependencies:** Tasks 3 and 4.

**Acceptance criteria:** Gate-off upload is 404; gate-on upload returns a 64-hex hash and file metadata; gate-off chat with an image is 400 before session writes; loopback stays unauthenticated.

- [ ] **Step 1: Write the failing tests**

Create `core/tests/multimodal_router.rs` with:

```rust
use axum::body::Body;
use axum::http::{Request, StatusCode};
use axum::response::Response;
use axum::Router;
use serde_json::{json, Value};
use tower::ServiceExt;

fn test_app(enabled: bool) -> Router {
    let root = std::env::temp_dir().join(format!("ai-upcycler-router-{}-{}", enabled, std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    build_test_router(enabled, root)
}

async fn json_body(res: Response) -> Value {
    let bytes = axum::body::to_bytes(res.into_body(), usize::MAX).await.unwrap();
    serde_json::from_slice(&bytes).unwrap()
}

async fn post_chat(app: &Router, body: Value) -> Response {
    app.clone().oneshot(
        Request::builder()
            .method("POST")
            .uri("/api/chat")
            .header("content-type", "application/json")
            .body(Body::from(serde_json::to_vec(&body).unwrap()))
            .unwrap(),
    ).await.unwrap()
}

fn image_chat_body() -> Value {
    json!({
        "messages":[{
            "role":"user",
            "content":[{
                "type":"image",
                "media_hash":"ab".repeat(32),
                "mime_type":"image/png",
                "file_name":"a.png",
                "byte_count":4
            }]
        }],
        "max_tokens":64
    })
}

#[tokio::test]
async fn upload_rejects_when_gate_off() {
    let res = test_app(false).oneshot(
        Request::builder().method("POST").uri("/api/media")
            .body(Body::from("png")).unwrap()
    ).await.unwrap();
    assert_eq!(res.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn upload_stores_bytes_when_gate_on() {
    let png = b"\x89PNG\r\n\x1a\nhello";
    let body = format!(
        "--x\r\nContent-Disposition: form-data; name=\"file\"; filename=\"a.png\"\r\nContent-Type: image/png\r\n\r\n{}\r\n--x--\r\n",
        String::from_utf8_lossy(png)
    );
    let res = test_app(true).oneshot(
        Request::builder().method("POST").uri("/api/media")
            .header("content-type", "multipart/form-data; boundary=x")
            .body(Body::from(body)).unwrap()
    ).await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let json = json_body(res).await;
    assert_eq!(json["mime_type"], "image/png");
    assert_eq!(json["file_name"], "a.png");
    assert_eq!(json["media_hash"].as_str().unwrap().len(), 64);
}

#[tokio::test]
async fn chat_with_image_is_rejected_when_gate_off() {
    let res = post_chat(&test_app(false), image_chat_body()).await;
    assert_eq!(res.status(), StatusCode::BAD_REQUEST);
    assert_eq!(json_body(res).await["error"], "multimodal_disabled");
}
```

`build_test_router(enabled, root)` is the existing router-builder helper in the test harness; if the core crate does not expose one, add it in this test file using the same `AppState` and router constructor used by `src-tauri/src/lib.rs`, with `with_multimodal(enabled).with_media_root(root)`.

- [ ] **Step 2: Run tests to verify they fail**

```powershell
cargo test -p ai-upcycler-core --test multimodal_router -- --exact upload_rejects_when_gate_off
```

Expected: 404 route missing (red).

- [ ] **Step 3: Write minimal implementation**

- Add `media: MediaStore` to `AppState`, initialize it in all constructors, and add `with_media` / `with_media_root`.
- Register `POST /api/media` in `core/src/api/mod.rs`:
  - If `!multimodal_enabled`, return `StatusCode::NOT_FOUND` without parsing the body.
  - Parse `axum::extract::Multipart`, require field name `file`, read `content_type()` and `file_name()`, call `MediaStore::put`.
  - Map `MediaError` to `400 {"error":"invalid_image_attachment"}`.
  - Return `StatusCode::OK` with `Json(MediaRef)`.
- In `core/src/api/chat.rs`, before session lookup or writes, scan messages for `ContentPart::Image`; if found and `!multimodal_enabled`, return `400 {"error":"multimodal_disabled"}`.
- In `src-tauri/src/lib.rs`, derive:

```rust
let media_root = app.path().app_data_dir()?.join("media");
let state = AppState::bootstrap(/* existing args */)
    .with_media_root(media_root)
    .with_multimodal(std::env::var("AI_UPCYCLER_MULTIMODAL").as_deref() == Ok("1"));
```

- [ ] **Step 4: Run tests to verify they pass**

```powershell
cargo fmt --all -- --check
cargo test -p ai-upcycler-core --test multimodal_router
cargo test -p ai-upcycler-core --test compat_content
```

Expected: all PASS.

- [ ] **Step 5: Commit**

```powershell
git add core/src/api/mod.rs core/src/api/chat.rs src-tauri/src/lib.rs core/tests/multimodal_router.rs
git commit -m "feat: add gated image upload endpoint and media root wiring"
```

**Checkpoint 5:** Upload 200/404 behavior green; compat suite still green; loopback auth unchanged. Continue only then.

---

### Task 6: Local and cluster fail-closed handling

**Files:**
- Modify: `core/src/api/chat.rs`, `core/src/api/distributed.rs`
- Test: `core/tests/multimodal_router.rs`, inline tests in `core/src/api/chat.rs`

**Interfaces:**
- Consumes: A's `RoutePolicy`, `ProviderRegistry`; `image_chat_body`, `post_chat`, `json_body` from Task 5.
- Produces: deterministic check order `gate → tool role → local/cluster → vision capability → adapter`; stream error chunk shape.

**Dependencies:** Task 5 and sub-project A. Stop and report if `core/src/providers/` or `core/src/routing.rs` are absent.

**Acceptance criteria:** Local route, cluster route, and tool turn each return 400 `image_attachments_require_vision_provider`; streaming emits exactly one error chunk; no session or attachment row is written on these paths.

- [ ] **Step 1: Write the failing tests**

Add to `core/tests/multimodal_router.rs`:

```rust
#[tokio::test]
async fn local_route_with_image_returns_explicit_error() {
    let res = post_chat(&test_app_with_local_route(true), image_chat_body()).await;
    assert_eq!(res.status(), StatusCode::BAD_REQUEST);
    assert_eq!(json_body(res).await["error"], "image_attachments_require_vision_provider");
}

#[tokio::test]
async fn cluster_route_with_image_returns_explicit_error() {
    let res = post_chat(&test_app_with_cluster_route(true), image_chat_body()).await;
    assert_eq!(res.status(), StatusCode::BAD_REQUEST);
    assert_eq!(json_body(res).await["error"], "image_attachments_require_vision_provider");
}

#[tokio::test]
async fn tool_turn_with_image_is_rejected_before_inference() {
    let body = json!({
        "messages":[{
            "role":"tool",
            "content":[{
                "type":"image",
                "media_hash":"ab".repeat(32),
                "mime_type":"image/png",
                "file_name":"a.png",
                "byte_count":4
            }]
        }],
        "max_tokens":64
    });
    let res = post_chat(&test_app(true), body).await;
    assert_eq!(res.status(), StatusCode::BAD_REQUEST);
    assert_eq!(json_body(res).await["error"], "image_attachments_require_vision_provider");
}
```

Define `test_app_with_local_route` and `test_app_with_cluster_route` by taking the `test_app(true)` builder and applying A's `RoutePolicy` local/cluster variants before `build_test_router`.

Add to `core/src/api/chat.rs` inline tests:

```rust
#[tokio::test]
async fn stream_yields_single_error_chunk_for_local_image() {
    let chunks = dispatch_image_turn_for_test(
        &test_state_with_local_route(),
        req_from_content(serde_json::json!([{
            "type":"image",
            "media_hash":"ab".repeat(32),
            "mime_type":"image/png",
            "file_name":"a.png",
            "byte_count":4
        }])),
    ).await;
    assert_eq!(chunks.len(), 1);
    assert_eq!(chunks[0].delta, "");
    assert_eq!(
        chunks[0].error.as_deref(),
        Some("image_attachments_require_vision_provider")
    );
}
```

`dispatch_image_turn_for_test` wraps the real stream dispatch with `tokio::sync::mpsc::channel(8)` and `futures::StreamExt::collect`; define it in the same test module.

- [ ] **Step 2: Run tests to verify they fail**

```powershell
cargo test -p ai-upcycler-core --test multimodal_router -- --exact local_route_with_image_returns_explicit_error
cargo test -p ai-upcycler-core --lib -- --exact api::chat::tests::stream_yields_single_error_chunk_for_local_image
```

Expected: images are dropped by `content_text` and text-only success is observed (red).

- [ ] **Step 3: Write minimal implementation**

In `chat.rs`:

```rust
let has_image = request.messages.iter().any(|m| matches!(
    &m.content,
    MessageContent::Parts(ps) if ps.iter().any(|p| matches!(p, ContentPart::Image { .. }))
));
```

- If `has_image` and any message `role == "tool"`, return `400 image_attachments_require_vision_provider`.
- If `has_image` and `RoutePolicy` resolves to local or cluster, return the same `400` without calling a backend.
- In the stream dispatch helper, before any backend call, send one `GenerationChunk` with `delta: String::new()`, `error: Some("image_attachments_require_vision_provider".into())`, `source: ChunkSource::Local`, `is_real: false`, then close.
- Keep `distributed.rs`'s Task 2 pre-POST refusal as defense in depth.
- Assert zero session writes on error paths by checking `attachments_for_message` and session history are empty in the tests.

- [ ] **Step 4: Run tests to verify they pass**

```powershell
cargo fmt --all -- --check
cargo test -p ai-upcycler-core --test multimodal_router
cargo test -p ai-upcycler-core --lib -- --exact api::chat::tests::stream_yields_single_error_chunk_for_local_image
```

Expected: all PASS.

- [ ] **Step 5: Commit**

```powershell
git add core/src/api/chat.rs core/src/api/distributed.rs core/tests/multimodal_router.rs
git commit -m "fix: fail closed on local and cluster image turns"
```

**Checkpoint 6:** All error-body tests and stream chunk test green; zero session writes on error. Continue only then.

---

### Task 7: Provider message mapping

**Files:**
- Create: `core/src/providers/messages.rs`
- Test: inline `#[cfg(test)]` in that file

**Interfaces:**
- Consumes: `ChatMessage`, `MessageContent`, `ContentPart` from Task 2.
- Produces: `to_openai_message(&ChatMessage, &str) -> serde_json::Value`, `to_gemini_parts(&ChatMessage, &str) -> Vec<serde_json::Value>`, `to_anthropic_content(&ChatMessage, &str) -> Vec<serde_json::Value>`.

**Dependencies:** Task 2 and sub-project A's provider directory. Mappers are pure; callers supply base64/data URL.

**Acceptance criteria:** OpenAI/Kimi use `image_url.url` data URL; Gemini uses `inlineData.data` plus `mimeType`; Anthropic uses `source.base64`; no mapper contains raw bytes.

- [ ] **Step 1: Write the failing tests**

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use ai_upcycler_core::api::backend::{ChatMessage, ContentPart, MessageContent};
    use serde_json::json;

    fn image_msg() -> ChatMessage {
        ChatMessage {
            role: "user".into(),
            content: MessageContent::Parts(vec![ContentPart::Image {
                media_hash: "ab".repeat(32),
                mime_type: "image/png".into(),
                file_name: "a.png".into(),
                byte_count: 4,
            }]),
        }
    }

    #[test]
    fn openai_maps_image_to_data_url() {
        let v = to_openai_message(&image_msg(), "data:image/png;base64,QUJD");
        assert_eq!(v["content"][0]["type"], "image_url");
        assert_eq!(v["content"][0]["image_url"]["url"], "data:image/png;base64,QUJD");
    }

    #[test]
    fn gemini_maps_image_to_inline_data() {
        let v = to_gemini_parts(&image_msg(), "QUJD");
        assert_eq!(v[0]["inlineData"]["mimeType"], "image/png");
        assert_eq!(v[0]["inlineData"]["data"], "QUJD");
    }

    #[test]
    fn anthropic_maps_image_to_source_base64() {
        let v = to_anthropic_content(&image_msg(), "QUJD");
        assert_eq!(v[0]["type"], "image");
        assert_eq!(v[0]["source"]["type"], "base64");
        assert_eq!(v[0]["source"]["media_type"], "image/png");
        assert_eq!(v[0]["source"]["data"], "QUJD");
    }
}
```

- [ ] **Step 2: Run tests to verify they fail**

```powershell
cargo test -p ai-upcycler-core --lib -- --exact providers::messages::tests::openai_maps_image_to_data_url
```

Expected: unresolved import or module missing (red).

- [ ] **Step 3: Write minimal implementation**

Register `pub mod messages;` in A's provider module and implement:

- `to_openai_message`: `{"role": msg.role, "content":[{"type":"text","text":t}, {"type":"image_url","image_url":{"url":data_url}}]}`; Kimi reuses this function.
- `to_gemini_parts`: `{"text": t}` and `{"inlineData":{"mimeType": mime, "data": base64_data}}`.
- `to_anthropic_content`: `{"type":"text","text":t}` and `{"type":"image","source":{"type":"base64","media_type":mime,"data":base64_data}}`.
- Text-only messages produce the single text shape A already uses.

- [ ] **Step 4: Run tests to verify they pass**

```powershell
cargo fmt --all -- --check
cargo test -p ai-upcycler-core --lib -- --exact providers::messages::tests::openai_maps_image_to_data_url
cargo test -p ai-upcycler-core --lib -- --exact providers::messages::tests::gemini_maps_image_to_inline_data
cargo test -p ai-upcycler-core --lib -- --exact providers::messages::tests::anthropic_maps_image_to_source_base64
```

Expected: all PASS.

- [ ] **Step 5: Commit**

```powershell
git add core/src/providers/messages.rs core/src/providers/mod.rs
git commit -m "feat: map image parts to provider payloads"
```

**Checkpoint 7:** Three mapper tests green; no raw bytes in mapper output. Continue only then.

---

### Task 8: Adapter integration with MediaStore

**Files:**
- Modify: A's adapter files under `core/src/providers/`, `core/src/api/mod.rs`
- Test: `core/tests/multimodal_router.rs`

**Interfaces:**
- Consumes: A's `CloudProvider::chat_stream(&self, GenerationRequest, &str) -> BoxStream<'static, Result<GenerationChunk, String>>`, `spawn_mock_provider`; mappers from Task 7; `MediaStore` from Task 3.
- Produces: `ProviderRegistry::with_media_store(self, MediaStore) -> Self`.

**Dependencies:** Task 7 and sub-project A.

**Acceptance criteria:** Mock OpenAI receives `data:image/png;base64,`; A's text-only provider target remains green; `GenerationChunk.source` remains `ChunkSource::Provider(id)`.

- [ ] **Step 1: Write the failing test**

Add to `core/tests/multimodal_router.rs`:

```rust
#[tokio::test]
async fn image_turn_posts_data_url_to_mock_openai() {
    let (registry, received) = spawn_mock_provider(ProviderId::OpenAI, |req| {
        assert!(req.contains("data:image/png;base64,"));
        json!({"choices":[{"message":{"content":"saw image"}}]})
    }).await;
    let app = test_app_with_registry(true, registry);
    let res = post_chat(&app, image_chat_body()).await;
    assert_eq!(res.status(), StatusCode::OK);
    assert!(received().await);
}
```

`test_app_with_registry` extends `test_app(true)` with `AppState.providers = registry`.

- [ ] **Step 2: Run tests to verify it fails**

```powershell
cargo test -p ai-upcycler-core --test multimodal_router -- --exact image_turn_posts_data_url_to_mock_openai
```

Expected: mock sees request without a data URL (red).

- [ ] **Step 3: Write minimal implementation**

- Add `with_media_store` to `ProviderRegistry` without changing other A fields.
- In each adapter's `chat_stream`, for each `ContentPart::Image`:
  1. `let bytes = media.get(&media_hash).await`; on `MediaError`, yield `GenerationChunk` with `error = Some("invalid_image_attachment".into())`.
  2. `let data_url = media.data_url(&media_hash, &mime_type).await`.
  3. OpenAI/Kimi pass full `data_url` to `to_openai_message`; Gemini pass the substring after `,` to `to_gemini_parts`; Anthropic pass that substring to `to_anthropic_content`.
- Preserve `ProviderDescriptor`, streaming loop, and `GenerationChunk.source = ChunkSource::Provider(id)`.
- Never log `bytes` or `data_url`.

- [ ] **Step 4: Run tests to verify they pass**

```powershell
cargo fmt --all -- --check
cargo test -p ai-upcycler-core --test multimodal_router -- --exact image_turn_posts_data_url_to_mock_openai
cargo test -p ai-upcycler-core --test provider_text_only
```

Expected: both PASS. If A named the text-only target differently, substitute that exact target name here; no other edits.

- [ ] **Step 5: Commit**

```powershell
git add core/src/providers core/src/api/mod.rs core/tests/multimodal_router.rs
git commit -m "feat: feed image URLs into cloud adapters"
```

**Checkpoint 8:** Mock data URL test and A's text-only target green. Continue only then.

---

### Task 9: Provider vision capability gate

**Files:**
- Modify: `core/src/api/chat.rs`, `core/tests/multimodal_router.rs`
- Test: `core/tests/multimodal_router.rs`

**Interfaces:**
- Consumes: A's `ProviderModel { id, supports_vision }`, `ProviderRegistry::{resolve, new}`, `spawn_mock_provider`.
- Produces: deterministic vision error ordering after Task 6's local/cluster check.

**Dependencies:** Task 8 and sub-project A.

**Acceptance criteria:** Non-vision model returns `provider_lacks_vision_support` with zero provider HTTP hits; empty registry returns `no_vision_capable_model_loaded`; omitted `provider` still follows `RoutePolicy`.

- [ ] **Step 1: Write the failing tests**

```rust
#[tokio::test]
async fn image_with_non_vision_model_returns_400_before_http() {
    let (registry, hits) = spawn_mock_provider(ProviderId::OpenAI, |_| json!({"ok":true})).await;
    let app = test_app_with_registry_and_model(true, registry, ProviderModel {
        id: "gpt-4o-mini".into(),
        supports_vision: false,
    });
    let res = post_chat(&app, image_chat_body()).await;
    assert_eq!(res.status(), StatusCode::BAD_REQUEST);
    assert_eq!(json_body(res).await["error"], "provider_lacks_vision_support");
    assert_eq!(hits().await, 0);
}

#[tokio::test]
async fn image_with_no_vision_provider_returns_400() {
    let app = test_app_with_registry(true, ProviderRegistry::new());
    let res = post_chat(&app, image_chat_body()).await;
    assert_eq!(res.status(), StatusCode::BAD_REQUEST);
    assert_eq!(json_body(res).await["error"], "no_vision_capable_model_loaded");
}
```

`test_app_with_registry_and_model` applies the `ProviderModel` to the registry returned by A's `resolve` before router construction.

- [ ] **Step 2: Run tests to verify they fail**

```powershell
cargo test -p ai-upcycler-core --test multimodal_router -- --exact image_with_non_vision_model_returns_400_before_http
```

Expected: mock receives the request (red).

- [ ] **Step 3: Write minimal implementation**

In `chat.rs`, after gate, tool-role, and local/cluster checks and before `registry.stream(...)`:

```rust
let model = registry.resolve(&route);
if !model.supports_vision {
    return bad_request("provider_lacks_vision_support");
}
```

If no route/model resolves with `supports_vision`, return `400 no_vision_capable_model_loaded`. Check order is exactly: gate → tool role → local/cluster → vision capability → adapter. When `provider` is absent, use `RoutePolicy` as before; do not require the field.

- [ ] **Step 4: Run tests to verify they pass**

```powershell
cargo fmt --all -- --check
cargo test -p ai-upcycler-core --test multimodal_router
cargo test -p ai-upcycler-core --test compat_content
```

Expected: all PASS; no provider HTTP call on vision failures.

- [ ] **Step 5: Commit**

```powershell
git add core/src/api/chat.rs core/tests/multimodal_router.rs
git commit -m "fix: gate images on provider vision capability"
```

**Checkpoint 9:** All router tests green; zero HTTP hits on vision failures. Continue only then.

---

### Task 10: Desktop attachment chips

**Files:**
- Modify: `src/lib/types.ts`, `src/lib/api.ts`, `src/components/ChatView.tsx`
- Test: Playwright MCP at `D:\ai-upcycler-temp\a.png` and `D:\ai-upcycler-temp\desktop-chips.png`

**Interfaces:**
- Consumes: `POST /api/media` from Task 5; `ChatContent` union must match `MessageContent` JSON from Task 2.
- Produces: `uploadImage(file: File) -> Promise<MediaRef>`; composer sends plain string with zero attachments and part array otherwise.

**Dependencies:** Task 9 compat suite green. No UI before this checkpoint.

**Acceptance criteria:** Chip shows file name, MIME, byte count; remove button has `aria-label="Remove a.png"`; focus ring is `#22C55E`; keyboard remove works; gate-off hides file input; text-only send posts a plain string.

- [ ] **Step 1: Write the failing browser assertions**

With `AI_UPCYCLER_MULTIMODAL=1`, open the desktop app in Playwright MCP at viewport `1280x800`, create `D:\ai-upcycler-temp\a.png` as a 1×1 PNG, attach it through the composer, and assert the five acceptance criteria above. Before implementation these assertions fail (no input/chip). Record the failure in the execution log.

- [ ] **Step 2: Write minimal implementation for types and client**

In `src/lib/types.ts` add:

```ts
export type ContentPart =
  | { type: "text"; text: string }
  | {
      type: "image";
      media_hash: string;
      mime_type: string;
      file_name: string;
      byte_count: number;
    };

export type ChatContent = string | ContentPart[];

export interface MediaRef {
  media_hash: string;
  mime_type: string;
  file_name: string;
  byte_count: number;
}
```

In `src/lib/api.ts` add:

```ts
export async function uploadImage(file: File): Promise<MediaRef> {
  const form = new FormData();
  form.append("file", file);
  const res = await fetch("/api/media", { method: "POST", body: form });
  if (!res.ok) {
    const body = await res.json();
    throw new Error(body.error ?? "invalid_image_attachment");
  }
  return res.json();
}
```

Keep `sendChat` accepting `content: ChatContent` so text-only callers stay unchanged.

- [ ] **Step 3: Write minimal implementation for UI**

In `src/components/ChatView.tsx`:

```tsx
const [attachments, setAttachments] = useState<MediaRef[]>([]);
const multimodalEnabled = /* value supplied by Tauri initial state or /api/config from Task 5 */;
```

- Render `<input type="file" accept="image/png,image/jpeg,image/webp,image/gif" />` only when `multimodalEnabled` is true.
- On select, call `uploadImage(file)`; append returned `MediaRef`; on error render `<p role="alert">Invalid image attachment</p>`.
- Chip markup:

```tsx
<ul aria-label="Image attachments">
  {attachments.map(a => (
    <li
      key={a.media_hash}
      className="attachment-chip"
      style={{ background: "#1B2336", border: "1px solid #475569", color: "#94A3B8" }}
    >
      <span>{a.file_name} · {a.mime_type} · {a.byte_count} B</span>
      <button
        aria-label={`Remove ${a.file_name}`}
        onClick={() => setAttachments(xs => xs.filter(x => x.media_hash !== a.media_hash))}
      >
        ×
      </button>
    </li>
  ))}
</ul>
```

- Send payload: if `attachments.length === 0`, `content: text`; else `content: [{ type: "text", text }, ...attachments.map(a => ({ type: "image", ...a }))]`.
- CSS: `:focus-visible { outline: 2px solid #22C55E; outline-offset: 2px }`, `transition: background 200ms ease`, `@media (prefers-reduced-motion: reduce) { .attachment-chip { transition: none } }`, destructive hover uses `#EF4444`.

- [ ] **Step 4: Verify with Playwright MCP**

```powershell
$env:npm_config_cache = "D:\ai-upcycler-npm-cache"
New-Item -ItemType Directory -Force -Path D:\ai-upcycler-temp | Out-Null
```

Run Playwright MCP checks: chip text, aria-label, computed focus-ring color, keyboard remove, gate-off absence, and network request body for text-only send. Save screenshot to `D:\ai-upcycler-temp\desktop-chips.png`.

- [ ] **Step 5: Commit**

```powershell
git add src/lib/types.ts src/lib/api.ts src/components/ChatView.tsx
git commit -m "feat: add desktop image attachment chips"
```

**Checkpoint 10:** Five browser assertions pass; text-only send still posts a plain string. Continue only then.

---

### Task 11: Android attachment chips

**Files:**
- Modify: B-produced files under `mobile-worker/android/app/src/main/java/org/aiupcycler/worker/chat/`
- Test: `ChatAttachmentTest.kt` in B's instrumentation source set; screenshot `D:\ai-upcycler-temp\android-chips.png`

**Interfaces:**
- Consumes: B's Compose chat composer, LAN REST client with bearer token, `MASTER.md` tokens; `POST /api/media` from Task 5.
- Produces: `AttachmentChip(data: MediaRef, onRemove: () -> Unit)` composable and `uploadImage` call in B's client.

**Dependencies:** Sub-project B (Compose screens, LAN client, `design-system/ai-upcycler/MASTER.md`). Stop and report if absent; do not create `MASTER.md`.

**Acceptance criteria:** Chip shows file name and remove content description; remove is clickable; gate-off hides attach button; screenshot exists; token not logged.

- [ ] **Step 1: Write the failing instrumentation test**

Add to B's existing chat instrumentation test (create `ChatAttachmentTest.kt` only if B has none):

```kotlin
@Test fun chipShowsFileNameAndRemoveLabel() {
    composeRule.onNodeWithText("a.png").assertIsDisplayed()
    composeRule.onNodeWithContentDescription("Remove a.png").assertIsDisplayed()
    composeRule.onNodeWithContentDescription("Remove a.png").assertHasClickAction()
}

@Test fun attachButtonHiddenWhenGateOff() {
    composeRule.onNodeWithContentDescription("Attach image").assertDoesNotExist()
}
```

- [ ] **Step 2: Run tests to verify they fail**

```powershell
adb connect 192.168.1.9:5555
adb shell am instrument -w -e class org.aiupcycler.worker.ChatAttachmentTest org.aiupcycler.worker.test/androidx.test.runner.AndroidJUnitRunner
```

Expected: node not found (red).

- [ ] **Step 3: Write minimal implementation**

Add composable in B's chat package:

```kotlin
@Composable
fun AttachmentChip(data: MediaRef, onRemove: () -> Unit) {
    Row(
        modifier = Modifier
            .heightIn(min = 48.dp)
            .border(1.dp, Color(0xFF475569))
            .background(Color(0xFF1B2336))
            .semantics { contentDescription = "image attachment, ${data.fileName}, remove button" },
        verticalAlignment = Alignment.CenterVertically
    ) {
        Text(
            "${data.fileName} · ${data.mimeType} · ${data.byteCount} B",
            color = Color(0xFF94A3B8)
        )
        IconButton(onClick = onRemove) {
            Icon(
                imageVector = Icons.Default.Close,
                contentDescription = "Remove ${data.fileName}",
                tint = Color(0xFFEF4444)
            )
        }
    }
}
```

- Use `focusBorder()` with `Color(0xFF22C55E)` for visible focus; respect reduced motion via the app's existing `MotionScheme` equivalent.
- Wire to B's LAN REST client: multipart POST `/api/media` with field `file` and bearer token from B's secure store; append returned `MediaRef` to composer state.
- Send payload mirrors desktop: plain string with zero attachments; part array otherwise.
- Gate off: hide the attach button entirely.

- [ ] **Step 4: Run tests and capture screenshot**

```powershell
adb shell am instrument -w -e class org.aiupcycler.worker.ChatAttachmentTest org.aiupcycler.worker.test/androidx.test.runner.AndroidJUnitRunner
adb shell screencap -p D:\ai-upcycler-temp\android-chips.png
```

Expected: both tests PASS; screenshot saved under `D:\ai-upcycler-temp\`.

- [ ] **Step 5: Commit**

```powershell
git add mobile-worker/android/app/src/main/java/org/aiupcycler/worker/chat
git commit -m "feat: add Android image attachment chips"
```

**Checkpoint 11:** Instrumentation green; screenshot saved; no token logged. Continue only then.

---

### Task 12: Documentation and final verification gate

**Files:**
- Modify: `docs/api.md`
- Test: full command set in Step 2 plus re-run of Task 10/11 checks

**Interfaces:**
- Consumes: all prior outputs.
- Produces: published API contract for `/api/media` and the content union; final verification log in the execution record.

**Dependencies:** Tasks 1–11.

**Acceptance criteria:** Docs contain exact error bodies and content shapes; every command exits 0; only Files Map files changed; no commits beyond the per-task commits.

- [ ] **Step 1: Update docs**

Append to `docs/api.md`:

```markdown
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
```

- [ ] **Step 2: Run the complete verification gate**

```powershell
$env:RUSTUP_TOOLCHAIN = "stable-x86_64-pc-windows-gnu"
$env:CARGO_TARGET_DIR = "D:\ai-upcycler-target"
$env:PATH = "C:\msys64\mingw64\bin;$env:PATH"
cargo fmt --all -- --check
cargo test -p ai-upcycler-core
cargo test -p ai-upcycler-core --test compat_content
cargo test -p ai-upcycler-core --test multimodal_router
```

Expected: every command exits 0. Then re-run Task 10 Playwright checks and Task 11 ADB checks once more.

- [ ] **Step 3: Self-review against spec**

Record in the execution log (not a file):

- §8: local/cluster have no vision; images only go to cloud with `supports_vision`.
- §12: chip tokens, a11y labels, focus, reduced motion, 48 dp touch target.
- §13: mock HTTP, real axum router, Playwright desktop, ADB Android checks executed.
- §14: compat tests landed before UI; gate stayed off until Tasks 1–9 green.
- Agent compatibility: no `provider` required, tool turns host-only, loopback unauthenticated, `/v1` endpoint text-only with image parts rejected before host/tool calls per `docs/AGENTS.md` §8.
- Only files in the Files Map were modified.

- [ ] **Step 4: Commit**

```powershell
git add docs/api.md
git commit -m "docs: document multimodal chat API and error bodies"
```

**Checkpoint 12 (FINAL):** Every command above exits 0; both screenshots exist under `D:\ai-upcycler-temp\`; no file outside the Files Map changed. Only then may C be reported complete.

---

## Self-Review (plan author)

- **Spec coverage:** §8 fail-closed semantics in Tasks 6 and 9; §12 tokens/a11y in Tasks 10–11; §13 verification methods in every task plus Task 12 Step 2; §14 ordering in Task 1 and the dependency chain.
- **Placeholder scan:** No `TBD`, `TODO`, `implement later`, `fill in details`, `add appropriate error handling`, `handle edge cases`, or `similar to Task N`. Every code step includes exact code; every verification step includes exact commands and expected results.
- **Type consistency:** `MessageContent` used uniformly in backend, chat, distributed, llama, store, providers, desktop TS, and Android Kotlin. `CloudProvider::chat_stream` matches A; `GenerationChunk.source` is untouched.
- **Compatibility:** Plain-string round-trip and missing/`null` tests are Task 1, before UI; tool turns and omitted-provider tests preserved.
- **Path/command fidelity:** All commands use `D:\ai-upcycler-target`, `D:\ai-upcycler-npm-cache`, `D:\ai-upcycler-temp\`, and ADB `192.168.1.9:5555`.
- **Task count:** Exactly 12 tasks, each with files, interfaces, dependencies, acceptance criteria, checkbox steps, verification, commit, and checkpoint.
