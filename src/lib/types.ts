export interface StorageMmap {
  enabled: boolean;
  path: string;
  mapped_layers_count: number;
}

export interface TopologyNode {
  node_id: string;
  role: string;
  device_name: string;
  connection: string;
  ram_total_mb: number;
  ram_assigned_mb: number;
  assigned_layers: [number, number][];
  ping_ms: number;
}

export interface Topology {
  cluster_id: string;
  status: string;
  active_model: string | null;
  total_pooled_ram_mb: number;
  used_pooled_ram_mb: number;
  nodes: TopologyNode[];
  storage_mmap: StorageMmap;
}

export interface ModelMeta {
  quantization: string;
  file_size_bytes: number;
  layers: number;
  recommended_min_ram_gb: number;
}

export interface ModelEntry {
  id: string;
  object: string;
  created: number;
  owned_by: string;
  meta: ModelMeta;
}

export interface ModelList {
  object: string;
  data: ModelEntry[];
}

export interface NodeEventData {
  node_id: string;
  device_name: string;
  connection_type: string;
  total_ram_bytes: number;
  free_ram_bytes: number;
}

export interface WorkerTelemetry {
  id: string;
  temp_celsius: number;
  battery_pct: number;
}

export interface GenerationMetrics {
  tokens_per_second: number;
  current_token: number;
  master_ram_utilization_pct: number;
  worker_nodes: WorkerTelemetry[];
}

export type DownloadStatus = 'running' | 'complete' | 'error';

export interface DownloadProgressData {
  model_id: string;
  file_name: string;
  received_bytes: number;
  total_bytes: number;
  status: DownloadStatus;
}

export type TelemetryEvent =
  | { event: 'node_connected'; timestamp: number; data: NodeEventData }
  | { event: 'node_disconnected'; timestamp: number; data: NodeEventData }
  | { event: 'generation_metrics'; timestamp: number; data: GenerationMetrics }
  | { event: 'model_download_progress'; timestamp: number; data: DownloadProgressData };

export interface ChatMessage {
  role: 'system' | 'user' | 'assistant';
  content: string;
}

export interface ChatRequest {
  model: string;
  messages: ChatMessage[];
  temperature?: number;
  max_tokens?: number;
  session_id?: string;
}

export interface DownloadRequest {
  repo_id: string;
  file_name: string;
}
