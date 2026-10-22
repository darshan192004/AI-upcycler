# Live Worker Telemetry — Design (2026-10-22)

> **Status:** spec.
> **Tracks:** `docs/tasks.md` §7 "Worker thermal/battery telemetry is emitted as `0.0`
> placeholders until the Android/iOS agents publish it"; the placeholder comment on
> `core/src/telemetry.rs::WorkerTelemetry`; the dashboard's master-RAM tile.
> **Epic edge:** everything here is implementable and verifiable device-free; the
> *values* on a physical phone remain hardware-gated.

## Intent

The workers already measure themselves. The Android worker reads real battery capacity
from `BatteryManager` and a best-effort CPU temperature from `/sys/class/thermal`
(`DeviceMetrics.kt`), and answers `GetHealthStatus` with them. The master **never
asks**: `core/src/api/chat.rs::publish_metrics` writes `temp_celsius: 0.0` and
`battery_pct: 0.0` for every worker, and `master_ram_utilization_pct: 0.0`, so the
dashboard's RAM tile reads a permanent "0%" and no worker card shows a temperature or
a battery level at all. `WorkerRpcClient::health()` has no production caller — only a
test.

This epic closes that loop: the master polls each worker's health on a background
cadence, keeps the freshest snapshot per node, and publishes it to the dashboard — real
numbers, or an honest "unknown" where a number does not exist. It also makes the
master's own RAM utilization real, and aligns the Rust worker's "I cannot measure this"
sentinel with the Android one so the master never mistakes "unknown" for "zero".

Out of scope: storing telemetry history, thermal-aware scheduling, the iOS agent's
thermal publishing, and any per-node alerting policy.

## Decisions (D-E1…D-E9)

### D-E1 — Poll off the hot path

`publish_metrics` fires **per streaming token** (`core/src/api/chat.rs:203`), so a
health RPC inside it would be one round trip per token. Instead a background task
polls every `HEALTH_POLL_INTERVAL` = **5 s**: one `GetHealthStatus` per discovered
node, wrapped in a per-node **2 s** timeout so one dead node cannot delay the cycle.
5 s is slow enough not to churn a phone's radio and fast enough that a dashboard
watching the socket sees battery and thermal move within a few seconds.

The poller is spawned next to the pairing-cleanup poll in `src-tauri/src/lib.rs`
(`tauri::async_runtime::spawn`), the existing precedent for a long-lived background
task in this codebase, and runs for the app's lifetime.

### D-E2 — The cache holds the last snapshot per node

`WorkerHealthCache` (`Arc<Mutex<HashMap<String, WorkerHealth>>>`) maps node id →
`WorkerHealth { temp_celsius: Option<f64>, battery_pct: Option<f64>, free_ram_bytes:
u64, observed_at_epoch_ms: u64 }`. The poller writes; the chat path and the API read.
A node that fails to answer **keeps its previous snapshot** (it was really measured;
the timestamp says how old it is) — the UI marks it stale once it is older than three
poll intervals. A node that has never answered has **no entry** at all, which reads as
"unknown" rather than zero.

### D-E3 — Unknown is null, never zero

The wire shape becomes honest: `temp_celsius` and `battery_pct` are
`Option<f64>` (`null` = unknown), and `master_ram_utilization_pct` becomes
`Option<f64>`. The workers signal "I cannot measure this" with a **negative
sentinel** — the Android worker already does (`UNKNOWN = -1f` in `DeviceMetrics.kt`),
and the master maps any negative value to `None`.

The Rust worker currently reports `0.0` for battery and temperature, which is
indistinguishable from a real zero and would render as "0%" / "0 °C" on the dashboard.
It adopts the same `-1.0` sentinel, so "the desktop has no battery" is distinguishable
from "the battery is empty".

### D-E4 — One dial rule, one helper

`ProbeBenchmarker::connect` already encodes the transport rule from spec
2026-10-21 (D-C3): **enrolled → mutual TLS, otherwise plaintext**, because that is how
each worker actually serves. The health poller must not grow a second, subtly
different copy of that rule, so the dial is extracted into a shared helper
(`core/src/rpc/engine.rs::dial_worker(transports, node)`) used by both the probe and
the poller. No behaviour change for the probe.

### D-E5 — A `worker_health` event, plus the same data inside generation metrics

The poller publishes a new telemetry event `worker_health` carrying the freshest
snapshot per node, so the dashboard updates whether or not a generation is running.
`GenerationMetrics.worker_nodes` keeps its shape but is filled **from the cache**
rather than from constants, so consumers of the existing event see the same real
numbers. The websocket payload grows one event variant; `docs/api.md` documents it.

### D-E6 — The master's own RAM utilization is real

`master_ram_utilization_pct` is computed from the master's own memory:
`utilization = (total - free) / total * 100`, read through **`sysinfo` 0.39** — the
same crate and version the `worker` crate already depends on, so this adds no new
supply-chain surface, only a new dependency edge (`core` → `sysinfo`). A failed read
yields `None` (unknown), not `0.0`.

### D-E7 — The dashboard shows what is true

