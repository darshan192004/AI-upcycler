export interface StorageMmap {
  enabled: boolean;
  path: string;
  mapped_layers_count: number;
}

export interface DiscoveryStatus {
  mdns: 'active' | 'unavailable';
}

/**
 * The thermal state one node was planned against.
 *
 * `capped: true` means the node's share of the model was limited because it
 * is hot — it is resting while the cooler workers carry the load.
 */
export interface TopologyThermal {
  node_id: string;
  temp_celsius: number;
  capped: boolean;
}

export interface TopologyNode {
  node_id: string;
  role: string;
  device_name: string;
  connection: string;
  ram_total_mb: number;
  /** Live used memory; `null` when the reading is unavailable. */
  ram_used_mb: number | null;
  /** Live available memory; `null` when the reading is unavailable. */
  ram_available_mb: number | null;
  /** The scheduler's plan, which is legitimately 0 for the master. */
  ram_assigned_mb: number;
  /** `master`, `usb_detected`, `discovered`, `paired`, or `ready`. */
  worker_state: string;
  assigned_layers: [number, number][];
  ping_ms: number;
}

export interface Topology {
  cluster_id: string;
  status: string;
  active_model: string | null;
  total_pooled_ram_mb: number;
  used_pooled_ram_mb: number;
  available_pooled_ram_mb: number;
  nodes: TopologyNode[];
  storage_mmap: StorageMmap;
  discovery?: DiscoveryStatus;
  /** One entry per worker with a measured temperature. */
  thermal?: TopologyThermal[];
  /** True when the cluster was hot enough that resting was not an option. */
  thermal_relaxed?: boolean;
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
  /** `null` when the worker cannot measure it — never a placeholder zero. */
  temp_celsius: number | null;
  /** `null` when the worker cannot measure it — never a placeholder zero. */
  battery_pct: number | null;
}

export interface GenerationMetrics {
  tokens_per_second: number;
  current_token: number;
  /** The master's own RAM utilization; `null` when it could not be read. */
  master_ram_utilization_pct: number | null;
  worker_nodes: WorkerTelemetry[];
}

/**
 * One worker's latest **measured** health, published every poll cycle.
 *
 * A worker that cannot measure a metric reports `null`; a worker that has
 * never answered is absent from the map entirely.
 */
export interface WorkerHealth {
  id: string;
  temp_celsius: number | null;
  battery_pct: number | null;
  free_ram_bytes: number;
  /** When the snapshot was taken (Unix epoch milliseconds). */
  observed_at_epoch_ms: number;
  /**
   * Whether the worker can execute layer RPCs (a model engine is open).
   * A metadata-only worker is never planned over.
   */
  supports_compute: boolean;
}

export interface WorkerHealthEventData {
  /** The master's own RAM utilization; `null` when it could not be read. */
  master_ram_utilization_pct: number | null;
  nodes: WorkerHealth[];
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
  | { event: 'worker_health'; timestamp: number; data: WorkerHealthEventData }
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
