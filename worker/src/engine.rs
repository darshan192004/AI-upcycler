//! `WorkerService` implementation: engine state machine behind RPC.
//!
//! `Handshake`/`GetHealthStatus` return real host metadata; `LoadLayers`/
//! `UnloadLayers` retain real materialized F32 weights for the requested
//! range. `ExecuteForward` refuses honestly until the forward slice lands.

use crate::gguf::GgufFile;
use crate::layers::{embed_ids, lm_head, run_block, BlockWeights, KvLayer};
use crate::math::rms_norm;
use crate::proto::aiupcycler::rpc::{
    tensor_buffer::DataType, worker_service_server::WorkerService, BenchmarkRequest,
    BenchmarkResponse, ExecuteForwardRequest, ExecuteForwardResponse, HandshakeRequest,
    HandshakeResponse, HealthRequest, HealthResponse, LoadLayersRequest, LoadLayersResponse,
    TensorBuffer, UnloadLayersRequest, UnloadLayersResponse,
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
    sessions: HashMap<String, Session>,
}

/// Concurrent forward sessions (KV-cache continuations). The cap bounds
/// per-worker RAM growth: each session multiplies a full context-length cache.
const MAX_SESSIONS: usize = 16;

/// A forward session: KV caches for the loaded range plus the cache length
/// reached. `kv[idx]` = cache of layer `start + idx`.
struct Session {
    kv: Vec<KvLayer>,
    kv_len: usize,
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
/// files that ship them (Qwen3-style shapes) must be retained too. Attention
/// QKV biases (`blk.*.attn_{q,k,v}.bias`) exist in Qwen 0.5B-class files and
/// must be added to the projections.
const BLOCK_TENSORS_OPTIONAL: [&str; 5] = [
    "attn_q_norm.weight",
    "attn_k_norm.weight",
    "attn_q.bias",
    "attn_k.bias",
    "attn_v.bias",
];

impl WorkerEngine {
    /// `device_name` empty resolves to the host name (or `"host"`). A model
    /// file that fails GGUF validation makes construction fail Ã¢â‚¬â€ the binary
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
                sessions: HashMap::new(),
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

/// Restores one block's weights to the loaded map so later forwards share
/// the same allocation (the inverse of `BlockWeights::take`).
fn put_block_back(tensors: &mut HashMap<String, Vec<f32>>, layer: u32, w: BlockWeights) {
    let name = |part: &str| format!("blk.{layer}.{part}");
    tensors.insert(name("attn_norm.weight"), w.attn_norm);
    tensors.insert(name("ffn_norm.weight"), w.ffn_norm);
    if let Some(v) = w.q_norm {
        tensors.insert(name("attn_q_norm.weight"), v);
    }
    if let Some(v) = w.k_norm {
        tensors.insert(name("attn_k_norm.weight"), v);
    }
    if let Some(v) = w.q_bias {
        tensors.insert(name("attn_q.bias"), v);
    }
    if let Some(v) = w.k_bias {
        tensors.insert(name("attn_k.bias"), v);
    }
    if let Some(v) = w.v_bias {
        tensors.insert(name("attn_v.bias"), v);
    }
    tensors.insert(name("attn_q.weight"), w.attn_q);
    tensors.insert(name("attn_k.weight"), w.attn_k);
    tensors.insert(name("attn_v.weight"), w.attn_v);
    tensors.insert(name("attn_output.weight"), w.attn_output);
    tensors.insert(name("ffn_gate.weight"), w.ffn_gate);
    tensors.insert(name("ffn_up.weight"), w.ffn_up);
    tensors.insert(name("ffn_down.weight"), w.ffn_down);
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
            supports_last_row_logits: true,
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

        // Ownership rule: token_embd iff `start == 0` (stage-0 input), OR the
        // model is tied (no `output.weight`) and this range ends the model —
        // a tied model's head is `token_embd` reused, so the split's tail
        // stage must hold it too. Plus the 11 blk.{i}.* tensors per owned
        // layer, and the head (output_norm, plus `output.weight` when the
        // model is untied) iff `end` is the last block.
        let mut names: Vec<String> = Vec::new();
        if start == 0
            || (end == block_count as i32 - 1 && state.model.tensor("output.weight").is_none())
        {
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
        state.sessions.clear();
        Ok(Response::new(UnloadLayersResponse {
            success: true,
            error_message: String::new(),
            freed_ram_bytes: freed as i64,
        }))
    }

    async fn execute_forward(
        &self,
        request: Request<ExecuteForwardRequest>,
    ) -> Result<Response<ExecuteForwardResponse>, Status> {
        let req = request.into_inner();
        let mut state = self.state()?;
        let hprops = state.model.hparams().clone();
        let st = &mut *state;

        #[allow(clippy::result_large_err)] // tonic::Status is large; returns are used immediately
        let refuse = |reason: String| -> Result<Response<ExecuteForwardResponse>, Status> {
            eprintln!("[worker] forward refused: {reason}");
            Ok(Response::new(ExecuteForwardResponse {
                success: false,
                output_activation_tensor: None,
                execution_time_us: 0,
            }))
        };

        // ---- validation order: cheap structural checks before session work
        let Some(loaded) = st.loaded.as_mut() else {
            return refuse("no layers loaded".to_string());
        };
        if req.start_layer != loaded.start as i32 || req.end_layer != loaded.end as i32 {
            return refuse(format!(
                "forward range {}..={} does not match loaded {}..={}",
                req.start_layer, req.end_layer, loaded.start, loaded.end
            ));
        }
        let Some(tb) = &req.activation_tensor else {
            return refuse("missing activation_tensor".to_string());
        };
        if tb.data_type != DataType::Fp32 as i32 {
            return refuse(format!(
                "activation data_type {} not supported (FP32 only)",
                tb.data_type
            ));
        }

        // Stage roles: start==0 carries token ids [1, n]; later stages carry
        // hidden activations [n, hidden].
        let hidden = hprops.embedding_length as usize;
        let stage0 = loaded.start == 0;
        let shape_ok = if stage0 {
            tb.shape.len() == 2 && tb.shape[0] == 1 && tb.shape[1] >= 1
        } else {
            tb.shape.len() == 2 && tb.shape[0] >= 1 && tb.shape[1] == hidden as i64
        };
        if !shape_ok {
            return refuse(format!(
                "activation shape {:?} not valid for a stage starting at {} (ids expect [1, n], hidden expects [n, {hidden}])",
                tb.shape, loaded.start
            ));
        }
        let n_tokens = (if stage0 { tb.shape[1] } else { tb.shape[0] }) as usize;

        let seq_pos = req.sequence_position;
        if seq_pos < 0 {
            return refuse(format!("negative sequence_position {seq_pos}"));
        }
        if (seq_pos as usize) + n_tokens > hprops.context_length as usize {
            return refuse(format!(
                "position {seq_pos} + {n_tokens} tokens exceeds context_length {}",
                hprops.context_length
            ));
        }

        // ---- session admission -------------------------------------------------
        if !st.sessions.contains_key(&req.session_id) && st.sessions.len() >= MAX_SESSIONS {
            return refuse(format!(
                "session cap ({MAX_SESSIONS}) reached; no free slot for {:?}",
                req.session_id
            ));
        }
        let kv_dim = (hprops.head_count_kv * hprops.head_dim) as usize;
        let layers_owned = (loaded.end - loaded.start) as usize + 1;
        let kv_len = st
            .sessions
            .get(&req.session_id)
            .map(|s| s.kv_len)
            .unwrap_or(0);
        if (seq_pos as usize) != kv_len {
            return refuse(format!(
                "sequence_position {seq_pos} != cached sequence length {kv_len} (out of order)"
            ));
        }

        // ---- decode/validate inputs ------------------------------------------
        let positions = kv_len..kv_len + n_tokens;
        let mut x: Vec<f32> = if stage0 {
            let Some(emb) = loaded.tensors.get("token_embd.weight") else {
                return refuse(
                    "stage 0 requires token_embd.weight in the loaded range".to_string(),
                );
            };
            let raw: Vec<f32> = tb
                .raw_data
                .as_chunks::<4>()
                .0
                .iter()
                .map(|c| f32::from_le_bytes(*c))
                .collect();
            if raw.len() != n_tokens {
                return refuse(format!(
                    "stage-0 payload {} f32 values != shape [1, {n_tokens}]",
                    raw.len()
                ));
            }
            let vocab = emb.len() / hidden;
            let mut ids: Vec<u32> = Vec::with_capacity(n_tokens);
            for (i, &v) in raw.iter().enumerate() {
                if !(v.is_finite() && v >= 0.0 && v.fract() == 0.0) {
                    return refuse(format!("activation[{i}] = {v} is not a token id"));
                }
                let id = v as u32;
                if id as usize >= vocab {
                    return refuse(format!("token id {id} at/above vocab size {vocab}"));
                }
                ids.push(id);
            }
            let mut x = vec![0.0f32; n_tokens * hidden];
            embed_ids(&ids, emb, hidden, &mut x);
            x
        } else {
            let vals: Vec<f32> = tb
                .raw_data
                .as_chunks::<4>()
                .0
                .iter()
                .map(|c| f32::from_le_bytes(*c))
                .collect();
            if vals.len() != n_tokens * hidden {
                return refuse(format!(
                    "hidden payload {} f32 values != shape [{} x {hidden}]",
                    vals.len(),
                    n_tokens
                ));
            }
            vals
        };

        // ---- run the owned range --------------------------------------------
        // Blocks are taken out of the tensor map one at a time and restored,
        // so concurrent sessions share one copy of the weights.
        let t0 = std::time::Instant::now();
        {
            let session = st
                .sessions
                .entry(req.session_id.clone())
                .or_insert_with(|| Session {
                    kv: (0..layers_owned)
                        .map(|_| KvLayer::new(kv_dim, hprops.context_length as usize))
                        .collect(),
                    kv_len: 0,
                });
            for idx in 0..layers_owned {
                let layer = loaded.start + idx as u32;
                let w = BlockWeights::take(&mut loaded.tensors, layer);
                run_block(
                    &w,
                    &hprops,
                    &mut session.kv[idx],
                    &mut x,
                    n_tokens,
                    positions.clone(),
                );
                put_block_back(&mut loaded.tensors, layer, w);
            }
            session.kv_len = kv_len + n_tokens;
        }

        // ---- head stage (last block of the model) ---------------------------
        let is_head = loaded.end == hprops.block_count - 1;
        // Prefill only needs the last row's next-token distribution. Asking for
        // it explicitly keeps the head matmul and the wire payload to one row
        // (spec 2026-10-21-last-row-logits, L2); every layer above still ran
        // for every token, so the KV cache is unaffected.
        let first_row = if is_head && req.last_logits_only {
            // Saturating: the shape guard above already refuses a zero-token
            // request (`[1, 0]` ids), so n_tokens is at least 1 here — but the
            // head stage should not depend on a validation further up the
            // function to avoid an underflow (review finding on #83).
            n_tokens.saturating_sub(1)
        } else {
            0
        };
        let (out, out_len, out_rows) = if is_head {
            {
                let Some(norm) = loaded.tensors.get("output_norm.weight") else {
                    return refuse("head stage requires output_norm.weight".to_string());
                };
                for t in first_row..n_tokens {
                    rms_norm(&mut x[t * hidden..(t + 1) * hidden], norm, hprops.rms_eps);
                }
                let head_part = if loaded.tensors.contains_key("output.weight") {
                    "output.weight"
                } else if loaded.tensors.contains_key("token_embd.weight") {
                    "token_embd.weight" // tied model: the embedding is the head
                } else {
                    return refuse(
                        "head stage loaded, but no output embedding/head found".to_string(),
                    );
                };
                let n_vocab = loaded.tensors[head_part].len() / hidden;
                let rows = n_tokens - first_row;
                let mut logits = vec![0.0f32; rows * n_vocab];
                for (row, logits_row) in logits.chunks_exact_mut(n_vocab).enumerate() {
                    let t = first_row + row;
                    lm_head(
                        &x[t * hidden..(t + 1) * hidden],
                        &loaded.tensors[head_part],
                        hidden,
                        logits_row,
                    );
                }
                (logits, n_vocab, rows)
            }
        } else {
            (x, hidden, n_tokens)
        };

        let us = t0.elapsed().as_micros().max(1) as i64;

        Ok(Response::new(ExecuteForwardResponse {
            success: true,
            output_activation_tensor: Some(TensorBuffer {
                shape: vec![out_rows as i64, out_len as i64],
                data_type: DataType::Fp32 as i32,
                raw_data: out.iter().flat_map(|v| v.to_le_bytes()).collect(),
            }),
            execution_time_us: us,
        }))
    }

    async fn get_health_status(
        &self,
        _request: Request<HealthRequest>,
    ) -> Result<Response<HealthResponse>, Status> {
        let (_, free) = Self::host_memory();
        Ok(Response::new(HealthResponse {
            // A desktop has no battery and no thermal sensor to read. Report
            // the "I cannot measure this" sentinel (-1), the same one the
            // Android worker uses, instead of a 0.0 the master would render
            // as "0% charged, 0 °C" (spec 2026-10-22, D-E3).
            battery_percentage: -1.0,
            cpu_temp_celsius: -1.0,
            free_ram_bytes: free as i64,
        }))
    }

    /// Measures this host's memory bandwidth and single-threaded f32 FMA
    /// rate with the bounded probes in [`crate::probe`] (spec 2026-10-21,
    /// D-C2).
    ///
    /// The buffer is allocated *before* the timer starts: `probe_duration_ms`
    /// is meant to let a master sanity-check a degenerate measurement (spec
    /// D-C2), so it must cover the probe loops only — not the 32 MiB
    /// allocation. The individual measurements are unaffected either way,
    /// since each probe times its own loop internally.
    ///
    /// The probe is pure and borrows nothing from `self`, so it runs on the
    /// runtime's blocking pool: the ~half-second of CPU work never occupies an
    /// async worker thread. That keeps the worker answering health checks and
    /// inference while a benchmark is in flight, without the probe spawning
    /// competing threads of its own (spec D-C2).
    async fn benchmark(
        &self,
        _request: Request<BenchmarkRequest>,
    ) -> Result<Response<BenchmarkResponse>, Status> {
        let measured = tokio::task::spawn_blocking(|| {
            let mut buffer = vec![1.0f32; crate::probe::BANDWIDTH_BUFFER_ELEMENTS];
            let started = std::time::Instant::now();
            let memory_bandwidth_mbs =
                crate::probe::probe_bandwidth_mbs(&mut buffer, crate::probe::BANDWIDTH_PASSES);
            let throughput_flops =
                crate::probe::probe_throughput_flops(crate::probe::FLOPS_ITERATIONS);
            (
                memory_bandwidth_mbs,
                throughput_flops,
                started.elapsed().as_secs_f64() * 1000.0,
            )
        })
        .await
        .map_err(|e| Status::internal(format!("benchmark probe task failed: {e}")))?;

        Ok(Response::new(BenchmarkResponse {
            memory_bandwidth_mbs: measured.0,
            throughput_flops: measured.1,
            probe_duration_ms: measured.2,
        }))
    }
}
