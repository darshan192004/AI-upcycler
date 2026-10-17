//! Dequantization tests (plan Task 4): hand-computed golden blocks for
//! Q4_0/Q8_0, exact F16 round-trip, structural bounds for K-quants, and
//! engine-level load differentials.
//!
//! Block layouts and arithmetic come from llama.cpp `ggml-common.h`
//! (`block_q4_0`/`block_q8_0`/`block_q4_K`/`block_q6_K`) and
//! `ggml-quants.c` (`dequantize_row_*`), fetched 2026-10-17.

mod common;

use ai_upcycler_worker::proto::aiupcycler::rpc::LoadLayersRequest;
use ai_upcycler_worker::quant::{dequantize, GgmlType};

#[test]
fn q4_0_block_matches_hand_computation() {
    // Layout (block_q4_0, 18 bytes): f16 `d` then 16 nibble bytes.
    // d = 0.5 exactly (f16 0x3800); qs[j] = (j << 4) | j so both the lo
    // and the hi nibble of byte j equal j.
    // Reference arithmetic (dequantize_row_q4_0):
    //   y[j]      = ((qs[j] & 0x0F) - 8) * d = (j - 8) * 0.5   for j in 0..16
    //   y[j + 16] = ((qs[j] >>   4) - 8) * d = (j - 8) * 0.5
    let mut raw: Vec<u8> = vec![0x00, 0x38]; // f16 0.5, little-endian
    raw.extend((0u8..16).map(|j| (j << 4) | j)); // qs[j] = 0xJJ

    let out = dequantize(GgmlType::Q4_0, &raw, &[32]).expect("dequant q4_0");

    let half = [
        -4.0, -3.5, -3.0, -2.5, -2.0, -1.5, -1.0, -0.5, // (j-8)*0.5, j = 0..8
        0.0, 0.5, 1.0, 1.5, 2.0, 2.5, 3.0, 3.5, // (j-8)*0.5, j = 8..16
    ];
    let want: Vec<f32> = half.iter().chain(half.iter()).copied().collect();
    assert_eq!(out, want);
}

#[test]
fn q5_0_block_matches_hand_computation() {
    // Layout (block_q5_0, 22 bytes): f16 `d`, u32 `qh` (5th bits), 16 nibble
    // bytes. d = 0.5; qs[j] = (j << 4) | j; qh = 0xAAAAAAAA sets exactly the
    // odd bits. Reference arithmetic (dequantize_row_q5_0):
    //   xh_0 = bit j of qh, presented as 0x10 → +16 for odd j
    //   xh_1 = bit (j + 16) of qh, via ((qh >> (j + 12)) & 0x10)
    //   x0   = ((qs[j] & 0x0F) | xh_0) - 16 ; y[j] = x0 * d
    //   x1   = ((qs[j] >>   4) | xh_1) - 16 ; y[j + 16] = x1 * d
    // j even → nibble = j,  x = j - 16      → (j - 16) * 0.5
    // j odd  → nibble = j,  x = (j|16) - 16 = j → j * 0.5
    let mut raw: Vec<u8> = vec![0x00, 0x38, 0xAA, 0xAA, 0xAA, 0xAA];
    raw.extend((0u8..16).map(|j| (j << 4) | j));

    let out = dequantize(GgmlType::Q5_0, &raw, &[32]).expect("dequant q5_0");

    let half = [
        -8.0, 0.5, // j=0: (0-16)/2 ; j=1: (1|16-16)/2
        -7.0, 1.5, // j=2, j=3
        -6.0, 2.5, // j=4, j=5
        -5.0, 3.5, // j=6, j=7
        -4.0, 4.5, // j=8, j=9
        -3.0, 5.5, // j=10, j=11
        -2.0, 6.5, // j=12, j=13
        -1.0, 7.5, // j=14, j=15
    ];
    let want: Vec<f32> = half.iter().chain(half.iter()).copied().collect();
    assert_eq!(out, want);
}

