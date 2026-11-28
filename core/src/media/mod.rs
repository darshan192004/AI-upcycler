//! Content-addressed storage for image attachments.
//!
//! Bytes are stored once, under `root/hash[0..2]/hash[2..]`, where the hash is
//! the lowercase hex SHA-256 of the original bytes. Only the four image types
//! allowed by the multimodal contract are accepted, and each file is capped
//! at 10 MiB. No image bytes are logged or embedded in errors.

use base64::engine::general_purpose::STANDARD;
use base64::Engine;
use sha2::{Digest, Sha256};
use std::fmt::{self, Write as _};
use std::path::PathBuf;
use tokio::io::AsyncWriteExt;
use uuid::Uuid;

const MAX_BYTES: usize = 10 * 1024 * 1024;
const ALLOWED_MIMES: [&str; 4] = ["image/png", "image/jpeg", "image/webp", "image/gif"];

/// Stores image bytes by content hash under a caller-supplied root.
pub struct MediaStore {
    root: PathBuf,
}

/// Metadata the API and store layers persist alongside a message part.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MediaRef {
    pub media_hash: String,
    pub mime_type: String,
    pub file_name: String,
    pub byte_count: u64,
}

/// Fail-closed media validation outcomes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MediaError {
    InvalidMime,
    InvalidMagic,
    TooLarge,
    BadHash,
    Io,
}

impl fmt::Display for MediaError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let message = match self {
            MediaError::InvalidMime => "invalid_mime",
            MediaError::InvalidMagic => "invalid_magic",
            MediaError::TooLarge => "too_large",
            MediaError::BadHash => "bad_hash",
            MediaError::Io => "io_error",
        };
        f.write_str(message)
    }
}

impl std::error::Error for MediaError {}

impl MediaStore {
    /// Creates a store rooted at `root`; the directory is created on first write.
    pub fn new(root: PathBuf) -> Self {
        Self { root }
    }

    /// Validates and writes `bytes`, returning the content-addressed reference.
    pub async fn put(
        &self,
        bytes: &[u8],
        mime_type: &str,
        file_name: &str,
    ) -> Result<MediaRef, MediaError> {
        if !ALLOWED_MIMES.contains(&mime_type) {
            return Err(MediaError::InvalidMime);
        }
        if bytes.len() > MAX_BYTES {
            return Err(MediaError::TooLarge);
        }
        if !magic_matches(bytes, mime_type) {
            return Err(MediaError::InvalidMagic);
        }

        let media_hash = hash_hex(bytes);
        let path = self.path_for(&media_hash)?;
        if let Some(parent) = path.parent() {
            tokio::fs::create_dir_all(parent)
                .await
                .map_err(|_| MediaError::Io)?;
        }

        // An existing object is immutable: verify it rather than truncating it.
        match tokio::fs::read(&path).await {
            Ok(existing) if existing == bytes => {}
            Ok(_) => return Err(MediaError::BadHash),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(_) => return Err(MediaError::Io),
        }

        // Write into a unique temp file in the same directory, then publish
        // with a hard link that fails (instead of overwriting) if another
        // writer won the race. The link is atomic on the local filesystem.
        let name = path
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_else(|| "object".to_string());
        let temp = path.with_file_name(format!("{name}.tmp-{}", Uuid::new_v4()));
        let published = async {
            let mut file = tokio::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&temp)
                .await
                .map_err(|_| MediaError::Io)?;
            file.write_all(bytes).await.map_err(|_| MediaError::Io)?;
            file.flush().await.map_err(|_| MediaError::Io)?;
            drop(file);

            match std::fs::hard_link(&temp, &path) {
                Ok(()) => Ok(()),
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                    let existing = tokio::fs::read(&path).await.map_err(|_| MediaError::Io)?;
                    if existing == bytes {
                        Ok(())
                    } else {
                        Err(MediaError::BadHash)
                    }
                }
                Err(_) => Err(MediaError::Io),
            }
        }
        .await;
        let _ = tokio::fs::remove_file(&temp).await;
        published?;

        Ok(MediaRef {
            media_hash,
            mime_type: mime_type.to_owned(),
            file_name: file_name.to_owned(),
            byte_count: bytes.len() as u64,
        })
    }

    /// Reads bytes back, rejecting malformed hashes and content drift.
    pub async fn get(&self, media_hash: &str) -> Result<Vec<u8>, MediaError> {
        let path = self.path_for(media_hash)?;
        let bytes = tokio::fs::read(path).await.map_err(|_| MediaError::Io)?;
        if hash_hex(&bytes) != media_hash {
            return Err(MediaError::BadHash);
        }
        Ok(bytes)
    }

    /// Builds a standard-base64 `data:` URL for stored bytes.
    pub async fn data_url(&self, media_hash: &str, mime_type: &str) -> Result<String, MediaError> {
        let bytes = self.get(media_hash).await?;
        Ok(format!(
            "data:{};base64,{}",
            mime_type,
            STANDARD.encode(bytes)
        ))
    }

    fn path_for(&self, media_hash: &str) -> Result<PathBuf, MediaError> {
        if !valid_hash(media_hash) {
            return Err(MediaError::BadHash);
        }
        Ok(self.root.join(&media_hash[..2]).join(&media_hash[2..]))
    }
}

