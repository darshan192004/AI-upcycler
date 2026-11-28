import { describe, expect, it } from 'vitest';
import { LayerPlannerError, planLayers } from './plan';

const workers = [
  { deviceId: 'phone-1', ramBytes: 4_000_000_000, bandwidthMbps: 80 },
  { deviceId: 'pc-1', ramBytes: 8_000_000_000, bandwidthMbps: 200 },
];

describe('planLayers', () => {
  it('sums allocations to the requested model size', () => {
    const result = planLayers({
      modelId: 'gemma-3-4b-q4',
      modelBytes: 3_000_000_000,
      totalLayers: 16,
      workers,
      reservedRamBytes: 500_000_000,
    });
    const assigned = result.assignments.reduce((sum, item) => sum + item.bytes, 0);
    expect(assigned).toBe(3_000_000_000);
    expect(result.unallocatedBytes).toBe(0);
    expect(result.assignments.at(-1)?.endLayer).toBe(16);
  });

  it('gives faster devices more bytes and stays deterministic', () => {
    const input = {
      modelId: 'gemma-3-4b-q4',
      modelBytes: 3_000_000_000,
      totalLayers: 16,
      workers,
    };
    const first = planLayers(input);
    const second = planLayers(input);
    expect(first).toEqual(second);
    expect(first.assignments[1]?.bytes).toBeGreaterThan(first.assignments[0]!.bytes);
  });

  it('rejects insufficient capacity and invalid input', () => {
    expect(() =>
      planLayers({ modelId: 'm', modelBytes: 100, totalLayers: 2, workers: [] }),
    ).toThrow(LayerPlannerError);
    expect(() =>
      planLayers({ modelId: 'm', modelBytes: 0, totalLayers: 2, workers }),
    ).toThrow('positive');
  });
});
