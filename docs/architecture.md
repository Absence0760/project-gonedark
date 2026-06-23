# Engine & Systems Architecture

> A performance-first reference for a *Company of Heroes*–class real-time tactics
> game with dozens-to-hundreds of agents on screen, targeting mid-range Android,
> **plus** a first-person embodiment layer (see [`game-design.md`](game-design.md)).
>
> Adapted from the "Mobile RTS game architecture" engineering reference. The
> embodiment notes (§ Embodiment) are specific to *Going Dark*.

## Targets

- **60 FPS** base / **120** flagship · render rate variable.
- **Sim 30 Hz**, fixed-tick, deterministic, fixed-point.
- **arm64-v8a / Vulkan 1.1**, 200+ units.

## The decision in one page

A custom **Rust engine core** — data-oriented ECS, **`wgpu`** renderer (native
Vulkan/D3D12/Metal per device), deterministic fixed-tick simulation, lockstep-capable
netcode. Thin platform shims (Kotlin/JNI on Android, Swift/Obj-C on iOS) handle OS
lifecycle, input, billing, services.

| Layer | Choice |
|---|---|
| Core | Rust · ECS (Bevy/hecs/legion or hand-rolled) |
| Render | `wgpu` → Vulkan / D3D12 / Metal · GPU-driven instancing |
| Sim | 30 Hz fixed · deterministic · fixed-point |
| Net | Lockstep · input-delay · desync checks |
| Platform | per-OS shim (Kotlin/JNI, Swift) · AAudio/CoreAudio |
| Build | cargo (+ Gradle/Xcode for store packaging) · arm64-v8a |

The hard constraint of an RTS is not graphics — it is **simulating many independent
agents deterministically inside a fixed budget**. That is solved by controlling
memory layout, threading, and the sim loop down to the cache line. Managed runtimes
and general-purpose engines abstract exactly those controls away. The trade is build
cost for ceiling: native gives you the ceiling.

### Language — Rust *(decided — [decisions.md](decisions.md) D10)*

**The engine is Rust.** Both Rust and C++ compile through LLVM to native code, so
performance is a wash; the decision is on engineering values over a long horizon. For a
greenfield, small-team, custom-native, determinism-critical engine across four
platforms, Rust wins on the load-bearing axes:
- **Compile-time data-race freedom** for the heavily-threaded deterministic sim — its
  worst bugs are silent, non-reproducible races and determinism leaks, and Rust kills
  that whole class at build time. (The decisive factor.)
- **`wgpu`** gives native Vulkan/D3D12/Metal per device for free (see Rendering / RHI).
- Type-system-enforceable determinism (newtype fixed-point), cargo toolchain simplicity,
  mature ECS/math/physics crates (Bevy/hecs, glam, rapier).

The one real cost is engine-code hot-reload (no stable ABI) — mitigated by
scripting/data hot-reload and the automated build loop (see roadmap). The C++ snippets
below are illustrative pseudocode; every system maps 1:1 to Rust (`wgpu` + `hecs`/
`legion`, or Bevy).

### Pragmatic fallbacks (if time-to-market beats peak performance)

- **Unity + DOTS/Burst + ECS** — native-speed hot paths, mature mobile toolchain,
  at the cost of a heavier runtime and GC discipline.
- **Godot 4 + GDExtension (C++)** — lighter and open, but renderer/scene model need
  real work to handle hundreds of agents.

Keep a fallback live until the vertical slice is validated on target hardware.

## Layered architecture

Strict downward dependency — each layer talks only to the one below through a narrow
interface. The simulation never touches Vulkan; rendering never mutates game state.
This is what makes the sim deterministic and portable, and lets the render rate
float independently of the tick rate.

```
Presentation   (variable render rate) — HUD/UI, camera + interpolation, VFX,
               input → orders, FPS embodied view
        ▼
Game systems   (30 Hz) — combat & suppression, cover / LoS, territory & resources,
               fog of war, abilities / orders
        ▼
Engine core    (Rust, data-oriented) — ECS world + scheduler, job system,
               pathfinding (flow fields + HPA*), collision / spatial hash,
               allocators, netcode
        ▼
Platform       (per-OS, via PAL) — wgpu surface, audio, input/touch,
               frame pacing, native shim (JNI/Swift), storage / mmap
```

