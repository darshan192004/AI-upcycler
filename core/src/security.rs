//! Master-side node identity and device pairing.
//!
//! A fresh ephemeral CA is generated in-memory at startup (the "ephemeral
//! self-signed certificate" model from `docs/security.md`). The CA signs the
//! master's gRPC server certificate and worker client certificates issued
//! during PIN pairing. The worker validates the server cert by the CA
//! fingerprint carried in the pairing QR payload; the master only trusts
//! clients holding a cert signed by this CA.

use crate::rpc::engine::{WorkerTlsConfig, WorkerTransport, WorkerTransportSource};
use crate::trust::TrustStore;
use rcgen::{
    string::Ia5String, BasicConstraints, CertificateParams, DistinguishedName, DnType,
    ExtendedKeyUsagePurpose, IsCa, Issuer, KeyPair, KeyUsagePurpose, SanType,
};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::net::IpAddr;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// Default pairing session lifetime (docs/security.md §3 QR/PIN flow).
pub const PAIRING_TTL: Duration = Duration::from_secs(300);

/// Errors from pairing session creation/redemption.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum PairingError {
    #[error("pairing PIN is incorrect")]
    InvalidPin,
    #[error("pairing session has expired")]
    Expired,
    #[error("pairing session has already been used")]
    AlreadyUsed,
    #[error("failed to generate credentials: {0}")]
    Credential(String),
}

/// Client certificate + private key issued to an enrolled worker.
#[derive(Clone)]
pub struct ClientIdentity {
    pub cert_pem: String,
    pub key_pem: String,
}

/// Server certificate + private key issued to an enrolled worker, so the master
/// can reach it over mTLS (the master is the TLS *client* on that link).
#[derive(Clone)]
pub struct ServerIdentity {
    pub cert_pem: String,
    pub key_pem: String,
}

/// Certificate authority plus the identities the master holds.
///
/// One CA signs all of them, so every certificate is pinned to a single role:
/// the master's two identities (gRPC server, outbound mTLS client) and each
/// worker's pair (server for master→worker, client for worker→master).
///
/// The CA outlives a restart when it is persisted, so a worker that paired once
/// keeps authenticating. The master's own two identities are still re-issued on
/// every start: they are cheap, and nothing pins them.
pub struct NodeCa {
    /// Signing half of the CA. Held as a plain [`Issuer`] rather than
    /// `CertifiedIssuer` so a CA loaded from disk — which has no in-memory
    /// certificate object — has the same shape as a freshly generated one.
    ca: Issuer<'static, KeyPair>,
    ca_cert_pem: String,
    /// Kept only so the CA can be persisted; the signing key itself lives
    /// inside `ca`.
    ca_key_pem: String,
    fingerprint: String,
    server_cert_pem: String,
    server_key_pem: String,
    client_cert_pem: String,
    client_key_pem: String,
}

impl NodeCa {
    /// Generates a fresh CA plus the master's server identity (`localhost` /
    /// `127.0.0.1`) and its client identity for outbound worker links.
    pub fn generate() -> Result<Self, PairingError> {
        let mut ca_params = CertificateParams::default();
        ca_params.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
        let ca_key = KeyPair::generate().map_err(cred_err)?;
        let ca_key_pem = ca_key.serialize_pem();
        let certificate = ca_params.self_signed(&ca_key).map_err(cred_err)?;
        let fingerprint = hex(&Sha256::digest(certificate.der()));
        let ca_cert_pem = certificate.pem();

        Self::assemble(
            Issuer::new(ca_params, ca_key),
            ca_cert_pem,
            ca_key_pem,
            fingerprint,
        )
    }

    /// Rebuilds a CA from persisted PEM material, so certificates issued before
    /// a restart keep authenticating after it.
    pub fn from_parts(ca_cert_pem: &str, ca_key_pem: &str) -> Result<Self, PairingError> {
        let key = KeyPair::from_pem(ca_key_pem).map_err(cred_err)?;
        let ca = Issuer::from_ca_cert_pem(ca_cert_pem, key).map_err(cred_err)?;
        let fingerprint = hex(&Sha256::digest(pem_der(ca_cert_pem)?));

        Self::assemble(
            ca,
            ca_cert_pem.to_string(),
            ca_key_pem.to_string(),
            fingerprint,
        )
    }

