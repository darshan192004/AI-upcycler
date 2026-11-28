import { describe, expect, it } from 'vitest';
import { createPairingSession, isExpired, revokeSession, verifyToken } from './pairingSession';

const now = new Date('2026-11-28T12:00:00.000Z');

describe('pairing sessions', () => {
  it('expires and verifies a token by hash only', async () => {
    const created = await createPairingSession({
      id: 'session-1',
      kind: 'qr',
      ttlMs: 60_000,
      now,
    });
    expect(created.record.tokenHash).not.toContain(created.rawToken);
    expect(await verifyToken(created.record, created.rawToken, now)).toBe(true);
    expect(await verifyToken(created.record, 'wrong-token', now)).toBe(false);
    expect(await verifyToken(created.record, created.rawToken, new Date('2026-11-28T12:01:01Z'))).toBe(
      false,
    );
    expect(isExpired(created.record, now)).toBe(false);
  });

  it('revokes immediately', async () => {
    const created = await createPairingSession({
      id: 'session-2',
      kind: 'otp',
      ttlMs: 60_000,
      now,
    });
    const revoked = revokeSession(created.record, now);
    expect(await verifyToken(revoked, created.rawToken, now)).toBe(false);
  });
});
