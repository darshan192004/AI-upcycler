//! Worker engine: real GGUF layer-range loading and Qwen2-family forward
//! math behind the `WorkerService` contract from `proto/cluster_rpc.proto`.

pub mod engine;
pub mod layers;
pub mod math;
pub mod probe;
pub mod proto;
pub mod quant;

/// The GGUF reader is the shared `model-io` crate's; this re-export keeps
/// every engine/example/test path (`crate::gguf::…`,
/// `ai_upcycler_worker::gguf::…`) valid while one source of truth parses
/// the format for both sides of the RPC.
pub use ai_upcycler_model_io::gguf;
