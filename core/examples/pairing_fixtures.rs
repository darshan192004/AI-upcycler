//! Generates the committed pairing fixtures the Android worker's JVM tests
//! replay (spec `2026-10-20-android-mtls-pairing-design.md`, D-B3/S3).
//!
//! The PEMs are issued by the real [`NodeCa`] — the same code path the
//! enrollment endpoint uses — so the Kotlin tests exercise exactly the shapes
//! the master hands out, including single-role EKUs and declared SANs. The
//! golden `enroll_response.json` is serialized by the endpoint's own
//! [`EnrollResponse`] type, pinning the Kotlin parser to the contract's real
//! JSON encoding rather than a hand-copied shape.
//!
//! Unlike the engine fixtures these files are not byte-regenerable (rcgen
//! keys are random), so there is no per-build sync guard: a Kotlin test pins
//! their structure (chain trust, EKU roles, SANs, fingerprint algorithm) and
//! this generator is run only to replace them deliberately:
//!
//! ```sh
//! cargo run -p ai-upcycler-core --example pairing_fixtures -- \
//!   mobile-worker/android/app/src/test/resources/pairing
//! ```
//!
//! rcgen's default certificate validity (1975-01-01..4096-01-01) keeps the
//! fixtures valid for any realistic CI clock; the Kotlin structure test
//! asserts a far-future `notAfter` so that stays true.

use ai_upcycler_core::api::pairing::EnrollResponse;
use ai_upcycler_core::security::NodeCa;
use std::fs;
use std::path::{Path, PathBuf};

fn write(dir: &Path, name: &str, contents: &str) -> std::io::Result<()> {
    fs::write(dir.join(name), contents)
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let dir = PathBuf::from(
        std::env::args()
            .nth(1)
            .unwrap_or_else(|| "mobile-worker/android/app/src/test/resources/pairing".into()),
    );
    fs::create_dir_all(&dir)?;

    // Fixture CA + the worker pair exactly as enrollment issues them. SANs are
    // the loopback addresses the JVM tests dial the fixture server at.
    let ca = NodeCa::generate()?;
    let server = ca.issue_server_cert(&["localhost".to_string(), "127.0.0.1".to_string()])?;
    let client = ca.issue_client_cert("fixture worker")?;

    write(&dir, "ca.pem", &ca.ca_pem())?;
    write(&dir, "server-cert.pem", &server.cert_pem)?;
    write(&dir, "server-key.pem", &server.key_pem)?;
    write(&dir, "client-cert.pem", &client.cert_pem)?;
    write(&dir, "client-key.pem", &client.key_pem)?;

    // A second, unrelated CA and one of its client certificates: the negative
    // path proving client auth actually rejects foreign chains.
    let rogue = NodeCa::generate()?;
    let rogue_client = rogue.issue_client_cert("rogue worker")?;
    write(&dir, "rogue-ca.pem", &rogue.ca_pem())?;
    write(&dir, "rogue-cert.pem", &rogue_client.cert_pem)?;
    write(&dir, "rogue-key.pem", &rogue_client.key_pem)?;

    // Golden enroll response, serialized by the endpoint's response type.
    let response = EnrollResponse {
        ca_pem: ca.ca_pem(),
        client_cert_pem: client.cert_pem.clone(),
        client_key_pem: client.key_pem.clone(),
        server_cert_pem: server.cert_pem.clone(),
        server_key_pem: server.key_pem.clone(),
        fingerprint: ca.fingerprint().to_string(),
    };
    write(
        &dir,
        "enroll_response.json",
        &serde_json::to_string_pretty(&response)?,
    )?;

    println!("wrote pairing fixtures to {}", dir.display());
    Ok(())
}
