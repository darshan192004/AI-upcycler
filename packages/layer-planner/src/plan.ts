import type { LayerAssignment, PlacementPlan } from '@ai-upcycler/protocol';

export interface WorkerCapacity {
  deviceId: string;
  ramBytes: number;
  bandwidthMbps: number;
}

export interface PlanLayersInput {
  modelId: string;
  modelBytes: number;
  totalLayers: number;
  workers: WorkerCapacity[];
  reservedRamBytes?: number;
}

export interface LayerPlanResult extends PlacementPlan {
  unallocatedBytes: number;
}

export class LayerPlannerError extends Error {
  readonly code: 'invalid_input' | 'insufficient_capacity';

  constructor(code: 'invalid_input' | 'insufficient_capacity', message: string) {
    super(message);
    this.name = 'LayerPlannerError';
    this.code = code;
  }
}

interface WeightedWorker {
  capacity: WorkerCapacity;
  usableBytes: number;
  weight: number;
}

function usableBytesFor(worker: WorkerCapacity, reservedRamBytes: number): number {
  return Math.max(0, Math.floor(worker.ramBytes - reservedRamBytes));
}

export function planLayers(input: PlanLayersInput): LayerPlanResult {
  if (
    !Number.isFinite(input.modelBytes) ||
    input.modelBytes <= 0 ||
    !Number.isInteger(input.totalLayers) ||
    input.totalLayers <= 0
  ) {
    throw new LayerPlannerError('invalid_input', 'modelBytes and totalLayers must be positive');
  }
  if (input.workers.length === 0) {
    throw new LayerPlannerError('insufficient_capacity', 'no workers available');
  }

  const reserved = input.reservedRamBytes ?? 0;
  const weighted: WeightedWorker[] = input.workers.map((capacity) => {
    const usableBytes = usableBytesFor(capacity, reserved);
    return {
      capacity,
      usableBytes,
      weight: usableBytes * Math.sqrt(Math.max(capacity.bandwidthMbps, 1)),
    };
  });
  const totalUsable = weighted.reduce((sum, worker) => sum + worker.usableBytes, 0);
  if (totalUsable < input.modelBytes) {
    throw new LayerPlannerError(
      'insufficient_capacity',
      `available ${totalUsable} bytes is less than model ${input.modelBytes} bytes`,
    );
  }

  const totalWeight = weighted.reduce((sum, worker) => sum + worker.weight, 0);
  const allocations = weighted.map((worker) =>
    Math.min(worker.usableBytes, Math.floor((worker.weight / totalWeight) * input.modelBytes)),
  );

  let remaining = input.modelBytes - allocations.reduce((sum, bytes) => sum + bytes, 0);
  for (let index = 0; remaining > 0 && index < allocations.length; index += 1) {
    const room = weighted[index]!.usableBytes - allocations[index]!;
    const add = Math.min(room, remaining);
    allocations[index] = allocations[index]! + add;
    remaining -= add;
  }

  let allocatedBytes = 0;
  const assignments: LayerAssignment[] = allocations.map((bytes, index) => {
    const startLayer = Math.floor((allocatedBytes / input.modelBytes) * input.totalLayers);
    allocatedBytes += bytes;
    const endLayer =
      index === allocations.length - 1
        ? input.totalLayers
        : Math.floor((allocatedBytes / input.modelBytes) * input.totalLayers);
    return {
      deviceId: weighted[index]!.capacity.deviceId,
      startLayer,
      endLayer: Math.max(endLayer, startLayer + 1),
      bytes,
    };
  });

  return {
    modelId: input.modelId,
    totalLayers: input.totalLayers,
    assignments,
    unallocatedBytes: remaining,
  };
}
