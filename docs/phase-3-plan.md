# Phase 3 plan — Scale & net

> **Status: IN PROGRESS — most of the codeable surface has landed.** Phase 1 (vertical slice,
> D22) and Phase 2 (game systems, D23–D26 + D29–D30, signed off systems-complete in D31) are done.
> Phase 3 makes the game hold up *at size* and *over the wire*. This doc is the synthesis of a
> four-way scouting pass over the four roadmap bullets; it is the product-of-record plan, sequenced
> by blast radius, updated as slices land and signed off at the end.
>
> **Landed so far:** **A** — stress harness, timing, flow-field caching (~8× → ~3.7 ms/tick),
> spatial-hash target acquisition, and the cross-arch stress-determinism CI job. **B** — the full
> in-process→wire lockstep stack (`core::lockstep`, `net-sim-runner` + `compare-net` CI,
> `pal::Transport` + loopback, engine wiring, avatar prediction, checksum-agreement broadcast, the
> **UDP** transport, and **RTT-adaptive delay** via the agreed `DelayChange` protocol). **C** — the
> authoritative snapshot (D28) and the **reconnect policy** (snapshot + buffered-command replay).
> **Still open:** **A** — rayon-into-`core` (needs a decision *and* is unjustified at ~3.7 ms) and
> the dual-rate re-eval (D21, needs on-device thermal numbers); **B** — the host-side RTT
> estimator wiring + relay/matchmaking ([Q9](open-questions.md)); **C** — the Wi-Fi↔cellular
> **handoff** (blocked on a QUIC transport); **D** — the PvP detection mechanism (blocked on a
> **Q2** `/decision`). The remaining items are gated on decisions or a physical device, not on more
> code.

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
5. **Spatial hash for target acquisition** ✅ **DONE** (`combat` + new `core::spatial`) — a
   deterministic `GRID`×`GRID` bucket grid reusing `flow_field`'s exact integer cell mapping
   (no floats, `core` deps stay empty) replaces the O(n²) `FireAtWill` scan. **Bit-identical to
   the brute-force pick** — the query's `(dist_sq, idx)` lexicographic comparator reproduces the
   same min-distance/lowest-index target independent of cell-visit order, and `can_engage` stays
   the sole range/LoS filter (phase2 `41e4d81992787504` + stress streams byte-identical;
   equivalence test vs a brute-force oracle over a seeded field + cross-bucket tie-break test).
   Rebuilt per tick, not sim state. Pure-perf structure (the ~3.7 ms tick didn't require it, but
   it's the right shape for the 200-unit/mid-range-arm64 target and reusable by fog/alerts/D).
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

**CI:** ✅ **DONE** — the stress scene now has cross-arch determinism coverage. Because the
`compare` job requires **all** `checksums-*.txt` to be identical, the stress stream rides a
**separately-grouped** `stress-checksums-<target>` artifact + a `compare-stress` job (mirroring
the net `compare-net` pattern), so it is diffed across arch independently of the phase2 and net
groups. ADD-ONLY (invariant #7): the existing `compare`/`compare-net` jobs are untouched, and the
`stress-` prefix is excluded from the `checksums-*` glob.

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
   identical to direct stepping. (Multiplayer per-frame submit *pacing* for `delay > 0` is part of
   the step 8 host-RTT wiring still owed.) `engine` tests 33 → 43, dev + release.
5. **Fill in `engine::predict_avatar`** ✅ **DONE** — replaced the stub with an `AvatarPrediction`
   (presentation-only): the embodied eye **leads** the authoritative ticks (`extrapolate_avatar`,
   by the avatar's authoritative velocity) and **reconciles** toward each tick (`reconcile_avatar`,
   ease + snap-past-threshold) instead of snapping; the first-person camera + audio listener read
   the predicted eye, aim (`yaw`) stays the local-instant value. The type never reaches `&mut Sim`,
   so it cannot desync (D15, invariant #1) — guarded by a byte-identical-checksum test (prediction
   on vs off) + pure predict/reconcile unit tests; **code-reviewer + determinism-auditor both
   clean**. `engine` tests 43 → 49, dev + release. *Honest scope:* embodied locomotion isn't a sim
   command yet and single-player runs at delay 0, so today's visible effect is sub-tick eye
   smoothing — the **boundary** is what lands now (as D15/`architecture.md` mandate: it goes in at
   the first netcode commit), ready for multiplayer delay + authoritative embodied motion. Two LOW
   feel-polish follow-ups noted in code (dt-independent smoothing; arch-stable ease/snap boundary).
5. **Fill in `engine::predict_avatar`** — presentation-only predict + reconcile; the
   byte-identical-checksum guard test. Highest-risk single commit (`audit-determinism`).
6. **Runtime cross-client checksum-agreement broadcast** ✅ **DONE** — the lockstep wire codec
   is now a tagged union (`version,kind` prefix; `WIRE_VERSION` 1→2), adding a **Checksum** frame.
   `Lockstep::record_checksum` (host records its post-`step` checksum into a bounded window),
   `drain_outbound` emits checksum reports (loss-tolerant resend), `deliver` compares an inbound
   report to the local one and queues a `Desync{tick,peer,local,remote}` (drained via
   `take_desyncs`, deduped per `(tick,peer)`); a `delay()` accessor was added. **Detection only —
   never alters stepping** (determinism-auditor confirmed clean separation; no false positives;
   streams byte-identical). `net-sim-runner` exercises it + asserts an injected divergence is
   caught. `core` 151→161 tests. **device CI status:** `aarch64-unknown-linux-gnu` +
   `aarch64-apple-darwin` already cover the Android/iOS **ship ISAs at the sim level**; a real
   on-**device** run (Android emulator/adb, iOS) is deferred to device-farm CI (GitHub Actions
   can't host it), and **iOS is additionally blocked until an iOS build target exists** — recorded
   in the `determinism.yml` comment, no flaky jobs added.
7. **Concrete transport** ✅ **DONE (UDP half)** — a real `UdpTransport` (`std::net::UdpSocket`,
   zero new deps) implements `pal::Transport` in `pal-desktop` (the real-socket sibling of the
   in-process `LoopbackTransport`): one frame ↔ one datagram, non-blocking drain, never-panic on
   socket errors (UDP loss is the lockstep retransmit window's job). `UdpTransport::pair()` gives a
   connected localhost pair for tests (13 tests, dev+release). **UDP first** per the plan (swappable
   behind the trait); **QUIC stays the documented future** (D27's Wi-Fi↔cellular lean).
8. **RTT-adaptive delay `D`** ✅ **DONE** — the mid-session delay change is now a *deterministic,
   agreed* protocol event, not a local decision. A new `DelayChange` wire frame (`FrameKind=2`,
   `WIRE_VERSION` 2→3) ships `(effective_tick, seq, new_delay)`; every peer applies the identical
   change at the identical tick. `propose_delay(new_delay, guard)` is the host API — `core` reads no
   clock, sees only integers (RTT proposes; the protocol commits). The load-bearing refactor
   decouples `submit_tick` (now a monotonic `next_submit_tick`) and warmup (an immutable
   `warmup_until`) from `delay`, so a change touches **only** the prune-window size — it can never
   re-stamp/drop a command or stall. The no-change path is byte-identical (net stream
   `2684f7afb6e334e5` unchanged); headline tests drive a mid-run increase (under loss) + decrease,
   asserting both peers stay checksum-identical to each other *and* to a no-change reference;
   determinism-auditor clean (no float/clock, no command-stream desync). **Still owed:** the
   host-side RTT estimator + hysteresis that *calls* `propose_delay` (thin `pal-desktop`/`engine`
   wiring, low determinism risk); relay / matchmaking ([Q9](open-questions.md)) untouched.

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
**Reconnect policy** ✅ **DONE** — `core::lockstep` gained an `executed` merged-command buffer
(captured in `try_advance`), a `retain_floor` + `retain_from(tick)` (the host installs it when it
snapshots), and `replay_range(from, to)` (returns `None` **loudly** if any tick was pruned, never a
silent short replay). `core::reconnect::resume_from_snapshot` drives `Sim::deserialize` + the replay
loop, rejecting a malformed/wrong-base snapshot, a pruned range, or an invalid `[from, to)` at the
boundary. Correct by construction over the D28 round-trip invariant; the buffer is pure side state
(phase2/stress/net streams byte-identical). Headline test resumes bit-identically across a
production spawn + a `Build` (non-trivial free list) and keeps stepping in lockstep;
`determinism-auditor` clean. `core` tests 177 → 184. **Still owed:** the **Wi-Fi↔cellular handoff**
half — it needs **QUIC connection migration** to survive a network switch without a full reconnect,
and only the UDP transport has landed, so it is **deferred until a QUIC `pal::Transport` exists**
(D28). The reconnect policy itself is transport-agnostic and ready for it.

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
