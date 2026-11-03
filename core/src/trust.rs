//! On-disk trust material for the master's certificate authority.
//!
//! The CA used to be regenerated on every start, so a worker's certificates
//! were signed by a CA that no longer existed after a restart and it had to
//! pair again. Persisting the CA is what makes a pairing outlive the process.
//!
//! Layout of the trust directory:
//!
//! - `ca-cert.pem`  - the CA certificate, public material.
//! - `ca-key.dpapi` - the CA signing key, DPAPI-wrapped (Windows).
//! - `ca-key.pem`   - the CA signing key, mode 0600 (other platforms).
//!
//! The key is never written in the clear on Windows: whoever can read it can
//! mint a client certificate this master will accept as an enrolled worker.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

const CA_CERT_FILE: &str = "ca-cert.pem";
const ENROLLMENTS_FILE: &str = "enrollments.json";

#[cfg(windows)]
const CA_KEY_FILE: &str = "ca-key.dpapi";
#[cfg(not(windows))]
const CA_KEY_FILE: &str = "ca-key.pem";

#[derive(Debug, thiserror::Error)]
pub enum TrustError {
    #[error("trust store I/O failed for {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    #[error("could not {action} the CA key: {detail}")]
    KeyProtection {
        action: &'static str,
        detail: String,
    },
    #[error("could not encode the enrollment store: {0}")]
    Encode(String),
}

/// One enrolled worker as it is written to disk.
#[derive(Serialize, Deserialize)]
struct StoredEnrollment {
    device_name: String,
    addresses: Vec<String>,
}

/// The CA's two halves, persisted under one directory.
#[derive(Clone, Debug)]
pub struct TrustStore {
    dir: PathBuf,
}

