# Implementation Plan: Replacement Cross-Device Cluster App

## Overview

Build a new React Native Android + React Native Windows app that replaces the
current Tauri dashboard. The app makes any device a master or worker, pairs
mobile workers by QR and desktop workers by OTP on the same LAN, shards GGUF
chat/vision model layers across paired devices, and generates images through a
cloud provider from the master. Legacy Tauri code remains untouched until the
replacement reaches parity.

## Architecture Decisions

- New workspace under `apps/` and `packages/`; no Tauri/Electron shell in v1.
- Shared TypeScript protocol is the single source of truth for roles, pairing,
  worker state, model placement, and generation events.
- Master owns the GGUF model and tokenizer/planner; workers hold and execute
  assigned layers.
- Pairing is LAN-only. Mobile workers scan a QR shown by the master; desktop
  workers enter a unique OTP shown by the master. Sessions are short-lived and
  tokens are revocable.
- Prefer wrapping/reusing the existing Rust core engine behind
  `packages/native-engine`; do not rewrite proven distributed inference unless
  Task 9 proves reuse impossible.
- Cloud image generation is a master-side adapter with OS keystore secret
  storage. Local diffusion sharding is out of scope.
- Work is sliced vertically: pairing/control plane first, then chat sharding,
  then cloud images, then role-reversal evidence.

## Task List

### Phase 0: Decisions and Foundation

- [x] Task 1: Resolve v1 decisions and record ADR
- [ ] Task 2: Scaffold new workspace and verification scripts
- [ ] Task 3: Define shared protocol schemas
- [ ] Task 4: Build and test layer-planner logic

### Checkpoint: Foundation
- [ ] Spec still matches confirmed intent
- [ ] `npm run typecheck`, `npm run lint`, `npm test`, `npm run build` pass
- [ ] Human reviews foundation before pairing work

### Phase 1: Pairing and Control Plane

- [ ] Task 5: Master pairing session service
- [ ] Task 6: Android QR pairing flow
- [ ] Task 7: Windows OTP pairing flow
- [ ] Task 8: Device list, role switch, unpair, revoke

### Checkpoint: Pairing
- [ ] QR mobile pairing works on LAN
- [ ] OTP desktop pairing works on LAN
- [ ] Revoked tokens stop worker access
- [ ] Human reviews pairing UX before engine work

### Phase 2: Chat Layer Sharding

- [ ] Task 9: Native engine boundary feasibility spike and mock
- [ ] Task 10: Master model catalog and load flow
- [ ] Task 11: Android worker runtime
- [ ] Task 12: Windows worker runtime
- [ ] Task 13: Master chat UI, streaming, split summary

### Checkpoint: Chat Sharding
- [ ] Windows master + Android worker streams a GGUF chat reply
- [ ] Split plan and contributing devices are visible
- [ ] Role reversal can begin from either device
- [ ] Human reviews real-device evidence before cloud image work

### Phase 3: Cloud Images

- [ ] Task 14: Cloud image provider adapter and secret storage
- [ ] Task 15: Image generation UI

### Checkpoint: Cloud Images
- [ ] Master performs one cloud image generation
- [ ] No provider secret appears in logs, UI, or tests
- [ ] Human reviews image flow before final verification

### Phase 4: Parity and Evidence

- [ ] Task 16: Two-device E2E runbook and evidence collector
- [ ] Task 17: Final docs, deprecation boundary, verification

### Checkpoint: Complete
- [ ] Full role-reversal demo passes
- [ ] All success criteria in the spec are met
- [ ] Ready for PR/review

## Risks and Mitigations

| Risk | Impact | Mitigation |
|---|---|---|
| Native layer sharding is harder than expected | High | Task 9 spikes reuse of existing Rust core before UI work; fail fast with ADR |
| React Native Windows has platform gaps | High | Task 2 includes a Windows smoke build before feature work |
| LAN pairing can be replayed or spoofed | High | Short-lived QR/OTP sessions, hashed tokens, revoke on unpair |
| Heterogeneous devices make placement unstable | Medium | Start with RAM/bandwidth heuristic and benchmark before placement |
| Cloud provider secrets leak | High | OS keystore only; no secrets in logs/UI/tests |
| Replacing Tauri too early breaks known-good flows | Medium | Legacy app stays untouched until full parity evidence exists |
| Role reversal leaves stale worker state | Medium | Explicit state machine and unpair/revoke tests in protocol layer |

## Parallelization Opportunities

- After Task 3, Tasks 6 and 7 can be developed in parallel by different agents.
- After Task 9, Tasks 11 and 12 can be developed in parallel against the same
  native-engine contract.
- Task 14 can proceed in parallel with Phase 2 once the cloud provider choice is
  made in Task 1, but its UI should not merge until Task 13 is stable.

## Open Questions

- Resolved in Task 1: cloud image provider, Windows packaging, Android signing,
  native engine strategy, default model/vision model.
- Any answer that expands OS scope beyond Android + Windows requires spec change
  and human approval first.
