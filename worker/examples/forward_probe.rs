//! Debug probe (NOT a shipped feature): runs the engine's full-range forward
//! in-process and prints the log-softmax top-k for the last row, for parity
//! bisects. Usage: forward_probe <model.gguf> [tok,tok,...] [-k N]
use ai_upcycler_worker::engine::WorkerEngine;
use ai_upcycler_worker::proto::aiupcycler::rpc::tensor_buffer::DataType;
use ai_upcycler_worker::proto::aiupcycler::rpc::worker_service_server::WorkerService;
use ai_upcycler_worker::proto::aiupcycler::rpc::{
    ExecuteForwardRequest, LoadLayersRequest, TensorBuffer,
};
use std::path::{Path, PathBuf};
use tonic::Request;

fn f32_tb(shape: &[i64], vals: &[f32]) -> TensorBuffer {
    debug_assert_eq!(shape.iter().product::<i64>() as usize, vals.len());
    TensorBuffer {
        shape: shape.to_vec(),
        data_type: DataType::Fp32 as i32,
        raw_data: vals.iter().flat_map(|v| v.to_le_bytes()).collect(),
    }
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let model = args[1].clone();
    let mut ids: Vec<u32> = vec![785];
    let mut k = 5usize;
    let mut dump: Option<String> = None; // "token_embd:785"
    let mut i = 2;
    while i < args.len() {
        if args[i] == "--dump" {
            dump = Some(args[i + 1].clone());
            i += 2;
        } else if args[i] == "-k" {
            k = args[i + 1].parse().unwrap();
            i += 2;
        } else {
            ids = args[i]
                .split(',')
                .map(|s| s.trim().parse().unwrap())
                .collect();
            i += 1;
        }
    }
    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .unwrap();
    rt.block_on(async move {
        let engine = WorkerEngine::new("probe", "debug", PathBuf::from(&model)).unwrap();
        let file = ai_upcycler_worker::gguf::GgufFile::open(Path::new(&model)).unwrap();
        if let Some(d) = dump {
            let (name, part) = d.split_once(':').expect("dump name:row");
            let row: usize = part.parse().expect("row");
            let desc = file.tensor(name).expect("tensor");
            let raw = file.read_raw(name).expect("read raw");
            let ty = ai_upcycler_worker::quant::GgmlType::from_id(desc.ggml_type).expect("type");
            let out = ai_upcycler_worker::quant::dequantize(ty, &raw, &desc.ne).expect("dequant");
            let ne0 = desc.ne[0] as usize;
            let ro = &out[row * ne0..(row + 1) * ne0];
            let max = ro.iter().copied().fold(f32::NEG_INFINITY, f32::max);
            let min = ro.iter().copied().fold(f32::INFINITY, f32::min);
            let first5: Vec<f32> = ro[..5].to_vec();
            println!(
                "dump {name}[{row}]: max {max} min {min} first5 {first5:?} dtype {}",
                desc.ggml_type
            );
            return;
        }
        let file = ai_upcycler_worker::gguf::GgufFile::open(Path::new(&model)).unwrap();
        let model_id = file.model_name().to_string();
        let last = file.hparams().block_count as i32 - 1;
        let n_embd = file.hparams().embedding_length as usize;
        let vocab = file.tensor("token_embd.weight").unwrap().ne[1] as usize;
        let resp = engine
            .load_layers(Request::new(LoadLayersRequest {
                model_id,
                start_layer: 0,
                end_layer: last,
                weights_chunk_data: Vec::new(),
            }))
            .await
            .unwrap();
        assert!(resp.into_inner().success, "load refused");
        let n = ids.len();
        let resp = engine
            .execute_forward(Request::new(ExecuteForwardRequest {
                session_id: "probe".into(),
                start_layer: 0,
                end_layer: last,
                activation_tensor: Some(f32_tb(
                    &[1, n as i64],
                    &ids.iter().map(|&i| i as f32).collect::<Vec<_>>(),
                )),
                sequence_position: 0,
                last_logits_only: false,
            }))
            .await
            .unwrap();
        let r = resp.into_inner();
        assert!(r.success, "forward refused");
        let tb = r.output_activation_tensor.unwrap();
        let got: Vec<f32> = tb
            .raw_data
            .as_chunks::<4>()
            .0
            .iter()
            .map(|c| f32::from_le_bytes(*c))
            .collect();
        let row = &got[(n - 1) * vocab..n * vocab];
        let max = row.iter().copied().fold(f32::NEG_INFINITY, f32::max);
        let lse = max as f64
            + row
                .iter()
                .map(|&v| (v as f64 - max as f64).exp())
                .sum::<f64>()
                .ln();
        let nan_count = row.iter().filter(|v| v.is_nan()).count();
        eprintln!("row stats: max {max} nan_count {nan_count} n {}", row.len());
        let mut ranking: Vec<usize> = (0..row.len()).collect();
        ranking.sort_by(|&a, &b| row[b].total_cmp(&row[a]));
        for (i, &tid) in ranking[..k].iter().enumerate() {
            println!(
                "probe top{}: id {} lp {:.6}",
                i + 1,
                tid,
                row[tid] as f64 - lse
            );
        }
        let _ = n_embd;
    });
}
