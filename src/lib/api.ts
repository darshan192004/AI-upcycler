import type {
  ChatRequest,
  DownloadRequest,
  ModelList,
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
    let detail = `${resp.status}`;
    try {
      const body = (await resp.json()) as { detail?: string };
      if (body.detail) detail = body.detail;
    } catch {
      // keep status code as the error detail
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
