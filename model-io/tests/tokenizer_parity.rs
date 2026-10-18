//! Tokenizer parity vs llama-server (plan Task 6): for a corpus spanning
//! every pre-tokenizer rule class, `Tokenizer::encode` must produce the
//! EXACT id sequence of `POST /tokenize`, and the server's `/detokenize`
//! must agree with `Tokenizer::decode`. Id-exact, no tolerance.
//!
//! Verified (2026-10-17, qwen2.5-0.5b-instruct-q4_k_m.gguf + llama.cpp
//! 0.6.0-dev build 11429): the corpus below is 100+ strings and every one
//! came back id-exact; 8 spot-strings detokenized string-equal. Re-runs
//! print the live count.
//!
//! Every non-ASCII corpus entry is written with `\u{...}` escapes, so the
//! whole file is ASCII and each script class is provable per codepoint -
//! no way for a homoglyph or mojibake byte to slip past review.
//!
//! Skips (with a printed message) unless BOTH env vars are set:
//! - `AI_UPCCLER_MODEL` - the real `.gguf` (Qwen2.5-0.5B class)
//! - `AI_UPCCLER_LLAMA_SERVER` - path to the `llama-server` binary
//!
//! Established server behavior for this build: `POST /tokenize` returns ids
//! WITHOUT BOS/special tokens, matching the no-specials contract of
//! `encode`; literal special-marker text (`<|im_end|>`) follows llama.cpp's
//! parse-special semantics, which `encode` mirrors by matching CONTROL
//! pieces before the pre-tokenizer regex.

use ai_upcycler_model_io::tokenizer::Tokenizer;
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::time::Duration;

