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
> retired; resolves [`open-questions.md`](open-questions.md) Q4. Throwaway prototype lives
> in [`../prototypes/phase0-controls/`](../prototypes/phase0-controls/) (kept through Phase
> 0.5, then deleted). Two caveats carried into D14: audio is still faked, and embodied feel
> *over the network* is untested — that's Phase 0.5, **the next gate.**

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
> [`open-questions.md`](open-questions.md) Q7; Q8 (tick rate) still open, leaning hold-30 Hz,
> to close early in Phase 1. **Phase 1 is now unblocked — the next gate.** Throwaway harness:
> [`../prototypes/phase0.5-netfeel/`](../prototypes/phase0.5-netfeel/). Plan:
> [`phase-0.5-plan.md`](phase-0.5-plan.md).

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

> **Status: IN PROGRESS — first real engine code has landed.** The Rust workspace scaffold
> is in: a deterministic fixed-point `core` (Q16.16 [D16→D17](decisions.md), hand-rolled SoA
> ECS [D18](decisions.md)), the PAL trait boundary, render/host/backend skeletons, a headless
> `sim-runner`, and the per-tick checksum CI matrix. Two of the three decide-first gates are
> locked (D17/D18); **sim rate (Q10) stays open**, parameterized as `core::sim::TICK_HZ`
> pending real-arm64 profiling. Still ahead (build-order steps 4–8): the real `wgpu`/`winit`
> renderer, the Android backend, on-device validation. Detailed plan:
> **[`phase-1-plan.md`](phase-1-plan.md)**.

**Goal:** the real engine spine, end to end, with one of everything.

- ECS world + scheduler; data-oriented component storage.
- Fixed-tick deterministic sim loop + render interpolation. **Settle the sim rate first
  ([`open-questions.md`](open-questions.md) Q10 / D16):** 30 Hz is too coarse for embodied
  combat (~60 Hz target) — profile **global-60 vs dual-rate** on a real target device and
  lock it *before* building the loop, since it drives netcode, budgets, and thermals.
- Embodiment as an input-source swap on a single entity; fog → avatar-only on embody. Wire
  the **avatar-local-prediction boundary (D15) from the first netcode commit** — presentation
  path only, never writing sim state.
- Minimal Vulkan renderer (instanced units), camera, top-down view.
- One unit type moving via a flow field on screen.
- **Validate on real mid-range arm64 hardware**, not just the emulator.
- **Exit criterion:** one unit, commandable and embodiable, running deterministically
  at target frame rate on a target device. Keep the Unity/Godot fallback live until
  this passes.

## Phase 2 — Game systems

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
| **Build cost** | A custom native engine is a real investment | De-risk with the Phase 1 vertical slice on real hardware; keep Unity/Godot fallback live until it passes |
| **Determinism bugs** | Any float leaking into the sim breaks lockstep silently | Enforce fixed-point in the sim layer; per-tick checksum diffing in CI from day one |
| **Device fragmentation** | Android GPU/thermal variance is wide | Quality tiers + dynamic scaling baked in early, not as a post-ship patch |
| **Blindness feels unfair** | "World goes dark" can read as robbery if mishandled | Thin alert thread, strong audio, visceral/constant blindness feedback, fast re-entry (design doc §6) |