    fn assemble(
        ca: Issuer<'static, KeyPair>,
        ca_cert_pem: String,
        ca_key_pem: String,
        fingerprint: String,
    ) -> Result<Self, PairingError> {
        let server = sign_server_cert(&ca, &["localhost".to_string(), "127.0.0.1".to_string()])?;
        let client = sign_client_cert(&ca, "master")?;

        Ok(Self {
            ca,
            ca_cert_pem,
            ca_key_pem,
            fingerprint,
            server_cert_pem: server.cert_pem,
            server_key_pem: server.key_pem,
            client_cert_pem: client.cert_pem,
            client_key_pem: client.key_pem,
        })
    }

    /// Lowercase hex SHA-256 of the CA certificate DER (the QR fingerprint).
    pub fn fingerprint(&self) -> &str {
        &self.fingerprint
    }

    pub fn ca_pem(&self) -> String {
        self.ca_cert_pem.clone()
    }

    /// The CA's private key, for persisting it. Never sent to a worker.
    pub fn ca_key_pem(&self) -> &str {
        &self.ca_key_pem
    }

    pub fn server_cert_pem(&self) -> &str {
        &self.server_cert_pem
    }

    pub fn server_key_pem(&self) -> &str {
        &self.server_key_pem
    }

    /// The master's client identity, presented on every outbound worker link.
    pub fn client_cert_pem(&self) -> &str {
        &self.client_cert_pem
    }

    pub fn client_key_pem(&self) -> &str {
        &self.client_key_pem
    }

    /// Issues a worker **server** certificate valid for `addresses` — the
    /// addresses the master will dial that worker at, as the worker declared
    /// them while pairing.
    ///
    /// Rejects an empty list: a server certificate with no subject alternative
    /// name can never satisfy the hostname check, so it would be unusable.
    pub fn issue_server_cert(&self, addresses: &[String]) -> Result<ServerIdentity, PairingError> {
        if addresses.is_empty() {
            return Err(PairingError::Credential(
                "a worker server certificate needs at least one address".into(),
            ));
        }
        sign_server_cert(&self.ca, addresses)
    }

    /// Issues a worker client certificate signed by this CA (the worker→master
    /// direction).
    pub fn issue_client_cert(&self, device_name: &str) -> Result<ClientIdentity, PairingError> {
        sign_client_cert(&self.ca, device_name)
    }
}

/// Signs a TLS **server** certificate (`ServerAuth`, non-CA) for `addresses`.
///
/// Each address is a subject alternative name: an IP literal becomes an IP SAN,
/// anything else has to be a well-formed DNS name.
fn sign_server_cert(
    ca: &Issuer<'static, KeyPair>,
    addresses: &[String],
) -> Result<ServerIdentity, PairingError> {
    let mut params = CertificateParams::default();
    params.is_ca = IsCa::ExplicitNoCa;
    params.key_usages = vec![
        KeyUsagePurpose::DigitalSignature,
        KeyUsagePurpose::KeyEncipherment,
    ];
    params.extended_key_usages = vec![ExtendedKeyUsagePurpose::ServerAuth];
    params.subject_alt_names = addresses
        .iter()
        .map(|address| san_type(address))
        .collect::<Result<Vec<_>, _>>()?;
    let key = KeyPair::generate().map_err(cred_err)?;
    let cert = params.signed_by(&key, ca).map_err(cred_err)?;
    Ok(ServerIdentity {
        cert_pem: cert.pem(),
        key_pem: key.serialize_pem(),
    })
}

