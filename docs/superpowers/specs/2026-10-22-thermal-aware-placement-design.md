# Thermal-Aware Layer Placement — Design (2026-10-22)

> **Status:** spec.
> **Tracks:** the note left by the live-worker-telemetry epic — "nothing acts on
> telemetry yet" (`docs/superpowers/specs/2026-10-22-live-worker-telemetry-design.md`,
> D-E9) — and the cluster's behaviour on a hot phone.
> **Epic edge:** the cap, the reporting and the wiring are implementable and
> verifiable device-free; tuning the thresholds on real phones is hardware-gated.

## Intent

The master now measures every worker's CPU temperature every 5 s (spec
2026-10-22). The layer planner ignores it: `LayerScheduler::compute_plan` fills
nodes greedily in list order, so a phone at 46 °C gets exactly the same share of
layers as a phone at 30 °C. That is the wrong call on a phone — Android starts
throttling hard when it gets hot, so the hot node slows the whole cluster down,
and it is also the node most likely to be damaged by sustained load.

This epic makes the placement thermal-aware: a worker that is already hot is
**capped**, so it rests while the cool ones carry the load, and the plan always
completes — if the cluster genuinely needs the hot phone, the cap is relaxed and
the plan says so.

Out of scope: draining a hot worker mid-session, battery-aware placement, and any
rewrite of the planner's greedy fill.

## Decisions (D-F1…D-F8)

### D-F1 — The planner takes the temperatures as an explicit input

`compute_plan` gains a `&ThermalHints` argument — a map from node id to the last
measured temperature — so the planner stays a pure function and its tests need no
clock, no cache and no worker. The caller supplies the numbers: the API reads them
from the telemetry cache, which the poll loop refreshes every 5 s (spec 2026-10-22,
D-E1). **No measurement means no penalty**: a node the master has never polled (or
one whose worker cannot measure temperature) is treated as unknown, never as hot.

### D-F2 — A hot worker rests, and the cap relaxes only as far as it must

A worker at or above `HOT_CELSIUS` (40 °C) is hot. The planner then tries the hot
worker's share in graded steps, taking the **first** one at which the whole model
fits:

1. **0%** — the hot worker rests entirely and the cool ones carry the load.
2. **`HOT_LAYER_SHARE` (50%)** — the hot worker takes half of what its free RAM
   would allow.
3. **100%** — the hot worker takes what it must, and the plan records that the cap
   was relaxed.

Each step is "the minimum the cluster needs", so a hot phone rests whenever the
cluster can afford it, and is used only when the model would otherwise not fit. A
hot worker is never *excluded* from the plan outright: that would leave the model
unpartitioned and silently fall back to the host backend, which is a worse outcome
than using a warm phone.

> **Amendment (2026-10-22, implementation slice 1).** This was first written as a
> flat 50% cap with an all-or-nothing relaxation. Two problems with that showed up
> while implementing it: a phone resting at half share is barely resting (it keeps
> heating), and an all-or-nothing relaxation hands a hot, large node *everything*,
> defeating the cap. The graded steps above fix both, and keep the promise that
> matters: the plan always places the model, and it reports what it had to give up.

40 °C is a policy constant, not a measurement: it is where a phone is plainly warm
and throttling becomes likely. It is a constant rather than configuration because
there is no real data yet to tune it against (D-F8).

### D-F3 — The plan never fails because of temperature

The plan always places the model. If the graded steps (D-F2) cannot place it, the
last step — the hot worker taking what it must — is used, and the plan records that
the relaxation was needed (`thermal_relaxed`). Resting is best-effort: the cluster
always works, and the operator can see that it worked by working *harder* than it
wanted to.

### D-F4 — The plan reports the thermal state it used

`PartitioningPlan` gains `thermal: Vec<NodeThermal>` — one entry per node with a
measurement: `node_id`, `temp_celsius`, and whether the node was `capped` — plus
`thermal_relaxed: bool`. The topology response carries the same information, so the
dashboard can say *why* a warm phone received fewer layers instead of the operator
guessing. Both additions are additive: existing fields keep their meaning.

### D-F5 — The master is never capped

The poll walks the discovered workers, so the master has no measured temperature —
unknown means uncapped (D-F1). That is also the right default: the host is where
the RAM headroom lives, and the phones are the constrained, heat-sensitive part of
the cluster.

