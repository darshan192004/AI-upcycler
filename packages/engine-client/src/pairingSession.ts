import type { PairingKind, PairingSession } from '@ai-upcycler/protocol';

export interface PairingRecord extends PairingSession {
  revokedAt?: string;
}

export interface CreatePairingInput {
  id: string;
  kind: PairingKind;
  ttlMs: number;
  now?: Date;
}

export interface CreatedPairing {
  record: PairingRecord;
  rawToken: string;
}

export function isExpired(session: PairingSession, now: Date = new Date()): boolean {
  return now.getTime() >= Date.parse(session.expiresAt);
}

export function revokeSession(session: PairingRecord, now: Date = new Date()): PairingRecord {
  return { ...session, revokedAt: now.toISOString() };
}

export async function sha256Hex(text: string): Promise<string> {
  const bytes = new TextEncoder().encode(text);
  const digest = await globalThis.crypto.subtle.digest('SHA-256', bytes);
  return Array.from(new Uint8Array(digest), (byte) => byte.toString(16).padStart(2, '0')).join('');
}

function randomTokenHex(): string {
  const bytes = new Uint8Array(16);
  globalThis.crypto.getRandomValues(bytes);
  return Array.from(bytes, (byte) => byte.toString(16).padStart(2, '0')).join('');
}

export async function createPairingSession(input: CreatePairingInput): Promise<CreatedPairing> {
  if (!Number.isFinite(input.ttlMs) || input.ttlMs <= 0) {
    throw new Error('ttlMs must be positive');
  }
  const now = input.now ?? new Date();
  const rawToken = randomTokenHex();
  const tokenHash = await sha256Hex(rawToken);
  return {
    record: {
      id: input.id,
      kind: input.kind,
      expiresAt: new Date(now.getTime() + input.ttlMs).toISOString(),
      tokenHash,
    },
    rawToken,
  };
}

export async function verifyToken(
  session: PairingRecord,
  rawToken: string,
  now: Date = new Date(),
): Promise<boolean> {
  if (session.revokedAt !== undefined || isExpired(session, now)) {
    return false;
  }
  return (await sha256Hex(rawToken)) === session.tokenHash;
}
