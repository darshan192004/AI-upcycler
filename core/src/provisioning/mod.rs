//! A GGUF on the host, addressed for the push protocol
//! (spec `2026-11-03-model-provisioning`, D-P2/D-P4).

use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};
use tokio::io::{AsyncReadExt, AsyncSeekExt, SeekFrom};

/// Bytes in one push message. Small enough that a dropped Wi-Fi link costs a
/// second to resend, large enough that a 1.9 GB model is not 1.9 million
/// round trips (spec D-P2).
pub const CHUNK_BYTES: usize = 1024 * 1024;

/// Ceiling on a single push message on the wire, independent of the much
/// larger activation limit this client also advertises.
pub const MAX_PUSH_MESSAGE_BYTES: usize = 8 * 1024 * 1024;

/// A GGUF the host holds and is willing to push.
///
/// Built from a model row's `local_file_path`, never from a caller-supplied
/// path (spec D-P10): [`open`](ModelSource::open) validates it once and every
/// later read goes back to that path.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModelSource {
    path: PathBuf,
    file_name: String,
    size_bytes: u64,
}

impl ModelSource {
    pub fn open(path: impl Into<PathBuf>) -> Result<Self, String> {
        let path = path.into();
        let meta = std::fs::metadata(&path)
            .map_err(|error| format!("cannot read {}: {error}", path.display()))?;
        if !meta.is_file() {
            return Err(format!("{} is not a file", path.display()));
        }
        if meta.len() == 0 {
            return Err(format!("{} is empty", path.display()));
        }
        let file_name = path
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or_default()
            .to_string();
        if !file_name.to_ascii_lowercase().ends_with(".gguf") {
            return Err(format!("{} is not a .gguf file", path.display()));
        }
        Ok(Self {
            path,
            file_name,
            size_bytes: meta.len(),
        })
    }

    /// The bare file name the worker validates and stores under.
    pub fn file_name(&self) -> &str {
        &self.file_name
    }

    pub fn size_bytes(&self) -> u64 {
        self.size_bytes
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Lowercase hex SHA-256 of the whole file (spec D-P3).
    ///
    /// Async and streaming: a 1.9 GB hash on the async runtime would stall
    /// the API, and reading it into memory would not fit in one.
    pub async fn sha256_hex(&self) -> Result<String, String> {
        let mut file = tokio::fs::File::open(&self.path)
            .await
            .map_err(|error| format!("cannot read {}: {error}", self.path.display()))?;
        let mut hasher = Sha256::new();
        let mut buffer = vec![0u8; CHUNK_BYTES];
        loop {
            let read = file
                .read(&mut buffer)
                .await
                .map_err(|error| format!("cannot hash {}: {error}", self.path.display()))?;
            if read == 0 {
                break;
            }
            hasher.update(&buffer[..read]);
        }
        Ok(format!("{:x}", hasher.finalize()))
    }

    /// Reads exactly `len` bytes starting at `offset`.
    ///
    /// `chunk_len` is what guarantees the request never runs past the end;
    /// an early EOF here is a file that shrank underneath us.
    pub async fn read_chunk(&self, offset: u64, len: usize) -> Result<Vec<u8>, String> {
        let mut file = tokio::fs::File::open(&self.path)
            .await
            .map_err(|error| format!("cannot read {}: {error}", self.path.display()))?;
        file.seek(SeekFrom::Start(offset))
            .await
            .map_err(|error| format!("cannot seek in {}: {error}", self.path.display()))?;
        let mut buffer = vec![0u8; len];
        file.read_exact(&mut buffer)
            .await
            .map_err(|error| format!("cannot read {len} bytes at {offset}: {error}"))?;
        Ok(buffer)
    }
}

/// Bytes to send for the chunk starting at `offset`.
///
/// Zero once `offset` reaches `total`, so the caller's stream terminates
/// without special-casing a short final chunk.
pub fn chunk_len(total: u64, offset: u64, chunk: usize) -> usize {
    if offset >= total || chunk == 0 {
        return 0;
    }
    let remaining = total - offset;
    remaining.min(chunk as u64) as usize
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir() -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("ai-upcycler-provisioning-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).expect("temp dir");
        dir
    }

    fn write(dir: &Path, name: &str, contents: &[u8]) -> PathBuf {
        let path = dir.join(name);
        std::fs::write(&path, contents).expect("write");
        path
    }

    #[test]
    fn chunk_len_covers_the_tail() {
        assert_eq!(4, chunk_len(5, 0, 4));
        assert_eq!(1, chunk_len(5, 4, 4));
        assert_eq!(4, chunk_len(8, 0, 4));
    }

    #[test]
    fn chunk_len_is_zero_at_and_past_the_end() {
        assert_eq!(0, chunk_len(5, 5, 4));
        assert_eq!(0, chunk_len(5, 9, 4));
    }

    #[test]
    fn a_non_gguf_file_is_refused() {
        let dir = temp_dir();
        let path = write(&dir, "model.bin", b"not a model");
        assert!(ModelSource::open(&path).unwrap_err().contains(".gguf"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_missing_file_is_refused() {
        let dir = temp_dir();
        let path = dir.join("absent.gguf");
        assert!(ModelSource::open(&path)
            .unwrap_err()
            .contains("cannot read"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn an_empty_file_is_refused() {
        let dir = temp_dir();
        let path = write(&dir, "empty.gguf", b"");
        assert!(ModelSource::open(&path).unwrap_err().contains("empty"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_gguf_opens_with_its_name_and_size() {
        let dir = temp_dir();
        let path = write(&dir, "m.gguf", b"hello");
        let source = ModelSource::open(&path).expect("open");

        assert_eq!("m.gguf", source.file_name());
        assert_eq!(5, source.size_bytes());
        assert_eq!(path, source.path());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn sha256_matches_a_known_digest() {
        let dir = temp_dir();
        let path = write(&dir, "abc.gguf", b"abc");
        let source = ModelSource::open(&path).expect("open");

        assert_eq!(
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad",
            source.sha256_hex().await.expect("digest"),
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn read_chunk_returns_the_requested_bytes() {
        let dir = temp_dir();
        let path = write(&dir, "m.gguf", b"0123456789");
        let source = ModelSource::open(&path).expect("open");

        assert_eq!(
            b"4567",
            source.read_chunk(4, 4).await.expect("chunk").as_slice()
        );
        let tail = source.read_chunk(8, 2).await.expect("tail");
        assert_eq!(b"89", tail.as_slice());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn read_chunk_past_the_end_fails() {
        let dir = temp_dir();
        let path = write(&dir, "m.gguf", b"0123456789");
        let source = ModelSource::open(&path).expect("open");

        assert!(source.read_chunk(8, 4).await.is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
