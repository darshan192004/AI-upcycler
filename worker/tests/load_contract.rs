//! LoadLayers/UnloadLayers contract tests against a real in-process gRPC
//! server (plan Task 3): allocation follows the ownership rule, refusals
//! leave no residue, unloads report exact bytes.

mod common;

use ai_upcycler_worker::engine::WorkerEngine;
use ai_upcycler_worker::proto::aiupcycler::rpc::{
    worker_service_client::WorkerServiceClient, LoadLayersRequest, UnloadLayersRequest,
};

type Client = WorkerServiceClient<tonic::transport::Channel>;

async fn connect(fx: &common::Fixture) -> Client {
    let engine =
        WorkerEngine::new("contract-1", "test-device", fx.path.clone()).expect("engine parses");
    let (port, _server) = common::serve_engine(engine).await;
    WorkerServiceClient::connect(format!("http://127.0.0.1:{port}"))
        .await
        .expect("connect")
}

fn load(model_id: &str, start: i32, end: i32) -> LoadLayersRequest {
    LoadLayersRequest {
        model_id: model_id.to_string(),
        start_layer: start,
        end_layer: end,
        weights_chunk_data: Vec::new(),
    }
}

#[tokio::test]
async fn load_full_range_allocates_expected_and_unload_reports_same() {
    let dir = tempfile::tempdir().unwrap();
    let fx = common::write_tiny_model(dir.path(), common::TinyModelConfig::default());
    let mut client = connect(&fx).await;

    let resp = client
        .load_layers(load(&fx.model_id, 0, 3))
        .await
        .expect("rpc")
        .into_inner();
    assert!(
        resp.success,
        "full load must succeed: {}",
        resp.error_message
    );

    // Two independent expressions must agree: the fixture's derived table
    // and the literal recomputation from the spec's tensor dims.
    let from_table = fx.expected_f32_bytes(0, 3);
    let inline = (256 * 64
        + 4 * (64
            + 16
            + 16
            + 64 * 64
            + 32 * 64
            + 32 * 64
            + 64 * 64
            + 64
            + 128 * 64
            + 128 * 64
            + 64 * 128)
        + 64)
        * 4;
    assert_eq!(resp.allocated_ram_bytes, from_table as i64);
    assert_eq!(resp.allocated_ram_bytes, inline, "inline must match table");

    let unload = client
        .unload_layers(UnloadLayersRequest {
            model_id: fx.model_id.clone(),
        })
        .await
        .expect("rpc")
        .into_inner();
    assert!(unload.success, "unload: {}", unload.error_message);
    assert_eq!(unload.freed_ram_bytes, resp.allocated_ram_bytes);
}

#[tokio::test]
async fn partial_load_includes_embd_but_not_head() {
    let dir = tempfile::tempdir().unwrap();
    let fx = common::write_tiny_model(dir.path(), common::TinyModelConfig::default());
    let mut client = connect(&fx).await;

    let resp = client
        .load_layers(load(&fx.model_id, 0, 1))
        .await
        .expect("rpc")
        .into_inner();
    assert!(resp.success, "{}", resp.error_message);
    // start==0 owns token_embd; end!=n_layers-1 owns no head tensors.
    assert_eq!(resp.allocated_ram_bytes, fx.expected_f32_bytes(0, 1) as i64);
}

#[tokio::test]
async fn wrong_model_id_is_refused_and_leaves_no_residue() {
    let dir = tempfile::tempdir().unwrap();
    let fx = common::write_tiny_model(dir.path(), common::TinyModelConfig::default());
    let mut client = connect(&fx).await;

    let refused = client
        .load_layers(load("other-model", 0, 3))
        .await
        .expect("rpc")
        .into_inner();
    assert!(!refused.success);
    assert!(!refused.error_message.is_empty());
    assert_eq!(refused.allocated_ram_bytes, 0);

    let ok = client
        .load_layers(load(&fx.model_id, 0, 3))
        .await
        .expect("rpc")
        .into_inner();
    assert!(ok.success, "{}", ok.error_message);
    assert_eq!(
        ok.allocated_ram_bytes,
        fx.expected_f32_bytes(0, 3) as i64,
        "refusal must not retain any bytes"
    );
}

