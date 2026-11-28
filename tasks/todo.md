# Task List: Replacement Cross-Device Cluster App

Reference plan: `tasks/plan.md`. Spec:
`docs/superpowers/specs/2026-11-28-replacement-cluster-app.md`.

## Phase 0: Decisions and Foundation

- [x] Task 1: Resolve v1 decisions and record ADR
  - **Description:** Lock the five open spec questions and record the rationale
    before code: cloud image provider, Windows packaging, Android signing,
    native engine strategy, default model/vision model.
  - **Acceptance:**
    - [x] All five decisions have a written answer.
    - [x] ADR records alternatives and consequences.
    - [x] Spec is updated if any answer changes scope.
  - **Verification:** Human review of ADR and spec diff.
  - **Dependencies:** None
  - **Files touched:**
    - `docs/adr/README.md`
    - `docs/adr/template.md`
    - `docs/adr/0001-v1-replacement-cluster-decisions.md`
    - `docs/superpowers/specs/2026-11-28-replacement-cluster-app.md`
  - **Estimated scope:** S

- [ ] Task 2: Scaffold new workspace and verification scripts
  - **Description:** Create the RN Android app, RNW desktop app, shared packages,
    strict TypeScript config, lint/test/build scripts, and D:-only artifact
    defaults.
  - **Acceptance:**
    - [ ] `npm run typecheck`, `npm run lint`, `npm test`, `npm run build` pass.
    - [ ] Android and Windows smoke builds launch.
    - [ ] Lockfiles are committed with pinned versions.
  - **Verification:** Run all four commands; launch both apps once.
  - **Dependencies:** Task 1
  - **Files likely touched:**
    - `package.json`
    - `tsconfig.base.json`
    - `apps/mobile/package.json`
    - `apps/desktop/package.json`
    - `packages/protocol/package.json`
  - **Estimated scope:** M

- [ ] Task 3: Define shared protocol schemas
  - **Description:** Define validated schemas for device role, pairing QR/OTP
    session, worker heartbeat, layer assignment, and generation events.
  - **Acceptance:**
    - [ ] Valid messages parse; malformed messages are rejected.
    - [ ] No schema contains a raw secret field that is logged or returned to UI.
    - [ ] Schema exports are used by both apps.
  - **Verification:** `npm test -- --grep protocol`
  - **Dependencies:** Task 2
  - **Files likely touched:**
    - `packages/protocol/src/schemas.ts`
    - `packages/protocol/src/index.ts`
    - `packages/protocol/src/schemas.test.ts`
  - **Estimated scope:** M

- [ ] Task 4: Build and test layer-planner logic
  - **Description:** Implement the pure RAM/bandwidth placement planner used by
    the master when loading a GGUF model.
  - **Acceptance:**
    - [ ] Plan respects device capacity and sums to requested model size.
    - [ ] Faster devices receive more weight; insufficient capacity errors clearly.
    - [ ] Output is deterministic for identical inputs.
  - **Verification:** `npm test -- --grep layer-planner`; coverage >=80% for package.
  - **Dependencies:** Task 3
  - **Files likely touched:**
    - `packages/layer-planner/src/plan.ts`
    - `packages/layer-planner/src/index.ts`
    - `packages/layer-planner/src/plan.test.ts`
  - **Estimated scope:** M

## Checkpoint: Foundation
- [ ] Spec and decisions are consistent.
- [ ] All four verification commands pass.
- [ ] Human reviews foundation before pairing work.

## Phase 1: Pairing and Control Plane

- [ ] Task 5: Master pairing session service
  - **Description:** Create short-lived QR sessions for mobile workers and OTP
    sessions for desktop workers, with hashed tokens and revocation.
  - **Acceptance:**
    - [ ] QR and OTP sessions expire.
    - [ ] Only token hash is stored; raw token is never returned to logs/UI.
    - [ ] Revoke invalidates the session immediately.
  - **Verification:** `npm test -- --grep pairing-session`
  - **Dependencies:** Task 3
  - **Files likely touched:**
    - `packages/engine-client/src/pairingSession.ts`
    - `packages/engine-client/src/pairingSession.test.ts`
    - `packages/engine-client/src/index.ts`
  - **Estimated scope:** M

- [ ] Task 6: Android QR pairing flow
  - **Description:** Add the Android worker screen that scans the master QR,
    validates it, and joins the LAN cluster.
  - **Acceptance:**
    - [ ] Valid QR joins as worker; expired/malformed QR shows clear error.
    - [ ] Device name, RAM, and bandwidth are reported to master.
    - [ ] Credentials are stored in Android Keystore.
  - **Verification:** Android build plus manual scan against a mock master.
  - **Dependencies:** Task 5
  - **Files likely touched:**
    - `apps/mobile/src/screens/PairingScreen.tsx`
    - `apps/mobile/src/components/QrScanner.tsx`
    - `apps/mobile/src/worker/WorkerJoin.ts`
    - `apps/mobile/src/worker/credentialStorage.ts`
  - **Estimated scope:** M

