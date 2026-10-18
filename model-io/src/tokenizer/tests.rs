//! Hermetic unit gates for the tokenizer pipeline (plan Tasks 2-5) on the
//! fixture writer's tiny BPE. The id-exact parity vs llama-server runs
//! asset-gated in PR-C; everything here proves the mechanics without a
//! network, device or model asset.

use super::*;
use crate::fixture::{write_tiny_model, TinyModelConfig, TokenizerFixture};

/// Fixture ids: bytes 0..255 in byte order, then merge results in merge
/// order (`default_merges`): "he", "the", "ab", " ab", " the".
const ID_THE: u32 = 257;
const ID_AB: u32 = 258;
const ID_G_AB: u32 = 259; // " ab" piece
const ID_G_THE: u32 = 260; // " the" piece

fn tokenizer_with(t: TokenizerFixture) -> (tempfile::TempDir, Tokenizer) {
    let tempdir = tempfile::tempdir().expect("tempdir");
    let fx = write_tiny_model(
        tempdir.path(),
        TinyModelConfig {
            tokenizer: Some(t),
            ..Default::default()
        },
    );
    // The reader releases its mmap at the end of `from_gguf`, so the fixture
    // may vanish when `tempdir` drops.
    let tokenizer = Tokenizer::from_gguf(&fx.path).expect("build tokenizer from fixture");
    (tempdir, tokenizer)
}

fn tokenizer() -> (tempfile::TempDir, Tokenizer) {
    tokenizer_with(TokenizerFixture::default())
}

#[test]
fn byte_table_matches_the_gpt2_map() {
    let (_dir, tok) = tokenizer();
    assert_eq!(tok.byte_piece[0x20], "Ġ", "space");
    assert_eq!(tok.byte_piece[0x0A], "Ċ", "LF");
    assert_eq!(tok.byte_piece[0x41], "A", "printable byte keeps itself");
    assert_eq!(tok.byte_piece[0xE9], "é", "0xA1..0xFF range keeps itself");
    // The shifted-by-256 order: byte 0 is the first non-printable byte.
    assert_eq!(tok.byte_piece[0x00], "Ā", "byte 0x00 -> U+0100");
    assert_eq!(
        tok.byte_piece[0x0A].chars().next().unwrap() as u32,
        266,
        "byte 0x0A -> chr(266), the 11th shifted codepoint"
    );
}

#[test]
fn all_256_bytes_round_trip_through_the_piece_table() {
    let (_dir, tok) = tokenizer();
    for (byte, piece) in tok.byte_piece.iter().enumerate() {
        assert_eq!(
            *piece,
            crate::fixture::byte_to_byte_piece(byte as u8).to_string(),
            "piece table matches the writer's map at {byte}"
        );
        if byte < 0x80 {
            // ASCII text: full encode/decode round trip through the id.
            let text = char::from_u32(byte as u32).unwrap().to_string();
            assert_eq!(tok.encode(&text).expect("encode"), vec![byte as u32]);
            assert_eq!(tok.decode(&[byte as u32]).expect("decode"), text);
        } else {
            // Every byte >= 0x80 is alone-invalid UTF-8: the strict decoder
            // must refuse (only multi-byte sequences carry them in text).
            assert!(
                matches!(
                    tok.decode(&[byte as u32]),
                    Err(TokenizerError::InvalidUtf8(_))
                ),
                "lone byte {byte:#x} refused by strict decode"
            );
        }
    }
    // The high bytes DO round-trip inside real multi-byte text.
    for text in [
        "\u{00E9}",
        "\u{4E2D}",
        "\u{1F9D4}\u{1F3FD}\u{200D}\u{2640}\u{FE0F}",
    ] {
        let ids = tok
            .encode(text)
            .expect("encode bytes pieces of multibyte char");
        assert_eq!(tok.decode(&ids).expect("decode"), *text);
    }
}

#[test]
fn vocab_is_256_byte_pieces_plus_merge_results() {
    let (_dir, tok) = tokenizer();
    assert_eq!(tok.n_pieces(), 256 + 5);
    assert_eq!(tok.pieces[ID_THE as usize], "the");
    assert_eq!(tok.pieces[ID_G_AB as usize], "Ġab");
}

