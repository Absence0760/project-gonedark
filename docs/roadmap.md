# Roadmap

> Build order and milestones. The sequencing reflects the project's three biggest
> risks: **touch controls** (a product risk, not an engine one), **embodied combat feel
> over the network** (the FPS layer rides RTS-optimal lockstep — see Phase 0.5), and
> **determinism** (a correctness risk that gets exponentially harder to retrofit). All
> are pulled as early as possible.
>
> Cross-platform (Windows/Linux/Android/iOS) threads through every phase rather than
> being a phase of its own — see [`platforms.md`](platforms.md). The key rule: the
> Platform Abstraction Layer boundary goes in at Phase 0 so platform code never leaks
> into the core. Develop on Linux desktop; ship Android-first; iOS last (most
> external friction).

## Phase 0 — Control prototype *(do this before anything else)*

> **Status: PASSED (2026-06-23, [`decisions.md`](decisions.md) D14).** The embody↔command
> loop feels good in hand, validated on real hardware (Galaxy S24). Touch-feel risk
> retired; resolves [`open-questions.md`](open-questions.md) Q4. The throwaway prototype
> (`prototypes/phase0-controls/`, a Godot build) has since been deleted on Phase 1
> completion (D22). Two caveats carried into D14: audio is still faked, and embodied feel
> *over the network* is untested — that's Phase 0.5.

**Goal:** prove the core interaction is fun on a touchscreen before building any
systems behind it.

The first risk to hit — and the one this phase exists to kill — is not the engine: it's
whether *CoH*-style command **and** a competent FPS scheme **and** an instant swap
between them feel good on a small touchscreen. If this isn't fun, no amount of engine
work saves it. (Embodied feel *over the network* is the next risk — Phase 0.5.)

- Throwaway prototype (can be in anything fast — even a non-final engine).
- One controllable unit; tap-to-select / order on the command layer.
- Embody → FPS controls → surface, with the swap feeling instant.
- The "world goes dark" vignette + an alert ping, faked.
- **Exit criterion:** the embody ↔ command loop feels good in hand. Kill or rework
  the concept here if it doesn't.

## Phase 0.5 — Embodiment-over-network latency spike *(before the engine spine)*

> **Status: PASSED (2026-06-23, [`decisions.md`](decisions.md) D15).** Embodied combat
> feels good over lockstep **with avatar-local prediction** (raw lockstep felt laggy),
> validated phone-vs-laptop over real Wi-Fi up to a simulated "cellular" link. Resolves
> [`open-questions.md`](open-questions.md) Q7; Q8 (tick rate) resolved in Phase 1 via D16
> (30 Hz too coarse) + D21 (global 60 Hz). The throwaway harness (`prototypes/phase0.5-netfeel/`)
> has since been deleted on Phase 1 completion (D22). Plan: [`phase-0.5-plan.md`](phase-0.5-plan.md).

**Goal:** prove embodied FPS combat feels acceptable under the chosen
deterministic-lockstep + input-delay netcode — *before* committing the full engine.

