//! `WorkerService` implementation: engine state machine behind RPC.
//!
//! `Handshake`/`GetHealthStatus` return real host metadata; `LoadLayers`/
//! `UnloadLayers` retain real materialized F32 weights for the requested
//! range. `ExecuteForward` refuses honestly until the forward slice lands.

use crate::gguf::GgufFile;
use crate::proto::aiupcycler::rpc::{
    worker_service_server::WorkerService, ExecuteForwardRequest, ExecuteForwardResponse,
    HandshakeRequest, HandshakeResponse, HealthRequest, HealthResponse, LoadLayersRequest,
    LoadLayersResponse, UnloadLayersRequest, UnloadLayersResponse,
};
use crate::quant::{dequantize, GgmlType};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use tonic::{Request, Response, Status};

/// One worker process: holds the parsed model file, the loaded layer range
/// (later slices) and the per-session forward state.
pub struct WorkerEngine {
    state: Arc<Mutex<EngineState>>,
}

struct EngineState {
    worker_id: String,
    device_name: String,
    model: GgufFile,
    loaded: Option<LoadedRange>,
}

/// Retained materialized weights for the loaded layer range; the forward
/// slices read `start`/`end` and the F32 tensors.
#[allow(dead_code)] // fields read by the forward/split slices
struct LoadedRange {
    model_id: String,
    start: u32,
    end: u32,
    tensors: HashMap<String, Vec<f32>>,
    bytes: u64,
}

/// The per-block tensors the qwen2 forward pass consumes; a file missing
/// any of these is refused loudly (never partially loaded).
const BLOCK_TENSORS_REQUIRED: [&str; 9] = [
    "attn_norm.weight",
    "ffn_norm.weight",
    "attn_q.weight",
    "attn_k.weight",
    "attn_v.weight",
    "attn_output.weight",
    "ffn_gate.weight",
    "ffn_up.weight",
    "ffn_down.weight",
];

/// Optional per-block extras: qwen2 files have no QK-norm tensors, but
/// files that ship them (Qwen3-style shapes) must be retained too.
const BLOCK_TENSORS_OPTIONAL: [&str; 2] = ["attn_q_norm.weight", "attn_k_norm.weight"];

impl WorkerEngine {
    /// `device_name` empty resolves to the host name (or `"host"`). A model
    /// file that fails GGUF validation makes construction fail — the binary
    /// exits 1 before ever announcing `READY`.
    pub fn new(worker_id: &str, device_name: &str, model_path: PathBuf) -> Result<Self, String> {
        let model = GgufFile::open(Path::new(&model_path)).map_err(|e| format!("model: {e}"))?;
        let device_name = if device_name.is_empty() {
            sysinfo::System::host_name().unwrap_or_else(|| "host".to_string())
        } else {
            device_name.to_string()
        };
        Ok(Self {
            state: Arc::new(Mutex::new(EngineState {
                worker_id: worker_id.to_string(),
                device_name,
                model,
                loaded: None,
            })),
        })
    }
}

impl WorkerEngine {
    #[allow(clippy::result_large_err)] // tonic::Status is large; all callers map it immediately
    fn state(&self) -> Result<std::sync::MutexGuard<'_, EngineState>, Status> {
        self.state
            .lock()
            .map_err(|_| Status::internal("engine state poisoned"))
    }

    fn host_memory() -> (u64, u64) {
        let mut sys = sysinfo::System::new();
        sys.refresh_memory();
        (sys.total_memory(), sys.free_memory())
    }
}

#[tonic::async_trait]
impl WorkerService for WorkerEngine {
    async fn handshake(
        &self,
        _request: Request<HandshakeRequest>,
    ) -> Result<Response<HandshakeResponse>, Status> {
        let state = self.state()?;
        let (total, free) = Self::host_memory();
        Ok(Response::new(HandshakeResponse {
            worker_id: state.worker_id.clone(),
            device_name: state.device_name.clone(),
            total_ram_bytes: total as i64,
            free_ram_bytes: free as i64,
            cpu_cores: std::thread::available_parallelism()
                .map(|n| n.get() as i32)
                .unwrap_or(1),
            has_npu_acceleration: false,
        }))
    }

