//! Weight dequantization for the ggml types the worker supports (plan Task 4).
//!
//! Sources, fetched 2026-10-17 from llama.cpp and cited inline:
//! - `ggml/src/ggml-common.h`: `block_q4_0` (f16 d + u8 qs[16] = 18 B),
//!   `block_q8_0` (f16 d + i8 qs[32] = 34 B), `block_q4_K` (f16 d,
//!   f16 dmin, u8 scales[12], u8 qs[128] = 144 B), `block_q6_K`
//!   (u8 ql[128], u8 qh[64], i8 scales[16], f16 d = 210 B).
//! - `ggml/src/ggml-quants.c`: `dequantize_row_q4_0` (line 459),
//!   `dequantize_row_q5_0` (500), `dequantize_row_q8_0` (553),
//!   `dequantize_row_q4_K` (1530) with `get_scale_min_k4` (880),
//!   `dequantize_row_q6_K` (1940).

/// ggml tensor types the worker can materialize; ids are the official
/// `ggml_type` values (`uint32_t` per gguf.md): 0/1/2/6/8/12/14.
/// Q5_0 (6) is in the set because Qwen's official q4_k_m assets mix Q5_0
/// tensors into attention projections. Anything else — including
/// Q5_1 (7) — is refused loudly at load time.
// Variant names intentionally match the official ggml type names verbatim
// (Q4_K, Q6_K are the ecosystem's spellings; renames would obfuscate).
#[allow(non_camel_case_types)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GgmlType {
    F32,
    F16,
    Q4_0,
    Q5_0,
    Q8_0,
    Q4_K,
    Q6_K,
}

impl GgmlType {
    /// Official ggml_type id → supported variant; unsupported ids → `None`.
    pub fn from_id(id: u32) -> Option<Self> {
        match id {
            0 => Some(Self::F32),
            1 => Some(Self::F16),
            2 => Some(Self::Q4_0),
            6 => Some(Self::Q5_0),
            8 => Some(Self::Q8_0),
            12 => Some(Self::Q4_K),
            14 => Some(Self::Q6_K),
            _ => None,
        }
    }

    /// Elements per quantized block (QK4_0/QK5_0/QK8_0 = 32, QK_K = 256);
    /// `None` for contiguous F32/F16.
    fn block_elems(self) -> Option<u64> {
        match self {
            Self::Q4_0 | Self::Q5_0 | Self::Q8_0 => Some(32),
            Self::Q4_K | Self::Q6_K => Some(256),
            Self::F32 | Self::F16 => None,
        }
    }

    /// Bytes per quantized block, from ggml-common.h static asserts.
    fn block_bytes(self) -> Option<u64> {
        match self {
            Self::Q4_0 => Some(18),
            Self::Q5_0 => Some(22),
            Self::Q8_0 => Some(34),
            Self::Q4_K => Some(144),
            Self::Q6_K => Some(210),
            Self::F32 | Self::F16 => None,
        }
    }
}

/// Whole-tensor dequantization to F32 (plan Task 4 interface).
///
/// `row = ne[0]`, `rows = product(ne[1..])`; row lengths of block types
/// must be divisible by the block size (32 for Q4_0/Q8_0, 256 for
/// Q4_K/Q6_K); output length is `product(ne)`.
pub fn dequantize(ty: GgmlType, raw: &[u8], ne: &[u64]) -> Result<Vec<f32>, String> {
    let (&row, trailing) = ne
        .split_first()
        .ok_or_else(|| "tensor has no dimensions".to_string())?;
    let rows: u64 = trailing.iter().product();
    if row == 0 || rows == 0 {
        return Err(format!("zero-sized tensor dims {ne:?}"));
    }
    let total = row
        .checked_mul(rows)
        .and_then(|n| usize::try_from(n).ok())
        .ok_or_else(|| format!("tensor dims {ne:?} exceed address space"))?;

    let row_bytes: u64 =
        if let (Some(block), Some(block_bytes)) = (ty.block_elems(), ty.block_bytes()) {
            if !row.is_multiple_of(block) {
                return Err(format!(
                    "row length {row} is not a multiple of {ty:?} block size {block}"
                ));
            }
            (row / block) * block_bytes
        } else {
            match ty {
                GgmlType::F32 => row * 4,
                GgmlType::F16 => row * 2,
                _ => return Err(format!("{ty:?} is missing its block layout")),
            }
        };
    let expected = row_bytes
        .checked_mul(rows)
        .ok_or_else(|| format!("tensor dims {ne:?} exceed address space"))?;
    if raw.len() as u64 != expected {
        return Err(format!(
            "byte size mismatch: file holds {} bytes, {ty:?} needs {expected}",
            raw.len()
        ));
    }

    let mut out = vec![0f32; total];
    let row_out = row as usize;
    let row_bytes = row_bytes as usize;
    for r in 0..rows as usize {
        dequant_row(
            ty,
            &raw[r * row_bytes..(r + 1) * row_bytes],
            &mut out[r * row_out..(r + 1) * row_out],
        );
    }
    Ok(out)
}

