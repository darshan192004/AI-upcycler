//! Compiles `proto/cluster_rpc.proto` into Rust using tonic-build.
//!
//! Source: https://crates.io/crates/tonic-build
fn main() -> Result<(), Box<dyn std::error::Error>> {
    tonic_build::compile_protos("../proto/cluster_rpc.proto")?;
    Ok(())
}