#[test]
fn merge_order_picks_the_lowest_rank_first() {
    let (_dir, tok) = tokenizer();
    // "the": pairs (t,h)? none; (h,e) rank 0 -> "t","he"; then (t,he)
    // rank 1 -> "the" (re-scan from scratch after every single merge).
    assert_eq!(tok.encode("the").expect("encode"), vec![ID_THE]);
    // " ab": (a,b) rank 2 -> " ", "ab"; then (" ",ab) rank 3 -> " ab".
    assert_eq!(tok.encode(" ab").expect("encode"), vec![ID_G_AB]);
    // both pre-tokens separately then concatenated ids
    assert_eq!(tok.encode("the ab").expect("encode"), vec![ID_THE, ID_G_AB]);
    // no merges available: every byte stands alone
    assert_eq!(
        tok.encode("xyz").expect("encode"),
        vec![b'x' as u32, b'y' as u32, b'z' as u32]
    );
}

#[test]
fn pre_tokenizer_splits_like_the_qwen2_rules() {
    let (_dir, tok) = tokenizer();
    // Case-insensitive contraction alternative exists in the split: "I'll"
    // is a letter chunk "I" + contraction chunk "'ll", so ids are 0x49
    // (byte I), 0x27 (byte '), 0x6C, 0x6C (the 'l' pair, unmerged here -
    // the fixture has no ("l","l") merge).
    assert_eq!(
        tok.encode("I'll").expect("encode"),
        vec![0x49, 0x27, 0x6C, 0x6C]
    );
    // Digits split one per pre-token (the Qwen2 rule that differs from
    // GPT-2/llama-3 number grouping).
    assert_eq!(
        tok.encode("2026").expect("encode"),
        vec![b'2' as u32, b'0' as u32, b'2' as u32, b'6' as u32]
    );
    // The optional single non-letter/non-digit prefix rides with the letter
    // run as ONE chunk - so the fixture's (a,b) rank-2 merge applies INSIDE
    // it, producing ")" + piece "ab" + "c".
    assert_eq!(
        tok.encode(")abc").expect("encode"),
        vec![b')' as u32, ID_AB, b'c' as u32]
    );
    // Punct runs absorb trailing CRLF as one chunk ("!!\r\n"); with no
    // punct merges the byte ids come out unchanged.
    assert_eq!(
        tok.encode("!!\r\n").expect("encode"),
        vec![b'!' as u32, b'!' as u32, b'\r' as u32, b'\n' as u32]
    );
    // A leading space merges into the " the" piece - proof that the space
    // rides along with the letter run instead of its own pre-token.
    assert_eq!(tok.encode(" the").expect("encode"), vec![ID_G_THE]);
}

#[test]
fn decode_round_trips_the_whole_hermetic_corpus() {
    let (_dir, tok) = tokenizer();
    let mut corpus: Vec<String> = vec![
        "The capital of France is".to_string(),
        "don't I'll we've can't should've".to_string(),
        "3.14159 20261017 42 7".to_string(),
        "{\"a\": [1,2], \"b\": null}".to_string(),
        "spaces   ...and    tabs\tinside".to_string(),
        "CAFE cafe cafe".to_string(),
        "line1\r\nline2\r\n\r\n\r\nline3".to_string(),
        "the the the ab ab the".to_string(),
    ];
    corpus.push("a".repeat(1000) + "b");
    for text in corpus {
        let ids = tok.encode(&text).expect("encode");
        assert!(!ids.is_empty());
        let back = tok.decode(&ids).expect("decode");
        assert_eq!(back, text, "round trip for {text:?}");
    }
}

#[test]
fn decode_refuses_out_of_range_and_invalid_utf8() {
    let (_dir, tok) = tokenizer();
    assert!(matches!(
        tok.decode(&[9999]),
        Err(TokenizerError::IdOutOfRange(9999, 261))
    ));
    // Byte piece for 0xE4 decodes to the raw byte - a lone continuation/
    // lead byte is invalid UTF-8 on its own.
    assert!(matches!(
        tok.decode(&[0xE4]),
        Err(TokenizerError::InvalidUtf8(_))
    ));
}