/// One row (or one F32/F16 span): sizes already validated by `dequantize`,
/// so every chunk split below is exact.
fn dequant_row(ty: GgmlType, src: &[u8], dst: &mut [f32]) {
    match ty {
        GgmlType::F32 => {
            for (d, c) in dst.iter_mut().zip(src.as_chunks::<4>().0) {
                *d = f32::from_le_bytes(*c);
            }
        }
        GgmlType::F16 => {
            for (d, c) in dst.iter_mut().zip(src.as_chunks::<2>().0) {
                *d = f16_to_f32(u16::from_le_bytes(*c));
            }
        }
        GgmlType::Q4_0 => {
            // dequantize_row_q4_0 (ggml-quants.c:459), block_q4_0 = 18 B
            for (y, b) in dst
                .as_chunks_mut::<32>()
                .0
                .iter_mut()
                .zip(src.as_chunks::<18>().0)
            {
                let d = f16_to_f32(u16::from_le_bytes([b[0], b[1]]));
                for j in 0..16 {
                    let q = b[2 + j];
                    y[j] = ((q & 0x0F) as f32 - 8.0) * d;
                    y[j + 16] = ((q >> 4) as f32 - 8.0) * d;
                }
            }
        }
        GgmlType::Q5_0 => {
            // dequantize_row_q5_0 (ggml-quants.c:500), block_q5_0 = 22 B:
            // f16 d, u32 qh (5th bits), u8 qs[16] (nibbles);
            // xh_0 = qh bit j as 0x10, xh_1 = qh bit (j+16) via >> (j+12).
            for (y, b) in dst
                .as_chunks_mut::<32>()
                .0
                .iter_mut()
                .zip(src.as_chunks::<22>().0)
            {
                let d = f16_to_f32(u16::from_le_bytes([b[0], b[1]]));
                let qh = u32::from_le_bytes(b[2..6].try_into().expect("4 bytes"));
                for j in 0..16 {
                    let xh_0 = ((qh >> j) << 4) & 0x10;
                    let xh_1 = (qh >> (j + 12)) & 0x10;
                    let x0 = ((b[6 + j] & 0x0F) as i32 | (xh_0 as i32)) - 16;
                    let x1 = ((b[6 + j] >> 4) as i32 | (xh_1 as i32)) - 16;
                    y[j] = x0 as f32 * d;
                    y[j + 16] = x1 as f32 * d;
                }
            }
        }
        GgmlType::Q8_0 => {
            // dequantize_row_q8_0 (ggml-quants.c:553), block_q8_0 = 34 B
            for (y, b) in dst
                .as_chunks_mut::<32>()
                .0
                .iter_mut()
                .zip(src.as_chunks::<34>().0)
            {
                let d = f16_to_f32(u16::from_le_bytes([b[0], b[1]]));
                for (o, &q) in b[2..34].iter().enumerate() {
                    y[o] = (q as i8 as f32) * d;
                }
            }
        }
        GgmlType::Q4_K => {
            // dequantize_row_q4_K (ggml-quants.c:1530), block_q4_K = 144 B:
            // four 64-element groups; group j uses scale pairs (is, is+1)
            // from get_scale_min_k4: 32 lows then 32 highs.
            for (y, b) in dst
                .as_chunks_mut::<256>()
                .0
                .iter_mut()
                .zip(src.as_chunks::<144>().0)
            {
                let d = f16_to_f32(u16::from_le_bytes([b[0], b[1]]));
                let min = f16_to_f32(u16::from_le_bytes([b[2], b[3]]));
                let scales: [u8; 12] = b[4..16].try_into().expect("12 bytes");
                let q = b[16..].as_chunks::<32>().0;
                let groups = y.as_chunks_mut::<64>().0;
                for (gi, group) in groups.iter_mut().enumerate() {
                    let is = gi * 2;
                    let (s0, m0) = get_scale_min_k4(is, &scales);
                    let (s1, m1) = get_scale_min_k4(is + 1, &scales);
                    let d1 = d * s0 as f32;
                    let x1 = min * m0 as f32;
                    let d2 = d * s1 as f32;
                    let x2 = min * m1 as f32;
                    for (o, &qv) in q[gi].iter().enumerate() {
                        group[o] = d1 * (qv & 0x0F) as f32 - x1;
                        group[32 + o] = d2 * (qv >> 4) as f32 - x2;
                    }
                }
            }
        }
        GgmlType::Q6_K => {
            // dequantize_row_q6_K (ggml-quants.c:1940), block_q6_K = 210 B:
            // two 128-element halves; q = (low nibble | high bits << 4) - 32.
            for (y, b) in dst
                .as_chunks_mut::<256>()
                .0
                .iter_mut()
                .zip(src.as_chunks::<210>().0)
            {
                let ql = b[0..128].as_chunks::<64>().0;
                let qh = b[128..192].as_chunks::<32>().0;
                let scales = b[192..208].as_chunks::<8>().0;
                let d = f16_to_f32(u16::from_le_bytes(b[208..210].try_into().expect("2 bytes")));
                for n in 0..2usize {
                    let base = n * 128;
                    let ql_h = ql[n];
                    let qh_h = qh[n];
                    let sc_h = scales[n];
                    for l in 0..32usize {
                        let is = l / 16;
                        let h = qh_h[l];
                        let q1 = ((ql_h[l] & 0x0F) | ((h & 0x03) << 4)) as i8;
                        let q2 = ((ql_h[l + 32] & 0x0F) | (((h >> 2) & 0x03) << 4)) as i8;
                        let q3 = ((ql_h[l] >> 4) | (((h >> 4) & 0x03) << 4)) as i8;
                        let q4 = ((ql_h[l + 32] >> 4) | (((h >> 6) & 0x03) << 4)) as i8;
                        y[base + l] = d * (sc_h[is] as i8) as f32 * (q1 as f32 - 32.0);
                        y[base + l + 32] = d * (sc_h[is + 2] as i8) as f32 * (q2 as f32 - 32.0);
                        y[base + l + 64] = d * (sc_h[is + 4] as i8) as f32 * (q3 as f32 - 32.0);
                        y[base + l + 96] = d * (sc_h[is + 6] as i8) as f32 * (q4 as f32 - 32.0);
                    }
                }
            }
        }
    }
}

