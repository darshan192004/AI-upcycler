//! Master-side node identity and device pairing.
//!
//! A fresh ephemeral CA is generated in-memory at startup (the "ephemeral
//! self-signed certificate" model from `docs/security.md`). The CA signs the
//! master's gRPC server certificate and worker client certificates issued
//! during PIN pairing. The worker validates the server cert by the CA
//! fingerprint carried in the pairing QR payload; the master only trusts
//! clients holding a cert signed by this CA.

use crate::rpc::engine::{WorkerTlsConfig, WorkerTransport, WorkerTransportSource};
use rcgen::{
    string::Ia5String, BasicConstraints, CertificateParams, CertifiedIssuer, DistinguishedName,
    DnType, ExtendedKeyUsagePurpose, IsCa, KeyPair, KeyUsagePurpose, SanType,
};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::net::IpAddr;
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

/// Ephemeral certificate authority plus the identities the master holds.
///
/// One CA signs all of them, so every certificate is pinned to a single role:
/// the master's two identities (gRPC server, outbound mTLS client) and each
/// worker's pair (server for master→worker, client for worker→master).
pub struct NodeCa {
    /// The CA certificate bundled with its signing key (`rcgen` 0.14's
    /// `CertifiedIssuer`); every issued cert is signed through it.
    ca: CertifiedIssuer<'static, KeyPair>,
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
        let ca = CertifiedIssuer::self_signed(ca_params, ca_key).map_err(cred_err)?;
        let fingerprint = hex(&Sha256::digest(ca.der()));

        let server = sign_server_cert(&ca, &["localhost".to_string(), "127.0.0.1".to_string()])?;
        let client = sign_client_cert(&ca, "master")?;

        Ok(Self {
            ca,
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
        self.ca.pem()
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
    ca: &CertifiedIssuer<'static, KeyPair>,
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
    ca: &CertifiedIssuer<'static, KeyPair>,
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
}

/// Workers that completed pairing, keyed by the `node_id` the master dials them
/// by (`DiscoveredNode::node_id` for mDNS, `adb:<serial>` for USB).
///
/// Deliberately in-memory. The CA is ephemeral and regenerated on every master
/// start, so certificates issued before a restart are already unusable and the
/// worker has to pair again; persisting enrollments would keep rows that can
/// never authenticate.
#[derive(Default)]
pub struct EnrollmentRegistry {
    workers: Mutex<HashMap<String, EnrolledWorker>>,
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

    /// Records a completed enrollment, replacing any earlier record for the
    /// same node (the CA is the same for the master's lifetime, so a re-pair
    /// only refreshes the declared details).
    pub fn enroll(&self, node_id: &str, device_name: &str, addresses: Vec<String>) {
        self.workers.lock().expect("enrollment lock").insert(
            node_id.to_string(),
            EnrolledWorker {
                device_name: device_name.to_string(),
                addresses,
            },
        );
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

/// CA, pairing sessions and enrollment records shared through
/// [`crate::api::AppState`].
pub struct SecurityState {
    pub ca: NodeCa,
    pub pairing: PairingManager,
    pub enrollments: EnrollmentRegistry,
}

impl Default for SecurityState {
    fn default() -> Self {
        Self {
            ca: NodeCa::generate().expect("ephemeral CA generation"),
            pairing: PairingManager::new(PAIRING_TTL),
            enrollments: EnrollmentRegistry::new(),
        }
    }
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

#[cfg(test)]
mod tests {
    use super::*;

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
