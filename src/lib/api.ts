import type {
  AvailableDevice,
  AvailableDevicesResponse,
  ChatRequest,
  DownloadRequest,
  ModelList,
  RevokeResponse,
  ScanResponse,
  Topology,
} from './types';

export const API_BASE = 'http://127.0.0.1:8080';
export const WS_BASE = 'ws://127.0.0.1:8080';

async function request<T>(path: string, init?: RequestInit): Promise<T> {
  const resp = await fetch(`${API_BASE}${path}`, {
    ...init,
    headers: { 'Content-Type': 'application/json', ...init?.headers },
  });
  if (!resp.ok) {
    // Error bodies are plain text from the device endpoints and JSON from the
    // older ones; read the text once so neither shape is lost.
    const text = await resp.text();
    let detail = text || `${resp.status}`;
    try {
      const body = JSON.parse(text) as {
        detail?: string;
        error?: { message?: string };
      };
      detail = body.detail ?? body.error?.message ?? detail;
    } catch {
      // plain text is already the detail
    }
    throw new Error(detail);
  }
  return (await resp.json()) as T;
}

export function getTopology(): Promise<Topology> {
  return request<Topology>('/api/cluster/topology');
}

export function listModels(): Promise<ModelList> {
  return request<ModelList>('/v1/models');
}

export function startDownload(req: DownloadRequest): Promise<void> {
  return request<void>('/api/models/download', {
    method: 'POST',
    body: JSON.stringify(req),
  });
}

export async function streamChat(
  body: ChatRequest,
  signal: AbortSignal,
  onDelta: (text: string) => void,
  onDone: () => void,
): Promise<void> {
  const resp = await fetch(`${API_BASE}/v1/chat/completions`, {
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify({ ...body, stream: true }),
    signal,
  });
  if (!resp.ok || !resp.body) {
    throw new Error(`chat request failed: ${resp.status}`);
  }
  const reader = resp.body.getReader();
  const decoder = new TextDecoder();
  let buffer = '';
  for (;;) {
    const { done, value } = await reader.read();
    if (done) break;
    buffer += decoder.decode(value, { stream: true });
    const lines = buffer.split('\n');
    buffer = lines.pop() ?? '';
    for (const line of lines) {
      if (!line.startsWith('data:')) continue;
      const payload = line.slice(5).trim();
      if (payload === '[DONE]') {
        onDone();
        return;
      }
      let parsed: {
        error?: { message?: string };
        choices?: { delta?: { content?: string } }[];
      };
      try {
        parsed = JSON.parse(payload);
      } catch {
        continue;
      }
      if (parsed.error) {
        throw new Error(parsed.error.message ?? 'stream error');
      }
      const delta = parsed.choices?.[0]?.delta?.content;
      if (delta) onDelta(delta);
    }
  }
  onDone();
}

// ---- device onboarding (spec 2026-11-03, slice A) --------------------------

export function getAvailableDevices(): Promise<AvailableDevicesResponse> {
  return request<AvailableDevicesResponse>('/api/devices/available');
}

/**
 * Starts onboarding. This can take up to two minutes: the server waits for the
 * user to approve Android's own "Allow USB debugging?" dialog, which we cannot
 * draw or dismiss ourselves.
 */
export function connectDevice(id: string): Promise<AvailableDevice> {
  return request<AvailableDevice>(
    `/api/devices/${encodeURIComponent(id)}/connect`,
    { method: 'POST' },
  );
}

export function installWorker(id: string): Promise<AvailableDevice> {
  return request<AvailableDevice>(
    `/api/devices/${encodeURIComponent(id)}/install`,
    { method: 'POST' },
  );
}

/** Removes the enrollment and drops the ADB connection. */
export function revokeDevice(id: string): Promise<RevokeResponse> {
  return request<RevokeResponse>(
    `/api/devices/${encodeURIComponent(id)}`,
    { method: 'DELETE' },
  );
}

/** One opt-in port sweep. `subnet` is the first three octets, e.g. 192.168.1. */
export function scanNetwork(subnet: string): Promise<ScanResponse> {
  return request<ScanResponse>(
    `/api/devices/scan?subnet=${encodeURIComponent(subnet)}`,
    { method: 'POST' },
  );
}