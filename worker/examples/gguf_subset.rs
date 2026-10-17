//! Debug tool (NOT a shipped feature): creates a truncated GGUF keeping all
//! metadata (with `qwen2.block_count` patched), all global tensors, and only
//! `blk.0.*` tensors, so the reference server and our engine can be compared on
//! a reduced model. Usage: gguf_subset <src.gguf> <dst.gguf> <new_block_count>
use std::io::{Read, Seek, SeekFrom, Write};

fn rd_u32(b: &[u8], o: usize) -> u32 {
    u32::from_le_bytes(b[o..o + 4].try_into().unwrap())
}
fn rd_u64(b: &[u8], o: usize) -> u64 {
    u64::from_le_bytes(b[o..o + 8].try_into().unwrap())
}

/// ggml type layout: (block elems, block bytes) or single-element size.
fn type_layout(ty: u32) -> Option<(u64, u64)> {
    Some(match ty {
        0 => (1, 4),      // F32
        1 => (1, 2),      // F16
        2 => (32, 18),    // Q4_0
        3 => (32, 20),    // Q4_1
        6 => (32, 22),    // Q5_0
        7 => (32, 24),    // Q5_1
        8 => (32, 34),    // Q8_0
        12 => (256, 144), // Q4_K
        13 => (256, 176), // Q5_K
        14 => (256, 210), // Q6_K
        15 => (256, 84),  // Q8_K
        16 => (1, 2),     // BF16
        _ => return None,
    })
}

/// Bytes of one metadata value of type `vt`, starting at `o`. None = stop.
fn scalar_size(vt: u32, b: &[u8], o: usize) -> Option<usize> {
    Some(match vt {
        0 | 1 | 7 => 1,
        2 | 3 => 2,
        4..=6 => 4,
        8 => 8 + rd_u64(b, o) as usize, // string: u64 length + bytes
        9 => {
            let et = rd_u32(b, o);
            let n = rd_u64(b, o + 4) as usize;
            let mut off = o + 12;
            for _ in 0..n {
                off += scalar_size(et, b, off)?;
            }
            off - o
        }
        _ => return None,
    })
}

struct TensorEntry {
    name: String,
    dims: u32,
    ne: Vec<u64>,
    ty: u32,
    old_off: u64,
    bytes: u64,
}

