export type DeviceRole = 'master' | 'worker' | 'off';
export type PairingKind = 'qr' | 'otp';

export interface PairingSession {
  id: string;
  kind: PairingKind;
  expiresAt: string;
  tokenHash: string;
}

export interface WorkerHeartbeat {
  deviceId: string;
  name: string;
  role: Extract<DeviceRole, 'worker'>;
  ramBytes: number;
  bandwidthMbps: number;
  observedAt: string;
}

export interface LayerAssignment {
  deviceId: string;
  startLayer: number;
  endLayer: number;
  bytes: number;
}

export interface PlacementPlan {
  modelId: string;
  totalLayers: number;
  assignments: LayerAssignment[];
}

export interface ChatChunk {
  requestId: string;
  deviceId: string;
  token: string;
  final: boolean;
}

export class ProtocolError extends Error {
  readonly field: string;

  constructor(field: string) {
    super(`invalid protocol message: ${field}`);
    this.name = 'ProtocolError';
    this.field = field;
  }
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === 'object' && value !== null && !Array.isArray(value);
}

function requireString(value: unknown, field: string): string {
  if (typeof value !== 'string' || value.length === 0) {
    throw new ProtocolError(field);
  }
  return value;
}

function requireFiniteNumber(value: unknown, field: string): number {
  if (typeof value !== 'number' || !Number.isFinite(value) || value < 0) {
    throw new ProtocolError(field);
  }
  return value;
}

function requireIsoDate(value: unknown, field: string): string {
  const text = requireString(value, field);
  if (!Number.isFinite(Date.parse(text))) {
    throw new ProtocolError(field);
  }
  return text;
}

export function parsePairingSession(input: unknown): PairingSession {
  if (!isRecord(input)) {
    throw new ProtocolError('pairingSession');
  }
  const kind = input.kind;
  if (kind !== 'qr' && kind !== 'otp') {
    throw new ProtocolError('kind');
  }
  return {
    id: requireString(input.id, 'id'),
    kind,
    expiresAt: requireIsoDate(input.expiresAt, 'expiresAt'),
    tokenHash: requireString(input.tokenHash, 'tokenHash'),
  };
}

export function parseWorkerHeartbeat(input: unknown): WorkerHeartbeat {
  if (!isRecord(input) || input.role !== 'worker') {
    throw new ProtocolError('role');
  }
  return {
    deviceId: requireString(input.deviceId, 'deviceId'),
    name: requireString(input.name, 'name'),
    role: 'worker',
    ramBytes: requireFiniteNumber(input.ramBytes, 'ramBytes'),
    bandwidthMbps: requireFiniteNumber(input.bandwidthMbps, 'bandwidthMbps'),
    observedAt: requireIsoDate(input.observedAt, 'observedAt'),
  };
}

export function parsePlacementPlan(input: unknown): PlacementPlan {
  if (!isRecord(input) || !Array.isArray(input.assignments)) {
    throw new ProtocolError('assignments');
  }
  const totalLayers = requireFiniteNumber(input.totalLayers, 'totalLayers');
  if (!Number.isInteger(totalLayers) || totalLayers === 0) {
    throw new ProtocolError('totalLayers');
  }
  const assignments = input.assignments.map((raw, index) => {
    if (!isRecord(raw)) {
      throw new ProtocolError(`assignments[${index}]`);
    }
    const startLayer = requireFiniteNumber(raw.startLayer, 'startLayer');
    const endLayer = requireFiniteNumber(raw.endLayer, 'endLayer');
    if (!Number.isInteger(startLayer) || !Number.isInteger(endLayer) || endLayer <= startLayer) {
      throw new ProtocolError(`assignments[${index}].endLayer`);
    }
    return {
      deviceId: requireString(raw.deviceId, 'deviceId'),
      startLayer,
      endLayer,
      bytes: requireFiniteNumber(raw.bytes, 'bytes'),
    };
  });
  return {
    modelId: requireString(input.modelId, 'modelId'),
    totalLayers,
    assignments,
  };
}

export function parseChatChunk(input: unknown): ChatChunk {
  if (!isRecord(input)) {
    throw new ProtocolError('chatChunk');
  }
  if (typeof input.final !== 'boolean') {
    throw new ProtocolError('final');
  }
  return {
    requestId: requireString(input.requestId, 'requestId'),
    deviceId: requireString(input.deviceId, 'deviceId'),
    token: requireString(input.token, 'token'),
    final: input.final,
  };
}