/// Signs a TLS **client** certificate (`ClientAuth`, non-CA) for `common_name`.
fn sign_client_cert(
    ca: &Issuer<'static, KeyPair>,
    common_name: &str,
) -> Result<ClientIdentity, PairingError> {
    let mut params = CertificateParams::default();
    params.is_ca = IsCa::ExplicitNoCa;
    params.key_usages = vec![KeyUsagePurpose::DigitalSignature];
    params.extended_key_usages = vec![ExtendedKeyUsagePurpose::ClientAuth];
    let mut dn = DistinguishedName::new();
    dn.push(DnType::CommonName, common_name);
    params.distinguished_name = dn;
    let key = KeyPair::generate().map_err(cred_err)?;
    let cert = params.signed_by(&key, ca).map_err(cred_err)?;
    Ok(ClientIdentity {
        cert_pem: cert.pem(),
        key_pem: key.serialize_pem(),
    })
}

fn san_type(address: &str) -> Result<SanType, PairingError> {
    if let Ok(ip) = address.parse::<IpAddr>() {
        return Ok(SanType::IpAddress(ip));
    }
    if !is_dns_name(address) {
        return Err(PairingError::Credential(format!(
            "invalid certificate address {address:?}: expected an IP address or a DNS name"
        )));
    }
    Ok(SanType::DnsName(
        Ia5String::try_from(address).map_err(cred_err)?,
    ))
}

fn is_dns_name(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 253
        && value.split('.').all(|label| {
            !label.is_empty()
                && label.len() <= 63
                && label.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')
                && !label.starts_with('-')
                && !label.ends_with('-')
        })
}

/// Whether `address` can serve as a subject alternative name: an IP literal or
/// a well-formed DNS name.
pub fn is_valid_certificate_address(address: &str) -> bool {
    san_type(address).is_ok()
}

/// Single-use 6-digit PIN sessions for QR pairing (docs/security.md §3).
pub struct PairingManager {
    sessions: Mutex<HashMap<String, Session>>,
    ttl: Duration,
}

#[derive(Clone, Copy)]
struct Session {
    expires_at: Instant,
    used: bool,
}

impl PairingManager {
    pub fn new(ttl: Duration) -> Self {
        Self {
            sessions: Mutex::new(HashMap::new()),
            ttl,
        }
    }

    /// Creates a fresh PIN session. Returns `(pin, qr_payload, expires_in_secs)`.
    pub fn create_session(&self, fingerprint: &str) -> (String, String, u64) {
        let mut sessions = self.sessions.lock().expect("pairing lock");
        let now = Instant::now();
        sessions.retain(|_, s| s.expires_at > now);

        let pin = loop {
            let candidate = six_digit_pin();
            if !sessions.contains_key(&candidate) {
                break candidate;
            }
        };
        sessions.insert(
            pin.clone(),
            Session {
                expires_at: now + self.ttl,
                used: false,
            },
        );
        let qr = format!("aiupcycler://pair?fp={fingerprint}&token={pin}");
        let expires_in_secs = self.ttl.as_secs();
        (pin, qr, expires_in_secs)
    }

    /// Consumes `pin`. Fails with [`PairingError::InvalidPin`] for unknown
    /// PINs, [`PairingError::Expired`] for stale sessions, and
    /// [`PairingError::AlreadyUsed`] for reuse.
    pub fn redeem(&self, pin: &str) -> Result<(), PairingError> {
        let mut sessions = self.sessions.lock().expect("pairing lock");
        match sessions.get_mut(pin) {
            None => Err(PairingError::InvalidPin),
            Some(session) if session.expires_at <= Instant::now() => {
                sessions.remove(pin);
                Err(PairingError::Expired)
            }
            Some(session) if session.used => Err(PairingError::AlreadyUsed),
            Some(session) => {
                session.used = true;
                Ok(())
            }
        }
    }

    /// Whether `pin` still names an unredeemed, unexpired session — the
    /// condition for keeping the pairing reverse tunnel that session backs
    /// alive (spec 2026-10-20, D-B8: tunnels are removed when the session
    /// completes or expires).
    pub fn session_usable(&self, pin: &str) -> bool {
        let sessions = self.sessions.lock().expect("pairing lock");
        sessions
            .get(pin)
            .is_some_and(|s| !s.used && s.expires_at > Instant::now())
    }
}

