# Phase 1 — Vertical slice *(plan)*

> **Status: NEXT — the first real engine code.** Phase 0 (D14) and Phase 0.5 (D15/D16)
> are done; the throwaway prototypes proved the feel, this builds the spine for real.
>
> **Goal (from [`roadmap.md`](roadmap.md)):** the real engine spine in **Rust** (D10), end
> to end, with **one of everything** — ECS, a deterministic fixed-tick sim, a minimal `wgpu`
> renderer, one commandable + embodiable unit — running **deterministically at target frame
> rate on a real mid-range arm64 device.**
>
> **Exit criterion:** one unit, commandable (tap-to-move, literal executor) and embodiable
> (input-swap + world-goes-dark), running deterministically at target frame rate on a target
> phone, with the cross-platform checksum matrix green. **Keep the Unity/Godot fallback live
> until this passes** — Phase 1 is also the de-risk of the build-cost bet.

---

## 1. What Phase 1 proves

Three things, all on **real hardware**, not the emulator:
1. The **deterministic fixed-tick sim** runs bit-identical (fixed-point, no float leaks).
2. The **PAL boundary** holds — one shared core, a thin per-platform backend (D9).
3. **Embodiment as an input-source swap + vision toggle** (D6/D7, invariant #5) works on a
   single real entity, at frame rate, on a phone.

Everything else (combat, economy, camps, multiple unit types, real netcode) is deliberately
**out of scope** — see §8.

## 2. Decide-FIRST gates (lock these before building the loop)

Each becomes a recorded `Dn` when locked — do not silently pick (CLAUDE.md):

| Gate | Why it's first | Lean (not locked) |
|---|---|---|
| **Sim rate — [Q10](open-questions.md)/D16** | Drives the loop, netcode, budgets, thermals; ~60 Hz is the target but global-60 vs dual-rate must be **profiled on a real device first** | Start global-60 for simplicity; fall to dual-rate only if the 200-unit power/thermal projection forces it |
| **Fixed-point representation** | The bedrock everything sits on; the newtype + scale + LUT trig must exist before any sim math | A thin `Fixed` newtype (e.g. the `fixed` crate or hand-rolled i32.16) + hand-rolled LUT sin/cos/sqrt; **no `libm`, fast-math off** (invariant #1) |
| **ECS approach** | Shapes the whole core | Hand-rolled SoA or **hecs**-style archetype store over Bevy — Bevy brings a scheduler/app model we don't want fighting the deterministic loop; full control over iteration order (determinism) matters more than ergonomics |

## 3. Invariants wired in from the first commit (not retrofitted)

The whole point of building to these now is that each is **far costlier to add later**:
- **No floats in the sim** — fixed-point + LUT only; floats live only in `render`. The
  `Fixed` type makes a stray `f32` a *compile error* in core. (invariant #1)
- **Shared core, zero platform deps** — the `core` crate must not pull `wgpu`/`winit`/JNI.
  PAL boundary in from commit one. (invariant #2, D9)
- **Sim/render decoupled, fixed deterministic tick** — render reads an interpolated snapshot;
  it never mutates sim state. (invariant #4)
- **Embodiment = input-source swap + vision toggle** — no character/respawn object; build the
  **input seam** so the same entity takes AI/orders *or* live player input. (invariant #5)
- **Avatar-local-prediction boundary (D15)** — even though Phase 1 is single-player and may
  ship no netcode yet, put the predict/commit seam in the input/presentation path now, so the
  rule "*prediction never writes sim state*" is structural, not bolted on in Phase 3.
- **Literal-executor AI** — the one unit just holds its last order + stance and executes it.
  No autonomy. (invariant #3, D3)
- **Per-tick checksum diffing in CI from day one** — even with one unit, across the full
  platform matrix. (invariant #7) See §6.

## 4. Crate skeleton

```
core/        no platform deps — Fixed-point math + LUTs, deterministic RNG, tick checksum,
             ECS world + scheduler, components, systems, sim loop, (stubbed) order/stance
pal/         trait definitions only — Rhi, Input, Window, Audio, Storage, Clock
pal-desktop/ winit + wgpu backend (dev/CI host: linux-gnu, win-msvc)
pal-android/ cargo-ndk + JNI shim (surface, touch, lifecycle) — the ship target
render/      wgpu renderer; consumes a read-only interpolated snapshot of core state
app/         wires core + pal + render; owns the run loop and the sim/render split
sim-runner/  headless core driver that emits per-tick checksums (for CI, §6)
```
`pal-ios/` is added later (most external friction; sequenced last per D9).

## 5. Build order (milestones, each independently demoable)

1. **Bedrock:** `Fixed` type + LUT trig + deterministic RNG + tick-checksum, with unit tests
   (incl. a cross-arch check: same inputs → same bits on x86_64 and arm64). Nothing sims until
   this is solid.
2. **ECS + one entity:** world, scheduler, SoA component storage; one unit with
   position/velocity/order/stance. Stable iteration order (no hash-map iteration).
3. **Deterministic sim loop** at the locked rate (§2), decoupled from render. Literal-executor
   move-order system; **flow-field** movement for the one unit. Headless first — prove it ticks
   identically via `sim-runner`.
4. **PAL + desktop render:** winit window + wgpu device through the PAL; triangle → instanced
   unit mesh, top-down camera, **render interpolation** between the last two sim ticks.
5. **Command + embodiment:** input → orders (tap/click-to-move); embody = swap the entity's
   input source to live player input + flip fog to **avatar-only** (world goes dark); FPS
   camera. Surface/eject back to command. (No respawn object — invariant #5.)
6. **Android backend:** cargo-ndk build, Gradle wrapper, JNI shim (surface/touch/lifecycle);
   deploy to the **real phone**; stand up the `edit → cargo build → adb install → am start →
   adb logcat` loop (roadmap dev workflow).
7. **Determinism CI:** wire the per-tick checksum matrix (§6) — green before the slice counts.
8. **Validate on real mid-range arm64:** deterministic, at target frame rate, embody↔command
   loop working. *This* is the exit gate; only now retire the fallback.

## 6. Determinism CI — from day one, even with one unit

Stand up the checksum-matrix harness (invariant #7, [`platforms.md`](platforms.md) §7) while
the sim is trivial, so a float leak is caught the day it lands, not in Phase 3:
- `sim-runner` plays a fixed input script and emits a **per-tick state checksum**.
- CI runs it on **`x86_64-pc-windows-msvc`, `x86_64-unknown-linux-gnu`,
  `aarch64-linux-android`, `aarch64-apple-ios`** and **diffs the checksum streams**; any
  mismatch fails the build. (iOS may lag on host-build friction — at minimum cross-check
  x86_64 ↔ arm64 early; a desync there is a real bug, never something to silence.)

## 7. Iteration loop (Rust's one real tradeoff, D10)

No free engine-code hot-reload. Mitigations, cheapest-first (roadmap dev workflow):
- the automated **edit→build→deploy→logcat** loop (a coding agent can drive and self-diagnose);
- **scripting/data + asset hot reload** for tuning game feel without recompiling;
- a reloadable game module (`hot-lib-reloader`) **only if** the above stop being enough.

## 8. Out of scope for Phase 1 (deliberately)

Combat/suppression/cover, economy/territory/camps, fog beyond the embody toggle, multiple unit
types, the real order/stance *vocabulary*, lockstep netcode over the wire, and the audio mix —
all Phase 2/3. Phase 1 ships **one** unit and the **spine**. (The prediction and order/stance
*seams* are stubbed so they aren't retrofits, but their content is later.)

## 9. Phase-1-specific risks

| Risk | Mitigation |
|---|---|
| A float leaks into the sim (silent desync) | `Fixed` newtype makes it a compile error; checksum matrix from day one (§6) |
| Sim-rate choice (Q10) wrong on real silicon | Profile global-60 vs dual-rate on the device **before** locking the loop (§2) |
| `wgpu`/surface lifecycle quirks on Android | Isolate behind the PAL; desktop backend first, Android second |
| Custom-engine build cost balloons | Keep the Unity/Godot fallback **live until the slice passes** (D8) |
| Weak engine hot-reload slows iteration | Scripting/data + asset reload + the automated loop (§7) |

---

**On completion:** record the locked decisions made along the way (sim rate → closes Q10;
fixed-point representation; ECS choice) as `Dn` entries, mark Phase 1 done in
[`roadmap.md`](roadmap.md), retire the fallback, and the throwaway prototypes can finally be
deleted. Then Phase 2 (game systems) begins.
