//! Real host inference via a managed `llama-server` process (llama.cpp).
//!
//! [`LlamaCppBackend`] implements [`GenerationBackend`] by spawning the
//! prebuilt llama.cpp OpenAI-compatible server on a loopback port and
//! proxying chat completions with `stream: true`, re-emitting the server's
//! SSE `delta.content` fragments as [`GenerationChunk`]s. The first request
//! triggers a lazy spawn plus a `/health` readiness poll; the child is killed
//! when the backend drops.
//!
//! Discovery ([`LlamaCppBackend::discover`]) looks for
//! `engine/llama-server[.exe]` next to the models directory plus
//! `models/qwen2.5-0.5b-instruct-q4_k_m.gguf`, overridable via the
//! `AI_UPCYCLER_LLAMA_SERVER` and `AI_UPCYCLER_MODEL` environment variables.
//! When either asset is missing, callers fall back to [`super::backend::StubBackend`].

use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::Arc;
use std::time::Duration;

use futures::stream::BoxStream;
use futures::StreamExt;
use serde_json::{json, Value};
use tokio::process::{Child, Command};
use tokio::sync::Mutex;
use tracing::info;

use super::backend::{GenerationBackend, GenerationChunk, GenerationRequest, ToolCallDelta};

/// Default GGUF used when no `AI_UPCYCLER_MODEL` override is set. Matches the
/// Hugging Face file name so models saved by the download endpoint line up.
pub const DEFAULT_MODEL_FILE: &str = "qwen2.5-0.5b-instruct-q4_k_m.gguf";

/// How the backend reaches llama-server.
#[derive(Debug, Clone)]
enum Mode {
    /// Use an already-running server (tests, external setups).
    Remote { base_url: String },
    /// Spawn the binary lazily on first use.
    Spawn {
        server_path: PathBuf,
        model_path: PathBuf,
    },
}

/// Child-process bookkeeping shared between `generate()` calls.
#[derive(Debug, Default)]
struct RunningServer {
    child: Option<Child>,
    base_url: Option<String>,
}

/// Chat backend backed by a local llama.cpp `llama-server`.
#[derive(Debug)]
pub struct LlamaCppBackend {
    client: reqwest::Client,
    mode: Mode,
    server: Arc<Mutex<RunningServer>>,
}

impl LlamaCppBackend {
    /// Talks to an already-running server at `base_url` (no process managed).
    pub fn remote(base_url: impl Into<String>) -> Self {
        Self {
            client: reqwest::Client::new(),
            mode: Mode::Remote {
                base_url: base_url.into(),
            },
            server: Arc::new(Mutex::new(RunningServer::default())),
        }
    }

    /// Spawns `server_path` with `model_path` on first generation.
    pub fn spawn(server_path: PathBuf, model_path: PathBuf) -> Self {
        Self {
            client: reqwest::Client::new(),
            mode: Mode::Spawn {
                server_path,
                model_path,
            },
            server: Arc::new(Mutex::new(RunningServer::default())),
        }
    }

    /// Discovers a runnable engine, or `None` when assets are missing.
    ///
    /// Looks in `models_dir/../engine/` for the server binary and in
    /// `models_dir/` for [`DEFAULT_MODEL_FILE`]; `AI_UPCYCLER_LLAMA_SERVER` /
    /// `AI_UPCYCLER_MODEL` override either path.
    pub fn discover(models_dir: &Path) -> Option<Self> {
        let engine_dir = models_dir.parent().unwrap_or(models_dir).join("engine");
        let server = std::env::var_os("AI_UPCYCLER_LLAMA_SERVER")
            .map(PathBuf::from)
            .unwrap_or_else(|| {
                engine_dir.join(format!("llama-server{}", std::env::consts::EXE_SUFFIX))
            });
        let model = std::env::var_os("AI_UPCYCLER_MODEL")
            .map(PathBuf::from)
            .unwrap_or_else(|| models_dir.join(DEFAULT_MODEL_FILE));
        if server.is_file() && model.is_file() {
            Some(Self::spawn(server, model))
        } else {
            None
        }
    }