#[test]
fn from_gguf_refuses_missing_required_keys() {
    let tempdir = tempfile::tempdir().expect("tempdir");
    let refused = |t: TokenizerFixture, key: &'static str| {
        let fx = write_tiny_model(
            tempdir.path(),
            TinyModelConfig {
                tokenizer: Some(t),
                ..Default::default()
            },
        );
        let err = Tokenizer::from_gguf(&fx.path).expect_err("must refuse");
        match err {
            TokenizerError::Gguf(GgufError::MissingKey(actual)) => {
                assert_eq!(actual, key, "refusal names the missing key");
            }
            other => panic!("expected MissingKey({key}), got {other:?}"),
        }
    };

    // No tokenizer section at all.
    let fx = write_tiny_model(
        tempdir.path(),
        TinyModelConfig {
            tokenizer: None,
            ..Default::default()
        },
    );
    match Tokenizer::from_gguf(&fx.path) {
        Err(TokenizerError::Gguf(GgufError::MissingKey("tokenizer.ggml.tokens"))) => {}
        other => panic!("expected missing tokens refusal, got {other:?}"),
    }

    refused(
        TokenizerFixture {
            include_merges: false,
            ..Default::default()
        },
        "tokenizer.ggml.merges",
    );

    // llama.cpp-aligned optional KVs: a fixture WITHOUT scores (the real
    // file's shape) or without token_type reads fine.
    for t in [
        TokenizerFixture {
            include_scores: false,
            ..Default::default()
        },
        TokenizerFixture {
            include_token_types: false,
            ..Default::default()
        },
    ] {
        let fx = write_tiny_model(
            tempdir.path(),
            TinyModelConfig {
                tokenizer: Some(t),
                ..Default::default()
            },
        );
        Tokenizer::from_gguf(&fx.path).expect("optional KVs are not required");
    }
}

#[test]
fn from_gguf_refuses_wrong_tokens_elem_type_and_wrong_markers() {
    let tempdir = tempfile::tempdir().expect("tempdir");
    let cfg_with = |t: TokenizerFixture| {
        write_tiny_model(
            tempdir.path(),
            TinyModelConfig {
                tokenizer: Some(t),
                ..Default::default()
            },
        )
    };

    // Tokens array written as u32 elements: refused loudly at the type...
    let fx = cfg_with(TokenizerFixture {
        tokens_elem_ty: 4,
        ..Default::default()
    });
    match Tokenizer::from_gguf(&fx.path) {
        Err(TokenizerError::Gguf(GgufError::Malformed(_))) => {}
        other => panic!("expected malformed-elem-type refusal, got {other:?}"),
    }

    // ...and so is a pre value the implementation does not implement.
    let fx = cfg_with(TokenizerFixture {
        pre: Some("gpt4".to_string()),
        ..Default::default()
    });
    match Tokenizer::from_gguf(&fx.path) {
        Err(TokenizerError::UnsupportedPre(pre)) => assert_eq!(pre, "gpt4"),
        other => panic!("expected unsupported-pre refusal, got {other:?}"),
    }

    // ...and a model marker that is not byte-level BPE.
    let fx = cfg_with(TokenizerFixture {
        model: Some("llama".to_string()),
        ..Default::default()
    });
    match Tokenizer::from_gguf(&fx.path) {
        Err(TokenizerError::UnsupportedTokenizerModel(model)) => assert_eq!(model, "llama"),
        other => panic!("expected unsupported-model refusal, got {other:?}"),
    }

    // ...and an entirely absent model KV (llama.cpp-aligned: optional at the
    // GGUF layer, refused at the tokenizer layer naming the gap).
    let fx = cfg_with(TokenizerFixture {
        model: None,
        ..Default::default()
    });
    match Tokenizer::from_gguf(&fx.path) {
        Err(TokenizerError::UnsupportedTokenizerModel(model)) => assert_eq!(model, "<missing>"),
        other => panic!("expected absent-model refusal, got {other:?}"),
    }
}

#[test]
fn token_type_reads_both_u32_and_i32_element_widths() {
    let tempdir = tempfile::tempdir().expect("tempdir");
    // Real gguf-py writers emit INT32 (5); UINT32 (4) is the other accepted
    // width. The default fixture already exercises i32 - assert u32 too.
    for elem in [4u32, 5u32] {
        let fx = write_tiny_model(
            tempdir.path(),
            TinyModelConfig {
                tokenizer: Some(TokenizerFixture {
                    token_types_elem_ty: elem,
                    ..Default::default()
                }),
                ..Default::default()
            },
        );
        let tok = Tokenizer::from_gguf(&fx.path).expect("both widths read");
        assert_eq!(tok.n_pieces(), 261);
    }
}

