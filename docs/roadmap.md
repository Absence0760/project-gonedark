# Roadmap

> Build order and milestones. The sequencing reflects the project's two biggest
> risks: **touch controls** (a product risk, not an engine one) and **determinism**
> (a correctness risk that gets exponentially harder to retrofit). Both are pulled
> as early as possible.

## Phase 0 — Control prototype *(do this before anything else)*

**Goal:** prove the core interaction is fun on a touchscreen before building any
systems behind it.

The single biggest risk in this project is not the engine — it's whether *CoH*-style
command **and** a competent FPS scheme **and** an instant swap between them feel good
on a small touchscreen. If this isn't fun, no amount of engine work saves it.

- Throwaway prototype (can be in anything fast — even a non-final engine).
- One controllable unit; tap-to-select / order on the command layer.
- Embody → FPS controls → surface, with the swap feeling instant.
- The "world goes dark" vignette + an alert ping, faked.
- **Exit criterion:** the embody ↔ command loop feels good in hand. Kill or rework
  the concept here if it doesn't.

## Phase 1 — Vertical slice

**Goal:** the real engine spine, end to end, with one of everything.

- ECS world + scheduler; data-oriented component storage.
- Fixed 30 Hz deterministic sim loop + render interpolation.
- Minimal Vulkan renderer (instanced units), camera, top-down view.
- One unit type moving via a flow field on screen.
- Embodiment as an input-source swap on a single entity; fog → avatar-only on embody.
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

Native C++/Rust doesn't hot-reload for free — that's the iteration cost of the
performance ceiling. Options, cheapest-value-first:

- **Automated edit→build→deploy→test loop** — `edit → cmake/gradle → adb install →
  am start → adb logcat`. A coding agent can script the whole cycle (~10–40 s native)
  and read logcat to self-diagnose crashes. The default; no special architecture.
- **Scripting / config hot reload** — keep tuning and balance in Lua or data files;
  reload instantly, zero recompile. Best value for iterating on game feel.
- **Asset hot reload** — watch textures/configs, reload at runtime. Easy, worth it
  early.
- **Reloadable game module** — game/sim logic in a swappable `.so`; host owns state so
  it survives reload. (Rust: harder, no stable ABI — see `hot-lib-reloader`.)

**Emulator caveat:** the Android Emulator runs x86_64 — build that ABI in debug for
fast iteration — but its GPU and thermal behavior won't match a mid-range arm64
phone. Iterate logic on the emulator; **profile performance on real target devices.**

---

## Top risks

| Risk | Why it's dangerous | Mitigation |
|---|---|---|
| **Touch controls** | CoH controls were built for mouse+keyboard; layering FPS + instant swap on a touchscreen is harder than any engine problem here | **Phase 0** — prototype controls before committing to systems |
| **Build cost** | A custom native engine is a real investment | De-risk with the Phase 1 vertical slice on real hardware; keep Unity/Godot fallback live until it passes |
| **Determinism bugs** | Any float leaking into the sim breaks lockstep silently | Enforce fixed-point in the sim layer; per-tick checksum diffing in CI from day one |
| **Device fragmentation** | Android GPU/thermal variance is wide | Quality tiers + dynamic scaling baked in early, not as a post-ship patch |
| **Blindness feels unfair** | "World goes dark" can read as robbery if mishandled | Thin alert thread, strong audio, visceral/constant blindness feedback, fast re-entry (design doc §6) |
