//! Worker binary: serve `WorkerService` on a loopback port.
//!
//! `ai-upcycler-worker --model <path> [--bind 127.0.0.1:0] [--worker-id host-worker]`
//!
//! Prints exactly one machine-readable line to stdout once listening:
//! `READY <ip:port>` (tests and future master orchestration parse it).

use ai_upcycler_worker::engine::WorkerEngine;
use ai_upcycler_worker::proto::aiupcycler::rpc::worker_service_server::WorkerServiceServer;
use std::net::SocketAddr;
use std::path::PathBuf;
use tokio_stream::wrappers::TcpListenerStream;
use tonic::transport::Server;

struct Args {
    model: PathBuf,
    bind: SocketAddr,
    worker_id: String,
}

fn parse_args() -> Result<Args, String> {
    let mut model: Option<PathBuf> = None;
    let mut bind = "127.0.0.1:0".to_string();
    let mut worker_id = "host-worker".to_string();

    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--model" => model = Some(PathBuf::from(args.next().ok_or("--model requires a path")?)),
            "--bind" => bind = args.next().ok_or("--bind requires an address")?,
            "--worker-id" => worker_id = args.next().ok_or("--worker-id requires a value")?,
            other => return Err(format!("unknown argument: {other}")),
        }
    }

    let model = model.ok_or("--model <path> is required")?;
    let bind: SocketAddr = bind
        .parse()
        .map_err(|e| format!("invalid --bind address: {e}"))?;
    Ok(Args {
        model,
        bind,
        worker_id,
    })
}

#[tokio::main]
async fn main() {
    let args = match parse_args() {
        Ok(args) => args,
        Err(err) => {
            eprintln!("usage: ai-upcycler-worker --model <path> [--bind 127.0.0.1:0] [--worker-id host-worker]");
            eprintln!("error: {err}");
            std::process::exit(2);
        }
    };

    let engine = match WorkerEngine::new(&args.worker_id, "", args.model) {
        Ok(engine) => engine,
        Err(err) => {
            eprintln!("{err}");
            std::process::exit(1);
        }
    };

    let listener = tokio::net::TcpListener::bind(args.bind)
        .await
        .expect("bind failed");
    let addr = listener.local_addr().expect("local addr");
    println!("READY {addr}");
    use std::io::Write;
    let _ = std::io::stdout().flush();

    let result = Server::builder()
        .add_service(WorkerServiceServer::new(engine))
        .serve_with_incoming(TcpListenerStream::new(listener))
        .await;
    if let Err(err) = result {
        eprintln!("worker server error: {err}");
        std::process::exit(1);
    }
}