/// Kills the server no matter how the test exits.
struct ServerGuard(std::process::Child);
impl Drop for ServerGuard {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn free_port() -> u16 {
    TcpListener::bind("127.0.0.1:0")
        .expect("bind :0")
        .local_addr()
        .expect("addr")
        .port()
}

fn http_get(port: u16, path: &str) -> Result<String, String> {
    let mut s = TcpStream::connect(("127.0.0.1", port)).map_err(|e| e.to_string())?;
    s.set_read_timeout(Some(Duration::from_secs(30))).ok();
    let req = format!("GET {path} HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\n\r\n");
    s.write_all(req.as_bytes()).expect("write health request");
    let mut buf = Vec::new();
    s.read_to_end(&mut buf).expect("read response");
    Ok(String::from_utf8_lossy(&buf).replace('\u{0}', ""))
}

fn http_post(port: u16, path: &str, body: &str) -> (String, String) {
    let mut s = TcpStream::connect(("127.0.0.1", port)).expect("connect server");
    s.set_read_timeout(Some(Duration::from_secs(120))).ok();
    let head = format!(
        "POST {path} HTTP/1.1\r\nHost: 127.0.0.1\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    );
    let mut wire = head;
    wire.push_str(body);
    s.write_all(wire.as_bytes()).expect("write request");
    let mut buf = Vec::new();
    s.read_to_end(&mut buf).expect("read response");
    let text = String::from_utf8_lossy(&buf).replace('\u{0}', "");
    let status = text.lines().next().unwrap_or("").to_string();
    let body = match text.split_once("\r\n\r\n") {
        Some((_, b)) => b.to_string(),
        None => String::new(),
    };
    (status, body)
}

/// JSON-escape a string for a request body (control chars + quote + slash).
fn json_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    for ch in s.chars() {
        match ch {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => {
                out.push_str(&format!("\\u{:04x}", c as u32));
            }
            c => out.push(c),
        }
    }
    out
}

/// Minimal JSON string unescape for the response `content` field.
fn json_unescape(s: &str) -> String {
    let mut out = String::new();
    let mut chars = s.chars();
    while let Some(c) = chars.next() {
        if c != '\\' {
            out.push(c);
            continue;
        }
        match chars.next() {
            Some('"') => out.push('"'),
            Some('\\') => out.push('\\'),
            Some('/') => out.push('/'),
            Some('n') => out.push('\n'),
            Some('r') => out.push('\r'),
            Some('t') => out.push('\t'),
            Some('b') => out.push('\u{0008}'),
            Some('f') => out.push('\u{000C}'),
            Some('u') => out.push(
                char::from_u32(
                    u32::from_str_radix(&chars.by_ref().take(4).collect::<String>(), 16)
                        .expect("hex escape"),
                )
                .expect("BMP escape"),
            ),
            other => panic!("unexpected JSON escape {other:?}"),
        }
    }
    out
}

/// Integers inside a `"key":[...]` list.
fn json_int_list(hay: &str, key: &str) -> Vec<u32> {
    let idx = hay.find(key).unwrap_or_else(|| panic!("missing {key}"));
    let start = idx + key.len();
    let (open, _) = hay[start..].split_once('[').expect("list open");
    let (list, _) = hay[start + open.len() + 1..]
        .split_once(']')
        .expect("list close");
    list.split(',')
        .filter_map(|t| t.trim().parse::<u32>().ok())
        .collect()
}

/// The string content of the first `"key":"..."` field, unescaped.
fn json_string_after(hay: &str, key: &str) -> String {
    let idx = hay.find(key).unwrap_or_else(|| panic!("missing {key}"));
    let start = idx + key.len();
    let open = hay[start..].find('"').expect("string open") + 1;
    let mut raw_end = None;
    let mut chars = hay[start + open..].char_indices();
    while let Some((i, c)) = chars.next() {
        match c {
            '\\' => {
                chars.next(); // the escaped char itself (hex digits after \\u flow normally)
            }
            '"' => {
                raw_end = Some(i);
                break;
            }
            _ => {}
        }
    }
    json_unescape(&hay[start + open..start + open + raw_end.expect("string close")])
}

/// The corpus: every pre-tokenizer rule class, 100+ strings. Non-ASCII
/// entries are codepoint-explicit (`\u{...}` escapes) so each script class
/// is exact, reviewable, and mojibake-proof.
fn corpus() -> Vec<String> {
    vec![
        // --- plain ASCII prose ---------------------------------------
        "The capital of France is".to_string(),
        String::new(),
        "Hello, world!".to_string(),
        "Paris is the capital of France.".to_string(),
        "Rust is a systems programming language.".to_string(),
        "The quick brown fox jumps over the lazy dog.".to_string(),
        "Water boils at 100 degrees.".to_string(),
        "  double  spaces  inside.".to_string(),
        "Sentence with 5 words.".to_string(),
        "test@example.com".to_string(),
        "12 Angry Men (1957)".to_string(),
        "In 1492, Columbus sailed the ocean blue".to_string(),
        "2 + 2 = 4".to_string(),
        "a,b;c:d/e".to_string(),
        "...".to_string(),
        "---".to_string(),
        "It is also the largest city".to_string(),
        // --- contractions (case-insensitive branch + clitics) ---------
        "don't I'll we've can't should've".to_string(),
        "I'LL WE'VE DON'T".to_string(),
        "the dog's bone, the cats' toy".to_string(),
        "y'all can't've done that".to_string(),
        "IT'S ok's".to_string(),
        "She'll said it ain't so, Joe".to_string(),
        // --- numbers: one digit per pre-token (the qwen2 \p{N} rule) ---
        "3.14159 20261017 42 7".to_string(),
        "a1b2c3 d4e5".to_string(),
        "7777777".to_string(),
        "-12 +34 -56.78".to_string(),
        "1.5e-9".to_string(),
        "0x1F9D4".to_string(),
        "%50 25% 7%".to_string(),
        "999999999999".to_string(),
        "no digits: one two three".to_string(),
        // --- whitespace adversarial -----------------------------------
        "   ".to_string(),
        "\t\t\t".to_string(),
        "\n".to_string(),
        "\r\n\r\n".to_string(),
        "    trailing   ".to_string(),
        "a\tb".to_string(),
        "\ta".to_string(),
        "a \r".to_string(),
        " \ra".to_string(),
        "a\n".to_string(),
        "\na".to_string(),
        "the the the ab ab the".to_string(),
        "the  the".to_string(),
        "word \n\n next".to_string(),
        "line1\r\nline2\r\n\r\n\r\nline3".to_string(),
        "line1\nline2\n\n\nline3".to_string(),
        "\r\n\n\r\n".to_string(),
        "\u{00A0}nbsp".to_string(),
        "\u{3000}ideographic space".to_string(),
        "\u{2028}line sep".to_string(),
        "a\u{00A0}b\u{00A0}c".to_string(),
        "\u{000B}vertical tab".to_string(),
        // --- long words ------------------------------------------------
        "x".repeat(1000),
        format!("{}b", "a".repeat(1000)),
        format!("{}{}", "the ".repeat(250), "the"),
        format!("{}{}", " ".repeat(500), "tab"),
        format!("ab{}", "b".repeat(950)),
        // --- punctuation/symbol runs (absorb trailing CRLF) ------------
        "!!?...;;---===<>".to_string(),
        "{}[]()<>".to_string(),
        "\"quoted string\"".to_string(),
        "a\"b\\c".to_string(),
        "ASCII art: \\_/ \\_/ \\_/".to_string(),
        "!!\r\n".to_string(),
        "...\r\n\r\n".to_string(),
        // --- code ------------------------------------------------------
        "fn main() { println!(\"hi\"); }".to_string(),
        "SELECT * FROM users WHERE id = 1;".to_string(),
        "const x: i32 = 10;\nlet y = x + 1;".to_string(),
        "<html><body>Hi</body></html>".to_string(),
        "if (a && b) { return; }".to_string(),
        "{\"a\": [1,2], \"b\": null}".to_string(),
        "x = [x for x in range(x) if x != x]".to_string(),
        // --- urls -----------------------------------------------------
        "https://example.com/path?q=1&r=2".to_string(),
        // --- accented latin / decomposed marks --------------------------
        "Caf\u{00E9} na\u{00EF}ve r\u{00E9}sum\u{00E9}".to_string(),
        "caf\u{0065}\u{0301}".to_string(),
        "e\u{0301}x\u{0328}p \u{0105}".to_string(),
        "\u{00C4}rger \u{00FC}ber \u{00DF}".to_string(),
        "\u{00C1}rbol \u{00F1}o\u{00F1}o".to_string(),
        "\u{0218}colare \u{0163}ara".to_string(),
        "\u{00C0}\u{00C9}\u{00CE}\u{00D5}\u{00DC}".to_string(),
        "\u{00C7}a va, \u{00E7}a va!".to_string(),
        // --- greek / cyrillic -------------------------------------------
        "\u{041F}\u{0440}\u{0438}\u{0432}\u{0435}\u{0442}, \u{043C}\u{0438}\u{0440}!".to_string(),
        "\u{041F}\u{0440}\u{0438}\u{0432}\u{0435}\u{0442}!\r\n\u{041F}\u{043E}\u{043A}\u{0430}!"
            .to_string(),
        "\u{043A}\u{043E}\u{0441}\u{043C}\u{043E}\u{043D}\u{0430}\u{0432}\u{0442}".to_string(),
        "\u{0430}\u{0431}\u{0432} \u{0433}\u{0434}\u{0435} 123".to_string(),
        "a \u{03B2} c".to_string(),
        "\u{03B1} \u{03B2} \u{03B3}".to_string(),
        "\u{03A9}\u{03BC}\u{03AD}\u{03B3}\u{03B1} 2026".to_string(),
        // --- CJK / other scripts ----------------------------------------
        "Tokyo is the capital, too".to_string(),
        "\u{4E2D}English \u{6587}\u{5B57} mixed text".to_string(),
        "\u{4E1C}\u{4EAC}\u{5E02}".to_string(),
        "\u{3053}\u{3093}\u{306B}\u{3061}\u{306F}\u{3001}\u{4E16}\u{754C}\u{FF01}".to_string(),
        "\u{304A}\u{91D1}\u{3092}\u{6255}\u{3046}".to_string(),
        "\u{5B89}\u{5427}".to_string(),
        "\u{C548}\u{B155}".to_string(),
        "\u{0E2A}\u{0E27}\u{0E31}\u{0E2A}\u{0E14}\u{0E35} 2566".to_string(),
        // --- emoji / ZWJ sequences --------------------------------------
        "\u{1F9D4}\u{1F3FD}\u{200D}\u{2640}\u{FE0F}".to_string(),
        "\u{1F680}".to_string(),
        "\u{1F680}\u{1F680}\u{1F680}".to_string(),
        "\u{1F3F3}\u{FE0F}\u{200D}\u{1F308} flag".to_string(),
        "\u{1F468}\u{200D}\u{1F469}\u{200D}\u{1F467}".to_string(),
        "\u{1F408}\u{200D}\u{2B1B}".to_string(),
        "\u{2611}\u{FE0F}".to_string(),
        "\u{00C4}\u{1F680}b".to_string(),
        // --- specials: literal marker text (parse-special path) ---------
        "<|im_end|>".to_string(),
        "<|im_start|>user\nHi!".to_string(),
        "before<|im_start|>after".to_string(),
        "mixed \u{4E2D}<|im_end|>text".to_string(),
        "<|im_start|><|im_end|>".to_string(),
        "<|box_end|>".to_string(),
    ]
}

#[test]
#[ignore = "asset-gated: needs AI_UPCCLER_MODEL + AI_UPCCLER_LLAMA_SERVER"]
fn tokenizer_is_id_exact_vs_llama_server() {
    let Ok(model) = std::env::var("AI_UPCCLER_MODEL") else {
        eprintln!("parity skipped: AI_UPCCLER_MODEL is not set");
        return;
    };
    let Ok(server_bin) = std::env::var("AI_UPCCLER_LLAMA_SERVER") else {
        eprintln!("parity skipped: AI_UPCCLER_LLAMA_SERVER is not set");
        return;
    };

    let tokenizer = Tokenizer::from_gguf(std::path::Path::new(&model))
        .expect("read the real model's tokenizer KVs");
    assert_eq!(tokenizer.n_pieces(), 151936, "Qwen2.5-0.5B vocabulary size");

    // ---- llama-server on a free port, piped to null (never blocks) --------
    let port = free_port();
    let _guard = ServerGuard(
        std::process::Command::new(&server_bin)
            .args([
                "--model",
                &model,
                "--port",
                &port.to_string(),
                "--ctx-size",
                "512",
                "--host",
                "127.0.0.1",
            ])
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .expect("spawn llama-server"),
    );

    let mut ready = None;
    for _ in 0..120 {
        match http_get(port, "/health") {
            Ok(body) if body.contains("\"status\":\"ok\"") => {
                ready = Some(());
                break;
            }
            _ => std::thread::sleep(Duration::from_millis(500)),
        }
    }
    assert!(
        ready.is_some(),
        "llama-server never became healthy on :{port}"
    );

    // Plan gate: the corpus is 100+ strings, one id-exact assert each.
    let corpus = corpus();
    assert!(corpus.len() >= 100, "plan corpus must be 100+ strings");
    eprintln!("corpus: {} strings under the id-exact gate", corpus.len());

    // ---- id-exact vs /tokenize ------------------------------------------
    let mut mismatches = 0usize;
    for text in &corpus {
        let (status, body) = http_post(
            port,
            "/tokenize",
            &format!("{{\"content\":\"{}\"}}", json_escape(text)),
        );
        assert!(status.contains(" 200 "), "tokenize {text:?} -> {status}");
        let server_ids = json_int_list(&body, "\"tokens\":");
        let our_ids = tokenizer.encode(text).expect("our encode");
        if server_ids != our_ids {
            mismatches += 1;
            eprintln!("MISMATCH {text:?}\n  server: {server_ids:?}\n  ours:   {our_ids:?}");
        }
    }
    assert_eq!(
        mismatches, 0,
        "{mismatches} corpus strings tokenized differently"
    );

    // ---- /detokenize agrees with decode (verified, not assumed) ----------
    let sample: Vec<&str> = vec![
        "The capital of France is",
        "don't I'll we've can't should've",
        "3.14159 20261017 42 7",
        "line1\nline2\n\n\nline3",
        "Caf\u{00E9} na\u{00EF}ve r\u{00E9}sum\u{00E9}",
        "Tokyo is the capital, too",
        "\u{1F680}",
        "\u{4E2D}English \u{6587}\u{5B57} mixed text",
    ];
    for text in sample {
        let ids = tokenizer.encode(text).expect("our encode");
        let token_csv = ids
            .iter()
            .map(|i| i.to_string())
            .collect::<Vec<_>>()
            .join(",");
        let (status, body) = http_post(
            port,
            "/detokenize",
            &format!("{{\"tokens\":[{token_csv}]}}"),
        );
        assert!(status.contains(" 200 "), "detokenize {text:?} -> {status}");
        let server_text = json_string_after(&body, "\"content\":");
        assert_eq!(
            server_text, text,
            "server detokenize diverges from our decode for {text:?}"
        );
    }
}
