import { create } from 'zustand';
import { getTopology, listModels } from '../lib/api';
import type {
  DownloadProgressData,
  GenerationMetrics,
  ModelEntry,
  TelemetryEvent,
  Topology,
} from '../lib/types';

const HISTORY_CAP = 120;

interface ClusterState {
  topology: Topology | null;
  apiOnline: boolean;
  models: ModelEntry[];
  generationHistory: number[];
  latestMetrics: GenerationMetrics | null;
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
