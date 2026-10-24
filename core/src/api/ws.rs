//! `GET /api/telemetry` — WebSocket fan-out of cluster telemetry.
//!
//! On connect, currently-known nodes are replayed as `node_connected` events so
//! late subscribers converge on state, then every bus event is forwarded.

use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::State;
use axum::http::{header, HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use tokio::sync::broadcast;

use crate::api::{browser_origin_is_allowed, AppState};
use crate::telemetry::{NodeEventData, TelemetryEvent};

pub async fn handler(
    ws: WebSocketUpgrade,
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Response {
    // Browsers do not apply CORS to WebSocket handshakes, so the upgrade checks
    // the origin itself (spec 2026-10-24, D-G3): only the bundled shell, a
    // localhost dev page, or a client that sends no origin may subscribe.
    if let Some(origin) = headers.get(header::ORIGIN) {
        let origin = origin.to_str().unwrap_or_default();
        if !browser_origin_is_allowed(origin) {
            tracing::warn!(origin, "refused telemetry WebSocket from a non-app origin");
            return (StatusCode::FORBIDDEN, "forbidden origin").into_response();
        }
    }
    ws.on_upgrade(move |socket| handle_socket(socket, state))
}

async fn handle_socket(mut socket: WebSocket, state: AppState) {
    let mut receiver = state.telemetry.subscribe();

    for node in state.registry.list_nodes() {
        let event = TelemetryEvent::node_connected(NodeEventData {
            node_id: node.node_id.clone(),
            device_name: node.device_name.clone(),
            connection_type: node.connection_type.clone(),
            total_ram_bytes: node.total_ram_bytes,
            free_ram_bytes: node.free_ram_bytes,
        });
        if send_event(&mut socket, &event).await.is_err() {
            return;
        }
    }

    loop {
        tokio::select! {
            received = receiver.recv() => match received {
                Ok(event) => {
                    if send_event(&mut socket, &event).await.is_err() {
                        break;
                    }
                }
                Err(broadcast::error::RecvError::Lagged(skipped)) => {
                    tracing::warn!("telemetry subscriber lagged; dropped {skipped} event(s)");
                    continue;
                }
                Err(broadcast::error::RecvError::Closed) => break,
            },
            incoming = socket.recv() => match incoming {
                None | Some(Err(_)) | Some(Ok(Message::Close(_))) => break,
                Some(Ok(_)) => {}
            },
        }
    }
}

async fn send_event(socket: &mut WebSocket, event: &TelemetryEvent) -> Result<(), axum::Error> {
    let payload = serde_json::to_string(event).unwrap_or_else(|_| "{}".to_string());
    socket.send(Message::Text(payload)).await
}
