# Hardware Benchmark Probes — Design (2026-10-21)

> **Status:** IMPLEMENTED (2026-10-21) — slices #81 and this PR; real phone
> *values* remain hardware-gated (tasks.md §6 checklist).
> **Tracks:** `docs/plan.md` §5.1 "On-device benchmark probes (replace `StubBenchmarker`)";
> `docs/tasks.md` §7 "`POST /api/cluster/benchmark` uses a deterministic `StubBenchmarker`".
> **Epic edge:** everything in this epic is implementable and verifiable device-free;
> only the *values* measured on physical phones remain hardware-gated.

## Intent

`POST /api/cluster/benchmark` invents its numbers today: `StubBenchmarker`
(`core/src/api/cluster.rs`) derives `memory_bandwidth_mbs` and `throughput_flops` from
an FNV-1a hash of the node id and fills `network_ping_ms` with discovery latency. The
rows are persisted to `node_benchmark`, and nothing consumes them yet — which is exactly
why fabricated measurements are tolerable *today* and untenable the moment anything
schedules on them.

This epic makes the endpoint honest. Each discovered node is probed **over the same
transport class it actually serves on** (mTLS for enrolled workers, plaintext for
not-yet-paired ones), and the *worker itself* measures its own memory bandwidth and
compute throughput with cheap, bounded probes. Nodes that cannot answer the probe —
pre-epic workers (gRPC `UNIMPLEMENTED`), dial failures, timeouts — are **counted, not
faked**: no benchmark row is written, and the response reports them under a new
`nodes_failed` field.

Out of scope: switching layer scheduling onto these numbers (nothing consumes
`node_benchmark` yet; that is a separate, deliberate change once real values exist),
background/continuous benchmarking, and any UI.

## Decisions (D-C1…D-C10)

### D-C1 — A new `Benchmark` RPC on the existing service

`proto/cluster_rpc.proto` gains:

```proto
rpc Benchmark(BenchmarkRequest) returns (BenchmarkResponse);

message BenchmarkRequest {}

message BenchmarkResponse {
  double memory_bandwidth_mbs = 1;
  double throughput_flops = 2;
  double probe_duration_ms = 3;
}
```

An old worker answers `UNIMPLEMENTED` (gRPC unknown-method), which the fallback in
D-C6 treats as "not benchmarkable", never as an error. The proto file's byte change
forces the Android vendored copy to resync — `mobile-worker/android/build.gradle.kts`
already guards this and fails the build on drift (pattern established in spec
2026-10-19, D-A5), so the Kotlin side cannot be forgotten.

### D-C2 — The worker measures itself, with a bounded, conservative probe

The probe runs **on the worker**, in a small pure module (`worker/src/probe.rs`) with
no I/O and no thread spawning — it must not compete with serving:

- **Memory bandwidth**: sequential read+write accumulation over a 32 MiB `f32` buffer,
  fixed small pass count, reported as MB/s. 32 MiB is deliberately conservative for a
  6–8 GB phone; the number is used for relative comparison, not absolute marketing.
- **Compute**: single-threaded `f32` fused multiply-add loop, reported as GFLOP/s.
  Single-threaded for determinism and because thread-count scaling is a scheduling
  concern, not a measurement one.
- Total wall time is bounded to roughly half a second by construction (fixed sizes and
  passes — no timing loops that run "until confident").

The response's `probe_duration_ms` is the worker's self-reported wall time, so a master
can sanity-check that a 30 ms "measurement" was not a degenerate run.

### D-C3 — Transport mirrors enrollment state

`ProbeBenchmarker` resolves the dial the same way the serving path does
(`AdaptiveBackend::try_cluster`, `core/src/api/distributed.rs`): **enrolled** nodes →
`WorkerRpcClient::connect_tls` with credentials from the existing `EnrolledWorkerSource`;
**not enrolled** (e.g. a freshly discovered mDNS worker, or the local Rust worker over an
ADB forward) → plaintext `connect`. This matches how each worker actually serves —
plaintext until paired, mTLS once paired — and adds no new plaintext exposure class:
handshake and ADB tunnel probes already speak plaintext to unpaired workers. Tests
override transports with the existing `with_transports` seam.