impl TrustStore {
    pub fn new(dir: impl Into<PathBuf>) -> Self {
        Self { dir: dir.into() }
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// The stored CA, or `None` when this is a first run or either half is
    /// unreadable. `None` is not an error: the caller generates a CA.
    pub fn load_ca(&self) -> Option<(String, String)> {
        let cert = fs::read_to_string(self.dir.join(CA_CERT_FILE)).ok()?;
        let key = self.load_key().ok()?;
        Some((cert, key))
    }

    /// Persists both halves of the CA.
    ///
    /// The key goes first: a store that was interrupted half-way must never
    /// look like a usable CA whose key is missing.
    pub fn save_ca(&self, cert_pem: &str, key_pem: &str) -> Result<(), TrustError> {
        fs::create_dir_all(&self.dir).map_err(|e| self.io_error(&self.dir, e))?;
        self.save_key(key_pem)?;
        let path = self.dir.join(CA_CERT_FILE);
        write_atomic(&path, cert_pem.as_bytes()).map_err(|e| self.io_error(&path, e))
    }

    /// Enrolled workers, as `(node_id, device_name, addresses)`.
    ///
    /// An unreadable file yields nothing rather than an error: the master must
    /// start, and the cost is that workers have to pair again.
    pub fn load_enrollments(&self) -> Vec<(String, String, Vec<String>)> {
        let path = self.dir.join(ENROLLMENTS_FILE);
        let Ok(raw) = fs::read_to_string(&path) else {
            return Vec::new();
        };
        match serde_json::from_str::<HashMap<String, StoredEnrollment>>(&raw) {
            Ok(stored) => stored
                .into_iter()
                .map(|(node_id, worker)| (node_id, worker.device_name, worker.addresses))
                .collect(),
            Err(error) => {
                tracing::warn!(
                    "ignoring unreadable enrollment store {}: {error}",
                    path.display()
                );
                Vec::new()
            }
        }
    }

    pub fn save_enrollments(
        &self,
        entries: &[(String, String, Vec<String>)],
    ) -> Result<(), TrustError> {
        fs::create_dir_all(&self.dir).map_err(|e| self.io_error(&self.dir, e))?;
        let stored: HashMap<&str, StoredEnrollment> = entries
            .iter()
            .map(|(node_id, device_name, addresses)| {
                (
                    node_id.as_str(),
                    StoredEnrollment {
                        device_name: device_name.clone(),
                        addresses: addresses.clone(),
                    },
                )
            })
            .collect();
        let json =
            serde_json::to_string_pretty(&stored).map_err(|e| TrustError::Encode(e.to_string()))?;
        let path = self.dir.join(ENROLLMENTS_FILE);
        write_atomic(&path, json.as_bytes()).map_err(|e| self.io_error(&path, e))
    }

    /// Drops every enrollment. Called when the CA had to be regenerated: those
    /// records describe workers whose certificates the new CA never signed.
    pub fn clear_enrollments(&self) {
        let _ = fs::remove_file(self.dir.join(ENROLLMENTS_FILE));
    }

    #[cfg(windows)]
    fn load_key(&self) -> Result<String, TrustError> {
        let path = self.dir.join(CA_KEY_FILE);
        let blob = fs::read(&path).map_err(|e| self.io_error(&path, e))?;
        let plain = unprotect(&blob)?;
        String::from_utf8(plain).map_err(|e| TrustError::KeyProtection {
            action: "decrypt",
            detail: e.to_string(),
        })
    }

    #[cfg(not(windows))]
    fn load_key(&self) -> Result<String, TrustError> {
        let path = self.dir.join(CA_KEY_FILE);
        fs::read_to_string(&path).map_err(|e| self.io_error(&path, e))
    }

    #[cfg(windows)]
    fn save_key(&self, key_pem: &str) -> Result<(), TrustError> {
        let blob = protect(key_pem.as_bytes())?;
        let path = self.dir.join(CA_KEY_FILE);
        write_atomic(&path, &blob).map_err(|e| self.io_error(&path, e))
    }

    #[cfg(not(windows))]
    fn save_key(&self, key_pem: &str) -> Result<(), TrustError> {
        let path = self.dir.join(CA_KEY_FILE);
        write_atomic(&path, key_pem.as_bytes()).map_err(|e| self.io_error(&path, e))?;
        restrict_to_owner(&path).map_err(|e| self.io_error(&path, e))
    }

    fn io_error(&self, path: &Path, source: io::Error) -> TrustError {
        TrustError::Io {
            path: path.to_path_buf(),
            source,
        }
    }
}

/// Writes through a sibling temp file so a crash cannot leave a truncated CA
/// behind: either the old file or the new one is in place, never half of each.
fn write_atomic(path: &Path, bytes: &[u8]) -> io::Result<()> {
    let temp = path.with_extension("tmp");
    fs::write(&temp, bytes)?;
    fs::rename(&temp, path)
}

#[cfg(unix)]
fn restrict_to_owner(path: &Path) -> io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(path, fs::Permissions::from_mode(0o600))
}

// Windows never reaches this: the key is DPAPI-wrapped, not permission-gated.
#[cfg(all(not(unix), not(windows)))]
fn restrict_to_owner(_path: &Path) -> io::Result<()> {
    Ok(())
}

/// Wraps `input` with the current user's DPAPI master key.
#[cfg(windows)]
fn protect(input: &[u8]) -> Result<Vec<u8>, TrustError> {
    crypt(true, input)
}

/// Unwraps a blob produced by [`protect`].
#[cfg(windows)]
fn unprotect(input: &[u8]) -> Result<Vec<u8>, TrustError> {
    crypt(false, input)
}

