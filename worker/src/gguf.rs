//! Minimal GGUF-v3 reader: header, key/value metadata, tensor table and
//! bounded raw reads over a memory-mapped file.
//!
//! Layout: magic `GGUF`, version, tensor/KV counts, the KV list, the tensor
//! info table (name, dims, type, data-section offset), then a
//! `general.alignment`-aligned (default 32) data section. Tensor types this
//! reader cannot size (ids beyond the supported set) keep their id visible so
//! `LoadLayers` can refuse them loudly; `read_raw` refuses them here.

use memmap2::Mmap;
use std::collections::HashMap;
use std::path::Path;

#[derive(Debug, thiserror::Error)]
pub enum GgufError {
    #[error("not a GGUF file (bad magic)")]
    BadMagic,
    #[error("unsupported GGUF version {0} (only version 3)")]
    UnsupportedVersion(u32),
    #[error("malformed GGUF file: {0}")]
    Malformed(&'static str),
    #[error("general.architecture is not qwen2: {0}")]
    UnsupportedArchitecture(String),
    #[error("missing required key/value: {0}")]
    MissingKey(&'static str),
    #[error("key {0} has an unexpected GGUF value type")]
    WrongKeyType(String),
    #[error("tensor {0} extends past the end of the file")]
    TensorOutOfBounds(String),
    #[error("tensor {0} has an unsupported ggml type id {1}")]
    UnknownTensorType(String, u32),
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
}

/// Model hyperparameters the decoder needs. Vocab size deliberately comes
/// from tensor rows, not from KV metadata.
#[derive(Debug, Clone)]
pub struct ModelHparams {
    pub block_count: u32,
    pub embedding_length: u32,
    pub feed_forward_length: u32,
    pub head_count: u32,
    pub head_count_kv: u32,
    pub head_dim: u32,
    pub rms_eps: f32,
    pub rope_theta: f32,
    pub context_length: u32,
}

#[derive(Debug, Clone)]
pub struct TensorDesc {
    pub ggml_type: u32,
    /// GGML dims: innermost-first (`[rows, cols]` for a 2-D matrix).
    pub ne: Vec<u64>,
    /// Offset inside the data section.
    pub offset: u64,
    /// Encoded byte length; `None` when the ggml type id is unknown here.
    pub nbytes: Option<u64>,
}

#[derive(Debug)]
pub struct GgufFile {
    map: Mmap,
    data_offset: u64,
    model_name: String,
    hparams: ModelHparams,
    tensors: HashMap<String, TensorDesc>,
}

impl GgufFile {
    pub fn open(path: &Path) -> Result<Self, GgufError> {
        let file = std::fs::File::open(path)?;
        // SAFETY: the file is only read; models are never mutated concurrently.
        let map = unsafe { Mmap::map(&file)? };
        let mut r = Cursor { map: &map, pos: 0 };

        if r.take(4)? != b"GGUF" {
            return Err(GgufError::BadMagic);
        }
        let version = r.u32()?;
        if version != 3 {
            return Err(GgufError::UnsupportedVersion(version));
        }
        let n_tensors = r.u64()?;
        let n_kv = r.u64()?;

        let mut model_name: Option<String> = None;
        let mut architecture: Option<String> = None;
        let mut alignment: Option<u64> = None;
        let mut block_count: Option<u32> = None;
        let mut context_length: Option<u32> = None;
        let mut embedding_length: Option<u32> = None;
        let mut feed_forward_length: Option<u32> = None;
        let mut head_count: Option<u32> = None;
        let mut head_count_kv: Option<u32> = None;
        let mut key_length: Option<u32> = None;
        let mut rms_eps: Option<f32> = None;
        let mut rope_theta: Option<f32> = None;

        for _ in 0..n_kv {
            let key = r.string()?;
            // value_type is an int32/uint32-wide enum on disk (gguf.md spec
            // and llama.cpp's reference reader/writer both use 4 bytes).
            let ty = r.u32()? as u64;
            let value = r.skip_value(ty)?;
            match key.as_str() {
                "general.name" => model_name = Some(as_string(ty, &value, "general.name")?),
                "general.architecture" => {
                    architecture = Some(as_string(ty, &value, "general.architecture")?)
                }
                "general.alignment" => alignment = Some(as_u64(ty, &value, "general.alignment")?),
                "qwen2.block_count" => block_count = Some(as_u64(ty, &value, &key)? as u32),
                "qwen2.context_length" => context_length = Some(as_u64(ty, &value, &key)? as u32),
                "qwen2.embedding_length" => {
                    embedding_length = Some(as_u64(ty, &value, &key)? as u32)
                }
                "qwen2.feed_forward_length" => {
                    feed_forward_length = Some(as_u64(ty, &value, &key)? as u32)
                }
                "qwen2.attention.head_count" => head_count = Some(as_u64(ty, &value, &key)? as u32),
                "qwen2.attention.head_count_kv" => {
                    head_count_kv = Some(as_u64(ty, &value, &key)? as u32)
                }
                "qwen2.attention.key_length" => key_length = Some(as_u64(ty, &value, &key)? as u32),
                "qwen2.attention.layer_norm_rms_epsilon" => {
                    rms_eps = Some(as_f32(ty, &value, &key)?)
                }
                "qwen2.rope.freq_base" => rope_theta = Some(as_f32(ty, &value, &key)?),
                _ => {}
            }
        }

        let architecture = architecture.ok_or(GgufError::MissingKey("general.architecture"))?;
        if architecture != "qwen2" {
            return Err(GgufError::UnsupportedArchitecture(architecture));
        }
        let model_name = model_name.ok_or(GgufError::MissingKey("general.name"))?;
        let need = |key: &'static str, v: Option<u32>| v.ok_or(GgufError::MissingKey(key));
        let block_count = need("qwen2.block_count", block_count)?;
        let context_length = need("qwen2.context_length", context_length)?;
        let embedding_length = need("qwen2.embedding_length", embedding_length)?;
        let feed_forward_length = need("qwen2.feed_forward_length", feed_forward_length)?;
        let head_count = need("qwen2.attention.head_count", head_count)?;
        let head_count_kv = need("qwen2.attention.head_count_kv", head_count_kv)?;
        let rms_eps = rms_eps.ok_or(GgufError::MissingKey(
            "qwen2.attention.layer_norm_rms_epsilon",
        ))?;
        let head_dim = key_length.unwrap_or_else(|| embedding_length / head_count.max(1));
        let rope_theta = rope_theta.unwrap_or(10_000.0);

        let mut tensors = HashMap::with_capacity(n_tensors as usize);
        for _ in 0..n_tensors {
            let name = r.string()?;
            let n_dims = r.u32()?;
            let mut ne = Vec::with_capacity(n_dims as usize);
            for _ in 0..n_dims {
                ne.push(r.u64()?);
            }
            let ggml_type = r.u32()?;
            let offset = r.u64()?;
            if ne.is_empty() {
                return Err(GgufError::Malformed("tensor with zero dimensions"));
            }
            let nbytes = tensor_nbytes(ggml_type as u64, &ne);
            tensors.insert(
                name,
                TensorDesc {
                    ggml_type,
                    ne,
                    offset,
                    nbytes,
                },
            );
        }

        let alignment = alignment.unwrap_or(32).max(1);
        let data_offset = align_up(r.pos as u64, alignment);

        // Every sized tensor must fit in the file, so truncation fails at
        // open, not at first read.
        let file_len = map.len() as u64;
        for (name, desc) in &tensors {
            if let Some(nbytes) = desc.nbytes {
                let end = data_offset
                    .checked_add(desc.offset)
                    .and_then(|o| o.checked_add(nbytes));
                match end {
                    Some(end) if end <= file_len => {}
                    _ => return Err(GgufError::TensorOutOfBounds(name.clone())),
                }
            }
        }

        Ok(Self {
            map,
            data_offset,
            model_name,
            hparams: ModelHparams {
                block_count,
                embedding_length,
                feed_forward_length,
                head_count,
                head_count_kv,
                head_dim,
                rms_eps,
                rope_theta,
                context_length,
            },
            tensors,
        })
    }

    pub fn model_name(&self) -> &str {
        &self.model_name
    }

    pub fn hparams(&self) -> &ModelHparams {
        &self.hparams
    }

    pub fn tensor(&self, name: &str) -> Option<&TensorDesc> {
        self.tensors.get(name)
    }

    pub fn tensor_names(&self) -> impl Iterator<Item = &String> {
        self.tensors.keys()
    }

    /// The tensor's exact encoded bytes from the data section.
    pub fn read_raw(&self, name: &str) -> Result<Vec<u8>, GgufError> {
        let desc = self
            .tensors
            .get(name)
            .ok_or_else(|| GgufError::TensorOutOfBounds(name.to_string()))?;
        let nbytes = desc
            .nbytes
            .ok_or_else(|| GgufError::UnknownTensorType(name.to_string(), desc.ggml_type))?;
        let start = self
            .data_offset
            .checked_add(desc.offset)
            .ok_or_else(|| GgufError::TensorOutOfBounds(name.to_string()))?;
        let end = start
            .checked_add(nbytes)
            .filter(|end| *end <= self.map.len() as u64)
            .ok_or_else(|| GgufError::TensorOutOfBounds(name.to_string()))?;
        Ok(self.map[start as usize..end as usize].to_vec())
    }
}

// ---------------------------------------------------------------------------
// internals
// ---------------------------------------------------------------------------

/// Encoded byte length of a tensor, or `None` for ggml type ids this reader
/// does not know how to size (kept visible for loud `LoadLayers` refusal).
fn tensor_nbytes(ggml_type: u64, ne: &[u64]) -> Option<u64> {
    let elems = ne.iter().try_fold(1u64, |acc, d| acc.checked_mul(*d))?;
    match ggml_type {
        0 => elems.checked_mul(4),          // F32
        1 => elems.checked_mul(2),          // F16
        2 => block_bytes(elems, 32, 18),    // Q4_0
        6 => block_bytes(elems, 32, 22),    // Q5_0
        7 => block_bytes(elems, 32, 24),    // Q5_1 (refused at load; sized only)
        8 => block_bytes(elems, 32, 34),    // Q8_0
        12 => block_bytes(elems, 256, 144), // Q4_K
        14 => block_bytes(elems, 256, 210), // Q6_K
        _ => None,
    }
}

/// Blocks of `per_block` elements encode to `block_size` bytes each.
fn block_bytes(elems: u64, per_block: u64, block_size: u64) -> Option<u64> {
    if !elems.is_multiple_of(per_block) {
        return None; // quantized dims must align with block boundaries
    }
    elems.checked_div(per_block)?.checked_mul(block_size)
}

fn align_up(pos: u64, alignment: u64) -> u64 {
    pos.div_ceil(alignment) * alignment
}

struct Cursor<'a> {
    map: &'a Mmap,
    pos: usize,
}

impl Cursor<'_> {
    fn take(&mut self, n: usize) -> Result<&[u8], GgufError> {
        let end = self
            .pos
            .checked_add(n)
            .filter(|end| *end <= self.map.len())
            .ok_or(GgufError::Malformed("unexpected end of file"))?;
        let s = &self.map[self.pos..end];
        self.pos = end;
        Ok(s)
    }