### D-F6 — Mid-session is out of scope

The plan is computed when a model is loaded. A worker that heats up *afterwards*
keeps its layers until the next load or unload. Draining a hot worker mid-session
means re-planning and re-loading a live pipeline while a request may be in flight —
a materially bigger change with real correctness risk, and it belongs in its own
spec with its own tests.

### D-F7 — Temperature only, not battery

A phone at 10% battery should rest too, but that is a second policy with different
semantics (battery is a resource budget, temperature is a health signal) and a
different failure mode (a phone at 10% may be the only node that can hold the
model). Deferred until there is a real measurement of how often each case bites.

### D-F8 — The thresholds are constants, and tuning them is hardware-gated

`HOT_CELSIUS = 40.0` and `HOT_LAYER_SHARE = 0.5` are documented constants. The
right values depend on real phones' thermal behaviour, which is hardware-gated
(tasks.md §6); until then, a conservative threshold and a visible report beat a
tuned guess.

## Commands

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test -p ai-upcycler-core -p ai-upcycler-worker
pnpm build
```

## Project Structure

**Modified**

- `core/src/scheduler/planner.rs` — `ThermalHints`, the cap, the relaxation pass,
  the reporting; planner unit tests.
- `core/src/api/distributed.rs` — the cluster serving path passes the measured
  temperatures into the plan.
- `core/src/api/cluster.rs` — the topology view passes them too, and surfaces the
  thermal state in the response.
- `core/src/api/mod.rs` — the `AdaptiveBackend` and the topology handler reach the
  telemetry cache.
- `docs/api.md` — the topology response's new field.
- `docs/tasks.md`, `docs/plan.md` — the tracker rows.

**Created**

- Nothing: the planner grows a small, well-named input type rather than a new
  module.

## Testing Strategy (Definition of done)

1. **The cap (planner unit test)**: a cool node (30 °C) and a hot node (46 °C) with
   equal RAM → the hot node's assignment is at most half its uncapped share, and the
   plan still assigns every layer.
2. **Unknown is not hot**: a node with no measurement keeps its full share — the
   existing planner tests are unchanged and still pass.
3. **The relaxation (planner unit test)**: every node hot and the model too large
   for the capped shares → the plan still assigns every layer, and
   `thermal_relaxed` is true.
4. **Reporting (planner unit test)**: the plan lists the measured temperature and
   the `capped` flag per node, and leaves nodes without a measurement out.
5. **Wiring (API test)**: with a mock worker reporting 46 °C, the topology response
   shows the worker capped; with the same worker cool, it is not capped.
6. **The cluster path**: the distributed plan uses the same cap (mock workers, no
   hardware).
7. **Mutation check**: the cap is removed and the tests fail; the cap is restored
   and they pass. (The cap is the whole point of the change, so this is the
   property to guard.)
8. All gates green; the PR-Agent guide comment is present.

**Hardware-gated (documented, not CI-asserted)**: the real threshold on a real
phone — how hot a phone gets under a layer share, and whether 40 °C is the right
line — added to the on-device checklist (tasks.md §6).

## Boundaries

- No mid-session draining, no battery policy (D-F6, D-F7).
- No rewrite of the greedy fill; the planner's algorithm is unchanged apart from
  the per-node cap.
- No new endpoints; the topology response gains fields, it does not change shape.
- No configuration surface for the thresholds (D-F8).
- The master is never capped (D-F5).

## Success Criteria

- A worker measured at or above the hot threshold demonstrably receives a smaller
  share of the layers in the plan, while the plan still assigns every layer.
- The operator can see the reason: the topology response reports each measured
  temperature and whether it was capped, and whether the cap had to be relaxed.
- Nothing about the cluster's ability to serve breaks: every plan assigns all
  layers, with or without the cap.

## Open Questions

- Should the cap be per node (as now) or cluster-wide (one hot phone caps the whole
  cluster's ambition)? (Per node for now — a cluster-wide cap would need a notion
  of "the cluster is hot" that no measurement supports yet.)
- Should a capped node's *existing* layers be unloaded when it heats up mid-session?
  (Deferred — D-F6.)
- Is 40 °C the right line for a phone that is also charging? (Hardware-gated; a
  charging phone is warmer, and the constant may need to be per-platform.)