/// `CryptProtectData`/`CryptUnprotectData` share a signature, so one call site
/// covers both directions and cannot drift in how it frees the output.
#[cfg(windows)]
fn crypt(protect: bool, input: &[u8]) -> Result<Vec<u8>, TrustError> {
    use windows_sys::Win32::Foundation::LocalFree;
    use windows_sys::Win32::Security::Cryptography::{
        CryptProtectData, CryptUnprotectData, CRYPTPROTECT_UI_FORBIDDEN, CRYPT_INTEGER_BLOB,
    };

    let action = if protect { "encrypt" } else { "decrypt" };
    let input_blob = CRYPT_INTEGER_BLOB {
        cbData: input.len() as u32,
        pbData: input.as_ptr() as *mut u8,
    };
    let mut output = CRYPT_INTEGER_BLOB::default();

    // `CRYPTPROTECT_UI_FORBIDDEN`: the master runs headless, so DPAPI must fail
    // rather than put up a prompt nobody can answer.
    let ok = unsafe {
        if protect {
            CryptProtectData(
                &input_blob,
                std::ptr::null(),
                std::ptr::null(),
                std::ptr::null(),
                std::ptr::null(),
                CRYPTPROTECT_UI_FORBIDDEN,
                &mut output,
            )
        } else {
            CryptUnprotectData(
                &input_blob,
                std::ptr::null_mut(),
                std::ptr::null(),
                std::ptr::null(),
                std::ptr::null(),
                CRYPTPROTECT_UI_FORBIDDEN,
                &mut output,
            )
        }
    };
    if ok == 0 {
        return Err(TrustError::KeyProtection {
            action,
            detail: io::Error::last_os_error().to_string(),
        });
    }

    let bytes =
        unsafe { std::slice::from_raw_parts(output.pbData, output.cbData as usize).to_vec() };
    unsafe {
        LocalFree(output.pbData as *mut core::ffi::c_void);
    }
    Ok(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir() -> PathBuf {
        let dir = std::env::temp_dir().join(format!("ai-upcycler-trust-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&dir).expect("temp dir");
        dir
    }

    #[test]
    fn a_saved_ca_round_trips_through_the_store() {
        let dir = temp_dir();
        let store = TrustStore::new(&dir);
        assert!(store.load_ca().is_none(), "an empty store holds no CA");

        store.save_ca("CERT PEM", "KEY PEM").expect("save the CA");

        assert_eq!(
            store.load_ca(),
            Some(("CERT PEM".to_string(), "KEY PEM".to_string()))
        );
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn a_half_written_store_is_not_mistaken_for_a_ca() {
        let dir = temp_dir();
        let store = TrustStore::new(&dir);
        store.save_ca("CERT PEM", "KEY PEM").expect("save");
        fs::remove_file(dir.join(CA_KEY_FILE)).expect("drop the key");

        assert!(
            store.load_ca().is_none(),
            "a certificate without its key cannot sign anything"
        );
        fs::remove_dir_all(&dir).ok();
    }

    #[cfg(windows)]
    #[test]
    fn the_ca_key_is_never_stored_in_plaintext_on_windows() {
        let dir = temp_dir();
        TrustStore::new(&dir)
            .save_ca("CERT PEM", "-----BEGIN PRIVATE KEY-----\nsecret\n")
            .expect("save");

        let raw = fs::read(dir.join(CA_KEY_FILE)).expect("key file");
        assert!(
            !raw.windows(11).any(|w| w == b"PRIVATE KEY"),
            "the CA key must be DPAPI-wrapped, not written in the clear"
        );
        assert!(
            !raw.windows(6).any(|w| w == b"secret"),
            "no part of the key material may survive unencrypted"
        );
        fs::remove_dir_all(&dir).ok();
    }

    #[cfg(not(windows))]
    #[test]
    fn the_ca_key_is_readable_only_by_its_owner_elsewhere() {
        use std::os::unix::fs::PermissionsExt;
        let dir = temp_dir();
        TrustStore::new(&dir)
            .save_ca("CERT PEM", "KEY PEM")
            .expect("save");

        let mode = fs::metadata(dir.join(CA_KEY_FILE))
            .expect("key file")
            .permissions()
            .mode();
        assert_eq!(mode & 0o077, 0, "group and other must have no access");
        fs::remove_dir_all(&dir).ok();
    }
}