    /// Human-readable description for logs.
    pub fn describe(&self) -> String {
        match &self.mode {
            Mode::Remote { base_url } => format!("llama.cpp at {base_url}"),
            Mode::Spawn {
                server_path,
                model_path,
            } => format!(
                "llama-server {} with {}",
                server_path.display(),
                model_path.display()
            ),
        }
    }

    /// Returns the base URL of a healthy server, spawning if needed.
    async fn ensure_base_url(&self) -> Result<String, String> {
        let (server_path, model_path) = match &self.mode {
            Mode::Remote { base_url } => return Ok(base_url.clone()),
            Mode::Spawn {
                server_path,
                model_path,
            } => (server_path, model_path),
        };

        let mut running = self.server.lock().await;
        let existing_url = running.base_url.clone();
        if let (Some(child), Some(base_url)) = (&mut running.child, existing_url) {
            match child.try_wait() {
                Ok(None) => return Ok(base_url),
                Ok(Some(status)) => {
                    tracing::warn!("llama-server exited ({status}); respawning");
                }
                Err(e) => {
                    tracing::warn!("llama-server wait failed ({e}); respawning");
                }
            }
            running.child = None;
            running.base_url = None;
        }

        let port = free_loopback_port()?;
        let base_url = format!("http://127.0.0.1:{port}");
        let mut cmd = Command::new(server_path);
        cmd.arg("-m")
            .arg(model_path)
            // Tool calls are rendered by the model's chat template, which
            // llama.cpp only evaluates when the Jinja path is on. Pass it
            // explicitly so a future default change cannot silently disable
            // the agent path (spec D-A6).
            .arg("--jinja")
            .arg("-c")
            .arg(context_size().to_string())
            .arg("--host")
            .arg("127.0.0.1")
            .arg("--port")
            .arg(port.to_string())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .kill_on_drop(true);
        let child = cmd
            .spawn()
            .map_err(|e| format!("failed to spawn {}: {e}", server_path.display()))?;
        running.child = Some(child);

        let deadline = tokio::time::Instant::now() + Duration::from_secs(60);
        loop {
            if let Some(child) = running.child.as_mut() {
                if let Ok(Some(status)) = child.try_wait() {
                    return Err(format!("llama-server exited during startup ({status})"));
                }
            }
            match self.client.get(format!("{base_url}/health")).send().await {
                Ok(resp) if resp.status().is_success() => break,
                _ => {}
            }
            if tokio::time::Instant::now() >= deadline {
                return Err(format!("llama-server at {base_url} not healthy within 60s"));
            }
            tokio::time::sleep(Duration::from_millis(250)).await;
        }

        info!("llama-server ready at {base_url}");
        running.base_url = Some(base_url.clone());
        Ok(base_url)
    }

    async fn chat_stream(
        &self,
        request: &GenerationRequest,
    ) -> Result<impl futures::Stream<Item = Result<GenerationChunk, String>> + 'static, String>
    {
        let base_url = self.ensure_base_url().await?;
        let mut body = json!({
            "model": request.model,
            "messages": request.messages,
            "temperature": request.temperature,
            "max_tokens": request.max_tokens,
            "stream": true,
        });
        // Pass the agent fields through unchanged: the model's chat template,
        // not this proxy, decides how a tool definition is rendered.
        let fields = body.as_object_mut().expect("chat body is an object");
        if let Some(tools) = &request.tools {
            fields.insert("tools".into(), json!(tools));
        }
        if let Some(tool_choice) = &request.tool_choice {
            fields.insert("tool_choice".into(), tool_choice.clone());
        }
        if let Some(stop) = &request.stop {
            fields.insert("stop".into(), json!(stop));
        }
        if let Some(top_p) = request.top_p {
            fields.insert("top_p".into(), json!(top_p));
        }
        if let Some(seed) = request.seed {
            fields.insert("seed".into(), json!(seed));
        }
        let resp = self
            .client
            .post(format!("{base_url}/v1/chat/completions"))
            .json(&body)
            .send()
            .await
            .map_err(|e| format!("llama-server request failed: {e}"))?;
        if !resp.status().is_success() {
            return Err(format!("llama-server returned HTTP {}", resp.status()));
        }
        Ok(sse_chunks(resp.bytes_stream()))
    }
}