**Side channel:** an offline asset pipeline cooks source art/audio into packed,
compressed, ready-to-mmap bundles. No runtime parsing, no decode-on-load.

## Data-oriented ECS — the hot path

Units are rows in tightly packed component arrays (struct-of-arrays), not objects
with virtual methods scattered across the heap. Systems iterate those arrays
linearly so the CPU prefetcher and cache do the work. At 200+ agents this is the
difference between shipping and dropping frames.

- **Do:** PODs in contiguous arrays · systems = functions over component spans ·
  archetypes / chunked storage · entity = index + generation handle (no pointers).
- **Avoid:** deep inheritance / virtual dispatch per unit · per-frame heap alloc on
  the hot path · shared-pointer graphs · branchy cache-thrashing loops.

## Simulation loop — fixed, deterministic, decoupled

The sim advances in fixed 30 Hz ticks driven purely by orders, in fixed-point math,
so every device computes bit-identical results. Rendering runs at a separate
variable rate and interpolates between the last two sim states.

```
acc += realDelta;
while (acc >= TICK) { applyOrders(); stepSim(TICK); acc -= TICK; }
float alpha = acc / TICK;
render(lerp(prevState, curState, alpha));
```

### Determinism checklist (lockstep desyncs silently without every item)

- **No floats in the sim** — fixed-point only; floats live only in rendering.
- **Deterministic transcendentals** — sin/cos/sqrt via fixed-point or LUTs, never
  `libm`. Disable fast-math.
- **Stable iteration order** — never iterate by hash-map order; arrays or sorted keys.
- **Seeded lockstep RNG** — identical seed and call sequence on every peer.
- **No uninitialized reads, no raw pointers in state.**
- **Per-tick checksum diffing in CI** across devices and compilers, from day one.

Free win: a deterministic sim gives **replays and tiny save files** for nothing —
store the input stream, not the world state.

## Embodiment — the *Going Dark* layer

> This section is specific to this game. The key finding: **embodiment is cheap to
> build because it's a presentation/vision change, not a sim change.**

- **The sim never stops while you're embodied.** The deterministic 30 Hz sim keeps
  grinding underneath, with your other units executing their last orders (see the
  unit-AI philosophy in the game design doc). This is exactly what the decoupled
  sim/render split already enables — no special pause/resume machinery.
- **"World goes dark" is a vision-culling toggle.** Fog of war is already a
  game-systems concern. Embodiment simply switches the local player's visibility to
  *avatar-only*. It does not touch sim state, so it cannot cause desyncs.
- **The embodied unit is just an ECS entity with an input source swapped.** Instead
  of its orders coming from the command layer / unit AI, they come from live player
  input. On death or surface, the input source reverts. No separate "player
  character" object, no FPS respawn system, no parallel state — a big complexity
  saving directly downstream of the death-is-a-demotion design decision.
- **Alerts are a thin presentation channel.** "Base under attack" pings derive from
  sim events (damage, capture) surfaced to the embodied HUD as direction + audio
  only — no map reveal. Audio routing (bleeding strategic-layer sound into the
  embodied mix) is the one system that needs real attention here.

## Rendering — Vulkan, draw-call disciplined

On mobile, draw calls and bandwidth are the wall, not triangles.

- **Batch & instance** units sharing a mesh/material; per-instance transforms in one
  buffer; sort by material. Aim for low-triple-digit draw calls regardless of unit
  count.
- **Cull early** — quadtree + frustum culling on the job system before submission;
  fog-of-war-hidden entities skip rendering. *(Note: the embodied first-person view
  has very different culling characteristics from the top-down view — budget for
  both camera modes.)*
- **ASTC + atlases** to cut bandwidth and binds.
- **Dynamic resolution** to hold frame time when GPU-bound or throttling; UI stays
  native res.

## Pathfinding & movement — the RTS bottleneck

Layered, because per-unit per-frame A* doesn't scale:

- **Layer 1 — Flow fields** for group movement: one field per destination, any number
  of units sample it in O(1).
- **Layer 2 — Hierarchical (HPA\*)** for long routes: coarse sector graph, refine
  only the local sector.
- **Layer 3 — Local avoidance (RVO / steering)** each tick; spatial hash keeps
  neighbor queries near-constant.

## AI — budgeted

Squad behavior on utility scoring over behavior trees, informed by influence maps
(low-res threat/control grids, updated incrementally). LOD scheduling: nearby/engaged
squads re-evaluate every tick, distant idle squads every N ticks, round-robin to
spread cost.