    async fn load_layers(
        &self,
        request: Request<LoadLayersRequest>,
    ) -> Result<Response<LoadLayersResponse>, Status> {
        let req = request.into_inner();
        let mut state = self.state()?;
        let model_name = state.model.model_name().to_string();
        let block_count = state.model.hparams().block_count;

        #[allow(clippy::result_large_err)] // tonic::Status is large; returns are used immediately
        let refuse = |msg: String| -> Result<Response<LoadLayersResponse>, Status> {
            Ok(Response::new(LoadLayersResponse {
                success: false,
                error_message: msg,
                allocated_ram_bytes: 0,
            }))
        };

        // Validation order: cheap refusals before any tensor materialization.
        if req.model_id != model_name {
            return refuse(format!(
                "unknown model_id {:?}; this worker holds {:?}",
                req.model_id, model_name
            ));
        }
        let (start, end) = (req.start_layer, req.end_layer);
        if start < 0 || start > end || end >= block_count as i32 {
            return refuse(format!(
                "invalid layer range {start}..={end} for a model with {block_count} blocks"
            ));
        }
        if state.loaded.is_some() {
            return refuse("layers already loaded; unload before loading again".to_string());
        }

        // Ownership rule: token_embd iff start==0, the 11 blk.{i}.* tensors
        // per owned layer, and the head (output_norm, plus output.weight when
        // the model is untied) iff end is the last block.
        let mut names: Vec<String> = Vec::new();
        if start == 0 {
            names.push("token_embd.weight".to_string());
        }
        for i in start..=end {
            for part in BLOCK_TENSORS_REQUIRED {
                names.push(format!("blk.{i}.{part}"));
            }
            for part in BLOCK_TENSORS_OPTIONAL {
                let name = format!("blk.{i}.{part}");
                if state.model.tensor(&name).is_some() {
                    names.push(name);
                }
            }
        }
        if end == block_count as i32 - 1 {
            names.push("output_norm.weight".to_string());
            if state.model.tensor("output.weight").is_some() {
                names.push("output.weight".to_string());
            }
        }

        let mut tensors: HashMap<String, Vec<f32>> = HashMap::with_capacity(names.len());
        let mut bytes = 0u64;
        for name in &names {
            let Some(desc) = state.model.tensor(name) else {
                return refuse(format!("model is missing tensor {name}"));
            };
            let Some(ty) = GgmlType::from_id(desc.ggml_type) else {
                return refuse(format!(
                    "tensor {name} has unsupported ggml type id {}",
                    desc.ggml_type
                ));
            };
            let raw = match state.model.read_raw(name) {
                Ok(raw) => raw,
                Err(e) => return refuse(format!("tensor {name} could not be read: {e}")),
            };
            let f32s = match dequantize(ty, &raw, &desc.ne) {
                Ok(f32s) => f32s,
                Err(e) => return refuse(format!("tensor {name}: {e}")),
            };
            bytes += (f32s.len() * 4) as u64;
            tensors.insert(name.clone(), f32s);
        }

        state.loaded = Some(LoadedRange {
            model_id: model_name,
            start: start as u32,
            end: end as u32,
            tensors,
            bytes,
        });
        Ok(Response::new(LoadLayersResponse {
            success: true,
            error_message: String::new(),
            allocated_ram_bytes: bytes as i64,
        }))
    }

    async fn unload_layers(
        &self,
        request: Request<UnloadLayersRequest>,
    ) -> Result<Response<UnloadLayersResponse>, Status> {
        let req = request.into_inner();
        let mut state = self.state()?;
        let model_name = state.model.model_name().to_string();
        if req.model_id != model_name {
            return Ok(Response::new(UnloadLayersResponse {
                success: false,
                error_message: format!(
                    "unknown model_id {:?}; this worker holds {:?}",
                    req.model_id, model_name
                ),
                freed_ram_bytes: 0,
            }));
        }
        // Never-loaded unload reports success with zero bytes so the master's
        // rollback path never gets stuck.
        let freed = state.loaded.take().map(|loaded| loaded.bytes).unwrap_or(0);
        Ok(Response::new(UnloadLayersResponse {
            success: true,
            error_message: String::new(),
            freed_ram_bytes: freed as i64,
        }))
    }

    async fn execute_forward(
        &self,
        _request: Request<ExecuteForwardRequest>,
    ) -> Result<Response<ExecuteForwardResponse>, Status> {
        eprintln!("[worker] execute_forward refused: not implemented yet");
        Ok(Response::new(ExecuteForwardResponse {
            success: false,
            output_activation_tensor: None,
            execution_time_us: 0,
        }))
    }

    async fn get_health_status(
        &self,
        _request: Request<HealthRequest>,
    ) -> Result<Response<HealthResponse>, Status> {
        let (_, free) = Self::host_memory();
        Ok(Response::new(HealthResponse {
            battery_percentage: 0.0,
            cpu_temp_celsius: 0.0,
            free_ram_bytes: free as i64,
        }))
    }
}