/// Workers that completed pairing, keyed by the `node_id` the master dials them
/// by (`DiscoveredNode::node_id` for mDNS, `adb:<serial>` for USB).
///
/// With a [`TrustStore`] attached, records are written through so a pairing
/// survives a restart — provided the CA did too, which is why
/// [`SecurityState::load_or_generate`] drops them whenever it has to mint a new
/// CA. Without a store the registry is in-memory, which is what tests use.
#[derive(Default)]
pub struct EnrollmentRegistry {
    workers: Mutex<HashMap<String, EnrolledWorker>>,
    store: Option<Arc<TrustStore>>,
}

/// A paired worker: what it declared about itself while enrolling.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EnrolledWorker {
    pub device_name: String,
    pub addresses: Vec<String>,
}

impl EnrollmentRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    /// A registry seeded from `store` that writes every enrollment back to it.
    pub fn with_store(store: Arc<TrustStore>) -> Self {
        let workers = store
            .load_enrollments()
            .into_iter()
            .map(|(node_id, device_name, addresses)| {
                (
                    node_id,
                    EnrolledWorker {
                        device_name,
                        addresses,
                    },
                )
            })
            .collect();
        Self {
            workers: Mutex::new(workers),
            store: Some(store),
        }
    }

    /// Records a completed enrollment, replacing any earlier record for the
    /// same node (re-pairing only refreshes the declared details).
    pub fn enroll(&self, node_id: &str, device_name: &str, addresses: Vec<String>) {
        let snapshot = {
            let mut workers = self.workers.lock().expect("enrollment lock");
            workers.insert(
                node_id.to_string(),
                EnrolledWorker {
                    device_name: device_name.to_string(),
                    addresses,
                },
            );
            // Persist outside the lock: the file write is small but it is still
            // I/O, and readers of the registry must not wait on the disk.
            self.store.as_ref().map(|_| workers.clone())
        };

        let (Some(store), Some(workers)) = (self.store.as_ref(), snapshot) else {
            return;
        };
        let entries: Vec<_> = workers
            .into_iter()
            .map(|(node_id, worker)| (node_id, worker.device_name, worker.addresses))
            .collect();
        if let Err(error) = store.save_enrollments(&entries) {
            tracing::warn!("could not persist the enrollment ({error})");
        }
    }

    pub fn get(&self, node_id: &str) -> Option<EnrolledWorker> {
        self.workers
            .lock()
            .expect("enrollment lock")
            .get(node_id)
            .cloned()
    }

    pub fn is_enrolled(&self, node_id: &str) -> bool {
        self.workers
            .lock()
            .expect("enrollment lock")
            .contains_key(node_id)
    }

    pub fn len(&self) -> usize {
        self.workers.lock().expect("enrollment lock").len()
    }

    pub fn is_empty(&self) -> bool {
        self.workers.lock().expect("enrollment lock").is_empty()
    }
}

/// The production [`WorkerTransportSource`]: a worker is dialed over mTLS only
/// once it has completed pairing against this master's CA.
///
/// A worker with no enrollment is an `Err`, never a plaintext dial, so an
/// unpaired or renamed node cannot be reached by downgrading the transport.
pub struct EnrolledWorkerSource {
    security: Arc<SecurityState>,
}

impl EnrolledWorkerSource {
    pub fn new(security: Arc<SecurityState>) -> Self {
        Self { security }
    }
}

impl WorkerTransportSource for EnrolledWorkerSource {
    fn transport_for(&self, node_id: &str) -> Result<WorkerTransport, String> {
        if !self.security.enrollments.is_enrolled(node_id) {
            return Err(format!(
                "worker {node_id} has no completed pairing enrollment"
            ));
        }
        Ok(WorkerTransport::MutualTls(WorkerTlsConfig {
            ca_pem: self.security.ca.ca_pem(),
            client_cert_pem: self.security.ca.client_cert_pem().to_string(),
            client_key_pem: self.security.ca.client_key_pem().to_string(),
        }))
    }
}