> **Design note:** *Going Dark* deliberately keeps unit AI **literal** (execute the
> last order, don't strategize) — see the game design doc §8. This is *cheaper* than
> smart LOD AI and more deterministic, so this section is lighter for us than for a
> conventional RTS.

## Netcode — deterministic lockstep

Because the sim is deterministic, clients exchange only **orders**, not world state.
Bandwidth scales with players, not the hundreds of units on the field.

- **Input delay** — orders execute a few ticks ahead so all peers receive them in
  time; tunable, masks latency without rollback complexity.
- **Desync detection** — per-tick state checksums compared across peers; mismatch →
  flag + recover.
- **Gotchas:** the slowest peer paces everyone (tune input delay dynamically; drop /
  AI-substitute a stalled peer) · reconnect needs a serialized snapshot, not replay
  from tick 0 · Wi-Fi↔cellular handoff drops the socket (reconnect with a brief
  pause, never a silent desync) · every client holds full world state, so fog is
  client-side only — assume a determined cheater sees the whole map; add server-side
  replay validation for ranked play. **(This last point compounds with embodiment:
  the "world goes dark" blindness is a client-side presentation rule, so it is NOT a
  competitive-integrity boundary — it shapes the intended experience, not a
  cheat-proof information wall.)**

## Memory & concurrency

- **Allocators:** frame arena (bump-allocate transient data, reset each frame),
  pools per component type preallocated to max capacity, **zero heap alloc on the
  per-tick hot path. No GC, ever** — a core reason to go native.
- **Job system:** work-stealing scheduler across performance cores (culling,
  pathfinding, AI, animation); sim stays deterministic (stable order, no data races);
  dedicated render-submission thread feeding Vulkan command buffers.

## Asset pipeline & loading

`source art/audio → cook (ASTC · atlas · pack anim) → pak bundle + LZ4 → mmap + async
stream`. Memory-map bundles so the OS pages assets in on demand; decompress with LZ4;
stream on background threads — never block sim or render on I/O. Audio as Opus via
AAudio's low-latency path.

## Mobile realities

- **Frame pacing** via Swappy; cap to 30/60 by device class. Smoothness > peak FPS.
- **Thermal scaling** — drop dynamic resolution and effect tiers before the OS
  throttles the whole SoC.
- **Battery** — idle/menu throttles to 30 FPS or pauses; avoid spin-waits.
- **Fragmentation & lifecycle** — Vulkan 1.1 baseline + runtime feature detection;
  quality tiers by GPU; handle surface loss on resume (recreate swapchain cleanly).

## Frame & sim budgets (design targets, not measurements)

Two independent budgets because render is decoupled from sim.

**Per render frame (every frame):** visibility/culling 1.0 · render submission 3.5 ·
interpolation+transforms 0.8 · UI/HUD 1.0 · present+pacing 0.4 → **~6.7 ms used.**
Headroom: 9.9 ms @60 · 1.6 ms @120 (120 is tight but feasible on flagship with
dynamic resolution).

**Per 30 Hz sim tick (~33 ms, amortized):** sim step 3.0 · pathfinding/flow 2.0 ·
AI (time-sliced) 1.5 · collision/spatial hash 1.5 · net (orders+checksums) 0.5 →
**~8.5 ms used.** ~24 ms headroom per tick absorbs spikes and does **not** shrink
when render rate rises — which is why 120 FPS is reachable.

## Stack at a glance

| Concern | Choice | Why |
|---|---|---|
| Language | **Rust** (D10) | No GC, full memory/threading control, compile-time data-race freedom for the threaded sim |
| Architecture | Data-oriented ECS | Cache-friendly iteration over hundreds of agents |
| Graphics | `wgpu` → Vulkan/D3D12/Metal | Low-overhead, multithreaded, native backend auto-selected per device |
| Audio | AAudio/CoreAudio + Opus | Low-latency native path; compact compressed assets |
| Networking | Deterministic lockstep | Bandwidth scales with players, not units |
| Platform shim | Kotlin/JNI · Swift/Obj-C | Lifecycle, input, billing, platform services — thin, off the hot path |
| Build | cargo (+ Gradle/Xcode packaging) | Ship arm64-v8a; native frame pacing per platform |