impl GenerationBackend for LlamaCppBackend {
    fn generate(
        &self,
        request: GenerationRequest,
    ) -> BoxStream<'static, Result<GenerationChunk, String>> {
        let backend = Self {
            client: self.client.clone(),
            mode: self.mode.clone(),
            server: Arc::clone(&self.server),
        };
        Box::pin(async_stream::stream! {
            match backend.chat_stream(&request).await {
                Ok(chunks) => {
                    futures::pin_mut!(chunks);
                    while let Some(item) = chunks.next().await {
                        yield item;
                    }
                }
                Err(e) => yield Err(e),
            }
        })
    }

    /// The host engine is the only backend that renders tools; its model
    /// template decides whether a given model actually calls them.
    fn tool_calling_available(&self) -> bool {
        true
    }
}

/// One decoded server-sent-event line.
#[derive(Debug, Clone, PartialEq, Eq)]
enum SseLine {
    /// `data: [DONE]` — the stream is finished.
    Done,
    /// A content or tool-call delta, and/or the finish reason.
    Delta(SseDelta),
    /// A keep-alive, empty delta, or other ignorable line.
    Skip,
    /// The server signalled an error inside the stream.
    Error(String),
}

/// The parts of `choices[0]` this proxy cares about.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
struct SseDelta {
    content: String,
    tool_calls: Option<Vec<ToolCallDelta>>,
    finish_reason: Option<String>,
}

/// Incremental `data:`-line decoder that tolerates arbitrary chunk boundaries.
#[derive(Debug, Default)]
struct SseDecoder {
    buf: String,
}

impl SseDecoder {
    fn new() -> Self {
        Self::default()
    }

    /// Feeds raw text and returns every completed line (CR stripped).
    fn push(&mut self, text: &str) -> Vec<SseLine> {
        self.buf.push_str(text);
        let mut lines = Vec::new();
        while let Some(idx) = self.buf.find('\n') {
            let line: String = self.buf.drain(..=idx).collect();
            lines.push(parse_sse_line(line.trim_end()));
        }
        lines
    }
}

fn parse_sse_line(line: &str) -> SseLine {
    let Some(payload) = line.strip_prefix("data:") else {
        return SseLine::Skip;
    };
    let payload = payload.trim_start();
    if payload == "[DONE]" {
        return SseLine::Done;
    }
    match serde_json::from_str::<Value>(payload) {
        Ok(value) => {
            if let Some(message) = value
                .get("error")
                .and_then(|e| e.get("message"))
                .and_then(Value::as_str)
            {
                return SseLine::Error(message.to_string());
            }
            let choice = value.pointer("/choices/0");
            let delta = choice.and_then(|c| c.get("delta"));
            let content = delta
                .and_then(|d| d.get("content"))
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string();
            let tool_calls = match delta.and_then(|d| d.get("tool_calls")) {
                Some(Value::Array(items)) => {
                    match serde_json::from_value::<Vec<ToolCallDelta>>(Value::Array(items.clone()))
                    {
                        Ok(calls) if !calls.is_empty() => Some(calls),
                        Ok(_) => None,
                        Err(e) => {
                            return SseLine::Error(format!("unparseable tool_calls: {e}"));
                        }
                    }
                }
                Some(_) => return SseLine::Error("tool_calls must be an array".into()),
                None => None,
            };
            let finish_reason = choice
                .and_then(|c| c.get("finish_reason"))
                .and_then(Value::as_str)
                .map(str::to_string);
            if content.is_empty() && tool_calls.is_none() && finish_reason.is_none() {
                SseLine::Skip
            } else {
                SseLine::Delta(SseDelta {
                    content,
                    tool_calls,
                    finish_reason,
                })
            }
        }
        Err(e) => SseLine::Error(format!("unparseable SSE payload: {e}")),
    }
}

