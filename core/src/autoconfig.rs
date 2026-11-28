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
            used_bytes: 4 * 1024 * 1024 * 1024 - 8 * 1024 * 1024,
            available_bytes: 8 * 1024 * 1024,
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
