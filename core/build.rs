//! Compiles `proto/cluster_rpc.proto` into Rust using tonic-build.
//!
//! `protoc-bin-vendored` supplies a prebuilt `protoc` so the build works on
//! hosts that do not have the protobuf compiler installed (e.g. CI runners).
//!
//! Source: https://crates.io/crates/tonic-build
fn main() -> Result<(), Box<dyn std::error::Error>> {
    if std::env::var_os("PROTOC").is_none() {
        std::env::set_var("PROTOC", protoc_bin_vendored::protoc_bin_path()?);
    }
    tonic_build::compile_protos("../proto/cluster_rpc.proto")?;
    Ok(())
}