#[test]
fn q8_0_block_matches_hand_computation() {
    // Layout (block_q8_0, 34 bytes): f16 `d` then 32 int8 quants.
    // d = 0.5; qs[j] = j - 16 (wrapping as u8).
    // Reference arithmetic (dequantize_row_q8_0): y[j] = qs[j] * d
    // → (j - 16) * 0.5 for j in 0..32: -8.0 ..= 7.5 in 0.5 steps.
    let mut raw: Vec<u8> = vec![0x00, 0x38];
    raw.extend((0u8..32).map(|j| j.wrapping_sub(16)));

    let out = dequantize(GgmlType::Q8_0, &raw, &[32]).expect("dequant q8_0");

    let want: Vec<f32> = (0..32).map(|j| (j as f32 - 16.0) * 0.5).collect();
    assert_eq!(
        out,
        vec![
            -8.0, -7.5, -7.0, -6.5, -6.0, -5.5, -5.0, -4.5, -4.0, -3.5, -3.0, -2.5, -2.0, -1.5,
            -1.0, -0.5, 0.0, 0.5, 1.0, 1.5, 2.0, 2.5, 3.0, 3.5, 4.0, 4.5, 5.0, 5.5, 6.0, 6.5, 7.0,
            7.5,
        ]
    );
    assert_eq!(want.len(), 32);
}

#[test]
fn f16_round_trip_for_exactly_representable_values() {
    // f16 bit patterns (sign:1 exp:5 mantissa:10), each exact in f16:
    //   0.5          = 2^-1        → exp 14 (01110) → 0x3800
    //   -1.0         = -2^0        → sign, exp 15    → 0xBC00
    //   2.0          = 2^1         → exp 15          → 0x4000
    //   0.25         = 2^-2        → exp 13 (01101)  → 0x3400
    //   1.0009765625 = 1 + 2^-10   → exp 15, mant 1  → 0x3C01
    let raw: Vec<u8> = [0x3800u16, 0xBC00, 0x4000, 0x3400, 0x3C01]
        .iter()
        .flat_map(|bits| bits.to_le_bytes())
        .collect();

    let out = dequantize(GgmlType::F16, &raw, &[5]).expect("dequant f16");

    assert_eq!(out, vec![0.5, -1.0, 2.0, 0.25, 1.0 + 1.0 / 1024.0]); // 0x3C01 = 1 + 2^-10
}

#[test]
fn k_quants_produce_finite_full_length_output() {
    // Synthetic super-blocks per ggml-common.h:
    //   block_q4_K (144B): f16 d, f16 dmin, scales[12], qs[128]
    //   block_q6_K (210B): ql[128], qh[64], int8 scales[16], f16 d
    // Structural assertions only (no golden values for hand-assembled
    // bytes): full length, finiteness, and the layout's value bound.

    // --- Q4_K: y = d * sc * q - dmin * m, sc/m are 6-bit (≤ 63), q ≤ 15
    let mut q4k = vec![0u8; 144];
    q4k[0..4].copy_from_slice(&[0x00, 0x38, 0x00, 0x34]); // d = 0.5, dmin = 0.25
    for (i, b) in q4k[4..].iter_mut().enumerate() {
        *b = ((i * 7 + 3) % 256) as u8; // deterministic pattern
    }
    let out = dequantize(GgmlType::Q4_K, &q4k, &[256]).expect("dequant q4_k");
    assert_eq!(out.len(), 256);
    assert!(out.iter().all(|v| v.is_finite()), "all values finite");
    let bound = 0.5 * 63.0 * 15.0 + 0.25 * 63.0; // |d·sc·q| + |dmin·m|
    assert!(
        out.iter().all(|v| v.abs() <= bound),
        "q4_k values within |y| ≤ |d|·63·15 + |dmin|·63 = {bound}"
    );

    // --- Q6_K: y = d * scale * q, scale int8 (≤ 127), q = (6 bits) - 32
    let mut q6k = vec![0u8; 210];
    for (i, b) in q6k[..208].iter_mut().enumerate() {
        *b = ((i * 11 + 5) % 256) as u8;
    }
    q6k[208..210].copy_from_slice(&[0x00, 0x38]); // d = 0.5 (last two bytes)
    let out = dequantize(GgmlType::Q6_K, &q6k, &[256]).expect("dequant q6_k");
    assert_eq!(out.len(), 256);
    assert!(out.iter().all(|v| v.is_finite()), "all values finite");
    let bound = 0.5 * 127.0 * 32.0; // |d| · max|scale| · max|q|
    assert!(
        out.iter().all(|v| v.abs() <= bound),
        "q6_k values within |y| ≤ |d|·127·32 = {bound}"
    );
}

