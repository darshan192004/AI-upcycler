//! SSD `mmap` zero-copy weight pager.
//!
//! Maps a weight file (GGUF) straight from storage with the OS memory mapper
//! so layer reads are zero-copy views onto demand-paged file contents instead
//! of multi-gigabyte heap allocations. The engine swap behind the
//! [`crate::api::backend::GenerationBackend`] trait will consume
//! [`MmapPager::bytes`] when real inference lands; until then the pager is a
//! standalone, tested primitive and the topology endpoint honestly reports
//! `storage_mmap.enabled: false` (nothing maps files on the serving path yet).

use std::ops::Range;
use std::path::{Path, PathBuf};

/// Errors produced by [`MmapPager`].
#[derive(Debug, thiserror::Error)]
pub enum PagerError {
    #[error("io error for {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("range {requested:?} outside mapped file of {len} bytes")]
    OutOfRange { requested: Range<u64>, len: u64 },
    #[error("range {requested:?} cannot be addressed on this platform")]
    TooLarge { requested: Range<u64> },
}

/// Read-only, memory-mapped view of a weight file on local storage.
///
/// The `unsafe` mapping is constructed once in [`MmapPager::open`]; every
/// other method is safe and only hands out bounds-checked slices of the map.
#[derive(Debug)]
pub struct MmapPager {
    mmap: Option<memmap2::Mmap>,
    len: u64,
    path: PathBuf,
}

impl MmapPager {
    /// Maps `path` read-only. Empty files map to an empty pager rather than
    /// failing, so callers can treat "no weights yet" uniformly.
    pub fn open(path: impl AsRef<Path>) -> Result<Self, PagerError> {
        let path = path.as_ref();
        let file = std::fs::File::open(path).map_err(|source| PagerError::Io {
            path: path.to_path_buf(),
            source,
        })?;
        let len = file
            .metadata()
            .map_err(|source| PagerError::Io {
                path: path.to_path_buf(),
                source,
            })?
            .len();
        // SAFETY: the file is opened read-only and the mapping is never
        // mutated; dropping the pager unmaps before any caller can hold a
        // reference (slices borrow `self`).
        let mmap = if len == 0 {
            None
        } else {
            Some(
                unsafe { memmap2::Mmap::map(&file) }.map_err(|source| PagerError::Io {
                    path: path.to_path_buf(),
                    source,
                })?,
            )
        };
        Ok(Self {
            mmap,
            len,
            path: path.to_path_buf(),
        })
    }

    /// Size of the mapped file in bytes.
    pub fn len(&self) -> u64 {
        self.len
    }

    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// File this pager was opened from.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Zero-copy view of `range` bytes of the mapped weights.
    pub fn bytes(&self, range: Range<u64>) -> Result<&[u8], PagerError> {
        if range.start > range.end || range.end > self.len {
            return Err(PagerError::OutOfRange {
                requested: range,
                len: self.len,
            });
        }
        let start = usize::try_from(range.start).map_err(|_| PagerError::TooLarge {
            requested: range.clone(),
        })?;
        let end =
            usize::try_from(range.end).map_err(|_| PagerError::TooLarge { requested: range })?;
        match &self.mmap {
            Some(mmap) => Ok(&mmap[start..end]),
            None => Ok(&[]),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_file(name: &str, bytes: &[u8]) -> PathBuf {
        let path =
            std::env::temp_dir().join(format!("ai-upcycler-mmap-{}-{name}", uuid::Uuid::new_v4()));
        std::fs::write(&path, bytes).expect("write temp file");
        path
    }

    #[test]
    fn maps_file_and_reads_back_bytes() {
        let path = temp_file("basic.gguf", b"GGUF-weights-payload");
        let pager = MmapPager::open(&path).expect("map");

        assert_eq!(pager.len(), 20);
        assert!(!pager.is_empty());
        assert_eq!(pager.path(), path.as_path());
        assert_eq!(pager.bytes(0..4).unwrap(), b"GGUF");
        assert_eq!(pager.bytes(5..12).unwrap(), b"weights");
        assert_eq!(pager.bytes(0..20).unwrap(), b"GGUF-weights-payload");

        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn maps_multi_mebibyte_files() {
        let payload: Vec<u8> = (0..3 * 1024 * 1024u32).map(|i| (i % 251) as u8).collect();
        let path = temp_file("large.gguf", &payload);
        let pager = MmapPager::open(&path).expect("map");

        assert_eq!(pager.len(), payload.len() as u64);
        let tail = pager.bytes(3 * 1024 * 1024 - 16..3 * 1024 * 1024).unwrap();
        assert_eq!(tail, &payload[3 * 1024 * 1024 - 16..]);

        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn rejects_out_of_range_and_inverted_ranges() {
        let path = temp_file("bounds.gguf", b"0123456789");
        let pager = MmapPager::open(&path).expect("map");

        assert!(matches!(
            pager.bytes(0..11),
            Err(PagerError::OutOfRange { .. })
        ));
        let (start, end) = (8, 3);
        assert!(matches!(
            pager.bytes(start..end),
            Err(PagerError::OutOfRange { .. })
        ));
        assert!(pager.bytes(10..10).unwrap().is_empty());

        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn empty_files_map_to_empty_pagers() {
        let path = temp_file("empty.gguf", b"");
        let pager = MmapPager::open(&path).expect("map empty");

        assert!(pager.is_empty());
        assert_eq!(pager.len(), 0);
        assert!(pager.bytes(0..0).unwrap().is_empty());
        assert!(matches!(
            pager.bytes(0..1),
            Err(PagerError::OutOfRange { .. })
        ));

        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn missing_files_surface_io_errors() {
        let path = std::env::temp_dir().join(format!(
            "ai-upcycler-mmap-missing-{}.gguf",
            uuid::Uuid::new_v4()
        ));
        assert!(matches!(MmapPager::open(&path), Err(PagerError::Io { .. })));
    }
}