    fn u32(&mut self) -> Result<u32, GgufError> {
        Ok(u32::from_le_bytes(self.take(4)?.try_into().unwrap()))
    }

    fn u64(&mut self) -> Result<u64, GgufError> {
        Ok(u64::from_le_bytes(self.take(8)?.try_into().unwrap()))
    }

    fn string(&mut self) -> Result<String, GgufError> {
        let len = self.u64()?;
        let len = usize::try_from(len).map_err(|_| GgufError::Malformed("string length"))?;
        let bytes = self.take(len)?;
        String::from_utf8(bytes.to_vec()).map_err(|_| GgufError::Malformed("non-utf-8 string"))
    }

    /// Skips a KV value of the given type, materializing scalars/strings so
    /// callers can inspect the ones they care about.
    fn skip_value(&mut self, ty: u64) -> Result<Value, GgufError> {
        Ok(match ty {
            0 | 7 => Value::U64(self.take(1)?[0] as u64), // u8, bool
            1 => Value::U64(self.take(1)?[0] as u64),     // i8
            2 | 3 => Value::U64(u16::from_le_bytes(self.take(2)?.try_into().unwrap()) as u64),
            4 | 5 => Value::U64(self.u32()? as u64), // u32, i32
            6 => Value::F32(f32::from_le_bytes(self.take(4)?.try_into().unwrap())),
            8 => Value::Str(self.string()?),
            9 => {
                // array: element type (u32), count (u64), then values
                let elem_ty = self.u32()? as u64;
                let count = self.u64()?;
                for _ in 0..count {
                    self.skip_value(elem_ty)?;
                }
                Value::U64(count)
            }
            10 | 11 => Value::U64(self.u64()?), // u64, i64
            12 => Value::F32(f64::from_bits(self.u64()?) as f32), // f64
            _ => return Err(GgufError::Malformed("unknown KV value type")),
        })
    }
}

enum Value {
    U64(u64),
    F32(f32),
    Str(String),
}

fn as_string(ty: u64, v: &Value, key: &str) -> Result<String, GgufError> {
    match v {
        Value::Str(s) if ty == 8 => Ok(s.clone()),
        _ => Err(GgufError::WrongKeyType(key.to_string())),
    }
}

fn as_u64(ty: u64, v: &Value, key: &str) -> Result<u64, GgufError> {
    match v {
        Value::U64(n) if ty != 8 => Ok(*n),
        _ => Err(GgufError::WrongKeyType(key.to_string())),
    }
}

fn as_f32(ty: u64, v: &Value, key: &str) -> Result<f32, GgufError> {
    match v {
        Value::F32(f) if ty != 8 => Ok(*f),
        _ => Err(GgufError::WrongKeyType(key.to_string())),
    }
}
