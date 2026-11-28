import { describe, expect, it } from 'vitest';
import { MockNativeEngine } from './mockEngine';

describe('mock native engine', () => {
  it('emits layer placement commands in order', async () => {
    const engine = new MockNativeEngine();
    await engine.load('D:/models/gemma-3-4b-q4.gguf');
    await engine.placeLayers({
      modelId: 'gemma-3-4b-q4',
      totalLayers: 8,
      assignments: [{ deviceId: 'phone-1', startLayer: 0, endLayer: 8, bytes: 100 }],
    });
    const output = await engine.forward({ requestId: 'r1', prompt: 'hello' });
    await engine.unload();

    expect(engine.commands.map((command) => command.kind)).toEqual([
      'load',
      'placeLayers',
      'forward',
      'unload',
    ]);
    expect(output.final).toBe(true);
  });
});