`NodeCard` gains a temperature and a battery chip per worker: the real value, "—"
when unknown, dimmed when the snapshot is stale. The dashboard's master-RAM tile
shows "—" instead of "0%" when the utilization is unknown. The frontend store gains
the `worker_health` event and a per-node map; `src/lib/types.ts` gains the event
variant and the nullable fields. The frontend has no test runner in this repo, so
`pnpm build` (typecheck + build) is the gate, and the behaviour is covered by the
core-side tests.

### D-E8 — One worker, one poll, no surprises

Each poll cycle is independent: the poller walks the registry snapshot, dials each
node with a 2 s timeout, and continues past failures. A worker that dies mid-cycle
cannot stall the loop or lose the other nodes' snapshots. Nodes that leave the
registry are dropped from the cache (their cards disappear with them).

### D-E9 — Epic edge

The epic ends when the dashboard shows a real master-RAM figure and real per-worker
temperature/battery, with "unknown" and "stale" honestly distinguished, verified
device-free against mock workers. What stays open on the other side of the edge:
telemetry history in SQLite, thermal-aware scheduling (e.g. "do not place layers on a
phone above 40 °C"), the iOS agent's thermal publishing, and the real values on a
physical phone (hardware-gated, tasks.md §6 checklist).

## Commands

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test -p ai-upcycler-core -p ai-upcycler-worker
pnpm build                                          # frontend typecheck + build
```

## Project Structure

**Created**

- `core/src/worker_health.rs` — `WorkerHealth`, `WorkerHealthCache`, the poll loop.
- `core/tests/worker_health.rs` — poller/cache/metrics integration tests over mock
  workers.
- `mobile-worker/android/.../worker/rpc/...` — unchanged (the Kotlin worker already
  publishes real values; only the Rust worker needs the sentinel fix).

**Modified**

- `core/src/rpc/engine.rs` — `dial_worker` helper (D-E4).
- `core/src/api/cluster.rs` — `ProbeBenchmarker` uses the shared helper (no behaviour
  change).
- `core/src/telemetry.rs` — `WorkerHealth` event variant; `WorkerTelemetry` and
  `GenerationMetrics` gain nullable fields (D-E3, D-E5).
- `core/src/api/chat.rs` — `publish_metrics` reads the cache; master RAM utilization
  (D-E1, D-E5, D-E6).
- `core/src/api/mod.rs` — `AppState` gains the cache; router unchanged otherwise.
- `src-tauri/src/lib.rs` — spawn the poll loop (D-E1).
- `worker/src/engine.rs` — `get_health_status` reports `-1.0` for battery/temperature
  it cannot measure (D-E3).
- `src/lib/types.ts`, `src/store/cluster.ts`, `src/components/NodeCard.tsx`,
  `src/components/DashboardView.tsx` — new event, nullable fields, honest rendering
  (D-E7).
- `docs/api.md`, `docs/tasks.md`, `docs/plan.md` — the new event, the ticked row, the
  roadmap note.

## Testing Strategy (Definition of done)

1. **Cache**: insert/read; a never-seen node has no entry; age is computable so
   staleness is derivable.
2. **Poll, measured path**: a mock worker serving `GetHealthStatus` → after one
   cycle the cache holds the mock's values and a `worker_health` event reached a
   subscriber. *Mutation check:* change the mock's values → the test fails (proving the
   numbers came from the wire).
3. **Poll, unreachable path**: a node whose address refuses connections → no crash,
   the loop continues to the next node, and the previous snapshot (if any) is retained.
4. **Sentinel mapping**: a worker reporting `-1.0` temperature/battery → the published
   value is `null`, never `0.0`.
5. **No per-token RPC**: a generation against a mock worker produces **no** health
   call in the mock's RPC log (the mock records every call) — the hot path reads the
   cache.
6. **Generation metrics**: `worker_nodes` carries the cached real values; with an
   empty cache it carries unknowns, not zeros.
7. **Master RAM**: an injected memory reader yields the expected utilization; a
   failing reader yields `null`.
8. **Rust worker sentinel**: `get_health_status` reports `-1.0` for battery and
   temperature (unit test on the worker).
9. **Frontend**: `pnpm build` green; the new event is typed and handled.
10. All gates green; PR-Agent guide comment present; mutation checks run on any
    "this number is real now" claim.

**Hardware-gated (documented, not CI-asserted)**: real battery/thermal values from a
physical Android worker — added to the on-device checklist (tasks.md §6).

## Boundaries

- No SQLite schema change, no telemetry history.
- No scheduler behaviour change; telemetry is observed, not acted upon.
- No new HTTP endpoint; the websocket carries the new event.
- No health RPC on the generation hot path (D-E1).
- One new dependency edge (`core` → `sysinfo` 0.39, already in the workspace graph);
  no other new dependencies.
- Poll cadence and timeout are constants, not configuration.

## Success Criteria

- The dashboard's master-RAM figure is a real measurement, and "unknown" never renders
  as "0%".
- Each worker's card shows its real temperature and battery, or "—" when the worker
  cannot measure them; a stale snapshot is visibly stale.
- The master reads worker health only from the background poller — the generation path
  makes zero health RPCs.

## Open Questions

- Should the poll cadence adapt (faster while a generation is running)? (Deferred —
  5 s is enough to start; adaptive cadence is a tuning decision with no data yet.)
- Should the poller also surface the worker's free RAM on the card? (Deferred — it is
  already in the snapshot and in the topology's pooled-RAM view; the card shows
  temperature and battery only.)