The netcode is RTS-optimal and FPS-hostile: input delay executes orders a few ticks
ahead, with no prediction/rollback/lag-comp (see
[`architecture.md`](architecture.md) §"Embodied combat over lockstep — the open
tension" and [`open-questions.md`](open-questions.md) Q7/Q8). Phase 0 can't surface
this — it's
single-unit and local. If embodied combat feels laggy over the wire, you want to know
*now*, not after building the ECS, renderer, and systems on top of an unfit netcode
model.

- Throwaway, like Phase 0 — minimal, not the final engine.
- **Two networked clients**, one embodied unit each, fighting under *real* input delay
  (and at the real 30 Hz tick, to test Q8 alongside Q7).
- Try **avatar-local prediction** (predict only your own embodied entity, reconcile
  against the tick) if raw lockstep feels bad — the current lean for Q7.
- **Exit criterion:** a credible path to good embodied combat feel over the net — *or*
  a decision to change the netcode model (Q7) or tick rate (Q8) **before** Phase 1.
  Retrofitting a prediction/rollback boundary into a finished sim is far costlier than
  designing to it.

## Phase 1 — Vertical slice

> **Status: DONE — PASSED ([`decisions.md`](decisions.md) D22).** The custom Rust engine is
> validated end-to-end on real arm64 (Galaxy S24, Adreno 750). **On-device evidence:**
> `pnpm android:checksum` confirmed the device sim-runner checksum stream **bit-identical** to
> desktop over 300 ticks (`4c34c6b5951edf57`); the `adb logcat` FPS heartbeat showed
> **120 fps** sustained at the locked **60 Hz** sim tick — demonstrating sim/render decoupling
> live on hardware. One unit moves via the flow field; tap-to-move works; the two-finger embody
> toggle flips the world dark. All three decide-first gates locked (D17, D18, D21). The
> **Unity/Godot fallback (D8) is retired**; the throwaway prototypes are deleted. **Phase 2
> (game systems) is the active phase.** Honest caveat: validated on a flagship; frame-rate/thermal
> on mid-range silicon and the 200-unit power budget are Phase 3 (D21). Detailed plan and
> sign-off record: **[`phase-1-plan.md`](phase-1-plan.md)**.

**Goal:** the real engine spine, end to end, with one of everything.

- ECS world + scheduler; data-oriented component storage.
- Fixed-tick deterministic sim loop + render interpolation. **Sim rate locked
  ([D21](decisions.md), closing Q10):** 30 Hz was too coarse for embodied combat (D16), so the
  loop runs a single **global 60 Hz** tick (`core::sim::TICK_HZ = 60`) — with one unit on real
  arm64 it has huge headroom, so dual-rate is unjustified now and **deferred to Phase 3** (the
  200-unit thermal re-evaluation), not killed.
- Embodiment as an input-source swap on a single entity; fog → avatar-only on embody. Wire
  the **avatar-local-prediction boundary (D15) from the first netcode commit** — presentation
  path only, never writing sim state.
- Minimal Vulkan renderer (instanced units), camera, top-down view.
- One unit type moving via a flow field on screen.
- **Validate on real arm64 hardware**, not just the emulator.
- **Exit criterion (met — D22):** one unit, commandable and embodiable, running
  deterministically at target frame rate on a target device. Passed on Galaxy S24;
  fallback retired (D22).

## Phase 2 — Game systems

> **Status: IN PROGRESS — systems spine ([`decisions.md`](decisions.md) D23) + host wiring
> ([D24](decisions.md)) landed.** A first,
> fully-deterministic implementation of every bullet below lives in `core` as eight new modules
> (`terrain, combat, economy, territory, fog, orders, alerts, event`): fixed-point combat with
> suppression/cover/line-of-sight, territory capture, resources/economy/camps + production, fog
> of war (a pure client-side derivation, not sim state), the widened order/stance vocabulary with
> a literal-executor + retreat trigger, and the alert channel. All fixed-point, float-free, and
> folded into the per-tick checksum (territory/economy are sim state; fog/alerts are excluded as
> derived presentation). `core` tests grew 57 → 128 (green dev + release); the headless
> `sim-runner` scenario now exercises the systems so the cross-arch determinism matrix covers
> Phase 2. **The host/presentation wiring is now in ([D24](decisions.md)):** fog rendering, the
> embodied alert HUD, the embodied audio mix, and the touch UI (multi-unit selection + the
> order/stance vocabulary on screen) — all pure presentation derivations, so the checksum stream
> stayed byte-identical and the suite grew 149 → 190 tests (green dev + release). **Honest caveats
> (still NOT done):** gameplay **balance** (the cost/time/damage tables are untuned placeholders —
> left for playtesting); real audio *output* (the mix is built + tested, the AAudio/desktop sink is
> still a no-op); a `Command` to set `Patrol`/`HoldPosition`/`FallBack` (the `Order`s exist but the
> touch vocabulary can't reach them yet — a small determinism-sensitive `core`-surface follow-up);
> and the netcode/lockstep layer (Phase 3). Open forks Q1/Q2/Q3 are deliberately left open — fog and
> alerts ship as a *mechanism*, not a lock.

**Goal:** the actual game.

- Combat, suppression, cover, line-of-sight.
- Territory capture, resources, economy.
- Camp building & upgrading.
- Fog of war (and its interaction with embodiment).
- The **order/stance system** — the real depth layer (patrol routes, engagement
  ranges, retreat triggers, trigger zones, queued production). This is where "smart
  play" lives, per the design.
- Literal-executor unit AI; abilities/orders.
- Alert channel + the embodied audio mix (strategic sound bleeding into FPS).

## Phase 3 — Scale & net

**Goal:** make it hold up at size and (if pursued) in multiplayer.

- 200-unit stress tests; job-system parallelism; profiling on target hardware.
- Deterministic lockstep netcode; input delay; per-tick checksum diffing in CI.
- Reconnect/snapshot handling; Wi-Fi↔cellular handoff.
- PvP attention mind-game tuning (see open questions: enemy detection of "gone dark").

## Phase 4 — Polish & ship

- Thermal/battery tuning; device quality tiers; dynamic resolution.
- New-player onboarding for the blindness mechanic (teach + telegraph the cost).
- Store, telemetry, live-ops scaffolding.

---

## Dev workflow & iteration

Native Rust doesn't hot-reload engine code for free — that's the iteration cost of the
performance ceiling, and the one real tradeoff of the language choice (D10). Options,
cheapest-value-first:

- **Automated edit→build→deploy→test loop** — `edit → cargo build (cargo-ndk for
  Android) → adb install → am start → adb logcat`. A coding agent can script the whole
  cycle and read logcat to self-diagnose crashes. The default; no special architecture.
- **Scripting / config hot reload** — keep tuning and balance in Lua or data files;
  reload instantly, zero recompile. **Best value for iterating on game feel — and the
  primary mitigation for Rust's weaker engine-code reload.** (iOS: interpreter mode
  only, no JIT.)
- **Asset hot reload** — watch textures/configs, reload at runtime. Easy, worth it
  early.
- **Reloadable game module** — game/sim logic behind a reload boundary so it survives a
  swap while the host owns state. In Rust this means `hot-lib-reloader` /
  `dexterous_developer` (hackier than a C++ `.so` swap) — adopt **only if** the build
  loop + scripting layer stop being enough, not up front.

**Emulator caveat:** the Android Emulator runs x86_64 — build that ABI in debug for
fast iteration — but its GPU and thermal behavior won't match a mid-range arm64
phone. Iterate logic on the emulator; **profile performance on real target devices.**

---

## Top risks

| Risk | Why it's dangerous | Mitigation |
|---|---|---|
| **Touch controls** | CoH controls were built for mouse+keyboard; layering FPS + instant swap on a touchscreen is harder than any engine problem here | **Phase 0 — PASSED (D14):** prototype felt good in hand on a Galaxy S24. Shipping touch UI still a Phase 2 design task |
| **Embodied combat feels laggy** | Lockstep + input delay is RTS-optimal but adds fixed input latency with no prediction/rollback — wrong for twitch FPS aim (Q7/Q8) | **Phase 0.5 — PASSED (D15):** avatar-local prediction makes it feel good across conditions. Tick rate (Q8) still to confirm early in Phase 1 |
| **One world, two views** | The same battlefield must work top-down as an RTS map *and* at eye level as an FPS space — double the asset/collision/LoD cost | Prove one space in both views in the **Phase 1** slice before scaling content; production-side answer (sourcing, tiers, two-view filter) in [`content-pipeline.md`](content-pipeline.md) |
| **Build cost** | A custom native engine is a real investment | **Phase 1 PASSED (D22):** vertical slice validated on Galaxy S24; Unity/Godot fallback retired |
| **Determinism bugs** | Any float leaking into the sim breaks lockstep silently | Enforce fixed-point in the sim layer; per-tick checksum diffing in CI from day one |
| **Device fragmentation** | Android GPU/thermal variance is wide | Quality tiers + dynamic scaling baked in early, not as a post-ship patch |
| **Blindness feels unfair** | "World goes dark" can read as robbery if mishandled | Thin alert thread, strong audio, visceral/constant blindness feedback, fast re-entry (design doc §6) |