fn sse_chunks<S>(bytes: S) -> impl futures::Stream<Item = Result<GenerationChunk, String>>
where
    S: futures::Stream<Item = reqwest::Result<bytes::Bytes>>,
{
    async_stream::stream! {
        let mut decoder = SseDecoder::new();
        futures::pin_mut!(bytes);
        while let Some(item) = bytes.next().await {
            let text = match item {
                Ok(chunk) => String::from_utf8_lossy(&chunk).into_owned(),
                Err(e) => {
                    yield Err(format!("llama-server stream error: {e}"));
                    return;
                }
            };
            for line in decoder.push(&text) {
                match line {
                    SseLine::Done => return,
                    SseLine::Delta(delta) => {
                        let tokens = if !delta.content.is_empty() || delta.tool_calls.is_some() {
                            1
                        } else {
                            0
                        };
                        yield Ok(GenerationChunk {
                            content: delta.content,
                            tokens_emitted: tokens,
                            tool_calls: delta.tool_calls,
                            finish_reason: delta.finish_reason,
                            ..Default::default()
                        });
                    }
                    SseLine::Skip => {}
                    SseLine::Error(e) => {
                        yield Err(e);
                        return;
                    }
                }
            }
        }
    }
}

/// Context window for the host engine: 8192 fits the 8 GB host with a 3B Q4
/// model and its KV cache. `AI_UPCYCLER_CONTEXT` overrides it for larger hosts.
fn context_size() -> u32 {
    std::env::var("AI_UPCYCLER_CONTEXT")
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(8192)
}

