# AI Upcycler — Android Worker Agent

On-device half of the pooled cluster. The Android worker:

1. **Advertises itself** over mDNS/DNS-SD as `_aiupcycler-worker._tcp` with the
   RAM/version TXT records the master parses (`core/src/discovery/mdns.rs`).
2. **Serves** the `aiupcycler.rpc.WorkerService` gRPC contract on port `50052`
   — the same port the master's `adb forward` and mDNS client dial.

The wire contract lives in the repository's canonical `proto/cluster_rpc.proto`;
it is vendored into `app/src/main/proto/` for the Java/Android toolchain and a
Gradle guard (`checkProtoInSync`) fails the build if the two copies diverge.

## Current status

This is the first slice: the app builds a real debug APK and implements the two
metadata RPCs the master needs to discover and place the node —
`Handshake` and `GetHealthStatus` (real RAM / battery / thermal readings). The
compute RPCs (`LoadLayers`, `ExecuteForward`, `UnloadLayers`) deliberately return
`UNIMPLEMENTED` until the on-device llama.cpp RPC engine lands, so the master
never schedules work this node cannot honour.

## Build

Requires JDK 17 and the Android SDK (platform 34, build-tools 34.0.0). Set
`ANDROID_HOME` or write `local.properties` with `sdk.dir=/path/to/Android/Sdk`.

```bash
cd mobile-worker/android
./gradlew :app:assembleDebug   # -> app/build/outputs/apk/debug/app-debug.apk
./gradlew :checkProtoInSync    # proto drift guard
./gradlew :app:testDebugUnitTest
```

## Install on a device

```bash
adb install -r app/build/outputs/apk/debug/app-debug.apk
# the master bridges the worker with:
adb forward tcp:50052 tcp:50052
```

`adb` and a physical device are only required to *run* the worker — building and
unit-testing need neither.
