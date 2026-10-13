//! `GET /v1/models` — OpenAI-compatible model listing backed by the store.

use crate::api::{ApiResult, AppState};
use crate::store::ModelRow;
use axum::extract::State;
use axum::Json;
use serde::Serialize;

#[derive(Debug, Serialize)]
pub struct ModelList {
    pub object: &'static str,
    pub data: Vec<ModelEntry>,
}

#[derive(Debug, Serialize)]
pub struct ModelEntry {
    pub id: String,
    pub object: &'static str,
    pub created: u64,
    pub owned_by: &'static str,
    pub meta: ModelMeta,
}

#[derive(Debug, Serialize)]
pub struct ModelMeta {
    pub quantization: String,
    pub file_size_bytes: i64,
    pub layers: i64,
    pub recommended_min_ram_gb: f64,
}

pub async fn list_models(State(state): State<AppState>) -> ApiResult<Json<ModelList>> {
    let models = state.store.list_models()?;
    Ok(Json(ModelList {
        object: "list",
        data: models.into_iter().map(entry_from_row).collect(),
    }))
}

fn entry_from_row(row: ModelRow) -> ModelEntry {
    ModelEntry {
        id: row.model_id,
        object: "model",
        created: crate::api::sqlite_timestamp_to_epoch(&row.added_at).unwrap_or(0),
        owned_by: "ai-upcycler-local",
        meta: ModelMeta {
            quantization: row.quantization_format,
            file_size_bytes: row.weight_size_bytes,
            layers: row.total_layers,
            recommended_min_ram_gb: round_to_one_decimal(
                row.weight_size_bytes as f64 / (1024.0 * 1024.0 * 1024.0),
            ),
        },
    }
}

fn round_to_one_decimal(value: f64) -> f64 {
    (value * 10.0).round() / 10.0
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row() -> ModelRow {
        ModelRow {
            model_id: "llama-8b".into(),
            file_name: "llama.gguf".into(),
            quantization_format: "q4_k_m".into(),
            total_layers: 32,
            weight_size_bytes: 4_750_000_000,
            local_file_path: "/llama.gguf".into(),
            added_at: "2025-01-01 00:00:00".into(),
        }
    }

    #[test]
    fn maps_row_to_openai_entry() {
        let entry = entry_from_row(row());
        assert_eq!(entry.id, "llama-8b");
        assert_eq!(entry.object, "model");
        assert_eq!(entry.created, 1_735_689_600);
        assert_eq!(entry.meta.layers, 32);
        assert_eq!(entry.meta.quantization, "q4_k_m");
        assert!((entry.meta.recommended_min_ram_gb - 4.4).abs() < 0.1);
    }

    #[test]
    fn invalid_timestamp_falls_back_to_zero() {
        let mut r = row();
        r.added_at = "not-a-time".into();
        assert_eq!(entry_from_row(r).created, 0);
    }
}
