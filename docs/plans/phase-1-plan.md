# Phase 1 — Vertical slice *(plan)*

> **Status: DONE — EXIT CRITERION MET ([`decisions.md`](../decisions.md) D22).** Phase 0
> (D14) and Phase 0.5 (D15/D16) are done. The deterministic `core` (fixed-point [D17], SoA
> ECS [D18]) plus a real **flow field** drive one unit; the shared `engine::Game` loop
> ([D20]) — sim+render+fixed-tick+cameras+command/embodiment — runs on a **real arm64 device**
> (Galaxy S24, Adreno 750). All three decide-first gates are locked — the last, **sim rate
> (Q10), is closed by [D21]: global 60 Hz** (`core::sim::TICK_HZ = 60`). **On-device
> evidence (D22):** `pnpm android:checksum` confirmed the device sim-runner checksum stream
> **bit-identical** to desktop over 300 ticks (`4c34c6b5951edf57`); the on-device FPS
> heartbeat showed **120 fps** sustained with the sim on its locked **60 Hz** tick.
> Determinism is proven **run-to-run**, **debug==release**, **cross-arch** in CI, and **on
> real arm64 silicon**. The Unity/Godot fallback (D8) is **retired**; the throwaway prototypes
> are deleted. **Phase 2 (game systems) is now active.** Progress per step: §2 and §5.
> **Honest caveat:** validated on a **flagship** (S24); frame-rate/thermal on mid-range
> silicon and the 200-unit power budget are explicitly **Phase 3** (D21).
>
> **Goal (from [`roadmap.md`](../roadmap.md)):** the real engine spine in **Rust** (D10), end
> to end, with **one of everything** — ECS, a deterministic fixed-tick sim, a minimal `wgpu`
> renderer, one commandable + embodiable unit — running **deterministically at target frame
> rate on a real arm64 device.**
>
> **Exit criterion (met):** one unit, commandable (tap-to-move, literal executor) and
> embodiable (input-swap + world-goes-dark), running deterministically at target frame rate
> on a target phone, with the cross-platform checksum matrix green. Passed on Galaxy S24
> (Adreno 750) — see D22. The Unity/Godot fallback (the build-cost de-risk bet of D8) is
> retired.

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