fn free_loopback_port() -> Result<u16, String> {
    let listener =
        std::net::TcpListener::bind("127.0.0.1:0").map_err(|e| format!("no free port: {e}"))?;
    listener
        .local_addr()
        .map(|addr| addr.port())
        .map_err(|e| format!("no free port: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::backend::{ChatMessage, FunctionCallDelta, FunctionDef, Stop, ToolDef};
    use axum::response::sse::{Event, KeepAlive, Sse};
    use futures::StreamExt;

    fn request() -> GenerationRequest {
        GenerationRequest {
            model: "qwen".into(),
            messages: vec![ChatMessage {
                role: "user".into(),
                content: "hi".into(),
                ..Default::default()
            }],
            temperature: 0.7,
            max_tokens: 32,
            session_id: "s-test".into(),
            ..Default::default()
        }
    }

    #[test]
    fn decoder_tolerates_split_boundaries() {
        let mut decoder = SseDecoder::new();
        assert!(decoder.push("da").is_empty());
        assert!(decoder.push("ta: {\"choices\":[{\"del").is_empty());
        let lines = decoder.push("ta\":{\"content\":\"Hi\"}}]}\n\n");
        assert_eq!(
            lines,
            vec![
                SseLine::Delta(SseDelta {
                    content: "Hi".into(),
                    ..Default::default()
                }),
                SseLine::Skip
            ],
            "split payload reassembles into one chunk plus a blank keep-alive line"
        );
        assert_eq!(decoder.push("data: [DONE]\n"), vec![SseLine::Done]);
    }

    #[test]
    fn parse_extracts_chunks_done_and_errors() {
        assert_eq!(parse_sse_line("data: [DONE]"), SseLine::Done);
        assert_eq!(
            parse_sse_line("data: {\"choices\":[{\"delta\":{\"content\":\"Hey\"}}]}"),
            SseLine::Delta(SseDelta {
                content: "Hey".into(),
                ..Default::default()
            })
        );
        assert_eq!(
            parse_sse_line("data: {\"choices\":[{\"delta\":{}}]}"),
            SseLine::Skip
        );
        assert_eq!(parse_sse_line(": keep-alive"), SseLine::Skip);
        assert_eq!(parse_sse_line("event: foo"), SseLine::Skip);

        let tool_payload = serde_json::json!({
            "choices": [{
                "delta": {
                    "tool_calls": [{
                        "index": 0,
                        "id": "call_1",
                        "type": "function",
                        "function": { "name": "read", "arguments": "{\"path\":" }
                    }]
                }
            }]
        });
        assert_eq!(
            parse_sse_line(&format!("data: {tool_payload}")),
            SseLine::Delta(SseDelta {
                content: String::new(),
                tool_calls: Some(vec![ToolCallDelta {
                    index: 0,
                    id: Some("call_1".into()),
                    kind: Some("function".into()),
                    function: Some(FunctionCallDelta {
                        name: Some("read".into()),
                        arguments: Some("{\"path\":".into()),
                    }),
                }]),
                finish_reason: None,
            })
        );
        assert_eq!(
            parse_sse_line("data: {\"choices\":[{\"delta\":{},\"finish_reason\":\"tool_calls\"}]}"),
            SseLine::Delta(SseDelta {
                finish_reason: Some("tool_calls".into()),
                ..Default::default()
            })
        );
        assert_eq!(
            parse_sse_line("data: {\"error\":{\"message\":\"boom\"}}"),
            SseLine::Error("boom".into())
        );
        assert!(matches!(
            parse_sse_line("data: {not json"),
            SseLine::Error(_)
        ));
    }

    async fn stub_llama_server() -> (String, tokio::task::JoinHandle<()>) {
        use axum::routing::post;
        let app = axum::Router::new().route(
            "/v1/chat/completions",
            post(|body: axum::Json<Value>| async move {
                assert_eq!(body.get("stream"), Some(&Value::Bool(true)));
                // `Event::data` adds the `data:` prefix itself.
                let events = vec![
                    "{\"choices\":[{\"delta\":{\"role\":\"assistant\"}}]}",
                    "{\"choices\":[{\"delta\":{\"content\":\"Hello\"}}]}",
                    "{\"choices\":[{\"delta\":{\"content\":\" world\"}}]}",
                    "[DONE]",
                ];
                let stream = futures::stream::iter(
                    events
                        .into_iter()
                        .map(|e| Ok::<_, std::convert::Infallible>(Event::default().data(e))),
                );
                Sse::new(stream).keep_alive(KeepAlive::default())
            }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let handle = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        (format!("http://{addr}"), handle)
    }

    #[tokio::test]
    async fn remote_backend_reassembles_streamed_tokens() {
        let (base_url, server) = stub_llama_server().await;
        let backend = LlamaCppBackend::remote(&base_url);

        let chunks: Vec<_> = backend
            .generate(request())
            .collect::<Vec<_>>()
            .await
            .into_iter()
            .map(|c| c.expect("chunk ok"))
            .collect();

        assert_eq!(chunks.len(), 2);
        let text: String = chunks.iter().map(|c| c.content.as_str()).collect();
        assert_eq!(text, "Hello world");
        server.abort();
    }

    #[tokio::test]
    async fn remote_backend_forwards_tool_calls_and_the_finish_reason() {
        use axum::routing::post;
        let app = axum::Router::new().route(
            "/v1/chat/completions",
            post(|body: axum::Json<Value>| async move {
                assert_eq!(body["tools"][0]["function"]["name"], "read");
                assert_eq!(body["tool_choice"], "auto");
                assert_eq!(body["stop"], serde_json::json!(["\n\n"]));
                assert_eq!(body["top_p"], 0.5);
                assert_eq!(body["seed"], 42);
                let events = vec![
                    "{\"choices\":[{\"delta\":{\"role\":\"assistant\"}}]}",
                    "{\"choices\":[{\"delta\":{\"tool_calls\":[{\"index\":0,\"id\":\"call_1\",\
                     \"type\":\"function\",\"function\":{\"name\":\"read\",\"arguments\":\"{\\\"path\\\":\"}}]}}]}",
                    "{\"choices\":[{\"delta\":{\"tool_calls\":[{\"index\":0,\"function\":\
                     {\"arguments\":\"\\\"a.txt\\\"}\"}}]}}]}",
                    "{\"choices\":[{\"delta\":{},\"finish_reason\":\"tool_calls\"}]}",
                    "[DONE]",
                ];
                let stream = futures::stream::iter(
                    events
                        .into_iter()
                        .map(|e| Ok::<_, std::convert::Infallible>(Event::default().data(e))),
                );
                Sse::new(stream).keep_alive(KeepAlive::default())
            }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        let backend = LlamaCppBackend::remote(format!("http://{addr}"));

        let mut req = request();
        req.tools = Some(vec![ToolDef {
            kind: "function".into(),
            function: FunctionDef {
                name: "read".into(),
                description: Some("Read a file".into()),
                parameters: Some(serde_json::json!({ "type": "object" })),
            },
        }]);
        req.tool_choice = Some(serde_json::json!("auto"));
        req.stop = Some(Stop::Many(vec!["\n\n".into()]));
        req.top_p = Some(0.5);
        req.seed = Some(42);

        let chunks: Vec<_> = backend
            .generate(req)
            .collect::<Vec<_>>()
            .await
            .into_iter()
            .map(|c| c.expect("chunk ok"))
            .collect();

        assert_eq!(chunks.len(), 3, "two tool deltas plus the finish reason");
        let first = chunks[0].tool_calls.as_ref().expect("first tool delta");
        assert_eq!(
            first[0].function.as_ref().unwrap().name.as_deref(),
            Some("read")
        );
        let second = chunks[1].tool_calls.as_ref().expect("second tool delta");
        assert_eq!(
            second[0].function.as_ref().unwrap().arguments.as_deref(),
            Some("\"a.txt\"}")
        );
        assert_eq!(chunks[2].finish_reason.as_deref(), Some("tool_calls"));
        assert_eq!(chunks[2].tokens_emitted, 0);
        server.abort();
    }

    #[tokio::test]
    async fn remote_backend_surfaces_http_errors() {
        let app = axum::Router::new().route(
            "/v1/chat/completions",
            axum::routing::post(|| async { axum::http::StatusCode::INTERNAL_SERVER_ERROR }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });

        let backend = LlamaCppBackend::remote(format!("http://{addr}"));
        let results: Vec<_> = backend.generate(request()).collect().await;
        assert_eq!(results.len(), 1);
        let err = results[0].as_ref().unwrap_err();
        assert!(err.contains("HTTP 500"), "got: {err}");
        server.abort();
    }

    #[tokio::test]
    async fn remote_backend_surfaces_connection_errors() {
        let port = free_loopback_port().unwrap();
        let backend = LlamaCppBackend::remote(format!("http://127.0.0.1:{port}"));
        let results: Vec<_> = backend.generate(request()).collect().await;
        assert_eq!(results.len(), 1);
        assert!(results[0].is_err());
    }

    /// Local-only end-to-end check against the real llama.cpp engine.
    ///
    /// Requires a Windows/Linux `llama-server` binary and a GGUF:
    /// `AI_UPCYCLER_LLAMA_SERVER=/path/to/llama-server`
    /// `AI_UPCYCLER_MODEL=/path/to/model.gguf`
    #[tokio::test]
    #[ignore = "requires llama.cpp binary + GGUF model (see env vars in doc comment)"]
    async fn real_engine_generates_tokens() {
        let server = std::env::var("AI_UPCYCLER_LLAMA_SERVER").expect("AI_UPCYCLER_LLAMA_SERVER");
        let model = std::env::var("AI_UPCYCLER_MODEL").expect("AI_UPCYCLER_MODEL");
        let backend = LlamaCppBackend::spawn(PathBuf::from(server), PathBuf::from(model));

        let mut request = request();
        request.max_tokens = 24;
        let results: Vec<_> = backend.generate(request).collect().await;

        let mut text = String::new();
        for item in results {
            text.push_str(&item.expect("real generation should not error").content);
        }
        println!("engine said: {text}");
        assert!(
            !text.trim().is_empty(),
            "expected real tokens, got empty stream"
        );
        assert!(!text.contains("[stub"), "engine leaked stub marker: {text}");
    }
}