#[test]
fn special_tokens_are_matched_before_bpe_like_llama_cpp() {
    let (_dir, tok) = tokenizer_with(TokenizerFixture {
        specials: vec!["<|t|>".to_string(), "<|textra|>".to_string()],
        ..Default::default()
    });
    // Pieces: 256 bytes + 5 merge results + 2 specials -> the specials are
    // ids 261 ("<|t|>", shortest) and 262 ("<|textra|>", longest first in
    // the match table).
    let start = "<|textra|>".to_string();
    assert_eq!(
        tok.encode("<|textra|>").expect("longest special"),
        vec![262]
    );
    assert_eq!(tok.encode("<|t|>").expect("short special"), vec![261]);
    // Intervening text is still pre-tokenized between special ids.
    assert_eq!(
        tok.encode(&format!("ab{start}")).expect("between specials"),
        vec![258, 262], // "ab" (merge result) then the special id
        "fragment before the special keeps its merges: {start}"
    );
    // And the specials fire mid-text, exactly on their literal runs.
    assert_eq!(
        tok.encode("a<|t|>the").expect("special mid text"),
        vec![b'a' as u32, 261, ID_THE],
        "special id breaks the text as llama.cpp's parse-special does"
    );
    // Their decode is the literal string, part of the round trip.
    let text = "x<|textra|>y";
    assert_eq!(
        tok.decode(&tok.encode(text).expect("encode"))
            .expect("decode"),
        text
    );
}

#[test]
fn apply_chatml_renders_the_qwen_template_bit_exactly() {
    let (_dir, tok) = tokenizer();
    let chat = tok
        .apply_chatml(&[
            ChatMessage {
                role: "system",
                content: "You are a helpful assistant.",
            },
            ChatMessage {
                role: "user",
                content: "Who won the series in 2020?",
            },
        ])
        .expect("chatml");
    assert_eq!(
        chat,
        "<|im_start|>system\nYou are a helpful assistant.<|im_end|>\n\
         <|im_start|>user\nWho won the series in 2020?<|im_end|>\n\
         <|im_start|>assistant\n"
    );
    // No messages at all is still valid: the generation prompt alone.
    assert_eq!(
        tok.apply_chatml(&[]).expect("empty chat"),
        "<|im_start|>assistant\n"
    );
    // The rendered template round-trips: with specials present the markers
    // BECOME their special ids, and their decode is the literal marker.
    let (_dir2, tok_sp) = tokenizer_with(TokenizerFixture {
        specials: vec![
            "<|im_start|>assistant".to_string(),
            "<|im_start|>".to_string(),
            "<|im_end|>".to_string(),
        ],
        ..Default::default()
    });
    let chat2 = tok_sp
        .apply_chatml(&[ChatMessage {
            role: "user",
            content: "hi",
        }])
        .expect("chatml with specials");
    assert_eq!(
        chat2,
        "<|im_start|>user\nhi<|im_end|>\n<|im_start|>assistant\n"
    );
    let ids = tok_sp.encode(&chat2).expect("encode chatml");
    assert_eq!(
        tok_sp.decode(&ids).expect("decode chatml"),
        chat2,
        "specials render through decode"
    );
    // Longest-first ordering made "<|im_start|>" fire as a whole special id
    // instead of byte pieces - so the first id is NOT byte '<' (0x3C).
    assert_ne!(ids.first(), Some(&0x3C), "must be a special id, not a byte");
}

#[test]
fn chatml_refuses_wrong_roles_and_wrong_architecture() {
    let (_dir, tok) = tokenizer();
    // Unknown roles are refused with the role in the message.
    assert!(matches!(
        tok.apply_chatml(&[ChatMessage { role: "tool", content: "" }]),
        Err(TokenizerError::UnsupportedRole(role)) if role == "tool"
    ));
    // A system turn mid-conversation is refused (template semantics).
    assert!(matches!(
        tok.apply_chatml(&[
            ChatMessage {
                role: "user",
                content: "a"
            },
            ChatMessage {
                role: "system",
                content: "b"
            },
        ]),
        Err(TokenizerError::UnsupportedRole(_))
    ));
    // And a non-qwen2 model never REACHES the template: the loader refuses.
    let tempdir = tempfile::tempdir().expect("tempdir");
    let fx = write_tiny_model(
        tempdir.path(),
        TinyModelConfig {
            architecture: "llama".to_string(),
            ..Default::default()
        },
    );
    match Tokenizer::from_gguf(&fx.path) {
        Err(TokenizerError::Gguf(GgufError::UnsupportedArchitecture(arch))) => {
            assert_eq!(arch, "llama");
        }
        other => panic!("expected architecture refusal, got {other:?}"),
    }
}

#[test]
fn bos_and_eos_ids_surface_when_written() {
    let (_dir, tok) = tokenizer_with(TokenizerFixture {
        bos: Some(0),
        eos: Some(255),
        ..Default::default()
    });
    assert_eq!(tok.bos_token_id, Some(0));
    assert_eq!(tok.eos_token_id, Some(255));
}
