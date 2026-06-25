# Phase 3 plan — Scale & net

> **Status: IN PROGRESS.** Phase 1 (vertical slice, D22) and Phase 2 (game systems,
> D23–D26) are done. Phase 3 makes the game hold up *at size* and *over the wire*. This
> doc is the synthesis of a four-way scouting pass over the four roadmap bullets; it is
> the product-of-record plan, sequenced by blast radius. Like the Phase 1 / Phase 0.5
> plans, it is updated as slices land and signed off at the end.

Phase 3 has four workstreams (`roadmap.md` §"Phase 3 — Scale & net"):

| # | Workstream | Risk | Needs a decision first? |
|---|---|---|---|
| A | **Scale & perf** — 200-unit stress, profiling, job-system, dual-rate re-eval | Low→Med | No (measure-first) |
| B | **Lockstep netcode** — input-delay exchange, avatar-local prediction (D15), CI | **High** | **D27 decided** (topology locked; code not yet landed) |
| C | **Reconnect / snapshot / handoff** — authoritative serialize + resume | Med→High | **D28 decided** (format locked; code not yet landed) |
| D | **PvP attention mind-game** — enemy detection of "gone dark" | Low (mostly design) | **Yes — Q2 via `/decision`** |

The load-bearing finding from the scouting pass: **every workstream has a pure-`core`,
no-network, fully-testable first slice.** The riskiest code (the wire, prediction) is
deferred behind in-process deterministic doubles, so most of Phase 3 is *additive
plumbing around an already-correct deterministic core* — the safest possible shape.

---

## Dependency order

```
A (scale/perf) ────────────────┐  independent; measure-first; unblocks the dual-rate call
                               │
B (lockstep netcode) ──┬───────┼──> C (reconnect/snapshot) ──> handoff
                       │       │
                       └───────┴──> D (PvP detection)  (needs 2-client lockstep to be real)
```

- **A is independent** and goes first — it's instrumentation, decision-free, and its
  numbers decide the dual-rate question (D21 re-eval) and whether parallelism is even
  needed.
- **B blocks C and D.** Land the in-process lockstep loop first; everything net-facing
  hangs off it.
- **C's pure-core slice** (authoritative serialize + round-trip-replay test) has *zero*
  net dependency and can land alongside A.
- **D is last** and starts with a `/decision`, not code.

---

## Workstream A — Scale & performance

**Goal:** know the real 60 Hz per-tick cost at 200 units on target hardware, then fix
the algorithmic bottlenecks before reaching for threads.

Grounding (read from the code): the sim is single-threaded, fixed-order
(`core::sim::Sim::step`: orders → combat → territory → economy). Two predicted hot
loops at scale, both **algorithmic, not parallelism** problems:
- `core::flow_field::FlowField::build` is rebuilt **per moving unit per tick** (a full
  128×128 integer Dijkstra). At ~200 movers this is the #1 cost — the module doc already
  flags "Phase 2 will cache one field per distinct goal."
- `core::combat::acquire_target` (FireAtWill) is O(n) per shooter → **O(n²)** overall,
  each call doing a `terrain.line_of_sight` DDA.

**Sequence (each commit green dev+release, path-scoped):**
1. **Stress harness** ✅ **DONE** — `sim-runner` scenario selector (default `phase2` scene
   byte-identical, verified) + a deterministic `stress` / `stress:<n>` scene (~200 units,
   camps producing, contested points, mixed orders, one embodied). Determinism-at-scale.
2. **Timing mode** ✅ **DONE** — `sim-runner --time` prints per-tick wall-clock stats
   (min / median / p99 / max) to **stderr** (checksum stream on stdout unchanged).
   Host-side `Instant` only. *Measured: 200 units ~30 ms/tick on desktop — ~2× over the
   16.6 ms 60 Hz budget.* (Android adb-profile path still TODO.)
3. **criterion benches** — *deferred*: the `--time` harness already produced the
   actionable scaling number (step 2), which was enough to act. Add isolated
   `FlowField::build` / `combat_system` benches if a finer breakdown is needed later
   (dev-dep on `sim-runner`, **never** in `core`).
4. **Flow-field caching** ✅ **DONE** (`flow_field`/`orders`/`systems`) — one field per
   distinct goal per tick (`FlowFieldCache`), bit-identical to the per-unit rebuild
   (phase2 stream byte-identical; equivalence test; determinism-auditor + code-reviewer
   clear). **Re-measured: ~30.4 ms → ~3.7 ms median (~8×), p99 ~3.9 ms — under budget.**
   This likely removes the need for sim-side parallelism (step 6) in Phase 3.