/// Verbatim port of llama.cpp `get_scale_min_k4` (ggml-quants.c:880):
/// 8 scales + 8 mins packed into 12 bytes (6 bits each).
fn get_scale_min_k4(j: usize, q: &[u8]) -> (u8, u8) {
    if j < 4 {
        (q[j] & 63, q[j + 4] & 63)
    } else {
        let d = (q[j + 4] & 0x0F) | ((q[j - 4] >> 6) << 4);
        let m = (q[j + 4] >> 4) | ((q[j] >> 6) << 4);
        (d, m)
    }
}

/// IEEE 754 half → f32 without a dependency: subnormals (exp 0),
/// inf/NaN (exp 31), normals via exact powers of two.
fn f16_to_f32(bits: u16) -> f32 {
    let sign = if bits & 0x8000 != 0 { -1.0f32 } else { 1.0f32 };
    let exp = (bits >> 10) & 0x1F;
    let mantissa = (bits & 0x03FF) as f32;
    match exp {
        0 => sign * mantissa * (2.0f32).powi(-24), // m · 2^-24
        31 if mantissa == 0.0 => sign * f32::INFINITY,
        31 => f32::NAN,
        _ => sign * (1.0 + mantissa / 1024.0) * (2.0f32).powi(exp as i32 - 15),
    }
}