/// Resolves a node's dial the way that node actually serves (spec 2026-10-21,
/// D-C3): mTLS once it has completed pairing against this master's CA,
/// plaintext until then.
///
/// This is deliberately **not** [`EnrolledWorkerSource`], which refuses any
/// unenrolled node. That refusal is right for the serving path — serving must
/// never downgrade an unknown worker to plaintext — but wrong for the
/// measurement paths: an unpaired mDNS worker, and the local Rust worker
/// behind an ADB forward, genuinely serve plaintext, so a probe that refused
/// them could never measure the very nodes the endpoint exists to compare. The
/// exposure class is unchanged: discovery handshakes and ADB tunnel probes
/// already speak plaintext to unpaired workers (spec D-C3).
///
/// Shared by the benchmark probe and the health poller so the rule cannot
/// drift between them (spec 2026-10-22, D-E4).
pub struct EnrollmentAwareTransports {
    security: Arc<SecurityState>,
}

impl EnrollmentAwareTransports {
    pub fn new(security: Arc<SecurityState>) -> Self {
        Self { security }
    }
}

impl WorkerTransportSource for EnrollmentAwareTransports {
    fn transport_for(&self, node_id: &str) -> Result<WorkerTransport, String> {
        if !self.security.enrollments.is_enrolled(node_id) {
            return Ok(WorkerTransport::Plaintext);
        }
        Ok(WorkerTransport::MutualTls(WorkerTlsConfig {
            ca_pem: self.security.ca.ca_pem(),
            client_cert_pem: self.security.ca.client_cert_pem().to_string(),
            client_key_pem: self.security.ca.client_key_pem().to_string(),
        }))
    }
}

/// CA, pairing sessions and enrollment records shared through
/// [`crate::api::AppState`].
pub struct SecurityState {
    pub ca: NodeCa,
    pub pairing: PairingManager,
    pub enrollments: EnrollmentRegistry,
}

impl Default for SecurityState {
    /// An in-memory CA that dies with the process. Used by tests and by any
    /// caller that has no data directory to persist to.
    fn default() -> Self {
        Self {
            ca: NodeCa::generate().expect("ephemeral CA generation"),
            pairing: PairingManager::new(PAIRING_TTL),
            enrollments: EnrollmentRegistry::new(),
        }
    }
}

impl SecurityState {
    /// Loads the master's CA from `trust_dir`, generating and persisting one
    /// when the directory holds nothing usable.
    ///
    /// An unreadable store is replaced rather than fatal: the master has to
    /// start, and the only cost is re-pairing, which is what happened on every
    /// start before the CA was persisted.
    pub fn load_or_generate(trust_dir: impl Into<PathBuf>) -> Self {
        let store = Arc::new(TrustStore::new(trust_dir));
        let ca = match store.load_ca() {
            Some((cert, key)) => match NodeCa::from_parts(&cert, &key) {
                Ok(ca) => ca,
                Err(error) => {
                    tracing::warn!(
                        "persisted CA in {} is unusable ({error}); generating a new one",
                        store.dir().display()
                    );
                    store.clear_enrollments();
                    generate_and_persist(&store)
                }
            },
            None => {
                // No usable CA: any stored enrollment was signed by one that no
                // longer exists, so keeping it would authorize nothing.
                store.clear_enrollments();
                generate_and_persist(&store)
            }
        };
        tracing::info!(
            "master CA {} ({})",
            &ca.fingerprint()[..8],
            store.dir().display()
        );
        Self {
            ca,
            pairing: PairingManager::new(PAIRING_TTL),
            enrollments: EnrollmentRegistry::with_store(store),
        }
    }
}

fn generate_and_persist(store: &TrustStore) -> NodeCa {
    let ca = NodeCa::generate().expect("CA generation");
    match store.save_ca(&ca.ca_pem(), ca.ca_key_pem()) {
        Ok(()) => tracing::info!("persisted the master CA in {}", store.dir().display()),
        Err(error) => tracing::warn!(
            "could not persist the master CA ({error}); workers will have to pair again after a restart"
        ),
    }
    ca
}

fn cred_err(e: impl std::fmt::Display) -> PairingError {
    PairingError::Credential(e.to_string())
}