5. **Spatial hash for target acquisition** (`combat` + new `core::spatial`) — only if a
   re-profile shows combat is now the wall (with the flow-field fixed, the tick is now
   ~3.7 ms, so this may be unnecessary). Equivalence + lowest-index-tie-break test.
   `/safe-edit`.
6. **rayon parallelism** — **only if still over budget on mid-range arm64.** Pattern:
   *parallel pure-read phase → deterministic serial ordered-write phase* (never
   `par_iter_mut` a system that writes other entities' slots — combat damage application
   is order-observable via `last_attacker`/kill attribution). Needs **a new decision** —
   rayon would be `core`'s first non-empty dependency. Feature-flagged; gated by a
   single-thread≡multi-thread checksum-equality test. `/safe-edit`.
7. **Dual-rate re-evaluation (D21)** — run the on-device `--time` thermal/sustained
   measurement. If p99 sits comfortably under the 16.6 ms 60 Hz budget → `/decision`
   confirming global-60 (close the re-eval). If over budget after 4–5 → quantify the
   RTS-bulk vs embodied-combat split before adopting dual-rate's two-clock complexity.

**CI:** the stress scene wants cross-arch determinism coverage, but the `compare` job in
`determinism.yml` currently requires **all** `checksums-*.txt` to be identical — so the
stress stream must be a **separately-grouped** comparison, not dumped beside the phase2
stream. That's an ADD-ONLY edit to the diff logic (invariant #7) and is its own careful,
`/safe-edit`-gated step — **not** bundled with the harness commit.

---

## Workstream B — Lockstep netcode (HIGH RISK)

The deterministic substrate is already complete and the seams are already cut:
`core::sim::Command` *is* the lockstep order unit (`Copy`, float-free); `Sim::step(&[Command])`
already applies a per-tick command set in stable order; `Sim::checksum` already folds
everything incl. RNG state; and `engine::predict_avatar` already exists as a stub with
the D15 "MUST NOT feed back into the sim" rule in its doc-comment.

