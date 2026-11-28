-- AI Upcycler embedded SQLite schema (see docs/database.md).
-- All statements are idempotent so the schema can be applied on every open.

CREATE TABLE IF NOT EXISTS nodes (
    node_id TEXT PRIMARY KEY,
    device_name TEXT NOT NULL,
    mac_address TEXT UNIQUE,
    connection_type TEXT NOT NULL, -- 'usb_adb', 'wifi_5g', 'native'
    total_ram_mb INTEGER NOT NULL,
    cpu_cores INTEGER NOT NULL,
    has_npu BOOLEAN DEFAULT 0,
    paired_at DATETIME DEFAULT CURRENT_TIMESTAMP
);

CREATE TABLE IF NOT EXISTS benchmarks (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    node_id TEXT NOT NULL,
    memory_bandwidth_mbs REAL NOT NULL,
    network_ping_ms REAL NOT NULL,
    throughput_flops REAL DEFAULT 0.0,
    tested_at DATETIME DEFAULT CURRENT_TIMESTAMP,
    FOREIGN KEY (node_id) REFERENCES nodes(node_id) ON DELETE CASCADE
);

CREATE TABLE IF NOT EXISTS models (
    model_id TEXT PRIMARY KEY,
    file_name TEXT NOT NULL,
    quantization_format TEXT NOT NULL, -- 'Q4_K_M', 'Q3_K_M', 'IQ2_XS'
    total_layers INTEGER NOT NULL,
    weight_size_bytes INTEGER NOT NULL,
    local_file_path TEXT NOT NULL,
    added_at DATETIME DEFAULT CURRENT_TIMESTAMP
);

CREATE TABLE IF NOT EXISTS chat_sessions (
    session_id TEXT PRIMARY KEY,
    title TEXT NOT NULL,
    model_id TEXT NOT NULL,
    created_at DATETIME DEFAULT CURRENT_TIMESTAMP,
    FOREIGN KEY (model_id) REFERENCES models(model_id)
);

CREATE TABLE IF NOT EXISTS chat_messages (
    message_id INTEGER PRIMARY KEY AUTOINCREMENT,
    session_id TEXT NOT NULL,
    role TEXT NOT NULL, -- 'system', 'user', 'assistant'
    content TEXT NOT NULL,
    token_count INTEGER DEFAULT 0,
    generation_time_sec REAL DEFAULT 0.0,
    created_at DATETIME DEFAULT CURRENT_TIMESTAMP,
    FOREIGN KEY (session_id) REFERENCES chat_sessions(session_id) ON DELETE CASCADE
);

-- Task workflow runs (spec 2026-11-27, §10). All DDL remains idempotent so
-- this file can be applied on every open alongside sub-project D's settings.
CREATE TABLE IF NOT EXISTS workflow_runs (
    run_id TEXT PRIMARY KEY,
    model TEXT NOT NULL,
    status TEXT NOT NULL,          -- 'running', 'completed', 'failed'
    created_at DATETIME DEFAULT CURRENT_TIMESTAMP,
    finished_at DATETIME
);

CREATE TABLE IF NOT EXISTS workflow_steps (
    run_id TEXT NOT NULL,
    step_id TEXT NOT NULL,
    prompt TEXT NOT NULL,
    depends_on TEXT NOT NULL,      -- JSON array of stable step ids
    provider TEXT,                 -- NULL/'auto'/'local'/provider id
    status TEXT NOT NULL,          -- pending|running|completed|failed|blocked
    output TEXT,
    source TEXT,
    error TEXT,
    position INTEGER NOT NULL,
    PRIMARY KEY (run_id, step_id),
    FOREIGN KEY (run_id) REFERENCES workflow_runs(run_id) ON DELETE CASCADE
);

CREATE TABLE IF NOT EXISTS attachments (
  message_id INTEGER NOT NULL REFERENCES chat_messages(message_id) ON DELETE CASCADE,
  media_hash TEXT NOT NULL,
  mime_type TEXT NOT NULL,
  file_name TEXT NOT NULL,
  byte_count INTEGER NOT NULL,
  created_at TEXT NOT NULL,
  PRIMARY KEY (message_id, media_hash)
);
CREATE INDEX IF NOT EXISTS idx_attachments_hash ON attachments(media_hash);

-- Task 4 (sub-project D): persistent non-secret application settings.
CREATE TABLE IF NOT EXISTS settings (
    key TEXT PRIMARY KEY NOT NULL,
    value_json TEXT NOT NULL,
    updated_at_epoch_ms INTEGER NOT NULL
);

-- Sub-project F Task 4: content-addressed generated-image metadata only;
-- image bytes live on disk, never in SQLite.
CREATE TABLE IF NOT EXISTS images (
    image_id TEXT PRIMARY KEY,
    provider TEXT NOT NULL,
    model TEXT NOT NULL,
    prompt TEXT NOT NULL,
    media_sha256 TEXT NOT NULL,
    media_path TEXT NOT NULL,
    mime_type TEXT NOT NULL,
    byte_len INTEGER NOT NULL,
    size TEXT NOT NULL,
    created_at_epoch_ms INTEGER NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_images_created_at
    ON images(created_at_epoch_ms DESC);