fn hash_hex(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    let mut hex = String::with_capacity(64);
    for byte in digest.iter() {
        let _ = write!(&mut hex, "{:02x}", byte);
    }
    hex
}

fn valid_hash(media_hash: &str) -> bool {
    media_hash.len() == 64
        && media_hash
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

fn magic_matches(bytes: &[u8], mime_type: &str) -> bool {
    match mime_type {
        "image/png" => bytes.starts_with(&[0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A]),
        "image/jpeg" => bytes.starts_with(&[0xFF, 0xD8, 0xFF]),
        "image/webp" => bytes.len() >= 12 && bytes[..4] == *b"RIFF" && bytes[8..12] == *b"WEBP",
        "image/gif" => bytes.starts_with(b"GIF87a") || bytes.starts_with(b"GIF89a"),
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn temp_root(tag: &str) -> PathBuf {
        let p = PathBuf::from(r"D:\ai-upcycler-temp")
            .join("ai-upcycler-media")
            .join(tag)
            .join(std::process::id().to_string());
        let _ = std::fs::remove_dir_all(&p);
        p
    }

    #[tokio::test]
    async fn put_is_idempotent_and_path_is_hash_derived() {
        let root = temp_root("idempotent");
        let store = MediaStore::new(root.clone());
        let bytes = b"\x89PNG\r\n\x1a\nrest";
        let a = store.put(bytes, "image/png", "a.png").await.unwrap();
        let b = store.put(bytes, "image/png", "copy.png").await.unwrap();
        assert_eq!(a.media_hash, b.media_hash);
        let expected = root.join(&a.media_hash[..2]).join(&a.media_hash[2..]);
        assert!(expected.exists());
        assert_eq!(store.get(&a.media_hash).await.unwrap(), bytes);
    }

    #[tokio::test]
    async fn rejects_wrong_magic_and_oversize() {
        let store = MediaStore::new(temp_root("reject"));
        assert!(matches!(
            store.put(b"not a png", "image/png", "x.png").await,
            Err(MediaError::InvalidMagic)
        ));
        let big = vec![0u8; 10 * 1024 * 1024 + 1];
        assert!(matches!(
            store.put(&big, "image/png", "big.png").await,
            Err(MediaError::TooLarge)
        ));
    }

    fn platform_temp_root(tag: &str) -> PathBuf {
        PathBuf::from(r"D:\ai-upcycler-temp").join(format!(
            "ai-upcycler-media-{}-{}-{}",
            tag,
            std::process::id(),
            Uuid::new_v4()
        ))
    }

    #[tokio::test]
    async fn duplicate_put_never_overwrites_an_existing_object() {
        let store = MediaStore::new(platform_temp_root("no-overwrite"));
        let bytes = b"\x89PNG\r\n\x1a\nfixture";
        let first = store.put(bytes, "image/png", "a.png").await.unwrap();
        let second = store.put(bytes, "image/png", "copy.png").await.unwrap();
        assert_eq!(first.media_hash, second.media_hash);

        let path = store.path_for(&first.media_hash).expect("path");
        assert_eq!(
            tokio::fs::read(&path).await.expect("existing bytes"),
            bytes.as_slice()
        );

        std::fs::write(&path, b"sentinel").expect("corrupt object");
        assert!(matches!(
            store.put(bytes, "image/png", "copy.png").await,
            Err(MediaError::BadHash)
        ));
        assert_eq!(
            tokio::fs::read(&path).await.expect("preserved bytes"),
            b"sentinel"
        );
    }

    #[tokio::test]
    async fn data_url_contains_base64_payload() {
        let store = MediaStore::new(temp_root("dataurl"));
        let bytes = b"\x89PNG\r\n\x1a\nabc";
        let r = store.put(bytes, "image/png", "a.png").await.unwrap();
        let url = store.data_url(&r.media_hash, "image/png").await.unwrap();
        assert!(url.starts_with("data:image/png;base64,"));
        assert_eq!(
            url,
            format!(
                "data:image/png;base64,{}",
                base64::engine::general_purpose::STANDARD.encode(bytes)
            )
        );
    }
}
