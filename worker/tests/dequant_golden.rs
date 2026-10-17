//! Dequantization golden vectors, cut from the production model file
//! (the one `AI_UPCCLER_MODEL` points every asset-gated test at) by the
//! committed `examples/golden_dump.rs` tool.
//!
//! These records are *regression tripwires*, not the primary evidence:
//! every port below was already verified line-by-line against llama.cpp
//! `ggml-quants.c` at d81235049 (see `dequant.rs` hand-computed blocks).
//! The goldens pin the verified bit patterns so any later edit that
//! changes a dequantized value fails loudly, including for the two
//! K-quant types that only previously had structural bounds checks —
//! Q4_K and Q6_K, where 64-element scale/sub-block arithmetic is too
//! wide for a hand-computable fixture.
//!
//! Fixture format (first non-`#` line of `tests/goldens/*.txt`):
//! `tensor|block_index|raw_block_hex|expected_f32_bit_patterns_comma`,
//! compared with exact bit equality.

use ai_upcycler_worker::quant::{dequantize, GgmlType};

const Q5_0: &str = include_str!("goldens/q5_0.txt");
const Q8_0: &str = include_str!("goldens/q8_0.txt");
const Q4_K: &str = include_str!("goldens/q4_k.txt");
const Q6_K: &str = include_str!("goldens/q6_k.txt");

fn check(records: &str, ty: GgmlType, block_elems: u64) {
    let mut checked = 0;
    for line in records.lines() {
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        checked += 1;
        let parts: Vec<&str> = line.split('|').collect();
        assert!(parts.len() == 4, "malformed record: {line:?}");
        let (name, block) = (parts[0], parts[1]);

        let hex = parts[2];
        assert!(
            hex.len().is_multiple_of(2),
            "odd-length raw hex in [{name} {block}]"
        );
        let mut raw = Vec::with_capacity(hex.len() / 2);
        for i in (0..hex.len()).step_by(2) {
            raw.push(u8::from_str_radix(&hex[i..i + 2], 16).expect("hex pair"));
        }

        let expected: Vec<u32> = parts[3]
            .split(',')
            .map(|h| u32::from_str_radix(h, 16).expect("hex bit pattern"))
            .collect();

        let out = dequantize(ty, &raw, &[block_elems])
            .unwrap_or_else(|e| panic!("{name} block {block}: {e}"));
        assert!(
            out.len() == expected.len(),
            "{name} block {block}: got {} f32 values, fixture holds {}",
            out.len(),
            expected.len()
        );
        for (i, (got, want)) in out.iter().zip(&expected).enumerate() {
            assert!(
                got.to_bits() == *want,
                "{name} block {block} element {i}: got {got:e} ({:08x}), want {:08x}",
                got.to_bits(),
                want
            );
        }
    }
    assert!(checked > 0, "no golden records parsed for {ty:?}");
}

#[test]
fn q5_0_golden_records_dequantize_bit_exactly() {
    check(Q5_0, GgmlType::Q5_0, 32);
}

#[test]
fn q8_0_golden_records_dequantize_bit_exactly() {
    check(Q8_0, GgmlType::Q8_0, 32);
}

/// Closes the structural-only hole: Q4_K previously had no exact check.
#[test]
fn q4_k_golden_records_dequantize_bit_exactly() {
    check(Q4_K, GgmlType::Q4_K, 256);
}

/// Closes the structural-only hole: Q6_K previously had no exact check.
#[test]
fn q6_k_golden_records_dequantize_bit_exactly() {
    check(Q6_K, GgmlType::Q6_K, 256);
}
