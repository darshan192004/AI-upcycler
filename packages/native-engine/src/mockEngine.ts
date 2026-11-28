import type { PlacementPlan } from '@ai-upcycler/protocol';
import type { ForwardInput, ForwardOutput, NativeEngine } from './types';

export type EngineCommand =
  | { kind: 'load'; modelPath: string }
  | { kind: 'placeLayers'; plan: PlacementPlan }
  | { kind: 'forward'; requestId: string }
  | { kind: 'unload' };

export class MockNativeEngine implements NativeEngine {
  readonly commands: EngineCommand[] = [];

  async load(modelPath: string): Promise<void> {
    this.commands.push({ kind: 'load', modelPath });
  }

  async placeLayers(plan: PlacementPlan): Promise<void> {
    this.commands.push({ kind: 'placeLayers', plan });
  }

  async forward(input: ForwardInput): Promise<ForwardOutput> {
    this.commands.push({ kind: 'forward', requestId: input.requestId });
    return { requestId: input.requestId, token: 'synthetic-token', final: true };
  }

  async unload(): Promise<void> {
    this.commands.push({ kind: 'unload' });
  }
}
