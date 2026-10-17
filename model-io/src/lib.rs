//! Model file I/O shared between the master and the worker engine: the
//! GGUF-v3 reader ([`gguf`]) and the deterministic GGUF fixture writer
//! ([`fixture`]) standing under both the reader's and the engine's tests.
//!
//! Dependency-minimal by design (cloned decision Q1 of the master-side
//! tokenizer spec): file bytes and memory maps only — no runtime, no RPC,
//! and no dependency on the `core` crate.

pub mod fixture;
pub mod gguf;