| Gate | Why it's first | Status |
|---|---|---|
| **Sim rate — [Q10](../open-questions.md)/D16** | Drives the loop, netcode, budgets, thermals; ~60 Hz is the target but global-60 vs dual-rate had to be **decided on real arm64 first** | **LOCKED — [D21](../decisions.md).** A single **global 60 Hz** tick (`core::sim::TICK_HZ = 60`); with one unit on real arm64 it has enormous headroom, so dual-rate is unjustified now. Dual-rate is **deferred to Phase 3** (the 200-unit thermal re-evaluation), not killed — `TICK_HZ` stays a named constant |
| **Fixed-point representation** | The bedrock everything sits on; the newtype + scale + LUT trig must exist before any sim math | **LOCKED — [D17](../decisions.md).** Hand-rolled Q16.16 `Fixed` newtype (`core::fixed`), no float conversions (a stray float won't compile); LUT/integer trig (`core::trig`), no `libm` |
| **ECS approach** | Shapes the whole core | **LOCKED — [D18](../decisions.md).** Hand-rolled struct-of-arrays (`core::ecs`); index iteration → stable order by construction; no archetype-ECS dependency |

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
pal-android/ cargo-ndk + android-activity shim (surface, touch, lifecycle) — the ship target
render/      wgpu renderer; consumes a read-only interpolated snapshot of core state
engine/      platform-agnostic game loop (sim+render+fixed-tick+cameras+command/embodiment);
             both hosts drive Game::frame; depends on core/render/pal, never winit/android (D20)
app/         thin desktop host: a winit run loop that drives engine::Game
sim-runner/  headless core driver that emits per-tick checksums (for CI, §6)
```
`pal-ios/` is added later (most external friction; sequenced last per D9). The shared
`engine` crate ([D20](../decisions.md)) is what lets Android's `android_main` run the *same* loop
as the desktop host instead of a forked one.

## 5. Build order (milestones, each independently demoable)

Status legend: **✓ done & verified** · **◐ coded, compile-verified (not run on device)** ·
**○ scaffold (not compile-verified for target)** · **☐ not started.**

1. **✓ Bedrock:** `Fixed` type + LUT trig + deterministic RNG + tick-checksum, with unit tests
   (incl. a cross-arch check: same inputs → same bits on x86_64 and arm64). Nothing sims until
   this is solid.
2. **✓ ECS + one entity:** world, scheduler, SoA component storage; one unit with
   position/velocity/order/stance. Stable iteration order (no hash-map iteration).
3. **✓ Deterministic sim loop** at the locked rate (§2), decoupled from render. Literal-executor
   move-order system; **flow-field** movement for the one unit. *Shipped:* `core::flow_field`
   (integer Dijkstra, Dial's bucket queue, 8-connected, over a 128×128 fixed grid spanning
   world `[-64, 64)`) feeds `movement_system`, preserving the arrival snap; all fixed-point.
   Verified headless — `sim-runner` is bit-identical run-to-run **and** debug==release.
4. **◐ PAL + desktop render:** winit window + wgpu device through the PAL; triangle → instanced
   unit mesh, top-down camera, **render interpolation** between the last two sim ticks.
   *Shipped:* real `wgpu` 29 renderer (`render/`) + `winit` 0.30 + `wgpu` desktop backend
   (`pal-desktop/`), interpolating prev→curr snapshots (invariant #4); per [D19](../decisions.md)
   the GPU device crosses at the concrete wiring layer, not the abstract PAL trait.
   *Compile-verified only — no GPU/display in this env, so not run.*
5. **◐ Command + embodiment:** input → orders (tap/click-to-move); embody = swap the entity's
   input source to live player input + flip fog to **avatar-only** (world goes dark); FPS
   camera. Surface/eject back to command. (No respawn object — invariant #5.) *Shipped:*
   `app/src/main.rs` — a real winit `ApplicationHandler` loop (fixed-tick accumulator, render
   interpolation, pointer-unproject tap-to-move quantized to `Fixed` at the input boundary,
   embody/surface swap with the near-black "gone dark" clear, top-down ortho + embodied
   perspective cameras; the D15 avatar-local-prediction seam kept presentation-only).
   *Compile-verified only — not run.*
6. **✓ Android backend:** cargo-ndk build, Gradle wrapper, JNI shim (surface/touch/lifecycle);
   deploy to the **real phone**; stand up the `edit → cargo build → adb install → am start →
   adb logcat` loop (roadmap dev workflow). *Shipped:* `pal-android/` (`android_main` + PAL
   impls, gated to `target_os = "android"` so the host build is empty) + an `android/` Gradle
   project. **Builds for `aarch64-linux-android`** via `cargo ndk -t arm64-v8a build` (NDK 28)
   **and assembles an installable arm64 debug APK** — `pnpm android:apk` runs cargo-ndk →
   `:app:assembleDebug` (committed Gradle 8.11 wrapper + AGP 8.7.2) → `app-debug.apk`
   bundling `libgonedark_pal_android.so`. **`android_main` drives the shared `engine::Game`
   loop** (the same sim+render the desktop host runs, via [D20](../decisions.md)) — not just a
   clear. An on-device run surfaced + fixed a real arm64 surface-config crash (texture limit);
   **on the real device (Adreno 750) the unit moves, tap-to-move works, and a provisional
   two-finger-tap embody toggle flips the world dark.** *Still ahead:* the *shipped* Android
   control scheme (on-screen sticks / gyro, the final embody gesture) is a Phase 2 design call —
   the two-finger toggle is a dev binding.
7. **✓ Determinism CI:** wire the per-tick checksum matrix (§6) — green before the slice counts.
   *Done:* the checksum matrix (`determinism.yml`) now also covers **native arm64 Linux**;
   `build.yml` carries a blocking `graphics-build` job (link deps + build/clippy the wgpu/winit
   crates) and an `android-build` cross-compile job.
8. **✓ Validate on real arm64:** deterministic, at target frame rate, embody↔command loop
   working — the exit gate. **Passed on Galaxy S24, Adreno 750 (D22):** `pnpm
   android:checksum` confirmed bit-identical checksum over 300 ticks; `adb logcat` FPS
   heartbeat showed **120 fps** sustained at the **60 Hz** sim tick. Fallback retired.

## 6. Determinism CI — from day one, even with one unit

Stand up the checksum-matrix harness (invariant #7, [`platforms.md`](../platforms.md) §7) while
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
| Sim-rate choice (Q10) wrong on real silicon | **Locked — [D21](../decisions.md):** global 60 Hz, with huge headroom for Phase 1's one unit on real arm64. The 200-unit power/thermal re-evaluation (could reopen dual-rate) is deferred to Phase 3 |
| `wgpu`/surface lifecycle quirks on Android | Isolate behind the PAL; desktop backend first, Android second |
| Custom-engine build cost balloons | De-risked: the Phase 1 slice **passed** on Galaxy S24 (D22); fallback retired |
| Weak engine hot-reload slows iteration | Scripting/data + asset reload + the automated loop (§7) |

## 10. On-device sign-offs (both passed — Phase 1 DONE, D22)

Build-order step 8 is complete. Both sign-offs passed on Galaxy S24, Adreno 750:

1. **On-device determinism — PASSED.** `pnpm android:checksum` ran the headless `sim-runner`
   on-device and diffed its per-tick checksum stream against the x86_64 desktop run over 300
   ticks: **bit-identical** (final checksum `4c34c6b5951edf57`). The fixed-point sim is
   deterministic on real arm64 silicon (invariant #1/#7).
2. **Target frame rate — PASSED.** The `adb logcat` FPS heartbeat showed **120 fps** sustained
   with the sim on its locked **60 Hz** tick — frames advancing ~120/s while ticks advance
   ~60/s, demonstrating sim/render decoupling (invariant #4) live on hardware.

**Caveat on the record (D22):** validated on a **flagship** (Galaxy S24). Determinism is
arch-level and device-independent (a mid-range chip yields identical checksums by construction);
frame-rate/thermal headroom on mid-range silicon and the 200-unit power budget are **Phase 3**.

---

**Phase 1 is complete.** The Unity/Godot fallback (D8) is retired; the throwaway prototypes
(`prototypes/phase0-controls`, `prototypes/phase0.5-netfeel`) are deleted. All Phase 1
decisions are recorded (D17, D18, D19, D20, D21, D22). **Phase 2 (game systems) is the
active phase** — see [`roadmap.md`](../roadmap.md).
