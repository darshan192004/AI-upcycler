//! Embedded SQLite persistence for the master node.
//!
//! Wraps a single [`rusqlite::Connection`] behind a mutex so the store can be
//! shared across async handlers (SQLite is single-writer). The schema mirrors
//! `docs/database.md` and is applied idempotently on open.

use rusqlite::{params, Connection, OptionalExtension};
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
}

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
    pub content: String,
    pub token_count: i64,
    pub generation_time_sec: f64,
    pub created_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NewChatMessage {
    pub session_id: String,
    pub role: String,
    pub content: String,
    pub token_count: i64,
    pub generation_time_sec: f64,
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
        conn.execute(
            "INSERT INTO chat_messages (session_id, role, content, token_count, generation_time_sec)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![
                message.session_id,
                message.role,
                message.content,
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
    Ok(ChatMessageRow {
        message_id: row.get(0)?,
        session_id: row.get(1)?,
        role: row.get(2)?,
        content: row.get(3)?,
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
        assert_eq!(messages[1].content, "hello!");
        assert_eq!(messages[1].generation_time_sec, 0.25);
        assert_eq!(store.list_sessions().unwrap().len(), 1);
    }
}