#[tokio::test]
async fn out_of_range_and_inverted_ranges_are_refused() {
    let dir = tempfile::tempdir().unwrap();
    let fx = common::write_tiny_model(dir.path(), common::TinyModelConfig::default());
    let mut client = connect(&fx).await;

    for (start, end) in [(0, 4), (2, 1)] {
        let resp = client
            .load_layers(load(&fx.model_id, start, end))
            .await
            .expect("rpc")
            .into_inner();
        assert!(!resp.success, "range {start}..{end} must be refused");
        assert!(!resp.error_message.is_empty());
        assert_eq!(resp.allocated_ram_bytes, 0);
    }
}

#[tokio::test]
async fn second_load_without_unload_is_refused() {
    let dir = tempfile::tempdir().unwrap();
    let fx = common::write_tiny_model(dir.path(), common::TinyModelConfig::default());
    let mut client = connect(&fx).await;

    let first = client
        .load_layers(load(&fx.model_id, 0, 1))
        .await
        .expect("rpc")
        .into_inner();
    assert!(first.success, "{}", first.error_message);

    let second = client
        .load_layers(load(&fx.model_id, 0, 3))
        .await
        .expect("rpc")
        .into_inner();
    assert!(!second.success, "double load must be refused");

    let unload = client
        .unload_layers(UnloadLayersRequest {
            model_id: fx.model_id.clone(),
        })
        .await
        .expect("rpc")
        .into_inner();
    assert!(unload.success);
    assert_eq!(
        unload.freed_ram_bytes, first.allocated_ram_bytes,
        "refused load must not change the retained state"
    );
}

#[tokio::test]
async fn unload_of_never_loaded_model_succeeds_with_zero_then_reload_works() {
    let dir = tempfile::tempdir().unwrap();
    let fx = common::write_tiny_model(dir.path(), common::TinyModelConfig::default());
    let mut client = connect(&fx).await;

    let fresh = client
        .unload_layers(UnloadLayersRequest {
            model_id: fx.model_id.clone(),
        })
        .await
        .expect("rpc")
        .into_inner();
    assert!(fresh.success, "fresh unload: {}", fresh.error_message);
    assert_eq!(fresh.freed_ram_bytes, 0);

    let partial = client
        .load_layers(load(&fx.model_id, 0, 1))
        .await
        .expect("rpc")
        .into_inner();
    assert!(partial.success, "{}", partial.error_message);
    let unload = client
        .unload_layers(UnloadLayersRequest {
            model_id: fx.model_id.clone(),
        })
        .await
        .expect("rpc")
        .into_inner();
    assert!(unload.success);

    // Rollback scenario: a reload after unload must not be stuck.
    let full = client
        .load_layers(load(&fx.model_id, 0, 3))
        .await
        .expect("rpc")
        .into_inner();
    assert!(full.success, "reload after unload: {}", full.error_message);
    assert_eq!(full.allocated_ram_bytes, fx.expected_f32_bytes(0, 3) as i64);
}

#[tokio::test]
async fn tied_fixture_loads_full_range() {
    let dir = tempfile::tempdir().unwrap();
    let fx = common::write_tiny_model(dir.path(), common::TinyModelConfig::default());
    let mut client = connect(&fx).await;

    // The default fixture ties word embeddings: no output.weight exists and
    // the full-range load must still succeed (lm_head = token_embd later).
    let resp = client
        .load_layers(load(&fx.model_id, 0, 3))
        .await
        .expect("rpc")
        .into_inner();
    assert!(
        resp.success,
        "tied fixture full load: {}",
        resp.error_message
    );
    assert_eq!(resp.allocated_ram_bytes, fx.expected_f32_bytes(0, 3) as i64);
}

#[tokio::test]
async fn unsupported_tensor_type_is_refused_loudly() {
    let dir = tempfile::tempdir().unwrap();
    let cfg = common::TinyModelConfig {
        quant: common::FixtureQuant::UnsupportedQ5_0,
        ..Default::default()
    };
    let fx = common::write_tiny_model(dir.path(), cfg);
    let mut client = connect(&fx).await;

    let resp = client
        .load_layers(load(&fx.model_id, 0, 3))
        .await
        .expect("rpc")
        .into_inner();
    assert!(!resp.success, "Q5_0 (type id 6) must be refused");
    assert!(
        resp.error_message.contains("ggml type id 6"),
        "message names the type id: {}",
        resp.error_message
    );
    assert_eq!(resp.allocated_ram_bytes, 0);
}
