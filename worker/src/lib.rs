//! Worker engine: real GGUF layer-range loading and Qwen2-family forward
//! math behind the `WorkerService` contract from `proto/cluster_rpc.proto`.

pub mod engine;
pub mod gguf;
pub mod proto;
