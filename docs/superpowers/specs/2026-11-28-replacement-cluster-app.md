# Spec: Replacement Cross-Device Cluster App

Date: 2026-11-28
Status: accepted
Supersedes: `2026-11-27-privatelm-style-extension-design.md` only where this
replacement direction conflicts with the earlier extension direction.
Related: `docs/AGENTS.md`, `docs/spec.md`, `docs/api.md`,
`2026-11-27-privatelm-style-extension-design.md`.

## Objective

Build new software for **Android phone + Windows laptop** that replaces the
current Tauri dashboard and gives the user a video-style wireless AI cluster:

- Any device can become the **master** or a **worker**.
- The master app is started first.
- Mobile workers pair by scanning a QR code shown on the master.
- Desktop workers pair by entering a unique OTP/code shown on the master.
- Pairing is same-network/LAN only.
- The master owns the GGUF model and places model layers onto workers.
- Chat/vision inference uses **local GGUF layer sharding** across paired devices.
- Image generation is **cloud-only from the master**; local diffusion sharding
  is out of scope for v1.
- The first version must pass an end-to-end role-reversal demo: Windows master
  with Android worker, then Android master with Windows worker.

The user is the owner of the devices. Success means the user can see paired
devices, see the model split, stream a chat reply, generate a cloud image, and
reverse roles on the same Wi-Fi without using the old Tauri app.

## Tech Stack

Target stack for v1, to be pinned in lockfiles during scaffold:

- **UI:** React Native + TypeScript.
- **Android:** React Native Android app.
- **Windows:** React Native Windows app.
- **Shared protocol:** TypeScript package with runtime schema validation.
- **Native inference:** llama.cpp-compatible native module/package capable of
  placing GGUF layers on remote workers. Prefer wrapping/reusing the existing
  tested Rust core engine behind this boundary instead of rewriting it; the
  legacy Tauri UI is replaced, not the proven engine, unless the feasibility
  spike proves reuse impossible.
- **Cloud images:** master-side provider adapter with OS keystore secret storage.
- **Tests:** Vitest/Jest for shared logic, native mock tests for layer planning,
  manual two-device E2E runbook for the authoritative acceptance test.

Version policy: no floating versions in committed lockfiles. Exact RN/RNW/native
versions are pinned when the workspace is scaffolded and verified against
official docs before implementation.

## Commands

These are the target commands after the new workspace is scaffolded:

```bash
npm install                 # install workspace dependencies
npm run typecheck           # TypeScript check across workspace
npm run lint                # lint all source packages
npm test                    # unit + protocol + planner tests
npm run build               # build all packages
npm run android             # build/launch Android app
npm run windows             # build/launch Windows desktop app
npm run dev:protocol        # watch shared protocol package
```

E2E acceptance commands/checks:

```bash
npm run verify:pairing      # automated protocol/pairing conformance suite
npm run verify:e2e-manual   # prints the two-device runbook and evidence checklist
```

Artifacts, screenshots, logs, and caches must be written only under `D:`.

## Project Structure

Proposed new workspace layout. The existing Tauri/Vite/Rust code stays in place
as a reference baseline until the new app reaches parity; do not delete it in v1.

```text
apps/
  mobile/                  # React Native Android app
    src/screens/           # MasterControls, Pairing, Chat, Images, Settings
    src/components/        # shared mobile UI components
    android/               # Android native project
  desktop/                 # React Native Windows app
    src/screens/           # desktop equivalents of mobile screens
    windows/               # Windows native project
packages/
  protocol/                # pairing, role, worker, and generation schemas
  engine-client/           # shared master/worker engine control client
  layer-planner/           # RAM/bandwidth-based layer placement logic
  cloud-images/            # master-side cloud image provider adapters
  native-engine/           # native llama.cpp-compatible module boundary
  ui/                      # shared TS UI utilities and tokens
docs/
  superpowers/specs/       # this spec and future design records
  runbooks/                # two-device acceptance runbook
tasks/
  plan.md                  # implementation plan (Phase 2)
  todo.md                  # ordered task list (Phase 3)
```

