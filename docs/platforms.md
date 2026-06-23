# Cross-Platform Plan — Windows · Linux · Android · iOS

> How one game runs on four platforms, each using the GPU/OS path that is *native
> and optimal* for that device — without forking the game itself.

## 1. The principle — one core, four optimized backends

"Each device optimized for that device" is right, with one critical boundary:

- **The game architecture is identical on every platform.** ECS, the deterministic
  fixed-point sim, game systems, pathfinding, AI, netcode — pure portable code
  (C++20 or Rust), no platform dependencies. It compiles and runs bit-identically
  everywhere.
- **The platform *backend* is native and optimized per device.** GPU API, audio,
  windowing, input, storage — each platform gets the path its hardware/OS does best
  (Metal on iOS, D3D12/Vulkan on Windows, Vulkan on Linux/Android).
- **The presentation *tier* is tuned per device.** Resolution, effect tiers, refresh
  target, thermal/battery policy, and control scheme adapt to the hardware — but they
  drive the *same* simulation.

**Why not literally fork the architecture per platform?** Two reasons, both fatal:

1. **Cross-play dies.** Deterministic lockstep (see [`architecture.md`](architecture.md))
   only works if every client computes the *same* sim from the same inputs. Diverge
   the core and a Windows player and an Android player can never share a match.
2. **You'd maintain four games.** Every gameplay change × four implementations × four
   bug surfaces. That's how small teams die.

So: **shared deterministic core, platform-optimized everything-around-it.** That *is*
per-device optimization done correctly.

```
        ┌──────────────────────────────────────────────────────┐
        │   PORTABLE GAME CORE  (identical on all platforms)     │
        │   ECS · deterministic fixed-point sim · game systems   │
        │   pathfinding · literal-executor AI · lockstep netcode │
        └───────────────────────────┬──────────────────────────┘
                                     │  narrow PAL interfaces
        ┌──────────┬──────────┬──────┴──────┬──────────┐
        ▼          ▼          ▼             ▼          ▼
     Render     Audio      Input       Window/      Storage
      (RHI)                            Lifecycle
        │          │          │             │          │
   ┌────┴────┐  per-platform native backends chosen at build time
   │ Windows │  D3D12 (or Vulkan) · WASAPI/XAudio2 · Win32 · mouse+kbd/pad
   │ Linux   │  Vulkan · PipeWire/ALSA · Wayland/X11 (SDL3) · mouse+kbd/pad
   │ Android │  Vulkan 1.1 · AAudio · NDK+JNI · touch/gyro
   │ iOS     │  Metal · CoreAudio/AVAudioEngine · UIKit · touch/gyro
   └─────────┘
```

## 2. What's shared vs what's per-platform

| Subsystem | Shared core | Windows | Linux | Android | iOS |
|---|---|---|---|---|---|
| Sim / ECS / AI / netcode | **100% shared** | — | — | — | — |
| GPU API (RHI backend) | RHI interface | **D3D12** (Vulkan fallback) | **Vulkan** | **Vulkan 1.1** | **Metal** |
| Audio | mixer/logic shared | WASAPI / XAudio2 | PipeWire / ALSA | AAudio | CoreAudio |
| Windowing / lifecycle | event abstraction | Win32 | Wayland/X11 (SDL3) | NDK + Kotlin/JNI | UIKit + CAMetalLayer |
| Input | order/intent layer shared | mouse+kbd, gamepad | mouse+kbd, gamepad | touch, gyro | touch, gyro |
| Filesystem / mmap | VFS interface | Win32 | POSIX | POSIX + AAsset | POSIX + bundle |
| Build output | — | `.exe` (MSVC/clang) | ELF (Flatpak/AppImage) | `.aab`/`.apk` | `.ipa` |

The **Platform Abstraction Layer (PAL)** is the set of narrow interfaces between the
core and these backends. Keep it *thin* — only what genuinely differs crosses it.

## 3. Rendering — the RHI (Render Hardware Interface)

