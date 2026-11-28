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
  /** Where the turn ran: `local`, `adb:<serial>`, or a provider id. */
  source: string;
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
  | { event: 'model_download_progress'; timestamp: number; data: DownloadProgressData }
  | { event: 'workflow_step_started'; timestamp: number; data: { run_id: string; step_id: string } }
  | { event: 'workflow_step_completed'; timestamp: number; data: {
      run_id: string;
      step_id: string;
      status: WorkflowStepStatus;
      output: string | null;
      source: string | null;
      error: string | null;
    } };

/** One part of a multimodal chat message (sub-project C). */
export type ContentPart =
  | { type: 'text'; text: string }
  | {
      type: 'image';
      media_hash: string;
      mime_type: string;
      file_name: string;
      byte_count: number;
    };

/** Wire content: a plain string for text-only turns, parts otherwise. */
export type ChatContent = string | ContentPart[];

/** The reference returned by `POST /api/media` for one stored image. */
export interface MediaRef {
  media_hash: string;
  mime_type: string;
  file_name: string;
  byte_count: number;
}

export interface ChatMessage {
  role: 'system' | 'user' | 'assistant';
  content: ChatContent;
}

export interface ChatRequest {
  model: string;
  messages: ChatMessage[];
  temperature?: number;
  max_tokens?: number;
  session_id?: string;
  /** Omit for `auto`; otherwise `local` or a configured provider id. */
  provider?: string;
}

export interface ProviderModelWire {
  id: string;
  label: string;
  supports_vision: boolean;
}

export interface ProviderStatus {
  id: 'openai' | 'anthropic' | 'gemini' | 'kimi';
  name: string;
  configured: boolean;
  models: ProviderModelWire[];
  supports_vision: boolean;
  supports_tools: boolean;
}

/**
 * `GET /api/providers` returns a bare array: the server's `ProviderList`
 * newtype is `#[serde(transparent)]`, so it serializes as its inner vector.
 */
export type ProviderList = ProviderStatus[];

export interface ChatSession {
  session_id: string;
  title: string;
  model_id: string;
  created_at: string;
}

export interface SessionList {
  sessions: ChatSession[];
}

/** Settings stored by the host (never provider secrets). */
export interface AppSettings {
  provider: string | null;
  model: string | null;
  context_tokens: number;
  max_tokens: number;
  show_source_badges: boolean;
  compact_mode: boolean;
}

/** Host-measured limits proposed for the current model and hardware. */
export interface SettingsRecommendation {
  context_tokens: number;
  max_tokens: number;
  reasons: string[];
}

/**
 * One persisted chat row. The server stores the same `ChatContent` union the
 * live chat uses, so older text-only rows are plain strings and multimodal
 * rows are part arrays; `contentToText()` renders both.
 */
export interface PersistedChatMessage {
  message_id: number;
  session_id: string;
  role: 'system' | 'user' | 'assistant';
  content: ChatContent;
  token_count: number;
  generation_time_sec: number;
  created_at: string;
}

export interface SessionMessages {
  messages: PersistedChatMessage[];
}

export interface DownloadRequest {
  repo_id: string;
  file_name: string;
}

/** How a phone is reachable (spec 2026-11-03, slice A). */
export type DeviceTransport = 'usb' | 'wireless_debugging' | 'wifi_adb';

/**
 * The onboarding state machine, as the server reports it.
 *
 * `needs_enrol` is deliberately not called "ready": the phone is reachable and
 * running the right app, but it is not in the cluster until the PIN hand-off
 * finishes.
 */
export type OnboardingState =
  | { state: 'discovered' }
  | { state: 'connecting' }
  | { state: 'awaiting_consent' }
  | { state: 'connected' }
  | { state: 'needs_install'; installed: string | null; available: string }
  | { state: 'installing' }
  | { state: 'needs_enrol'; installed: string }
  | { state: 'ready'; node_id: string }
  | { state: 'failed'; reason: string; remedy: string };

export interface AvailableDevice {
  id: string;
  display_name: string;
  address: string;
  transport: DeviceTransport;
  state: OnboardingState;
  updated_epoch_ms: number;
}

export interface AvailableDevicesResponse {
  devices: AvailableDevice[];
}

export interface ScanResponse {
  found: number;
  devices: AvailableDevice[];
}

export interface RevokeResponse {
  revoked: boolean;
}

/**
 * A model push in progress, as the server reports it (spec 2026-11-03-model-
 * provisioning, D-P8).
 *
 * `done` means the worker verified the bytes and installed the file, not that
 * the engine is serving it: the worker reopens the model a moment later, so
 * `supports_compute` flips on the next handshake rather than on this reply.
 */
export type ProvisionState =
  | { state: 'idle' }
  | { state: 'running'; bytes_done: number; total_bytes: number }
  | { state: 'done'; bytes: number }
  | { state: 'failed'; message: string };

// ---- task workflows (spec 2026-11-27, §10/§11) ------------------------------

export type WorkflowRunStatus = 'running' | 'completed' | 'failed';
export type WorkflowStepStatus = 'pending' | 'running' | 'completed' | 'failed' | 'blocked';
export type WorkflowProvider = 'auto' | 'local' | 'openai' | 'anthropic' | 'gemini' | 'kimi';

export interface WorkflowStepDraft {
  id: string;
  prompt: string;
  depends_on: string[];
  provider: WorkflowProvider;
}

export interface WorkflowRequest {
  model: string;
  steps: WorkflowStepDraft[];
}

export interface WorkflowRunRow {
  run_id: string;
  model: string;
  status: WorkflowRunStatus;
  created_at: string;
  finished_at: string | null;
}

export interface WorkflowStepRow {
  run_id: string;
  step_id: string;
  prompt: string;
  depends_on: string[];
  provider: string | null;
  status: WorkflowStepStatus;
  output: string | null;
  source: string | null;
  error: string | null;
  position: number;
}

export interface WorkflowRunDetail {
  run: WorkflowRunRow;
  steps: WorkflowStepRow[];
}

export interface CreateWorkflowResponse {
  run_id: string;
  status: WorkflowRunStatus;
}

export interface WorkflowListResponse {
  runs: WorkflowRunRow[];
}