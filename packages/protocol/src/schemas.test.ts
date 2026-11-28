import { describe, expect, it } from 'vitest';
import {
  parseChatChunk,
  parsePairingSession,
  parsePlacementPlan,
  parseWorkerHeartbeat,
  ProtocolError,
} from './schemas';

describe('protocol schemas', () => {
  it('parses valid pairing sessions', () => {
    const parsed = parsePairingSession({
      id: 'session-1',
      kind: 'qr',
      expiresAt: '2026-11-28T12:00:00.000Z',
      tokenHash: 'synthetic-hash',
    });
    expect(parsed.kind).toBe('qr');
  });

  it('rejects malformed pairing sessions', () => {
    expect(() => parsePairingSession({ kind: 'pin' })).toThrow(ProtocolError);
    expect(() =>
      parsePairingSession({
        id: 'session-1',
        kind: 'otp',
        expiresAt: 'not-a-date',
        tokenHash: 'synthetic-hash',
      }),
    ).toThrow('expiresAt');
  });

  it('parses worker heartbeat and placement plan', () => {
    const heartbeat = parseWorkerHeartbeat({
      deviceId: 'phone-1',
      name: 'Phone',
      role: 'worker',
      ramBytes: 4_000_000_000,
      bandwidthMbps: 80,
      observedAt: '2026-11-28T12:00:00.000Z',
    });
    expect(heartbeat.role).toBe('worker');

    const plan = parsePlacementPlan({
      modelId: 'gemma-3-4b-q4',
      totalLayers: 8,
      assignments: [{ deviceId: 'phone-1', startLayer: 0, endLayer: 4, bytes: 100 }],
    });
    expect(plan.assignments[0]?.endLayer).toBe(4);
  });

  it('rejects reversed layer ranges and bad chat chunks', () => {
    expect(() =>
      parsePlacementPlan({
        modelId: 'gemma-3-4b-q4',
        totalLayers: 8,
        assignments: [{ deviceId: 'phone-1', startLayer: 4, endLayer: 2, bytes: 100 }],
      }),
    ).toThrow(ProtocolError);
    expect(() => parseChatChunk({ requestId: 'r', deviceId: 'd', token: 'x', final: 'yes' })).toThrow(
      ProtocolError,
    );
  });
});