- [ ] Task 7: Windows OTP pairing flow
  - **Description:** Add the Windows worker screen that enters the OTP shown on
    the master and joins the LAN cluster.
  - **Acceptance:**
    - [ ] Valid OTP joins as worker; expired/replayed OTP is rejected.
    - [ ] Credentials are stored using Windows OS secret storage.
    - [ ] Connection errors are actionable.
  - **Verification:** Windows build plus manual OTP join against a mock master.
  - **Dependencies:** Task 5
  - **Files likely touched:**
    - `apps/desktop/src/screens/PairingScreen.tsx`
    - `apps/desktop/src/components/OtpForm.tsx`
    - `apps/desktop/src/worker/WorkerJoin.ts`
  - **Estimated scope:** M

- [ ] Task 8: Device list, role switch, unpair, revoke
  - **Description:** Add master controls to see paired devices, switch role
    between master/worker/off, and unpair a device.
  - **Acceptance:**
    - [ ] Master lists device name, role, health, RAM, and bandwidth.
    - [ ] Role changes unload/restart the correct runtime.
    - [ ] Unpair revokes token and removes the device from both sides.
  - **Verification:** Protocol tests plus manual role switch on Android + Windows.
  - **Dependencies:** Tasks 6, 7
  - **Files likely touched:**
    - `packages/ui/src/DeviceList.tsx`
    - `apps/mobile/src/screens/MasterScreen.tsx`
    - `apps/desktop/src/screens/MasterScreen.tsx`
    - `packages/engine-client/src/deviceState.ts`
  - **Estimated scope:** M

## Checkpoint: Pairing
- [ ] QR mobile pairing works on LAN.
- [ ] OTP desktop pairing works on LAN.
- [ ] Revoked tokens stop worker access.
- [ ] Human reviews pairing UX before engine work.

## Phase 2: Chat Layer Sharding

- [ ] Task 9: Native engine boundary feasibility spike and mock
  - **Description:** Prove the native contract for load, place layers, forward,
    unload, and telemetry; decide reuse vs rewrite of the Rust core engine.
  - **Acceptance:**
    - [ ] ADR states reuse/replace decision with evidence.
    - [ ] Mock engine emits layer placement commands in order.
    - [ ] Android and Windows can load the native boundary library.
  - **Verification:** `npm test -- --grep native-engine`; both app builds.
  - **Dependencies:** Tasks 1, 2
  - **Files likely touched:**
    - `packages/native-engine/src/types.ts`
    - `packages/native-engine/src/mockEngine.ts`
    - `packages/native-engine/src/mockEngine.test.ts`
    - `packages/native-engine/src/index.ts`
    - `docs/adr/0002-native-engine.md`
  - **Estimated scope:** M

- [ ] Task 10: Master model catalog and load flow
  - **Description:** Let the master own/select a GGUF model, show load progress,
    invoke the planner with paired workers, and unload cleanly.
  - **Acceptance:**
    - [ ] Master shows only models it owns.
    - [ ] Load calls planner and displays split plan.
    - [ ] Unload clears split state on master and workers.
  - **Verification:** Planner tests plus manual load on Windows master.
  - **Dependencies:** Tasks 4, 8, 9
  - **Files likely touched:**
    - `packages/engine-client/src/modelCatalog.ts`
    - `packages/engine-client/src/modelCatalog.test.ts`
    - `apps/mobile/src/screens/ModelsScreen.tsx`
    - `apps/desktop/src/screens/ModelsScreen.tsx`
  - **Estimated scope:** M

- [ ] Task 11: Android worker runtime
  - **Description:** Implement the Android worker side of the native engine
    contract: receive layers, hold weights, execute forward calls, and report
    telemetry.
  - **Acceptance:**
    - [ ] Assigned layers are loaded and unloaded deterministically.
    - [ ] Forward calls return correctly ordered output in mock tests.
    - [ ] RAM/bandwidth telemetry reaches the master.
  - **Verification:** Unit tests plus real Android worker smoke test.
  - **Dependencies:** Task 9
  - **Files likely touched:**
    - `packages/native-engine/android/src/main/java/org/cluster/engine/LlamaEngineModule.kt`
    - `apps/mobile/src/worker/WorkerRuntime.ts`
    - `apps/mobile/src/worker/WorkerService.ts`
    - `packages/native-engine/src/types.ts`
  - **Estimated scope:** M

- [ ] Task 12: Windows worker runtime
  - **Description:** Implement the Windows worker side of the same native engine
    contract and verify it can join an Android master.
  - **Acceptance:**
    - [ ] Windows worker loads/unloads assigned layers deterministically.
    - [ ] Forward calls pass the mock contract tests.
    - [ ] Windows worker reports telemetry to an Android master.
  - **Verification:** Unit tests plus manual OTP join to Android master.
  - **Dependencies:** Task 9
  - **Files likely touched:**
    - `packages/native-engine/windows/EngineBridge.cpp`
    - `apps/desktop/src/worker/WorkerRuntime.ts`
    - `apps/desktop/src/worker/WorkerService.ts`
    - `packages/native-engine/src/types.ts`
  - **Estimated scope:** M

