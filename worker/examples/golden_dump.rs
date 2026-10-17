//! Regenerates the dequant golden-vector fixtures under `tests/goldens/`
//! (consumed by `tests/dequant_golden.rs`).
//!
//! Provenance: the vectors come straight from the production model file
//! pointed at via the `AI_UPCCLER_MODEL` environment variable — the same
//! file the engine's parity and split runs use. The dequantization ports
//! were verified line-by-line against llama.cpp `ggml-quants.c` before
//! these vectors were cut, so the committed fixtures are *regression
//! tripwires* (they catch later edits that change dequantized bits), not
//! an independent substitute for that source verification.
//!
//! Usage:
//! `AI_UPCCLER_MODEL=<model.gguf> cargo run -p ai-upcycler-worker \
//!   --example golden_dump -- worker/tests/goldens`

use std::error::Error;
use std::path::PathBuf;

use ai_upcycler_worker::gguf::GgufFile;
use ai_upcycler_worker::quant::{dequantize, GgmlType};

/// (type, ggml id, elements per block, fixture file, records to emit).
/// K-quants take 3 tensors x 2 blocks, legacy types 2 tensors x 2 blocks,
/// so the fixtures stay a few KiB while spanning several layers.
const TARGETS: &[(GgmlType, u32, usize, &str, usize)] = &[
    (GgmlType::Q5_0, 6, 32, "q5_0.txt", 4),
    (GgmlType::Q8_0, 8, 32, "q8_0.txt", 4),
    (GgmlType::Q4_K, 12, 256, "q4_k.txt", 6),
    (GgmlType::Q6_K, 14, 256, "q6_k.txt", 6),
];

fn main() -> Result<(), Box<dyn Error>> {
    let model_path = std::env::var("AI_UPCCLER_MODEL")
        .map_err(|_| "AI_UPCCLER_MODEL is not set; point it at the gguf to cut vectors from")?;
    let out_dir = PathBuf::from(
        std::env::args()
            .nth(1)
            .unwrap_or_else(|| "worker/tests/goldens".into()),
    );

    let file = GgufFile::open(model_path.as_ref())?;
    std::fs::create_dir_all(&out_dir)?;
    let model_name = PathBuf::from(&model_path)
        .file_name()
        .expect("model path has a file name")
        .to_string_lossy()
        .into_owned();

    // Sorted for a deterministic tensor choice (the reader stores both
    // tensor names and records in map order).
    let mut names: Vec<String> = file.tensor_names().cloned().collect();
    names.sort();

    for (ty, id, elems, fname, want) in TARGETS {
        let mut body = String::new();
        'tensors: for name in &names {
            let desc = file.tensor(name).expect("name comes from the reader");
            if desc.ggml_type != *id {
                continue;
            }
            let elems_u64 = *elems as u64;
            let per_row = desc.ne[0];
            let total_elems: u64 = desc.ne.iter().product();
            if per_row < elems_u64 || per_row % elems_u64 != 0 || total_elems < 2 * elems_u64 {
                continue; // too small for a first + middle block pick
            }
            let raw = file.read_raw(name)?;
            let blocks = total_elems / elems_u64;
            let block_bytes = raw.len() as u64 / blocks;
            let dequantized =
                dequantize(*ty, &raw, &desc.ne).map_err(|e| format!("{name}: {e}"))?;

            for &block in &[0u64, blocks / 2] {
                let start = (block * elems_u64) as usize;
                let span = &dequantized[start..start + *elems];
                let raw_start = (block * block_bytes) as usize;
                let raw_span = &raw[raw_start..raw_start + block_bytes as usize];

                let mut line = format!("{name}|{block}|");
                for &b in raw_span {
                    line.push_str(&format!("{b:02x}"));
                }
                line.push('|');
                for (i, f) in span.iter().enumerate() {
                    if i > 0 {
                        line.push(',');
                    }
                    line.push_str(&format!("{:08x}", f.to_bits()));
                }
                body.push_str(&line);
                body.push('\n');

                if body.lines().count() >= *want {
                    break 'tensors;
                }
            }
        }
        if body.lines().count() < *want {
            return Err(format!(
                "model {model_name} produced only {} records for {ty:?}, wanted {want}",
                body.lines().count()
            )
            .into());
        }

        let mut text = format!(
            "# Dequant golden vectors for {ty:?} — regression tripwires.\n\
             # Source model: {model_name}\n\
             # Regenerate: AI_UPCCLER_MODEL=<model.gguf> cargo run -p ai-upcycler-worker \
             --example golden_dump -- worker/tests/goldens\n\
             # Format: tensor|block_index|raw_block_hex|expected_f32_bit_patterns_comma\n"
        );
        text.push_str(&body);
        std::fs::write(out_dir.join(fname), text)?;
        println!("wrote {} records → {}", body.lines().count(), fname);
    }
    Ok(())
}