**Crate topology (decided in [`D27`](decisions.md#d27--netcode-topology-deterministic-lockstep-in-core-transport-behind-a-pal-trait); code is next):**

| Concern | Lives in |
|---|---|
| Lockstep loop (command-delay buffer, per-tick set assembly, gate/stall, wire codec, checksum-agreement) | **`core::lockstep`** (new; platform-free, deps stay empty) |
| Transport trait (opaque byte frames, no socket type) | **`pal::Transport`** (new; mirrors `pal::Audio`) |
| Concrete transport (UDP/QUIC/relay), matchmaking, relay | **`pal-desktop`** + **`server`** |

Note for C/handoff: **QUIC connection migration** survives a Wi-Fi↔cellular switch
without a full reconnect — a strong input to the transport choice in D27.

**Avatar-local prediction (D15):** the `engine` crate owns it, presentation-only, in new
`Game` fields parallel to the existing `yaw: f32` — `predict_avatar` only ever takes
shared refs to the snapshot/world, never `&mut Sim`. The load-bearing guard is *"sim
checksum byte-identical with prediction on vs off."*

**Sequence (every step `/safe-edit`; sim/netcode/embodiment blast radius):**
1. **Smallest first slice — in-process deterministic 2-client lockstep, NO sockets.**
   ✅ **DONE** — `core::lockstep` (D27): the `Command`/tick **wire codec** (LE +
   `Fixed::to_bits`, mirrors the checksum; decode never panics, rejects bad
   version/tag/peer/trailing-bytes) and the **sans-I/O** `Lockstep` state machine (input
   stamped at `T+delay`, peers merged in fixed peer order, gate-clears-only-when-all-present,
   empty set is the explicit "proceed" signal, warmup `[0,delay)`, pruned retransmit window).
   It produces/consumes byte frames — no transport, no `pal` dep. Tests (9, dev+release, on
   the determinism matrix): codec round-trip over all variants, malformed-frame rejection, the
   gate/merge logic, and the **two-instance lockstep run over a seeded lossy+jittery+reordering
   in-process channel** asserting both peers' checksum streams agree *and* match a no-network
   reference, across several delays. `phase2` sim-runner stream byte-identical. (Refines D27:
   `core::lockstep` is sans-I/O, not a `&mut dyn Transport` consumer — keeps `core` off `pal`.)
2. **CI: `net-sim-runner` + a new ADD-ONLY job** ✅ **DONE** — the headless
   `net-sim-runner` crate drives **two** in-process `core::lockstep` peers over a seeded
   deterministic channel (peer 0 = player commands, peer 1 = enemy, so the fixed-peer-order
   merge is exercised), asserts both peers agree on every per-tick checksum *and* match a
   no-network single-`Sim` reference (exit 1 on any desync), and emits the agreed
   `<tick> <checksum>` stream (`pnpm desktop:sim:net`). `determinism.yml` gains an **ADD-ONLY**
   `net-checksums-<target>` artifact + a separate `compare-net` job — the existing single-client
   `checksum`/`compare` jobs are untouched (invariant #7). 6 tests, dev + release.
3. **`pal::Transport` trait** + loopback impl in `pal-desktop` ✅ **DONE** — `pal::Transport`
   (opaque byte frames: `send(&[u8])` / `poll() -> Vec<Vec<u8>>`, object-safe, names no socket
   type; mirrors `pal::Audio`, [D27](decisions.md)) plus an in-process `LoopbackTransport::pair()`
   in `pal-desktop` (per-direction FIFO, byte-exact framing). Trait + impl + 6 tests only — wiring
   into `engine::Game` is step 4.
4. **Wire lockstep into `engine::Game::frame`** ✅ **DONE** — the fixed-tick accumulator now
   drives each tick through a `core::lockstep::Lockstep` (the per-tick command set comes from
   `try_advance`, not directly from local input) via an extracted wgpu-free `drive_lockstep`
   seam (submit → pump transport → step). Single-player keeps working bit-identically via a
   1-peer, **delay-0** session with a `None`/`NullTransport` (no input latency, no socket);
   `Game::new`/`frame` signatures unchanged so `app`/`pal-android` need no edits. The
   load-bearing guard test asserts the lockstep-driven single-player checksum stream is
   identical to direct stepping. (Multiplayer per-frame submit *pacing* for `delay > 0` is
   left to step 7.) `engine` tests 33 → 43, dev + release.
5. **Fill in `engine::predict_avatar`** — presentation-only predict + reconcile; the
   byte-identical-checksum guard test. Highest-risk single commit (`audit-determinism`).
   *(Next; sequential after step 4 — same file, reconciles against the tick step 4 sources.)*
5. **Fill in `engine::predict_avatar`** — presentation-only predict + reconcile; the
   byte-identical-checksum guard test. Highest-risk single commit (`audit-determinism`).
6. **android-arm64 + ios-arm64 device entries** (the `determinism.yml` TODO) + runtime
   cross-client checksum-agreement broadcast.
7. **Concrete UDP/relay transport** + RTT-driven adaptive delay `D`. Last, lowest-risk
   to correctness (validated against the in-process double).

---

## Workstream C — Reconnect / snapshot / handoff

**The two-snapshots distinction is load-bearing.** `core::snapshot` is the *render*
snapshot: lossy (alive units only, `health.fraction()`, no RNG, no free-list), **not**
checksummed, **unfit for resume**. This workstream needs a *new* **authoritative
serialization** — every bit needed for a bit-identical resume.

What must be captured (enumerated from the code): all `World` component arrays incl. dead
slots, the liveness triple (`generation` / `alive` / `free` — free-list *order* decides
the next spawn's slot, so a wrong order desyncs instantly), `Resources`, `Territory`,
**`Rng (state, inc)`** (the single most important non-obvious field — omit it and the
draw-count divergence the checksum exists to catch is guaranteed), and `tick`. `events`
are transient — exclude. Terrain → serialize a `map_id`, not the grid (it's static).

**Highest-value structural safeguard:** refactor so `Sim::checksum` and `Sim::serialize`
share one field-walk (`fn fold<S: StateSink>`), so *anything added to the checksum is
serialized for free*. This refactor of `Sim::checksum` is the one determinism-sensitive
change → `/safe-edit`. Format: a hand-rolled LE `Writer`/`Reader` generalizing the
existing `Checksum` byte discipline — **no serde/bincode in `core`** (keeps deps empty;
`Fixed` crosses as `to_bits()`, never float).

**The core invariant (the headline test):** serialize@T → deserialize → replay
`cmds[T..L]` yields a checksum stream **bit-identical** to the never-interrupted run on
every arch. Because it lives in `core`'s test module, it rides the existing arch matrix
automatically. Reconnect then = snapshot + replay-buffered-commands (a plain `step` loop)
— correct *by construction* once the round-trip invariant holds.

**Format decided in [D28](decisions.md#d28--authoritative-snapshot-format-a-hand-rolled-le-serialization-sharing-the-checksum-walk)**
(hand-rolled LE `Writer`/`Reader` sharing the checksum field-walk; `Rng(state, inc)` captured;
terrain by `map_id`; serde-free in `core`) — the first slice is now **unblocked**.

**First slice (no net dependency)** ✅ **DONE** — `core::persist` (hand-rolled LE `Writer`/`Reader`,
serde-free), `Sim::serialize/deserialize` driving a shared `fold<S: StateSink>` walk (so the
checksum bytes are unchanged — both runner streams byte-identical — while serialize additionally
captures the resume-only liveness triple `generation`/`alive`/free-list **order**), `Rng::from_state`,
terrain by `map_id` (unknown ids rejected **loudly**, not silently defaulted), and the headline
round-trip-replay determinism test (serialize@T → deserialize → replay `cmds[T..L]` bit-identical to
the uninterrupted run, riding the arch matrix). `core` deps stay empty, float-free. A
`determinism-auditor` pass confirmed no Critical/High hazards. `core` tests 141 → 151, dev + release.
**Still owed for workstream C:** the reconnect *policy* + Wi-Fi↔cellular handoff (consumes this
slice + `core::lockstep`'s command buffer); these are net-facing and follow workstream B.

---

## Workstream D — PvP attention mind-game (design-led, LAST)

The live fork is **Q2** (`open-questions.md`): *can the enemy tell you've gone dark?* —
options: no signal / soft tell (marked hero unit) / no tell at all; current lean
undecided, "soft tell" most interesting but needs playtest. Bounded by **Q1** (how thin
the thread back to command; lean: alerts-only) and **Q3** (possession leashed vs global).
The shipped *posture* today is "no tell" by omission. **Resolving Q2 is a `/decision`
co-authored with the user — the first step, before any code.**

The deliverable mirrors the Phase 2 house style (D23/D26): ship a **tunable mechanism,
not a locked design.** A `core::detection::DetectionConfig` (a `tell_mode:
Hidden|Subtle|Marked` switch + fixed-point `tell_range` / `tell_linger_ticks`) drives a
**pure, checksum-excluded derivation** `detectable_embodiment(...)` — same side of the
line as fog/alerts, never sim state, LoS-gated via the existing `terrain.line_of_sight`.
One build covers all three Q2 options for A/B playtesting. The same config bounds the
**PvE AI's permitted knowledge** so "no omniscient peek" (invariant #3, D2) is structural
— the load-bearing test: *AI behavior bit-identical whether or not a player is embodied,
in Hidden mode.*

Single-client now: the config, the derivation, its tests, a scripted enemy. Genuinely
needs the net layer: the *actual* two-human mind game.

---

## Decisions Phase 3 will need (record each via `/decision`)

- **D27 — netcode topology** ✓ **DECIDED** (lockstep loop + wire codec in `core::lockstep`;
  transport behind `pal::Transport`; sockets in `pal-desktop`/`server`). `architecture.md`
  §Netcode updated. *Code not yet landed; unlocks workstream B.*
- **D28 — authoritative snapshot format** ✓ **DECIDED** (hand-rolled LE `Writer`/`Reader`
  sharing the checksum field-walk; `Rng(state, inc)` captured; terrain by `map_id`; serde-free
  in `core`). [`decisions.md`](decisions.md#d28--authoritative-snapshot-format-a-hand-rolled-le-serialization-sharing-the-checksum-walk)
  §D28. *Format locked; the `core::persist` slice is unblocked, code not yet landed.*
- **Dn — Q2 resolution** (enemy detection of "gone dark"), or an explicit "ship the
  tunable mechanism, defer the lock" entry. *Gates workstream D.*
- **Dn — D21 dual-rate re-evaluation outcome** (confirm global-60, or adopt dual-rate
  with the two-clock contract), informed by workstream A's on-device numbers.
- **Dn (conditional) — rayon into `core`** if A's measurements prove parallelism is
  needed (justifying the first non-empty `core` dependency against invariant #2).
</content>
</invoke>