fn six_digit_pin() -> String {
    let bytes = *uuid::Uuid::new_v4().as_bytes();
    let n = u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]);
    format!("{:06}", n % 1_000_000)
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// DER bytes of a PEM certificate, so a CA loaded from disk can be
/// fingerprinted exactly like a generated one.
fn pem_der(pem: &str) -> Result<Vec<u8>, PairingError> {
    use base64::Engine as _;
    let body: String = pem
        .lines()
        .filter(|line| !line.starts_with("-----"))
        .collect();
    base64::engine::general_purpose::STANDARD
        .decode(body)
        .map_err(cred_err)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn enrollment_aware_transports_pair_with_mtls_and_leave_the_rest_plaintext() {
        let security = Arc::new(SecurityState::default());
        security.enrollments.enroll("paired", "phone", vec![]);
        let source = EnrollmentAwareTransports::new(security.clone());

        match source.transport_for("paired").expect("enrolled") {
            WorkerTransport::MutualTls(config) => {
                assert_eq!(config.ca_pem, security.ca.ca_pem());
                assert_eq!(config.client_cert_pem, security.ca.client_cert_pem());
            }
            WorkerTransport::Plaintext => panic!("an enrolled node must be dialed over mTLS"),
        }

        // Unpaired is plaintext, not an error: that is how the node serves.
        assert!(matches!(
            source
                .transport_for("unpaired")
                .expect("unpaired never errors"),
            WorkerTransport::Plaintext
        ));
    }

    #[test]
    fn generates_ca_with_stable_fingerprint() {
        let ca = NodeCa::generate().expect("ca");
        assert_eq!(ca.fingerprint().len(), 64);
        assert_eq!(ca.fingerprint(), ca.fingerprint());
        assert!(ca.ca_pem().contains("BEGIN CERTIFICATE"));
        assert!(ca.server_cert_pem().contains("BEGIN CERTIFICATE"));
        assert!(ca.server_key_pem().contains("PRIVATE KEY"));
    }

    #[test]
    fn fingerprint_differs_per_instance() {
        let a = NodeCa::generate().expect("a");
        let b = NodeCa::generate().expect("b");
        assert_ne!(a.fingerprint(), b.fingerprint());
    }

    #[test]
    fn issued_client_identity_contains_cert_and_key() {
        let ca = NodeCa::generate().expect("ca");
        let id = ca.issue_client_cert("Pixel 8").expect("issue");
        assert!(id.cert_pem.contains("BEGIN CERTIFICATE"));
        assert!(id.key_pem.contains("PRIVATE KEY"));
    }

    #[test]
    fn pin_is_six_digits_and_qr_carries_token() {
        let ca = NodeCa::generate().expect("ca");
        let pairing = PairingManager::new(PAIRING_TTL);
        let (pin, qr, ttl) = pairing.create_session(ca.fingerprint());
        assert_eq!(pin.len(), 6);
        assert!(pin.chars().all(|c| c.is_ascii_digit()));
        assert!(qr.contains(&format!("token={pin}")));
        assert!(qr.contains(&format!("fp={}", ca.fingerprint())));
        assert!(qr.starts_with("aiupcycler://pair?"));
        assert_eq!(ttl, PAIRING_TTL.as_secs());
    }

    #[test]
    fn redeem_consumes_pin_once() {
        let pairing = PairingManager::new(PAIRING_TTL);
        let (pin, _, _) = pairing.create_session("fp");
        assert_eq!(pairing.redeem(&pin), Ok(()));
        assert_eq!(pairing.redeem(&pin), Err(PairingError::AlreadyUsed));
    }

    #[test]
    fn wrong_pin_is_invalid() {
        let pairing = PairingManager::new(PAIRING_TTL);
        assert_eq!(pairing.redeem("000000"), Err(PairingError::InvalidPin));
    }

    #[test]
    fn expired_session_is_rejected() {
        let pairing = PairingManager::new(Duration::from_millis(1));
        let (pin, _, _) = pairing.create_session("fp");
        std::thread::sleep(Duration::from_millis(5));
        assert_eq!(pairing.redeem(&pin), Err(PairingError::Expired));
    }

    #[test]
    fn session_is_usable_until_it_is_redeemed() {
        let pairing = PairingManager::new(PAIRING_TTL);
        let (pin, _, _) = pairing.create_session("fp");
        assert!(pairing.session_usable(&pin));
        pairing.redeem(&pin).expect("redeem");
        assert!(!pairing.session_usable(&pin));
    }

    #[test]
    fn expired_and_unknown_sessions_are_not_usable() {
        // Zero TTL: the session is already past `expires_at` on the next
        // clock read, so this pins expiry without a sleep.
        let expired = PairingManager::new(Duration::ZERO);
        let (pin, _, _) = expired.create_session("fp");
        assert!(!expired.session_usable(&pin));

        let pairing = PairingManager::new(PAIRING_TTL);
        assert!(!pairing.session_usable("000000"));
    }

    #[test]
    fn generates_master_client_identity_for_outbound_links() {
        let ca = NodeCa::generate().expect("ca");
        assert!(ca.client_cert_pem().contains("BEGIN CERTIFICATE"));
        assert!(ca.client_key_pem().contains("PRIVATE KEY"));
        // Role separation: the master's server identity is not its client one.
        assert_ne!(ca.client_cert_pem(), ca.server_cert_pem());
    }

    #[test]
    fn issues_worker_server_certificate_for_declared_addresses() {
        let ca = NodeCa::generate().expect("ca");
        let server = ca
            .issue_server_cert(&["127.0.0.1".to_string(), "pixel.example".to_string()])
            .expect("issue");
        assert!(server.cert_pem.contains("BEGIN CERTIFICATE"));
        assert!(server.key_pem.contains("PRIVATE KEY"));
    }

    #[test]
    fn rejects_a_server_certificate_with_no_addresses() {
        let ca = NodeCa::generate().expect("ca");
        assert!(ca.issue_server_cert(&[]).is_err());
    }

    #[test]
    fn rejects_an_address_that_is_neither_an_ip_nor_a_dns_name() {
        let ca = NodeCa::generate().expect("ca");
        for bad in ["", "a/b", "a..b", "-lead.example", "trail-.example"] {
            assert!(
                ca.issue_server_cert(&[bad.to_string()]).is_err(),
                "{bad:?} should be rejected as a subject alternative name"
            );
        }
    }

    #[test]
    fn enrollment_registry_records_what_the_worker_declared() {
        let registry = EnrollmentRegistry::new();
        assert!(registry.is_empty());
        assert!(!registry.is_enrolled("phone-a"));

        registry.enroll("phone-a", "Pixel 8", vec!["10.0.0.5".into()]);
        assert_eq!(registry.len(), 1);
        let worker = registry.get("phone-a").expect("enrolled");
        assert_eq!(worker.device_name, "Pixel 8");
        assert_eq!(worker.addresses, vec!["10.0.0.5".to_string()]);

        // Re-pairing refreshes the record instead of duplicating it.
        registry.enroll("phone-a", "Pixel 8", vec!["10.0.0.6".into()]);
        assert_eq!(registry.len(), 1);
        assert_eq!(
            registry.get("phone-a").expect("enrolled").addresses,
            vec!["10.0.0.6".to_string()]
        );
    }

    fn temp_trust_dir() -> PathBuf {
        let dir = std::env::temp_dir().join(format!("ai-upcycler-ca-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).expect("temp dir");
        dir
    }

    #[test]
    fn a_persisted_ca_reloads_with_the_same_fingerprint() {
        let dir = temp_trust_dir();
        let first = SecurityState::load_or_generate(&dir);
        let second = SecurityState::load_or_generate(&dir);

        assert_eq!(
            first.ca.fingerprint(),
            second.ca.fingerprint(),
            "a restart must not rotate the CA, or every pairing is lost"
        );
        assert_eq!(first.ca.ca_pem(), second.ca.ca_pem());

        // The reloaded CA still signs: a worker that paired before the restart
        // keeps a certificate this master accepts.
        let identity = second.ca.issue_client_cert("phone").expect("issue");
        assert!(identity.cert_pem.contains("BEGIN CERTIFICATE"));

        // The master's own identities are re-issued per start, which is fine —
        // nothing pins them.
        assert!(second.ca.server_cert_pem().contains("BEGIN CERTIFICATE"));
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn a_corrupt_ca_is_replaced_instead_of_blocking_startup() {
        let dir = temp_trust_dir();
        let original = SecurityState::load_or_generate(&dir);
        std::fs::write(dir.join("ca-cert.pem"), "not a certificate").expect("corrupt the CA");

        let recovered = SecurityState::load_or_generate(&dir);
        assert_ne!(
            original.ca.fingerprint(),
            recovered.ca.fingerprint(),
            "an unusable CA has to be replaced"
        );

        // The replacement is persisted, not regenerated on every start.
        let again = SecurityState::load_or_generate(&dir);
        assert_eq!(recovered.ca.fingerprint(), again.ca.fingerprint());
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn the_fingerprint_of_a_loaded_ca_matches_its_certificate_bytes() {
        let dir = temp_trust_dir();
        let generated = SecurityState::load_or_generate(&dir);
        let expected = hex(&Sha256::digest(
            pem_der(&generated.ca.ca_pem()).expect("decode the CA"),
        ));
        assert_eq!(generated.ca.fingerprint(), expected);

        let reloaded = SecurityState::load_or_generate(&dir);
        assert_eq!(reloaded.ca.fingerprint(), expected);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn enrollments_survive_a_restart() {
        let dir = temp_trust_dir();
        let first = SecurityState::load_or_generate(&dir);
        first
            .enrollments
            .enroll("adb:16284c49", "Redmi", vec!["127.0.0.1".into()]);
        first
            .enrollments
            .enroll("Redmi.local.", "Redmi", vec!["192.168.1.13".into()]);

        let restarted = SecurityState::load_or_generate(&dir);
        assert_eq!(
            restarted.ca.fingerprint(),
            first.ca.fingerprint(),
            "the CA has to be the same one for the records to mean anything"
        );
        assert_eq!(restarted.enrollments.len(), 2);
        let worker = restarted
            .enrollments
            .get("adb:16284c49")
            .expect("still enrolled");
        assert_eq!(worker.device_name, "Redmi");
        assert_eq!(worker.addresses, vec!["127.0.0.1".to_string()]);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn regenerating_the_ca_drops_enrollments_it_never_signed() {
        let dir = temp_trust_dir();
        let first = SecurityState::load_or_generate(&dir);
        first
            .enrollments
            .enroll("adb:16284c49", "Redmi", vec!["127.0.0.1".into()]);
        std::fs::write(dir.join("ca-cert.pem"), "not a certificate").expect("corrupt the CA");

        let recovered = SecurityState::load_or_generate(&dir);
        assert_ne!(first.ca.fingerprint(), recovered.ca.fingerprint());
        assert!(
            recovered.enrollments.is_empty(),
            "a new CA must not inherit workers enrolled against the old one"
        );
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn an_unreadable_enrollment_store_is_ignored() {
        let dir = temp_trust_dir();
        SecurityState::load_or_generate(&dir);
        std::fs::write(dir.join("enrollments.json"), "{ not json").expect("corrupt the store");

        let reloaded = SecurityState::load_or_generate(&dir);
        assert!(
            reloaded.enrollments.is_empty(),
            "a garbage enrollment file must not stop the master starting"
        );
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn enrolled_source_dials_only_enrolled_workers_and_never_plaintext() {
        let security = Arc::new(SecurityState::default());
        let source = EnrolledWorkerSource::new(security.clone());

        assert!(
            source.transport_for("phone-a").is_err(),
            "an unpaired worker must not resolve to a transport"
        );

        security
            .enrollments
            .enroll("phone-a", "Pixel 8", vec!["127.0.0.1".into()]);
        match source.transport_for("phone-a").expect("enrolled") {
            WorkerTransport::MutualTls(config) => {
                assert_eq!(config.ca_pem, security.ca.ca_pem());
                assert_eq!(config.client_cert_pem, security.ca.client_cert_pem());
            }
            WorkerTransport::Plaintext => {
                panic!("an enrolled worker must never be dialed over plaintext")
            }
        }
    }
}
