//! Debug probe (not a test): lists ggml type id usage per tensor part across
//! all blocks of the real model, so we know which dequant paths feed parity.
use ai_upcycler_worker::gguf::GgufFile;
use std::path::Path;

fn main() {
    let model = std::env::var("AI_UPCCLER_MODEL").expect("AI_UPCCLER_MODEL");
    let f = GgufFile::open(Path::new(&model)).expect("open");
    // name pattern -> set of type ids
    let mut table: std::collections::BTreeMap<String, Vec<u32>> = Default::default();
    for name in f.tensor_names() {
        let desc = f.tensor(name).unwrap();
        let key = name
            .split('.')
            .enumerate()
            .filter(|(i, part)| !(part.chars().all(|c| c.is_ascii_digit()) && (*i == 1 || *i == 2)))
            .map(|(_i, part)| part)
            .collect::<Vec<_>>()
            .join(".");
        let types = table.entry(key).or_default();
        if !types.contains(&desc.ggml_type) {
            types.push(desc.ggml_type);
        }
    }
    for (name, types) in table {
        println!("{} types {:?}", name, types);
    }
}