- [ ] Task 13: Master chat UI, streaming, split summary
  - **Description:** Add the video-style chat surface that streams tokens from
    the sharded master engine and shows contributing devices.
  - **Acceptance:**
    - [ ] Prompt streams tokens without buffering the whole reply.
    - [ ] Split summary and source badges show which devices contributed.
    - [ ] Stop/error states leave no stale generation state.
  - **Verification:** Generation tests plus Windows master + Android worker manual chat.
  - **Dependencies:** Tasks 10, 11, 12
  - **Files likely touched:**
    - `packages/ui/src/ChatScreen.tsx`
    - `apps/mobile/src/screens/ChatScreen.tsx`
    - `apps/desktop/src/screens/ChatScreen.tsx`
    - `packages/engine-client/src/generation.ts`
    - `packages/engine-client/src/generation.test.ts`
  - **Estimated scope:** M

## Checkpoint: Chat Sharding
- [ ] Windows master + Android worker streams a GGUF chat reply.
- [ ] Split plan and contributing devices are visible.
- [ ] Role reversal can begin from either device.
- [ ] Human reviews real-device evidence before cloud image work.

## Phase 3: Cloud Images

- [ ] Task 14: Cloud image provider adapter and secret storage
  - **Description:** Implement the chosen master-side cloud image provider with
    OS keystore secret storage and a no-real-key test mode.
  - **Acceptance:**
    - [ ] Provider test mode returns an image without real credentials.
    - [ ] API key is stored only in OS keystore.
    - [ ] No secret appears in logs, UI state, API responses, or test fixtures.
  - **Verification:** `npm test -- --grep cloud-images`; secret scan of diff.
  - **Dependencies:** Tasks 1, 2, 8
  - **Files likely touched:**
    - `packages/cloud-images/src/provider.ts`
    - `packages/cloud-images/src/provider.test.ts`
    - `packages/cloud-images/src/secretStore.ts`
    - `packages/cloud-images/src/index.ts`
    - `docs/adr/0003-cloud-image-provider.md`
  - **Estimated scope:** M

- [ ] Task 15: Image generation UI
  - **Description:** Add the master-side image screen with prompt, size/options
    exposed by the chosen provider, result preview, and error states.
  - **Acceptance:**
    - [ ] Master can submit a prompt and see a generated image.
    - [ ] Missing-key and provider errors are clear.
    - [ ] UI never renders the raw key.
  - **Verification:** Manual generation plus app builds.
  - **Dependencies:** Task 14
  - **Files likely touched:**
    - `packages/ui/src/ImagesScreen.tsx`
    - `apps/mobile/src/screens/ImagesScreen.tsx`
    - `apps/desktop/src/screens/ImagesScreen.tsx`
  - **Estimated scope:** S

## Checkpoint: Cloud Images
- [ ] Master performs one cloud image generation.
- [ ] No provider secret appears in logs, UI, or tests.
- [ ] Human reviews image flow before final verification.

## Phase 4: Parity and Evidence

- [ ] Task 16: Two-device E2E runbook and evidence collector
  - **Description:** Write the exact Windows→Android and Android→Windows
    acceptance procedure and a script that checks required evidence files.
  - **Acceptance:**
    - [ ] Runbook covers QR, OTP, chat sharding, cloud image, and unpair.
    - [ ] Evidence paths are all under `D:`.
    - [ ] Dry run reports missing evidence clearly.
  - **Verification:** `npm run verify:e2e-manual`
  - **Dependencies:** Tasks 13, 15
  - **Files likely touched:**
    - `docs/runbooks/two-device-e2e.md`
    - `scripts/verify-e2e-manual.mjs`
    - `scripts/verify-env.mjs`
  - **Estimated scope:** S

- [ ] Task 17: Final docs, deprecation boundary, verification
  - **Description:** Update user-facing docs for the replacement app, state that
    legacy Tauri code is retained until parity, and run the full gate.
  - **Acceptance:**
    - [ ] Docs describe roles, pairing, sharding, and cloud images.
    - [ ] Legacy Tauri app is not deleted or silently changed.
    - [ ] All spec success criteria have evidence.
  - **Verification:** Full commands plus role-reversal E2E evidence under `D:`.
  - **Dependencies:** Task 16
  - **Files likely touched:**
    - `docs/architecture.md`
    - `docs/AGENTS-replacement.md`
    - `docs/launch-readiness.md`
    - `docs/api.md`
  - **Estimated scope:** M

## Checkpoint: Complete
- [ ] Full role-reversal demo passes.
- [ ] All success criteria in the spec are met.
- [ ] Ready for PR/review.
