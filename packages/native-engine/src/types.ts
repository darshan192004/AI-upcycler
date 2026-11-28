import type { PlacementPlan } from '@ai-upcycler/protocol';

export interface ForwardInput {
  requestId: string;
  prompt: string;
}

export interface ForwardOutput {
  requestId: string;
  token: string;
  final: boolean;
}

export interface NativeEngine {
  load(modelPath: string): Promise<void>;
  placeLayers(plan: PlacementPlan): Promise<void>;
  forward(input: ForwardInput): Promise<ForwardOutput>;
  unload(): Promise<void>;
}
