//! `WorkerService` implementation: engine state machine behind RPC.
//!
//! Slice 1 (scaffold): `Handshake`/`GetHealthStatus` return real host
//! metadata; the compute RPCs refuse honestly until the load/forward
//! slices land.

use crate::proto::aiupcycler::rpc::{
    worker_service_server::WorkerService, ExecuteForwardRequest, ExecuteForwardResponse,
    HandshakeRequest, HandshakeResponse, HealthRequest, HealthResponse, LoadLayersRequest,
    LoadLayersResponse, UnloadLayersRequest, UnloadLayersResponse,
};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use tonic::{Request, Response, Status};

/// One worker process: holds the model path, the loaded layer range
/// (later slices) and the per-session forward state.
pub struct WorkerEngine {
    state: Arc<Mutex<EngineState>>,
}

struct EngineState {
    worker_id: String,
    device_name: String,
    #[allow(dead_code)] // consumed by the GGUF-load slice
    model_path: PathBuf,
}

impl WorkerEngine {
    /// `device_name` empty resolves to the host name (or `"host"`).
    pub fn new(worker_id: &str, device_name: &str, model_path: PathBuf) -> Result<Self, String> {
        let device_name = if device_name.is_empty() {
            sysinfo::System::host_name().unwrap_or_else(|| "host".to_string())
        } else {
            device_name.to_string()
        };
        Ok(Self {
            state: Arc::new(Mutex::new(EngineState {
                worker_id: worker_id.to_string(),
                device_name,
                model_path,
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
        Ok(Response::new(LoadLayersResponse {
            success: false,
            error_message: format!(
                "not implemented yet (worker engine slice 1): model {}",
                req.model_id
            ),
            allocated_ram_bytes: 0,
        }))
    }

    async fn unload_layers(
        &self,
        request: Request<UnloadLayersRequest>,
    ) -> Result<Response<UnloadLayersResponse>, Status> {
        let req = request.into_inner();
        Ok(Response::new(UnloadLayersResponse {
            success: false,
            error_message: format!(
                "not implemented yet (worker engine slice 1): model {}",
                req.model_id
            ),
            freed_ram_bytes: 0,
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