The renderer talks to a small internal RHI; each platform implements it with its
native API. Three realistic ways to get there:

- **If the engine is Rust:** use **`wgpu`**. It already targets Vulkan (Linux/Android/
  Windows), **D3D12** (Windows), and **Metal** (iOS) and *picks the optimal backend
  per device automatically*. This is the cleanest path to the exact goal here and is a
  strong reason to lean Rust for a multi-platform target. (See
  [`decisions.md`](decisions.md) D8 — language is still open.)
- **If the engine is C++:** either hand-roll a thin RHI over Vulkan + Metal + D3D12,
  or adopt a multi-backend layer — **The Forge** (AAA, Vulkan/D3D12/Metal, shipping
  games), **bgfx**, or **Diligent Engine**. Hand-rolling gives the most control (the
  whole point of going native); a library gets you cross-platform faster.
- **Pragmatic shortcut:** author one **Vulkan** renderer and run it on iOS via
  **MoltenVK** (Vulkan-over-Metal). Ships on all four with a single backend, then a
  *native Metal* path can be added later only if iOS profiling demands it. Lowest
  effort to first-playable-everywhere; slightly off "fully native per device" until
  the Metal backend lands.

**Recommendation:** if Rust → `wgpu` and you're basically done. If C++ → start with the
Vulkan + MoltenVK shortcut to hit all four platforms, add a native Metal (and optional
D3D12) backend as an optimization pass, not a prerequisite.

> **macOS bonus:** an iOS Metal backend extends to macOS for nearly free. Not a target
> now, but worth knowing the door is open.

## 4. Windowing / audio / input — collapse the PAL with SDL3

Most of the non-GPU PAL surface can be handled by **SDL3**, which covers windowing,
input, gamepad, and audio across **all four** targets. That leaves you to hand-roll
only the RHI and a few platform services (billing, push, mmap specifics). On Linux
this also gives you clean **Wayland** support out of the box.

Audio alternative if not via SDL: **miniaudio** (single-header) backends to WASAPI,
CoreAudio, AAudio, PipeWire/ALSA — native per platform, one API. Either way, the
strategic-layer audio mix that bleeds into the embodied "world goes dark" view (design
doc §6) is engine-side and identical everywhere.

## 5. Input — the real per-platform divergence

This is where platforms genuinely differ, and it ties straight to the project's top
product risk (the touch-control problem, roadmap Phase 0). The trick: the core only
ever consumes **orders/intents**, never raw input. Each platform's input layer
*translates* its native scheme into the same intent vocabulary.

| Input class | Platforms | Command layer | Embodied (FPS) layer |
|---|---|---|---|
| **Mouse + keyboard** | Windows, Linux | native CoH paradigm — *easy* | mouse-look + WASD — *easy, familiar* |
| **Touch + gyro** | Android, iOS | the hard problem (Phase 0) | touch sticks / gyro aim — *hard* |
| **Gamepad** | all (optional) | cursor/radial — *medium* | twin-stick — *medium* |

**Design consequence:** desktop gets the *native* CoH control feel almost for free,
while mobile is where the control scheme must be invented. Prototyping controls
(Phase 0) should cover **both** input classes, because the embody↔command swap feels
different on each — and the game must ship feeling good on all of them.

## 6. Per-device optimization & quality tiers

Same sim, tuned presentation:

- **Mobile (Android/iOS):** Vulkan 1.1 / Metal feature detection; ASTC textures;
  dynamic resolution; thermal + battery scaling; 30/60/120 tiers by device class;
  Swappy (Android) / CADisplayLink-paced (iOS) frame pacing.
- **Desktop (Windows/Linux):** higher fidelity ceilings, higher unit counts, larger
  draw budgets, uncapped/high-refresh; mouse precision enables denser UI; no thermal
  throttling concern (but still honor frame pacing).
- The **device-tier system** (low/mid/flagship on mobile; low/high on desktop) is data
  driven — one table, per-platform defaults — not branched code paths.

