//! Embedded SQLite persistence for the master node.
//!
//! Wraps a single [`rusqlite::Connection`] behind a mutex so the store can be
//! shared across async handlers (SQLite is single-writer). The schema mirrors
//! `docs/database.md` and is applied idempotently on open.

use crate::api::backend::MessageContent;
use crate::settings::AppSettings;
use rusqlite::{params, Connection, OptionalExtension};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use std::path::Path;
use std::sync::{Mutex, MutexGuard};

/// Schema DDL applied on every open (all statements are `IF NOT EXISTS`).
pub const SCHEMA: &str = include_str!("schema.sql");

#[derive(Debug, thiserror::Error)]
pub enum StoreError {
    #[error("sqlite error: {0}")]
    Sqlite(#[from] rusqlite::Error),
    #[error("store mutex poisoned")]
    Poisoned,
    /// A cloud model id collides with an existing local GGUF (or vice versa).
    #[error("{0}")]
    Conflict(String),
    /// Image attachment metadata is malformed; the message was rolled back.
    #[error("invalid attachment")]
    InvalidAttachment,
    #[error("invalid settings json: {0}")]
    SettingsJson(#[from] serde_json::Error),
    #[error("invalid setting: {0}")]
    InvalidSettings(String),
}

/// Marker prefix for catalog rows that represent cloud-only models. Such rows
/// keep routing resolvable but are hidden from `/v1/models` because there is
/// no GGUF to download or serve locally.
pub const VIRTUAL_MODEL_PATH_PREFIX: &str = "virtual:";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NodeRow {
    pub node_id: String,
    pub device_name: String,
    pub mac_address: Option<String>,
    pub connection_type: String,
    pub total_ram_mb: i64,
    pub cpu_cores: i64,
    pub has_npu: bool,
    pub paired_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NewNode {
    pub node_id: String,
    pub device_name: String,
    pub mac_address: Option<String>,
    pub connection_type: String,
    pub total_ram_mb: i64,
    pub cpu_cores: i64,
    pub has_npu: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelRow {
    pub model_id: String,
    pub file_name: String,
    pub quantization_format: String,
    pub total_layers: i64,
    pub weight_size_bytes: i64,
    pub local_file_path: String,
    pub added_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NewModel {
    pub model_id: String,
    pub file_name: String,
    pub quantization_format: String,
    pub total_layers: i64,
    pub weight_size_bytes: i64,
    pub local_file_path: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BenchmarkRow {
    pub id: i64,
    pub node_id: String,
    pub memory_bandwidth_mbs: f64,
    pub network_ping_ms: f64,
    pub throughput_flops: f64,
    pub tested_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NewBenchmark {
    pub node_id: String,
    pub memory_bandwidth_mbs: f64,
    pub network_ping_ms: f64,
    pub throughput_flops: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatSessionRow {
    pub session_id: String,
    pub title: String,
    pub model_id: String,
    pub created_at: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ChatMessageRow {
    pub message_id: i64,
    pub session_id: String,
    pub role: String,
    pub content: MessageContent,
    pub token_count: i64,
    pub generation_time_sec: f64,
    pub created_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NewChatMessage {
    pub session_id: String,
    pub role: String,
    pub content: MessageContent,
    pub token_count: i64,
    pub generation_time_sec: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct NewWorkflowStep {
    pub step_id: String,
    pub prompt: String,
    pub depends_on_json: String,
    pub provider: Option<String>,
    pub position: i64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WorkflowRunRow {
    pub run_id: String,
    pub model: String,
    pub status: String,
    pub created_at: String,
    pub finished_at: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WorkflowStepRow {
    pub run_id: String,
    pub step_id: String,
    pub prompt: String,
    pub depends_on: Vec<String>,
    pub provider: Option<String>,
    pub status: String,
    pub output: Option<String>,
    pub source: Option<String>,
    pub error: Option<String>,
    pub position: i64,
}

/// Content-addressed image metadata attached to a chat message.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AttachmentRow {
    pub message_id: i64,
    pub media_hash: String,
    pub mime_type: String,
    pub file_name: String,
    pub byte_count: i64,
    pub created_at: String,
}

/// SQLite-backed store for cluster, model, benchmark and chat persistence.
pub struct Store {
    conn: Mutex<Connection>,
}

impl Store {
    /// Opens (or creates) a database file and applies the schema.
    pub fn open(path: impl AsRef<Path>) -> Result<Self, StoreError> {
        Self::from_connection(Connection::open(path)?)
    }

    /// Opens a transient in-memory database (used by tests and dry runs).
    pub fn open_in_memory() -> Result<Self, StoreError> {
        Self::from_connection(Connection::open_in_memory()?)
    }

    fn from_connection(conn: Connection) -> Result<Self, StoreError> {
        conn.execute_batch("PRAGMA foreign_keys = ON;")?;
        conn.execute_batch(SCHEMA)?;
        Ok(Self {
            conn: Mutex::new(conn),
        })
    }

    fn lock(&self) -> Result<MutexGuard<'_, Connection>, StoreError> {
        self.conn.lock().map_err(|_| StoreError::Poisoned)
    }

    // --- nodes -------------------------------------------------------------

    pub fn upsert_node(&self, node: &NewNode) -> Result<(), StoreError> {
        let conn = self.lock()?;
        conn.execute(
            "INSERT INTO nodes (node_id, device_name, mac_address, connection_type, total_ram_mb, cpu_cores, has_npu)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
             ON CONFLICT(node_id) DO UPDATE SET
                device_name = excluded.device_name,
                mac_address = excluded.mac_address,
                connection_type = excluded.connection_type,
                total_ram_mb = excluded.total_ram_mb,
                cpu_cores = excluded.cpu_cores,
                has_npu = excluded.has_npu",
            params![
                node.node_id,
                node.device_name,
                node.mac_address,
                node.connection_type,
                node.total_ram_mb,
                node.cpu_cores,
                node.has_npu,
            ],
        )?;
        Ok(())
    }

    pub fn get_node(&self, node_id: &str) -> Result<Option<NodeRow>, StoreError> {
        let conn = self.lock()?;
        conn.query_row(
            "SELECT node_id, device_name, mac_address, connection_type, total_ram_mb, cpu_cores, has_npu, paired_at
             FROM nodes WHERE node_id = ?1",
            params![node_id],
            map_node,
        )
        .optional()
        .map_err(Into::into)
    }

    pub fn list_nodes(&self) -> Result<Vec<NodeRow>, StoreError> {
        let conn = self.lock()?;
        let mut stmt = conn.prepare(
            "SELECT node_id, device_name, mac_address, connection_type, total_ram_mb, cpu_cores, has_npu, paired_at
             FROM nodes ORDER BY paired_at, node_id",
        )?;
        let rows = stmt.query_map([], map_node)?;
        rows.collect::<Result<Vec<_>, _>>().map_err(Into::into)
    }

    // --- benchmarks --------------------------------------------------------

    pub fn record_benchmark(&self, benchmark: &NewBenchmark) -> Result<i64, StoreError> {
        let conn = self.lock()?;
        conn.execute(
            "INSERT INTO benchmarks (node_id, memory_bandwidth_mbs, network_ping_ms, throughput_flops)
             VALUES (?1, ?2, ?3, ?4)",
            params![
                benchmark.node_id,
                benchmark.memory_bandwidth_mbs,
                benchmark.network_ping_ms,
                benchmark.throughput_flops,
            ],
        )?;
        Ok(conn.last_insert_rowid())
    }

    pub fn list_benchmarks(&self, node_id: &str) -> Result<Vec<BenchmarkRow>, StoreError> {
        let conn = self.lock()?;
        let mut stmt = conn.prepare(
            "SELECT id, node_id, memory_bandwidth_mbs, network_ping_ms, throughput_flops, tested_at
             FROM benchmarks WHERE node_id = ?1 ORDER BY tested_at DESC, id DESC",
        )?;
        let rows = stmt.query_map(params![node_id], map_benchmark)?;
        rows.collect::<Result<Vec<_>, _>>().map_err(Into::into)
    }

    // --- models ------------------------------------------------------------

    pub fn upsert_model(&self, model: &NewModel) -> Result<(), StoreError> {
        let conn = self.lock()?;
        conn.execute(
            "INSERT INTO models (model_id, file_name, quantization_format, total_layers, weight_size_bytes, local_file_path)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)
             ON CONFLICT(model_id) DO UPDATE SET
                file_name = excluded.file_name,
                quantization_format = excluded.quantization_format,
                total_layers = excluded.total_layers,
                weight_size_bytes = excluded.weight_size_bytes,
                local_file_path = excluded.local_file_path",
            params![
                model.model_id,
                model.file_name,
                model.quantization_format,
                model.total_layers,
                model.weight_size_bytes,
                model.local_file_path,
            ],
        )?;
        Ok(())
    }

    pub fn get_model(&self, model_id: &str) -> Result<Option<ModelRow>, StoreError> {
        let conn = self.lock()?;
        conn.query_row(
            "SELECT model_id, file_name, quantization_format, total_layers, weight_size_bytes, local_file_path, added_at
             FROM models WHERE model_id = ?1",
            params![model_id],
            map_model,
        )
        .optional()
        .map_err(Into::into)
    }

    /// Registers a cloud-only model id without a local GGUF. Existing virtual
    /// rows are refreshed idempotently; a real local model with the same id is
    /// a conflict, so a cloud route can never shadow a downloaded GGUF.
    pub fn ensure_virtual_model(&self, model_id: &str, provider: &str) -> Result<(), StoreError> {
        if let Some(existing) = self.get_model(model_id)? {
            if !existing
                .local_file_path
                .starts_with(VIRTUAL_MODEL_PATH_PREFIX)
            {
                return Err(StoreError::Conflict(format!(
                    "model id '{model_id}' is already a local GGUF"
                )));
            }
        }
        self.upsert_model(&NewModel {
            model_id: model_id.to_string(),
            file_name: model_id.to_string(),
            quantization_format: "cloud".to_string(),
            total_layers: 0,
            weight_size_bytes: 0,
            local_file_path: format!("{VIRTUAL_MODEL_PATH_PREFIX}{provider}"),
        })
    }

    pub fn list_models(&self) -> Result<Vec<ModelRow>, StoreError> {
        let conn = self.lock()?;
        let mut stmt = conn.prepare(
            "SELECT model_id, file_name, quantization_format, total_layers, weight_size_bytes, local_file_path, added_at
             FROM models ORDER BY added_at, model_id",
        )?;
        let rows = stmt.query_map([], map_model)?;
        rows.collect::<Result<Vec<_>, _>>().map_err(Into::into)
    }

    // --- chat --------------------------------------------------------------

    pub fn create_session(
        &self,
        session_id: &str,
        title: &str,
        model_id: &str,
    ) -> Result<(), StoreError> {
        let conn = self.lock()?;
        conn.execute(
            "INSERT INTO chat_sessions (session_id, title, model_id) VALUES (?1, ?2, ?3)",
            params![session_id, title, model_id],
        )?;
        Ok(())
    }

    pub fn get_session(&self, session_id: &str) -> Result<Option<ChatSessionRow>, StoreError> {
        let conn = self.lock()?;
        conn.query_row(
            "SELECT session_id, title, model_id, created_at FROM chat_sessions WHERE session_id = ?1",
            params![session_id],
            |row| {
                Ok(ChatSessionRow {
                    session_id: row.get(0)?,
                    title: row.get(1)?,
                    model_id: row.get(2)?,
                    created_at: row.get(3)?,
                })
            },
        )
        .optional()
        .map_err(Into::into)
    }

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

    pub fn list_sessions(&self) -> Result<Vec<ChatSessionRow>, StoreError> {
        let conn = self.lock()?;
        let mut stmt = conn.prepare(
            "SELECT session_id, title, model_id, created_at FROM chat_sessions ORDER BY created_at, session_id",
        )?;
        let rows = stmt.query_map([], |row| {
            Ok(ChatSessionRow {
                session_id: row.get(0)?,
                title: row.get(1)?,
                model_id: row.get(2)?,
                created_at: row.get(3)?,
            })
        })?;
        rows.collect::<Result<Vec<_>, _>>().map_err(Into::into)
    }

    pub fn append_message(&self, message: &NewChatMessage) -> Result<i64, StoreError> {
        let conn = self.lock()?;
        // Text content still serializes to a JSON string, so existing rows and
        // readers that treat the column as plain text keep working.
        let content = serde_json::to_string(&message.content).map_err(|error| {
            StoreError::Sqlite(rusqlite::Error::ToSqlConversionFailure(Box::new(error)))
        })?;
        conn.execute(
            "INSERT INTO chat_messages (session_id, role, content, token_count, generation_time_sec)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![
                message.session_id,
                message.role,
                content,
                message.token_count,
                message.generation_time_sec,
            ],
        )?;
        Ok(conn.last_insert_rowid())
    }

    pub fn list_messages(&self, session_id: &str) -> Result<Vec<ChatMessageRow>, StoreError> {
        let conn = self.lock()?;
        let mut stmt = conn.prepare(
            "SELECT message_id, session_id, role, content, token_count, generation_time_sec, created_at
             FROM chat_messages WHERE session_id = ?1 ORDER BY message_id",
        )?;
        let rows = stmt.query_map(params![session_id], map_message)?;
        rows.collect::<Result<Vec<_>, _>>().map_err(Into::into)
    }

    // --- workflows ----------------------------------------------------------

    pub fn create_workflow_run(
        &self,
        run_id: &str,
        model: &str,
        steps: &[NewWorkflowStep],
    ) -> Result<(), StoreError> {
        let mut conn = self.lock()?;
        let tx = conn.transaction()?;
        tx.execute(
            "INSERT INTO workflow_runs (run_id, model, status) VALUES (?1, ?2, 'running')",
            params![run_id, model],
        )?;
        for step in steps {
            tx.execute(
                "INSERT INTO workflow_steps \
                 (run_id, step_id, prompt, depends_on, provider, status, position) \
                 VALUES (?1, ?2, ?3, ?4, ?5, 'pending', ?6)",
                params![
                    run_id,
                    step.step_id,
                    step.prompt,
                    step.depends_on_json,
                    step.provider,
                    step.position,
                ],
            )?;
        }
        tx.commit()?;
        Ok(())
    }

    pub fn list_workflow_runs(&self) -> Result<Vec<WorkflowRunRow>, StoreError> {
        let conn = self.lock()?;
        let mut stmt = conn.prepare(
            "SELECT run_id, model, status, created_at, finished_at \
             FROM workflow_runs ORDER BY created_at, run_id",
        )?;
        let rows = stmt.query_map([], |row| {
            Ok(WorkflowRunRow {
                run_id: row.get(0)?,
                model: row.get(1)?,
                status: row.get(2)?,
                created_at: row.get(3)?,
                finished_at: row.get(4)?,
            })
        })?;
        rows.collect::<Result<Vec<_>, _>>().map_err(Into::into)
    }

    pub fn get_workflow_run(&self, run_id: &str) -> Result<Option<WorkflowRunRow>, StoreError> {
        let conn = self.lock()?;
        conn.query_row(
            "SELECT run_id, model, status, created_at, finished_at \
             FROM workflow_runs WHERE run_id = ?1",
            params![run_id],
            |row| {
                Ok(WorkflowRunRow {
                    run_id: row.get(0)?,
                    model: row.get(1)?,
                    status: row.get(2)?,
                    created_at: row.get(3)?,
                    finished_at: row.get(4)?,
                })
            },
        )
        .optional()
        .map_err(Into::into)
    }

    pub fn list_workflow_steps(&self, run_id: &str) -> Result<Vec<WorkflowStepRow>, StoreError> {
        let conn = self.lock()?;
        let mut stmt = conn.prepare(
            "SELECT run_id, step_id, prompt, depends_on, provider, status, \
             output, source, error, position FROM workflow_steps \
             WHERE run_id = ?1 ORDER BY position, step_id",
        )?;
        let rows = stmt.query_map(params![run_id], |row| {
            let raw: String = row.get(3)?;
            let depends_on: Vec<String> = serde_json::from_str(&raw).map_err(|error| {
                rusqlite::Error::FromSqlConversionFailure(
                    3,
                    rusqlite::types::Type::Text,
                    Box::new(error),
                )
            })?;
            Ok(WorkflowStepRow {
                run_id: row.get(0)?,
                step_id: row.get(1)?,
                prompt: row.get(2)?,
                depends_on,
                provider: row.get(4)?,
                status: row.get(5)?,
                output: row.get(6)?,
                source: row.get(7)?,
                error: row.get(8)?,
                position: row.get(9)?,
            })
        })?;
        rows.collect::<Result<Vec<_>, _>>().map_err(Into::into)
    }

    pub fn set_workflow_step_status(
        &self,
        run_id: &str,
        step_id: &str,
        status: &str,
        output: Option<&str>,
        source: Option<&str>,
        error: Option<&str>,
    ) -> Result<(), StoreError> {
        let conn = self.lock()?;
        conn.execute(
            "UPDATE workflow_steps SET status = ?1, output = ?2, source = ?3, error = ?4 \
             WHERE run_id = ?5 AND step_id = ?6",
            params![status, output, source, error, run_id, step_id],
        )?;
        Ok(())
    }

    pub fn set_workflow_run_status(
        &self,
        run_id: &str,
        status: &str,
        finished: bool,
    ) -> Result<(), StoreError> {
        let conn = self.lock()?;
        if finished {
            conn.execute(
                "UPDATE workflow_runs SET status = ?1, finished_at = CURRENT_TIMESTAMP \
                 WHERE run_id = ?2",
                params![status, run_id],
            )?;
        } else {
            conn.execute(
                "UPDATE workflow_runs SET status = ?1 WHERE run_id = ?2",
                params![status, run_id],
            )?;
        }
        Ok(())
    }

    /// Inserts a chat message and its attachment metadata in one transaction.
    /// Invalid attachment metadata aborts the whole message.
    pub async fn insert_message_with_attachments(
        &self,
        msg: &NewChatMessage,
        attachments: &[(String, String, String, u64)],
    ) -> Result<i64, StoreError> {
        let content = serde_json::to_string(&msg.content).map_err(|error| {
            StoreError::Sqlite(rusqlite::Error::ToSqlConversionFailure(Box::new(error)))
        })?;
        let mut conn = self.lock()?;
        let tx = conn.transaction()?;
        let result = (|| -> Result<i64, StoreError> {
            tx.execute(
                "INSERT INTO chat_messages (session_id, role, content, token_count, generation_time_sec)
                 VALUES (?1, ?2, ?3, ?4, ?5)",
                params![
                    msg.session_id,
                    msg.role,
                    content,
                    msg.token_count,
                    msg.generation_time_sec,
                ],
            )?;
            let message_id = tx.last_insert_rowid();
            let created_at: String =
                tx.query_row("SELECT strftime('%Y-%m-%dT%H:%M:%fZ', 'now')", [], |row| {
                    row.get(0)
                })?;
            for (media_hash, mime_type, file_name, byte_count) in attachments {
                let valid_hash = media_hash.len() == 64
                    && media_hash
                        .bytes()
                        .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase());
                let byte_count =
                    i64::try_from(*byte_count).map_err(|_| StoreError::InvalidAttachment)?;
                if !valid_hash || byte_count == 0 {
                    return Err(StoreError::InvalidAttachment);
                }
                tx.execute(
                    "INSERT INTO attachments \
                     (message_id, media_hash, mime_type, file_name, byte_count, created_at) \
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                    params![message_id, media_hash, mime_type, file_name, byte_count, created_at,],
                )?;
            }
            Ok(message_id)
        })();
        match result {
            Ok(message_id) => {
                tx.commit()?;
                Ok(message_id)
            }
            Err(error) => {
                let _ = tx.rollback();
                Err(error)
            }
        }
    }

    /// Stored attachment metadata for one message.
    pub async fn attachments_for_message(
        &self,
        message_id: i64,
    ) -> Result<Vec<AttachmentRow>, StoreError> {
        let conn = self.lock()?;
        let mut stmt = conn.prepare(
            "SELECT message_id, media_hash, mime_type, file_name, byte_count, created_at \
             FROM attachments WHERE message_id = ?1 ORDER BY rowid",
        )?;
        let rows = stmt.query_map(params![message_id], |row| {
            Ok(AttachmentRow {
                message_id: row.get(0)?,
                media_hash: row.get(1)?,
                mime_type: row.get(2)?,
                file_name: row.get(3)?,
                byte_count: row.get(4)?,
                created_at: row.get(5)?,
            })
        })?;
        rows.collect::<Result<Vec<_>, _>>().map_err(Into::into)
    }

    // --- settings -----------------------------------------------------------

    pub fn get_settings(&self) -> Result<AppSettings, StoreError> {
        // Missing rows fall back to the application defaults, not to the
        // primitive `Default` values (0/false).
        let defaults = AppSettings::default();
        Ok(AppSettings {
            provider: self.setting("provider")?.unwrap_or(defaults.provider),
            model: self.setting("model")?.unwrap_or(defaults.model),
            context_tokens: self
                .setting("context_tokens")?
                .unwrap_or(defaults.context_tokens),
            max_tokens: self.setting("max_tokens")?.unwrap_or(defaults.max_tokens),
            show_source_badges: self
                .setting("show_source_badges")?
                .unwrap_or(defaults.show_source_badges),
            compact_mode: self
                .setting("compact_mode")?
                .unwrap_or(defaults.compact_mode),
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

    fn setting<T: DeserializeOwned>(&self, key: &str) -> Result<Option<T>, StoreError> {
        self.setting_json(key)?
            .map(|text| serde_json::from_str(&text).map_err(StoreError::SettingsJson))
            .transpose()
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

fn map_node(row: &rusqlite::Row<'_>) -> rusqlite::Result<NodeRow> {
    Ok(NodeRow {
        node_id: row.get(0)?,
        device_name: row.get(1)?,
        mac_address: row.get(2)?,
        connection_type: row.get(3)?,
        total_ram_mb: row.get(4)?,
        cpu_cores: row.get(5)?,
        has_npu: row.get(6)?,
        paired_at: row.get(7)?,
    })
}

fn map_model(row: &rusqlite::Row<'_>) -> rusqlite::Result<ModelRow> {
    Ok(ModelRow {
        model_id: row.get(0)?,
        file_name: row.get(1)?,
        quantization_format: row.get(2)?,
        total_layers: row.get(3)?,
        weight_size_bytes: row.get(4)?,
        local_file_path: row.get(5)?,
        added_at: row.get(6)?,
    })
}

fn map_benchmark(row: &rusqlite::Row<'_>) -> rusqlite::Result<BenchmarkRow> {
    Ok(BenchmarkRow {
        id: row.get(0)?,
        node_id: row.get(1)?,
        memory_bandwidth_mbs: row.get(2)?,
        network_ping_ms: row.get(3)?,
        throughput_flops: row.get(4)?,
        tested_at: row.get(5)?,
    })
}

fn map_message(row: &rusqlite::Row<'_>) -> rusqlite::Result<ChatMessageRow> {
    let raw: String = row.get(3)?;
    // New rows hold serialized `MessageContent`; legacy rows held the raw text
    // without JSON quotes. Falling back keeps old databases readable.
    let content = match serde_json::from_str::<MessageContent>(&raw) {
        Ok(content) => content,
        Err(_) => MessageContent::Text(raw),
    };
    Ok(ChatMessageRow {
        message_id: row.get(0)?,
        session_id: row.get(1)?,
        role: row.get(2)?,
        content,
        token_count: row.get(4)?,
        generation_time_sec: row.get(5)?,
        created_at: row.get(6)?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn store() -> Store {
        Store::open_in_memory().expect("in-memory store")
    }

    #[test]
    fn node_upsert_is_idempotent_and_updates() {
        let store = store();
        let node = NewNode {
            node_id: "adb:SERIAL1".into(),
            device_name: "Pixel 8".into(),
            mac_address: None,
            connection_type: "usb_adb".into(),
            total_ram_mb: 8192,
            cpu_cores: 8,
            has_npu: true,
        };
        store.upsert_node(&node).unwrap();

        let updated = NewNode {
            device_name: "Pixel 8 Pro".into(),
            total_ram_mb: 12288,
            ..node
        };
        store.upsert_node(&updated).unwrap();

        let nodes = store.list_nodes().unwrap();
        assert_eq!(nodes.len(), 1);
        assert_eq!(nodes[0].device_name, "Pixel 8 Pro");
        assert_eq!(nodes[0].total_ram_mb, 12288);
        assert!(nodes[0].has_npu);
        assert_eq!(store.get_node("adb:SERIAL1").unwrap().unwrap().cpu_cores, 8);
    }

    #[test]
    fn models_round_trip_and_order() {
        let store = store();
        store
            .upsert_model(&NewModel {
                model_id: "llama-8b".into(),
                file_name: "Llama-3-8B-Q4_K_M.gguf".into(),
                quantization_format: "Q4_K_M".into(),
                total_layers: 32,
                weight_size_bytes: 4_920_000_000,
                local_file_path: "/models/llama.gguf".into(),
            })
            .unwrap();

        let models = store.list_models().unwrap();
        assert_eq!(models.len(), 1);
        assert_eq!(models[0].total_layers, 32);
        assert!(store.get_model("missing").unwrap().is_none());
    }

    #[test]
    fn records_and_reads_back_benchmarks_newest_first() {
        let store = store();
        store
            .upsert_node(&NewNode {
                node_id: "n1".into(),
                device_name: "d1".into(),
                mac_address: None,
                connection_type: "native".into(),
                total_ram_mb: 8192,
                cpu_cores: 8,
                has_npu: false,
            })
            .unwrap();

        store
            .record_benchmark(&NewBenchmark {
                node_id: "n1".into(),
                memory_bandwidth_mbs: 100.0,
                network_ping_ms: 1.5,
                throughput_flops: 12.0,
            })
            .unwrap();
        store
            .record_benchmark(&NewBenchmark {
                node_id: "n1".into(),
                memory_bandwidth_mbs: 200.0,
                network_ping_ms: 0.5,
                throughput_flops: 24.0,
            })
            .unwrap();

        let benchmarks = store.list_benchmarks("n1").unwrap();
        assert_eq!(benchmarks.len(), 2);
        assert_eq!(benchmarks[0].memory_bandwidth_mbs, 200.0);
    }

    #[test]
    fn benchmark_for_unknown_node_is_rejected_by_foreign_key() {
        let store = store();
        let result = store.record_benchmark(&NewBenchmark {
            node_id: "ghost".into(),
            memory_bandwidth_mbs: 1.0,
            network_ping_ms: 1.0,
            throughput_flops: 0.0,
        });
        assert!(result.is_err(), "FK constraint must reject unknown node");
    }

    #[test]
    fn chat_session_and_messages_round_trip() {
        let store = store();
        store
            .upsert_model(&NewModel {
                model_id: "m1".into(),
                file_name: "m1.gguf".into(),
                quantization_format: "Q4_K_M".into(),
                total_layers: 32,
                weight_size_bytes: 1,
                local_file_path: "/m1.gguf".into(),
            })
            .unwrap();
        store.create_session("s1", "Hello", "m1").unwrap();
        store
            .append_message(&NewChatMessage {
                session_id: "s1".into(),
                role: "user".into(),
                content: "hi".into(),
                token_count: 1,
                generation_time_sec: 0.0,
            })
            .unwrap();
        store
            .append_message(&NewChatMessage {
                session_id: "s1".into(),
                role: "assistant".into(),
                content: "hello!".into(),
                token_count: 2,
                generation_time_sec: 0.25,
            })
            .unwrap();

        let messages = store.list_messages("s1").unwrap();
        assert_eq!(messages.len(), 2);
        assert_eq!(messages[1].content, MessageContent::Text("hello!".into()));
        assert_eq!(messages[1].generation_time_sec, 0.25);
        assert_eq!(store.list_sessions().unwrap().len(), 1);
    }

    use crate::api::backend::ContentPart;

    #[tokio::test]
    async fn insert_message_with_attachments_is_atomic() {
        let db = store();
        db.upsert_model(&NewModel {
            model_id: "m1".into(),
            file_name: "m1.gguf".into(),
            quantization_format: "Q4_K_M".into(),
            total_layers: 32,
            weight_size_bytes: 1,
            local_file_path: "/m1.gguf".into(),
        })
        .unwrap();
        db.create_session("1", "s1", "m1").unwrap();
        let msg = NewChatMessage {
            session_id: "1".into(),
            role: "user".into(),
            content: MessageContent::Parts(vec![
                ContentPart::Text {
                    text: "look".into(),
                },
                ContentPart::Image {
                    media_hash: "cd".repeat(32),
                    mime_type: "image/png".into(),
                    file_name: "a.png".into(),
                    byte_count: 4,
                },
            ]),
            token_count: 0,
            generation_time_sec: 0.0,
        };
        db.insert_message_with_attachments(
            &msg,
            &[("cd".repeat(32), "image/png".into(), "a.png".into(), 4)],
        )
        .await
        .unwrap();
        let rows = db.attachments_for_message(1).await.unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].mime_type, "image/png");
    }

    #[tokio::test]
    async fn invalid_attachment_rolls_back_message() {
        let db = store();
        db.upsert_model(&NewModel {
            model_id: "m1".into(),
            file_name: "m1.gguf".into(),
            quantization_format: "Q4_K_M".into(),
            total_layers: 32,
            weight_size_bytes: 1,
            local_file_path: "/m1.gguf".into(),
        })
        .unwrap();
        db.create_session("2", "s2", "m1").unwrap();
        let msg = NewChatMessage {
            session_id: "2".into(),
            role: "user".into(),
            content: MessageContent::Text("x".into()),
            token_count: 0,
            generation_time_sec: 0.0,
        };
        let res = db
            .insert_message_with_attachments(
                &msg,
                &[("bad-hash".into(), "image/png".into(), "a.png".into(), 4)],
            )
            .await;
        assert!(res.is_err());
        assert!(db.list_messages("2").unwrap().is_empty());
    }

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
        assert_eq!(
            store.setting_json("context_tokens").unwrap().unwrap(),
            "16384"
        );
        assert_eq!(
            store.setting_json("provider").unwrap().unwrap(),
            "\"openai\""
        );
    }

    #[test]
    fn settings_default_when_table_is_empty() {
        let store = store();
        assert_eq!(
            store.get_settings().unwrap(),
            crate::settings::AppSettings::default()
        );
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
        assert_eq!(
            store.get_settings().unwrap(),
            crate::settings::AppSettings::default()
        );
    }

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
                content: MessageContent::Text("keep me out after delete".into()),
                token_count: 3,
                generation_time_sec: 0.1,
            })
            .unwrap();

        assert!(store.delete_session("s1").unwrap());
        assert!(store.get_session("s1").unwrap().is_none());
        assert!(store.list_messages("s1").unwrap().is_empty());
        assert!(!store.delete_session("s1").unwrap());
    }
}
