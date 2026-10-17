//! Compiles `proto/cluster_rpc.proto` into Rust using tonic-build.
//!
//! Mirrors `core/build.rs`: `protoc-bin-vendored` supplies a prebuilt
//! `protoc` so the build works on hosts without a protobuf compiler.
fn main() -> Result<(), Box<dyn std::error::Error>> {
    if std::env::var_os("PROTOC").is_none() {
        std::env::set_var("PROTOC", protoc_bin_vendored::protoc_bin_path()?);
    }
    tonic_build::compile_protos("../proto/cluster_rpc.proto")?;
    Ok(())
}