### D-C4 — `network_ping_ms` stays the discovery latency

The stub already used `node.latency_ms`, which is a *real* measured round-trip from
mDNS/ADB discovery. The benchmark RPC's round-trip is dominated by probe compute (D-C2)
and would be a misleading "ping". Keeping discovery latency means **one RPC per node**,
no extra health round-trip just for a ping number.

### D-C5 — `HardwareBenchmarker` becomes a native async trait (no new dependency)

The trait must go async because the probe is an RPC. Native async-fn-in-trait, not the
`async-trait` crate (no new dependency; the workspace has none):

```rust
#[allow(async_fn_in_trait)] // monomorphized at both call sites; never used as dyn
pub trait HardwareBenchmarker: Send + Sync {
    async fn benchmark(&self, node: &DiscoveredNode) -> Result<NodeBenchmark, String>;
}
```

Return type changes from `NodeBenchmark` to `Result<NodeBenchmark, String>` because a
real probe can fail. The `#[allow]` silences the `async_fn_in_trait` lint that would
otherwise fail `-D warnings`; the comment states why it is sound here (concrete types at
both call sites, no `dyn`). Two implementations: `StubBenchmarker` (now test-only, and
trivially `Ok(..)`) and the new `ProbeBenchmarker`.

### D-C6 — Failure is counted, never faked

Per-node probe wrapped in a 2 s `tokio::time::timeout`. Any failure — dial error, timeout,
`UNIMPLEMENTED`, missing fields — logs the node id and increments a new
`nodes_failed: u32` field on `BenchmarkResponse`; no row is written. The frontend does
not consume this endpoint (verified: zero matches in `src/`), so extending the response
is safe. `status` becomes `"success"` when at least one node was measured, `"no_nodes"`
when there was nothing to probe, and `"partial"` when some failed — enough for an
operator to see the difference without inventing a retry mechanism.

### D-C7 — The handler wires `ProbeBenchmarker`; the stub leaves production code

`benchmark()` constructs `ProbeBenchmarker` from `AppState` (registry + security
transports) instead of `StubBenchmarker`. `StubBenchmarker` remains in the crate for
tests and fixtures, with its doc comment updated to say it is test-only. Nothing in
production derives a number from a hash.

### D-C8 — Rust worker serves the probe; real host numbers, verifiable device-free

`WorkerEngine` (`worker/src/engine.rs`) implements `benchmark()` by calling the pure
probe module. Because the Rust worker runs on the dev machine, the full loop —
endpoint → transport → RPC → measured values → SQLite row — is verifiable with **real
measurements** in this environment, no phone required. `bench(cluster_rpc.proto)`
succeeds, so the tonic server trait gains the method with a working default only if
generated code provides one; otherwise every implementor (just `WorkerEngine`) implements
it explicitly — decided in the implementation slice, not here.

### D-C9 — Kotlin worker implements the same contract, pure Kotlin

`WorkerRpcService` (`mobile-worker/android/.../rpc/WorkerRpcService.kt`) implements
`benchmark()` with the same buffer size, pass count, and FMA shape as D-C2 — pure
Kotlin, no JNI, unit-testable on the JVM. Device-free tests assert only positivity and
plausibility bounds (bandwidth > 0, flops > 0, duration reported); the *values* stay
hardware-gated, appended to the D-B10-style on-device checklist rather than asserted in CI.

### D-C10 — Epic edge

The epic ends when: the endpoint records only measured values end-to-end against the
Rust worker (device-free), old/unreachable nodes are counted not faked, the Kotlin probe
is in place with device-free tests, and docs (`tasks.md`, `plan.md`) are ticked. What it
does **not** include, and what stays open on the other side of the edge: real phone
numbers (hardware-gated), scheduling on benchmark rows, continuous benchmarking, and any
consumer of the stored values.

