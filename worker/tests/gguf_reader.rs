//! GGUF-v3 reader tests: metadata, tensor table, raw reads, and loud
//! rejections of malformed fixtures (plan Task 2).

mod common;

use ai_upcycler_worker::gguf::GgufFile;

#[test]
fn parses_default_fixture_metadata() {
    let dir = tempfile::tempdir().unwrap();
    let fx = common::write_tiny_model(dir.path(), common::TinyModelConfig::default());

    let gguf = GgufFile::open(&fx.path).expect("open fixture");
    assert_eq!(gguf.model_name(), "tiny-qwen2-f32");

    let h = gguf.hparams();
    assert_eq!(h.block_count, 4);
    assert_eq!(h.embedding_length, 64);
    assert_eq!(h.feed_forward_length, 128);
    assert_eq!(h.head_count, 4);
    assert_eq!(h.head_count_kv, 2);
    assert_eq!(h.head_dim, 16);
    assert!((h.rms_eps - 1e-5).abs() < 1e-9, "got {}", h.rms_eps);
    assert_eq!(h.rope_theta, 10_000.0);
    assert_eq!(h.context_length, 64);
}

#[test]
fn tensor_table_matches_the_fixture_spec() {
    let dir = tempfile::tempdir().unwrap();
    let fx = common::write_tiny_model(dir.path(), common::TinyModelConfig::default());
    let gguf = GgufFile::open(&fx.path).expect("open fixture");

    // Exactly the declared names: token_embd + 11 per-block tensors x 4
    // layers + output_norm; tied fixture has no output.weight.
    let mut expected: Vec<String> = vec!["token_embd.weight".into()];
    for i in 0..fx.n_layers {
        for name in [
            "attn_norm.weight",
            "ffn_norm.weight",
            "attn_q_norm.weight",
            "attn_k_norm.weight",
            "attn_q.weight",
            "attn_k.weight",
            "attn_v.weight",
            "attn_output.weight",
            "ffn_gate.weight",
            "ffn_up.weight",
            "ffn_down.weight",
        ] {
            expected.push(format!("blk.{i}.{name}"));
        }
    }
    expected.push("output_norm.weight".into());
    assert_eq!(gguf.tensor_names().count(), expected.len());
    for name in &expected {
        assert!(gguf.tensor(name).is_some(), "missing tensor {name}");
    }
    assert!(gguf.tensor("output.weight").is_none(), "tied fixture");
    assert!(
        gguf.tensor("blk.4.attn_q.weight").is_none(),
        "only 4 layers"
    );

    let q = gguf.tensor("blk.0.attn_q.weight").expect("attn_q");
    assert_eq!(q.ggml_type, 0, "F32");
    assert_eq!(q.ne, vec![64, 64]);
    assert_eq!(q.nbytes, Some(64 * 64 * 4));
    let k = gguf.tensor("blk.0.attn_k.weight").expect("attn_k");
    assert_eq!(k.ne, vec![64, 32]);
    let down = gguf.tensor("blk.3.ffn_down.weight").expect("ffn_down");
    assert_eq!(down.ne, vec![128, 64]);
}

#[test]
fn read_raw_returns_the_bytes_the_writer_produced() {
    let dir = tempfile::tempdir().unwrap();
    let fx = common::write_tiny_model(dir.path(), common::TinyModelConfig::default());
    let gguf = GgufFile::open(&fx.path).expect("open fixture");

    let got = gguf.read_raw("blk.0.attn_q.weight").expect("read");
    let want = fx.expected_raw_tensor("blk.0.attn_q.weight");
    assert_eq!(got.len(), 64 * 64 * 4);
    assert_eq!(got, want, "reader bytes must round-trip the writer");

    let embd = gguf.read_raw("token_embd.weight").expect("read");
    assert_eq!(embd.len(), 64 * 256 * 4);
    assert_eq!(embd, fx.expected_raw_tensor("token_embd.weight"));
}

#[test]
fn rejects_bad_magic() {
    let dir = tempfile::tempdir().unwrap();
    let fx = common::write_tiny_model(dir.path(), common::TinyModelConfig::default());
    let mut bytes = std::fs::read(&fx.path).unwrap();
    bytes[0] = b'X';
    let bad = dir.path().join("bad-magic.gguf");
    std::fs::write(&bad, bytes).unwrap();

    assert!(GgufFile::open(&bad).is_err(), "bad magic must be rejected");
}

#[test]
fn rejects_wrong_architecture() {
    let dir = tempfile::tempdir().unwrap();
    let cfg = common::TinyModelConfig {
        architecture: "llama".into(),
        ..Default::default()
    };
    let fx = common::write_tiny_model(dir.path(), cfg);

    let err = GgufFile::open(&fx.path).expect_err("non-qwen2 arch rejected");
    assert!(
        err.to_string().contains("llama"),
        "error names the arch: {err}"
    );
}

#[test]
fn rejects_truncated_file() {
    let dir = tempfile::tempdir().unwrap();
    let fx = common::write_tiny_model(dir.path(), common::TinyModelConfig::default());
    let bytes = std::fs::read(&fx.path).unwrap();

    // Cuts across header, key/values, tensor infos and the data section.
    for cut in [24usize, 400, 900, 1500, bytes.len() / 2, bytes.len() - 1] {
        let bad = dir.path().join(format!("truncated-{cut}.gguf"));
        std::fs::write(&bad, &bytes[..cut]).unwrap();
        assert!(
            GgufFile::open(&bad).is_err(),
            "truncation at {cut} bytes must be rejected"
        );
    }
}

#[test]
fn absent_rope_freq_base_defaults_to_10000() {
    let dir = tempfile::tempdir().unwrap();
    let cfg = common::TinyModelConfig {
        rope_theta: None,
        ..Default::default()
    };
    let fx = common::write_tiny_model(dir.path(), cfg);

    let gguf = GgufFile::open(&fx.path).expect("open fixture");
    assert_eq!(gguf.hparams().rope_theta, 10_000.0);
}
