import { WS_BASE } from './api';
import type { TelemetryEvent } from './types';

const MIN_RETRY_MS = 1000;
const MAX_RETRY_MS = 10000;

export function connectTelemetry(onEvent: (event: TelemetryEvent) => void): () => void {
  let socket: WebSocket | null = null;
  let retryMs = MIN_RETRY_MS;
  let closed = false;
  let timer: ReturnType<typeof setTimeout> | null = null;

  const connect = () => {
    if (closed) return;
    try {
      socket = new WebSocket(`${WS_BASE}/api/telemetry`);
    } catch {
      scheduleRetry();
      return;
    }
    socket.onopen = () => {
      retryMs = MIN_RETRY_MS;
    };
    socket.onmessage = (message) => {
      try {
        onEvent(JSON.parse(message.data as string) as TelemetryEvent);
      } catch {
        // ignore malformed frames
      }
    };
    socket.onclose = () => scheduleRetry();
    socket.onerror = () => socket?.close();
  };

  const scheduleRetry = () => {
    if (closed || timer) return;
    timer = setTimeout(() => {
      timer = null;
      connect();
    }, retryMs);
    retryMs = Math.min(retryMs * 2, MAX_RETRY_MS);
  };

  connect();

  return () => {
    closed = true;
    if (timer) clearTimeout(timer);
    if (socket) {
      socket.onclose = null;
      socket.close();
    }
  };
}