## Commands

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test -p ai-upcycler-core -p ai-upcycler-worker
cd mobile-worker/android && ./gradlew test          # device-free JVM tests
pnpm build                                          # unchanged gate
```

## Project Structure

**Modified**

- `proto/cluster_rpc.proto` — `Benchmark` RPC + request/response messages (D-C1).
- `core/src/api/cluster.rs` — async `HardwareBenchmarker` (D-C5), `ProbeBenchmarker`
  (D-C3), handler wiring + `nodes_failed`/status (D-C6, D-C7), stub demoted to tests.
- `core/src/rpc/client.rs` — `WorkerRpcClient::benchmark()` (one call per node, D-C4).
- `worker/src/engine.rs` — `WorkerService::benchmark` implementation (D-C8).
- `mobile-worker/android/app/src/main/proto/cluster_rpc.proto` — vendored resync (D-C1).
- `mobile-worker/android/app/src/main/java/org/aiupcycler/worker/rpc/WorkerRpcService.kt`
  — Kotlin probe (D-C9).
- `docs/tasks.md`, `docs/plan.md` — tick the tracked rows.

**Created**

- `worker/src/probe.rs` — pure bandwidth/FLOP probe functions (D-C2).
- `worker/src/probe.rs` unit tests; `core/tests/benchmark_probes.rs` — in-proc mock
  worker covering measured / failed / mixed paths.

## Testing Strategy (Definition of done)

1. **Pure probe (worker)**: `probe_bandwidth_mbs()` and `probe_throughput_flops()` return
   positive finite values on this dev host; sizes/passes bounded (assert buffer length and
   pass constants, not wall-clock timing).
2. **Client + endpoint, measured path**: in-proc tonic mock worker serving `Benchmark`
   with fixed values → endpoint returns `nodes_tested = 1`, `nodes_failed = 0`, and the
   store row equals the mock's values (mutation check: corrupt a value → test fails).
3. **Endpoint, unimplemented path**: mock worker with a `WorkerService` that answers
   `UNIMPLEMENTED` for `Benchmark` → `nodes_failed = 1`, `nodes_tested = 0`, **no** store
   row, `status = "partial"`; endpoint still 200.
4. **Endpoint, unreachable path**: no worker listening on the node's address → counted as
   failed within the 2 s timeout, no panic, no row.
5. **Mixed**: one measurable + one unimplemented node → one row, one count, `partial`.
6. **Stub is test-only** (mutation check on D-C7): grep-level assertion or test that the
   production handler path yields `status != "success"` with fabricated-free rows; the
   FNV-1a derivation no longer reachable from the handler.
7. **Rust worker end-to-end**: real `WorkerEngine` probe on this host returns > 0 values
   (device-free real measurement, D-C8).
8. **Kotlin**: JVM unit test for probe positivity/plausibility (D-C9).
9. All gates above green; PR-Agent guide comment confirms; mutation checks on any claim
   of the form "X is now real".

**Hardware-gated (documented, not CI-asserted)**: actual bandwidth/FLOP values from a
physical Android worker — appended to the on-device checklist alongside D-B10's pairing
demo.

## Boundaries

- No scheduler changes; `node_benchmark` gains no consumers.
- No new crates (`async-trait` deliberately rejected, D-C5); no UI; no DB schema change
  (`record_benchmark` fields unchanged).
- No background benchmarking loop; probing happens only when the endpoint is called.
- No probing of nodes outside the discovery registry; no new plaintext exposure class
  (D-C3).
- The proto change is additive-only (new RPC + new messages; no field renumbering).

## Success Criteria

- `POST /api/cluster/benchmark` records **only** values measured by the worker; the
  fabricated path (`fnv1a` derivation) is unreachable from production code.
- Pre-epic and unreachable workers degrade gracefully: counted in `nodes_failed`, never
  fatal, never faked.
- The full loop is proven device-free against the Rust worker with real host
  measurements, and against mock workers for every failure shape.

## Open Questions

- Should `network_ping_ms` eventually be refreshed by a cheap periodic RPC instead of
  discovery-time latency? (Deferred — nothing consumes the column.)
- Should the probe spawn a thread per core when measuring bandwidth on phones with
  big.LITTLE asymmetry? (Deferred — single-threaded determinism first; revisit when a
  scheduler consumes the numbers.)