## Code Style

Use strict TypeScript, explicit return types on exported functions, and small
pure functions for protocol and planning logic.

```ts
export type DeviceRole = 'master' | 'worker' | 'off';

export interface PairingSession {
  id: string;
  kind: 'qr' | 'otp';
  expiresAt: string;
  tokenHash: string;
}

export function isExpired(session: PairingSession, now = new Date()): boolean {
  return now.getTime() >= Date.parse(session.expiresAt);
}
```

Conventions:

- `camelCase` for variables/functions, `PascalCase` for types/components.
- No secrets in plain objects that are logged or serialized to UI/API responses.
- Protocol messages must be validated at the boundary; no unchecked `as` casts.
- Platform-specific code lives behind a package boundary, not inside screens.

## Testing Strategy

- **Unit:** Vitest/Jest for protocol schemas, OTP expiry, QR payload parsing,
  role transitions, and layer planner math.
- **Integration:** mocked worker endpoints verify pairing, heartbeat, model
  placement, and token revocation without real devices.
- **Native boundary:** mock the native engine to assert layer placement commands
  are emitted correctly; real-device behavior is verified by the runbook.
- **E2E:** manual two-device acceptance runbook is authoritative for v1:
  Windows master + Android worker, then Android master + Windows worker.
- **Coverage target:** 80%+ for `packages/protocol` and `packages/layer-planner`.
- No real provider keys, bearer tokens, CA keys, PINs, or secrets in tests.

## Boundaries

- **Always:**
  - Keep master/worker traffic LAN-only unless explicitly approved.
  - Store provider secrets in OS keystore/keychain surfaces only.
  - Validate all pairing payloads and reject expired/replayed sessions.
  - Run `npm run typecheck`, `npm run lint`, and `npm test` before commits.
  - Keep artifacts on `D:` only.

- **Ask first:**
  - Adding new runtime dependencies or a new cloud image provider.
  - Deleting or rewriting the legacy Tauri app directories.
  - Any internet-exposed listener, tunnel, proxy, or firewall rule.
  - Changing the approved platform matrix (currently Android + Windows only).
  - Using real provider credentials for manual verification.

- **Never:**
  - Commit real keys, tokens, PINs, or certificates.
  - Claim parity without the full role-reversal evidence.
  - Implement local diffusion sharding in v1.
  - Modify the legacy loopback agent API to expose it beyond loopback.
  - Write local artifacts under `C:`.

## Success Criteria

The v1 implementation is complete only when all of these are true:

1. Windows laptop starts as master and shows a QR for mobile pairing.
2. Android phone scans the QR and appears as a paired worker with device name,
   RAM estimate, and bandwidth estimate.
3. Windows desktop worker can pair by entering a unique OTP shown on the master.
4. Master loads a GGUF model it owns and displays a split plan across devices.
5. Chat/vision request streams tokens and the UI shows which devices contributed.
6. Master generates one image through a configured cloud image provider.
7. Role reversal works: Android becomes master, Windows joins as worker by OTP,
   and the same chat flow succeeds.
8. Unpair/revoke removes a worker and invalidates its token.
9. `npm run typecheck`, `npm run lint`, `npm test`, and `npm run build` pass.
10. Two-device E2E evidence is saved under `D:` with screenshots/logs.

## Resolved Decisions

The v1 decisions below are accepted in
[`docs/adr/0001-v1-replacement-cluster-decisions.md`](../../adr/0001-v1-replacement-cluster-decisions.md):

- Cloud image provider: OpenAI Images.
- Windows packaging: MSIX.
- Android test build: debug APK.
- Native engine: wrap the existing tested Rust core behind `packages/native-engine`;
  rewrite only if Task 9 proves reuse impossible.
- Default chat + vision GGUF: Gemma 3 4B Q4 vision. Task 9 must verify native-engine
  compatibility or trigger a superseding ADR.

No open v1 decisions remain.
