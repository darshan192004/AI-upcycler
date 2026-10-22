import { create } from 'zustand';
import { getTopology, listModels } from '../lib/api';
import type {
  DownloadProgressData,
  GenerationMetrics,
  ModelEntry,
  TelemetryEvent,
  Topology,
  WorkerHealth,
} from '../lib/types';

const HISTORY_CAP = 120;

interface ClusterState {
  topology: Topology | null;
  apiOnline: boolean;
  models: ModelEntry[];
  generationHistory: number[];
  latestMetrics: GenerationMetrics | null;
  /** The freshest measured health per worker id. */
  workerHealth: Record<string, WorkerHealth>;
  /** The master's own RAM utilization; `null` when it could not be read. */
  masterRamUtilizationPct: number | null;
  downloads: Record<string, DownloadProgressData>;
  refreshTopology: () => Promise<void>;
  refreshModels: () => Promise<void>;
  applyTelemetry: (event: TelemetryEvent) => void;
}

export const useClusterStore = create<ClusterState>((set, get) => ({
  topology: null,
  apiOnline: false,
  models: [],
  generationHistory: [],
  latestMetrics: null,
  workerHealth: {},
  masterRamUtilizationPct: null,
  downloads: {},

  refreshTopology: async () => {
    try {
      const topology = await getTopology();
      set({ topology, apiOnline: true });
    } catch {
      set({ apiOnline: false });
    }
  },

  refreshModels: async () => {
    try {
      const list = await listModels();
      set({ models: list.data, apiOnline: true });
    } catch {
      set({ apiOnline: false });
    }
  },

  applyTelemetry: (event) => {
    switch (event.event) {
      case 'node_connected':
      case 'node_disconnected': {
        void get().refreshTopology();
        break;
      }
      case 'generation_metrics': {
        const history = get().generationHistory;
        set({
          latestMetrics: event.data,
          generationHistory: [
            ...history,
            event.data.tokens_per_second,
          ].slice(-HISTORY_CAP),
        });
        break;
      }
      case 'worker_health': {
        // Replaced wholesale, not merged: a worker that left the registry
        // drops out of the map instead of showing a value forever.
        const workerHealth: Record<string, WorkerHealth> = {};
        for (const node of event.data.nodes) {
          workerHealth[node.id] = node;
        }
        set({
          workerHealth,
          masterRamUtilizationPct: event.data.master_ram_utilization_pct,
        });
        break;
      }
      case 'model_download_progress': {
        set((state) => ({
          downloads: {
            ...state.downloads,
            [event.data.file_name]: event.data,
          },
        }));
        if (event.data.status !== 'running') {
          void get().refreshTopology();
          if (event.data.status === 'complete') {
            void get().refreshModels();
          }
        }
        break;
      }
    }
  },
}));
