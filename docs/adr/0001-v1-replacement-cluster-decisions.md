# ADR-0001: V1 Replacement Cluster Decisions

**Date**: 2026-11-28
**Status**: accepted
**Deciders**: device owner (user), AI implementation agent

## Context

The replacement app must support Android + Windows, LAN-only pairing, a master
that owns the GGUF, layer-sharded chat/vision inference, and cloud-only images.
Before scaffolding, five choices must be fixed so tasks 2-17 do not encode
conflicting assumptions. The existing Rust core already has cloud image
adapters, a distributed engine, and LAN/pairing code that can be reused.

## Decision

We choose the following v1 defaults:

1. Use **OpenAI Images** as the first cloud image provider.
2. Package the Windows app as **MSIX**.
3. Sign the Android test build as a **debug APK**.
4. **Wrap the existing Rust core engine** behind `packages/native-engine`;
   rewrite only if Task 9 proves reuse impossible.
5. Use **Gemma 3 4B Q4 vision GGUF** as the first default chat + vision model.

## Alternatives Considered

### OpenAI Images
- **Pros**: already implemented in `core/src/imagegen/registry.rs`; b64_json path is tested.
- **Cons**: requires an OpenAI API key stored in OS keystore.
- **Why not**: selected.

### Gemini Images
- **Pros**: also implemented in the existing registry.
- **Cons**: different model/response behavior; not the requested first provider.
- **Why not**: deferred; can be added later without changing the adapter boundary.

### Stability AI
- **Pros**: matches the reference app's cloud-image wording.
- **Cons**: needs a new provider adapter, credential scope, and tests.
- **Why not**: adds scope before the first end-to-end demo.

### MSIX
- **Pros**: production-like Windows packaging and clean install/update identity.
- **Cons**: more setup than a portable ZIP.
- **Why not**: selected for v1 packaging.

### Portable ZIP
- **Pros**: fastest local test loop; no package identity constraints.
- **Cons**: weaker distribution story and no MSIX integration.
- **Why not**: rejected; v1 should prove the real Windows package path.

### Installer EXE
- **Pros**: familiar distribution format.
- **Cons**: extra NSIS/MSI work without adding acceptance value.
- **Why not**: deferred.

### Debug APK
- **Pros**: simplest repeatable install on the test phone.
- **Cons**: not suitable for public distribution.
- **Why not**: selected for v1 local acceptance.

### Local release keystore
- **Pros**: release-signed test artifact.
- **Cons**: adds keystore generation, storage, and rotation work on D:.
- **Why not**: deferred until distribution is needed.

### Play App Bundle
- **Pros**: production distribution path.
- **Cons**: unnecessary for a two-device local demo.
- **Why not**: out of v1 scope.

### Wrap Rust core
- **Pros**: reuses tested distributed inference, secrets, and pairing foundations.
- **Cons**: requires a native bridge boundary and careful ownership/lifecycle design.
- **Why not**: selected.

### Direct llama.cpp RPC
- **Pros**: closer to upstream engine behavior.
- **Cons**: discards proven repo logic and increases integration risk.
- **Why not**: fallback only if the bridge spike fails.

### New native module
- **Pros**: maximum control.
- **Cons**: highest implementation risk and slowest path to parity.
- **Why not**: rejected.

### Gemma 3 4B Q4 vision
- **Pros**: single multimodal GGUF supports the chat + vision requirement.
- **Cons**: native-engine compatibility must be verified with the chosen llama.cpp build.
- **Why not**: selected; Task 9 must confirm support or trigger a superseding ADR.

### Qwen2.5-Coder-3B Q4
- **Pros**: already tested on this host for chat.
- **Cons**: not a vision model; would need a second GGUF path.
- **Why not**: rejected as the default because it breaks the one-model sharding flow.

### Qwen2.5-VL-3B Q4
- **Pros**: multimodal and smaller than Gemma 3 4B.
- **Cons**: not the selected default; still needs engine-compatibility checks.
- **Why not**: fallback candidate.

## Consequences

### Positive
- Tasks 2-17 have a stable provider, packaging, signing, engine, and model baseline.
- The v1 uses tested repo code where possible instead of rewriting distributed inference.
- The acceptance demo has one master-owned multimodal GGUF and one cloud-image path.

### Negative
- MSIX and Gemma 3 vision compatibility add verification work in Task 2 and Task 9.
- Debug APK is not a distributable release artifact.
- OpenAI Images requires secure runtime key entry and OS keystore storage.

### Risks
- If the Rust core cannot be bridged to RN Android/Windows cleanly, Task 9 must record a
  superseding ADR before any rewrite.
- If Gemma 3 4B Q4 is not supported by the pinned native engine, choose a compatible
  multimodal GGUF and supersede this decision instead of silently swapping models.
- If MSIX packaging blocks the local two-device loop, record the blocker before
  switching to portable output.