fn tensor_bytes(ne: &[u64], ty: u32) -> Option<u64> {
    let (be, bb) = type_layout(ty)?;
    let &row = ne.first()?;
    if row % be != 0 {
        return None;
    }
    let rows: u64 = ne[1..].iter().product();
    Some((row / be) * bb * rows)
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let (src, dst, new_blocks) = (
        args[1].clone(),
        args[2].clone(),
        args[3].parse::<u32>().unwrap(),
    );

    let mut f = std::fs::File::open(&src).unwrap();
    // Read enough to cover header + metadata + tensor entries.
    let mut head = Vec::new();
    let mut take = (&mut f).take(8 * 1024 * 1024);
    take.read_to_end(&mut head).unwrap();

    assert_eq!(rd_u32(&head, 4), 3, "only GGUF v3 supported");
    let tensor_count = rd_u64(&head, 8) as usize;
    let kv_count = rd_u64(&head, 16);

    // Parse metadata; kv_end = end of the metadata block.
    let mut off = 24usize;
    for _ in 0..kv_count {
        let klen = rd_u64(&head, off) as usize;
        off += 8 + klen;
        let vt = rd_u32(&head, off);
        off += 4;
        off += scalar_size(vt, &head, off).expect("unsupported metadata value");
    }
    let kv_end = off;

    // Parse tensor entries.
    let mut entries: Vec<TensorEntry> = Vec::new();
    off = kv_end;
    for _ in 0..tensor_count {
        let klen = rd_u64(&head, off) as usize;
        let name =
            String::from_utf8(head[off + 8..off + 8 + klen].to_vec()).expect("tensor name utf8");
        off += 8 + klen;
        let dims = rd_u32(&head, off);
        off += 4;
        let mut ne = Vec::new();
        for _ in 0..dims {
            ne.push(rd_u64(&head, off));
            off += 8;
        }
        let ty = rd_u32(&head, off);
        off += 4;
        let old_off = rd_u64(&head, off);
        off += 8;
        let bytes = tensor_bytes(&ne, ty).expect("unknown type layout");
        entries.push(TensorEntry {
            name,
            dims,
            ne,
            ty,
            old_off,
            bytes,
        });
    }
    let tensor_info_end = off;
    let alignment = 32u64;

    // Select tensors: global (not blk.N) + blk.{0..new_blocks-1}.*
    let kept_block = |name: &str| -> bool {
        if let Some(rest) = name.strip_prefix("blk.") {
            match rest.split_once('.') {
                Some((idx, _)) => match idx.parse::<u32>() {
                    Ok(i) => i < new_blocks,
                    Err(_) => false,
                },
                None => false,
            }
        } else {
            true
        }
    };
    // `--f32` also dequantizes every kept tensor and stores it as F32 (type 0)
    // — weights identical to what the engine computes, so llama-server reads
    // exactly our values.
    let to_f32 = args.get(4).map(|a| a == "--f32").unwrap_or(false);
    let keep: Vec<&TensorEntry> = entries.iter().filter(|e| kept_block(&e.name)).collect();
    for e in &keep {
        if !e.name.starts_with("blk.") {
            println!("global tensor: {} ne {:?} ty {}", e.name, e.ne, e.ty);
        }
    }

    // Patch block_count in a metadata copy.
    let mut meta = head[24..kv_end].to_vec();
    let key = b"qwen2.block_count";
    let mut found = false;
    for i in 0..meta.len().saturating_sub(key.len()) {
        if &meta[i..i + key.len()] == key {
            let vo = i + key.len() + 4; // skip value-type u32 before value
            meta[vo..vo + 4].copy_from_slice(&new_blocks.to_le_bytes());
            found = true;
            break;
        }
    }
    assert!(found, "qwen2.block_count not found in metadata");

    // Write header with new tensor entries and fresh offsets.
    let mut w = std::fs::File::create(&dst).unwrap();
    let mut hdr = Vec::new();
    hdr.extend_from_slice(&[0x47, 0x47, 0x55, 0x46]); // "GGUF"
    hdr.extend_from_slice(&3u32.to_le_bytes());
    hdr.extend_from_slice(&(keep.len() as u64).to_le_bytes());
    hdr.extend_from_slice(&kv_count.to_le_bytes());
    hdr.extend_from_slice(&meta);
    let mut data_off = 0u64;
    let mut new_offs = Vec::new();
    for e in &keep {
        let (wty, wbytes) = if to_f32 {
            (0u32, e.ne.iter().product::<u64>() * 4)
        } else {
            (e.ty, e.bytes)
        };
        new_offs.push(data_off);
        let nb = e.name.as_bytes();
        hdr.extend_from_slice(&(nb.len() as u64).to_le_bytes());
        hdr.extend_from_slice(nb);
        hdr.extend_from_slice(&e.dims.to_le_bytes());
        for d in &e.ne {
            hdr.extend_from_slice(&d.to_le_bytes());
        }
        hdr.extend_from_slice(&wty.to_le_bytes());
        let aligned = data_off.div_ceil(alignment) * alignment;
        hdr.extend_from_slice(&aligned.to_le_bytes());
        data_off = aligned + wbytes;
    }
    w.write_all(&hdr).unwrap();
    let data_start = w.stream_position().unwrap().div_ceil(alignment) * alignment;
    loop {
        w.write_all(&[0]).unwrap();
        if w.stream_position().unwrap() == data_start {
            break;
        }
    }
    // Copy tensor data. Tensor offsets are relative to the ALIGNED start of
    // the data region, not the raw end of the tensor-info block.
    let src_data_start = tensor_info_end.div_ceil(alignment as usize) * alignment as usize;
    eprintln!(
        "data start {src_data_start}, {len} kept of {tensor_count}",
        len = keep.len()
    );
    let src_data_start = src_data_start as u64;
    let mut buf = vec![0u8; 1 << 20];
    for (e, new) in keep.iter().zip(&new_offs) {
        f.seek(SeekFrom::Start(src_data_start + e.old_off)).unwrap();
        w.seek(SeekFrom::Start(data_start + *new)).unwrap();
        if to_f32 {
            let mut raw = vec![0u8; e.bytes as usize];
            f.read_exact(&mut raw).unwrap();
            let ty = ai_upcycler_worker::quant::GgmlType::from_id(e.ty)
                .unwrap_or_else(|| panic!("unknown ggml type {} for {}", e.ty, e.name));
            let name2 = e.name.clone();
            let vals = ai_upcycler_worker::quant::dequantize(ty, &raw, &e.ne)
                .unwrap_or_else(|err| panic!("dequant {name2}: {err}"));
            let mut bw = std::io::BufWriter::with_capacity(8 << 20, &mut w);
            for v in &vals {
                bw.write_all(&v.to_le_bytes()).unwrap();
            }
            bw.flush().unwrap();
            println!("f32 {} ({} vals @ {})", e.name, vals.len(), new);
        } else {
            let mut left = e.bytes;
            while left > 0 {
                let n = f.read(&mut buf).unwrap().min(left as usize);
                if n == 0 {
                    panic!("EOF reading {}", e.name);
                }
                w.write_all(&buf[..n]).unwrap();
                left -= n as u64;
            }
            println!("copied {} ({} B @ {})", e.name, e.bytes, new);
        }
    }
    println!("done: {} tensors, data end {}", keep.len(), data_off);
}