#[tokio::test]
async fn q4_0_fixture_allocates_same_as_f32_fixture() {
    let dir_f32 = tempfile::tempdir().unwrap();
    let dir_q4 = tempfile::tempdir().unwrap();
    let f32_fx = common::write_tiny_model(dir_f32.path(), common::TinyModelConfig::default());
    let q4_fx = common::write_tiny_model(
        dir_q4.path(),
        common::TinyModelConfig {
            quant: common::FixtureQuant::Q4_0,
            model_name: "tiny-qwen2-q4_0".to_string(),
            ..Default::default()
        },
    );

    let mut f32_client = common::connect_engine(&f32_fx).await;
    let f32_resp = f32_client
        .load_layers(LoadLayersRequest {
            model_id: f32_fx.model_id.clone(),
            start_layer: 0,
            end_layer: 3,
            weights_chunk_data: Vec::new(),
        })
        .await
        .expect("rpc")
        .into_inner();
    assert!(f32_resp.success, "{}", f32_resp.error_message);

    let mut q4_client = common::connect_engine(&q4_fx).await;
    let q4_resp = q4_client
        .load_layers(LoadLayersRequest {
            model_id: q4_fx.model_id.clone(),
            start_layer: 0,
            end_layer: 3,
            weights_chunk_data: Vec::new(),
        })
        .await
        .expect("rpc")
        .into_inner();
    assert!(
        q4_resp.success,
        "q4_0 fixture must load: {}",
        q4_resp.error_message
    );

    // Dequantized storage claim: Q4_0 weights occupy exactly the F32 bytes.
    assert_eq!(q4_resp.allocated_ram_bytes, f32_resp.allocated_ram_bytes);
    assert_eq!(
        q4_resp.allocated_ram_bytes,
        f32_fx.expected_f32_bytes(0, 3) as i64
    );
}

#[tokio::test]
async fn unsupported_type_refused_with_type_id() {
    // The supported set is exactly ids 0/1/2/6/8/12/14; everything else —
    // including Q5_1 (7) — maps to None and is refused loudly at load.
    assert_eq!(GgmlType::from_id(0), Some(GgmlType::F32));
    assert_eq!(GgmlType::from_id(1), Some(GgmlType::F16));
    assert_eq!(GgmlType::from_id(2), Some(GgmlType::Q4_0));
    assert_eq!(GgmlType::from_id(6), Some(GgmlType::Q5_0));
    assert_eq!(GgmlType::from_id(8), Some(GgmlType::Q8_0));
    assert_eq!(GgmlType::from_id(12), Some(GgmlType::Q4_K));
    assert_eq!(GgmlType::from_id(14), Some(GgmlType::Q6_K));
    assert_eq!(GgmlType::from_id(7), None, "Q5_1 stays unsupported");
    assert_eq!(GgmlType::from_id(99), None);

    let dir = tempfile::tempdir().unwrap();
    let fx = common::write_tiny_model(
        dir.path(),
        common::TinyModelConfig {
            quant: common::FixtureQuant::UnsupportedQ5_1,
            model_name: "tiny-qwen2-q5_1".to_string(),
            ..Default::default()
        },
    );
    let mut client = common::connect_engine(&fx).await;
    let resp = client
        .load_layers(LoadLayersRequest {
            model_id: fx.model_id.clone(),
            start_layer: 0,
            end_layer: 3,
            weights_chunk_data: Vec::new(),
        })
        .await
        .expect("rpc")
        .into_inner();
    assert!(!resp.success);
    assert!(
        resp.error_message.contains("ggml type id 7"),
        "message names the type id: {}",
        resp.error_message
    );
}