## 7. Cross-platform deterministic multiplayer — the payoff

Because the sim is **float-free fixed-point**, it produces bit-identical results across
*CPU architectures* (x86-64 and arm64) and *compilers*, not just across devices of one
kind. That means:

- **A Windows player, a Linux player, an Android player, and an iOS player can share
  one lockstep match.** Bandwidth still scales with players, not units.
- **Hard requirement:** the per-tick **checksum-diffing CI must run across every target
  platform/compiler/arch from day one** — not just Android. Cross-arch determinism is
  the single thing that makes cross-play possible, and it desyncs *silently* if any
  platform diverges. This extends the determinism checklist in
  [`architecture.md`](architecture.md) to a build matrix:
  `{Windows/MSVC-x64, Linux/Clang-x64, Android/Clang-arm64, iOS/Clang-arm64}`.
- iOS caveat for any scripting VM: **no JIT on iOS** — run Lua (or similar) in
  interpreter mode; LuaJIT's JIT is unavailable. Affects the scripting-hot-reload dev
  convenience (roadmap), not shipping determinism.

## 8. Build & distribution

Unify on **CMake** as the meta-build (already chosen for Android via CMake+Gradle),
with one toolchain file per target:

| Platform | Toolchain | Package | Store / channel | Notes |
|---|---|---|---|---|
| Linux | GCC/Clang | AppImage or **Flatpak** | Flathub, itch, Steam | Easiest desktop bring-up; your dev OS |
| Windows | MSVC or clang-cl | `.exe` + installer | Steam, itch | D3D12 needs PIX for profiling |
| Android | NDK + Gradle | `.aab` | Google Play | Existing primary target |
| iOS | Xcode + clang | `.ipa` | App Store | **Needs a macOS build host**, code signing, review |

**Friction ranking (low→high to bring up):** Linux ≈ Windows (desktop, mouse+kbd,
Vulkan/D3D12) → Android (existing) → **iOS** (Apple toolchain, macOS build host,
signing, App Store review, Metal). Budget extra time for iOS — it's the only target
that needs hardware/toolchain you don't already have on this Fedora workstation.

## 9. Phased rollout (threads through the main roadmap)

Develop on desktop from day one for fast iteration; ship in risk order.

- **Phase 0–1 — Develop on Linux desktop.** Fastest edit→build→run loop, your native
  OS, Vulkan shared with Android. Build the PAL boundary *now* so platform code never
  leaks into the core — retrofitting portability later is far costlier.
- **Phase 1 milestone — Android + Linux parity.** Both Vulkan; proves the PAL with the
  two cheapest backends and validates the slice on real arm64 hardware.
- **Phase 2–3 — Add Windows.** Vulkan first (reuse), native D3D12 as an optimization
  pass. Desktop mouse+kbd controls mature here.
- **Phase 3 — Stand up cross-platform lockstep + the full checksum CI matrix.** Don't
  let platforms diverge before this gate.
- **Phase 4 — Add iOS.** Metal backend (or MoltenVK shortcut first), Apple toolchain,
  signing, review. Last because it carries the most external friction.

## 10. Risks specific to going cross-platform

| Risk | Mitigation |
|---|---|
| Platform code leaks into the core, killing portability | Enforce the PAL boundary from Phase 0; core has zero platform `#include`s; CI builds the core standalone |
| Cross-arch sim divergence → silent desync | Fixed-point only; checksum-diff CI across the full platform/compiler/arch matrix from day one |
| iOS toolchain/process shock | Plan for a macOS build host + signing + review early; don't discover it at ship time |
| Renderer rewritten N times | One RHI; `wgpu` (Rust) or Vulkan+MoltenVK shortcut (C++) before native D3D12/Metal optimization |
| Controls good on one input class, bad on another | Phase 0 prototypes **both** touch and mouse+kbd; intent layer keeps the core input-agnostic |
| Maintenance multiplies | Shared core is non-negotiable; only the thin PAL is per-platform |
