//! Master-side node identity and device pairing.
//!
//! A fresh ephemeral CA is generated in-memory at startup (the "ephemeral
//! self-signed certificate" model from `docs/security.md`). The CA signs the
//! master's gRPC server certificate and worker client certificates issued
//! during PIN pairing. The worker validates the server cert by the CA
//! fingerprint carried in the pairing QR payload; the master only trusts
//! clients holding a cert signed by this CA.

use rcgen::{
    BasicConstraints, Certificate, CertificateParams, DistinguishedName, DnType,
    ExtendedKeyUsagePurpose, Ia5String, IsCa, KeyPair, KeyUsagePurpose, SanType,
};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::net::{IpAddr, Ipv4Addr};
use std::sync::Mutex;
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

/// Ephemeral certificate authority plus the master's gRPC server identity.
pub struct NodeCa {
    ca_cert: Certificate,
    ca_key: KeyPair,
    fingerprint: String,
    server_cert_pem: String,
    server_key_pem: String,
}

impl NodeCa {
    /// Generates a fresh CA and a `localhost`/`127.0.0.1` server certificate.
    pub fn generate() -> Result<Self, PairingError> {
        let mut ca_params = CertificateParams::default();
        ca_params.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
        let ca_key = KeyPair::generate().map_err(cred_err)?;
        let ca_cert = ca_params.self_signed(&ca_key).map_err(cred_err)?;
        let fingerprint = hex(&Sha256::digest(ca_cert.der()));

        let mut server_params = CertificateParams::default();
        // Server and worker certificates share one CA; pin the role so a
        // certificate issued for one direction cannot authenticate the other.
        server_params.is_ca = IsCa::ExplicitNoCa;
        server_params.key_usages = vec![
            KeyUsagePurpose::DigitalSignature,
            KeyUsagePurpose::KeyEncipherment,
        ];
        server_params.extended_key_usages = vec![ExtendedKeyUsagePurpose::ServerAuth];
        server_params.subject_alt_names = vec![
            SanType::DnsName(Ia5String::try_from("localhost").map_err(cred_err)?),
            SanType::IpAddress(IpAddr::V4(Ipv4Addr::LOCALHOST)),
        ];
        let server_key = KeyPair::generate().map_err(cred_err)?;
        let server_cert = server_params
            .signed_by(&server_key, &ca_cert, &ca_key)
            .map_err(cred_err)?;

        Ok(Self {
            ca_cert,
            ca_key,
            fingerprint,
            server_cert_pem: server_cert.pem(),
            server_key_pem: server_key.serialize_pem(),
        })
    }

    /// Lowercase hex SHA-256 of the CA certificate DER (the QR fingerprint).
    pub fn fingerprint(&self) -> &str {
        &self.fingerprint
    }

    pub fn ca_pem(&self) -> String {
        self.ca_cert.pem()
    }

    pub fn server_cert_pem(&self) -> &str {
        &self.server_cert_pem
    }

    pub fn server_key_pem(&self) -> &str {
        &self.server_key_pem
    }

    /// Issues a worker client certificate signed by this CA.
    pub fn issue_client_cert(&self, device_name: &str) -> Result<ClientIdentity, PairingError> {
        let mut params = CertificateParams::default();
        params.is_ca = IsCa::ExplicitNoCa;
        params.key_usages = vec![KeyUsagePurpose::DigitalSignature];
        params.extended_key_usages = vec![ExtendedKeyUsagePurpose::ClientAuth];
        let mut dn = DistinguishedName::new();
        dn.push(DnType::CommonName, device_name);
        params.distinguished_name = dn;
        let key = KeyPair::generate().map_err(cred_err)?;
        let cert = params
            .signed_by(&key, &self.ca_cert, &self.ca_key)
            .map_err(cred_err)?;
        Ok(ClientIdentity {
            cert_pem: cert.pem(),
            key_pem: key.serialize_pem(),
        })
    }
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

/// CA + pairing state shared through [`crate::api::AppState`].
pub struct SecurityState {
    pub ca: NodeCa,
    pub pairing: PairingManager,
}

impl Default for SecurityState {
    fn default() -> Self {
        Self {
            ca: NodeCa::generate().expect("ephemeral CA generation"),
            pairing: PairingManager::new(PAIRING_TTL),
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
}
