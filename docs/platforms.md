# Cross-Platform Plan — Windows · Linux · Android · iOS

> How one game runs on four platforms, each using the GPU/OS path that is *native
> and optimal* for that device — without forking the game itself.

## 1. The principle — one core, four optimized backends

"Each device optimized for that device" is right, with one critical boundary:

- **The game architecture is identical on every platform.** ECS, the deterministic
  fixed-point sim, game systems, pathfinding, AI, netcode — pure portable code
  (Rust), no platform dependencies. It compiles and runs bit-identically
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
   │ Linux   │  Vulkan · PipeWire/ALSA · Wayland/X11 (winit) · mouse+kbd/pad
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
| Windowing / lifecycle | `winit` + event abstraction | Win32 | Wayland/X11 | NDK + Kotlin/JNI | UIKit + CAMetalLayer |
| Input | order/intent layer shared | mouse+kbd, gamepad | mouse+kbd, gamepad | touch, gyro | touch, gyro |
| Filesystem / mmap | VFS interface | Win32 | POSIX | POSIX + AAsset | POSIX + bundle |
| Build output | `cargo` (target triple) | `.exe` | ELF (Flatpak/AppImage) | `.aab`/`.apk` | `.ipa` |

The **Platform Abstraction Layer (PAL)** is the set of narrow interfaces between the
core and these backends. Keep it *thin* — only what genuinely differs crosses it.

## 3. Rendering — the RHI (Render Hardware Interface)

The renderer talks to a small internal RHI; each platform implements it with its
native API. **With Rust chosen ([`decisions.md`](decisions.md) D10), the RHI is
[`wgpu`](https://wgpu.rs):** it targets Vulkan (Linux/Android/Windows), **D3D12**
(Windows), and **Metal** (iOS), and *picks the optimal backend per device
automatically* — the exact per-device-native goal, handed to you. No hand-rolled RHI,
no MoltenVK translation layer, no per-platform renderer rewrite.

For reference, had the engine been C++, the equivalents would have been: hand-roll a
thin RHI over Vulkan + Metal + D3D12, adopt a multi-backend layer (**The Forge**,
**bgfx**, **Diligent Engine**), or take the pragmatic shortcut of one **Vulkan**
renderer run on iOS via **MoltenVK**. `wgpu` makes all of that unnecessary — it's one
of the decisive reasons Rust won the language call.

**Recommendation:** `wgpu` for all four platforms. Drop to a raw backend (e.g. `ash`
for Vulkan) only for a specific hotspot if profiling ever demands it — not up front.

> **macOS bonus:** an iOS Metal backend extends to macOS for nearly free. Not a target
> now, but worth knowing the door is open.

## 4. Windowing / audio / input — the rest of the PAL (Rust crates)

With Rust + `wgpu`, the canonical pairing is **`winit`** for windowing/lifecycle/input
across all four targets (clean **Wayland** support on Linux, Android via
`android-activity`, iOS supported), **`gilrs`** for gamepad, and **`cpal`** (or a
higher-level engine like `kira`/`rodio`) for audio backending to WASAPI, CoreAudio,
AAudio, and PipeWire/ALSA — native per platform, one API. (The **`sdl3`** crate is a
single-dependency alternative that bundles windowing+input+audio if you'd rather not
assemble the pieces.) That leaves you to hand-roll only a few platform services
(billing, push, mmap specifics) behind the PAL.

The strategic-layer audio mix that bleeds into the embodied "world goes dark" view
(design doc §6) is engine-side and identical everywhere — and the per-voice *render* math
(pan/gain/muffle/sum) is shared too, in the host-tested `pal::mix` seam, so every backend
mixes identically. **Implemented backends:** desktop renders the mix via `cpal` (opt-in
`audio` feature, D26); **Android renders it via `oboe` — a real low-latency AAudio output
stream (D29)** (the §1 table's "Android · AAudio" entry). iOS (CoreAudio) is later.

## 5. Input — the real per-platform divergence

This is where platforms genuinely differ, and it ties straight to the project's top
product risk (the touch-control problem, roadmap Phase 0). The trick: the core only
ever consumes **orders/intents**, never raw input. Each platform's input layer
*translates* its native scheme into the same intent vocabulary.

| Input class | Platforms | Command layer | Embodied (FPS) layer |
|---|---|---|---|
| **Mouse + keyboard** | Windows, Linux | native CoH paradigm — *easy* | mouse-look + WASD — *easy, familiar* |
| **Touch + gyro** | Android, iOS | tap-select / tap-command, two-finger embody (D43) | **shipped: left move stick + right drag-look + Fire/Crouch/Reload/Surface buttons** (D51) |
| **Gamepad** | all (optional) | cursor/radial — *medium* | twin-stick — *medium* |

**Design consequence:** desktop gets the *native* CoH control feel almost for free,
while mobile is where the control scheme must be invented. The Phase-0 prototype
validated the embodied scheme on real hardware ([D14](decisions.md)); the shipping
COD-style on-screen HUD is now built ([D51](decisions.md)). The raw touch points cross
the PAL as `InputFrame.touches`; the pure `engine::touch_controls` seam (not the
per-platform backend) maps them to intents, so the core stays input-agnostic and the
mapping is host-testable. **The on-screen embodied GUI is Android-only**; desktop keeps
keyboard+mouse. While embodied, two fingers mean move+look, so ejecting to command is the
on-screen **Surface** button — not the two-finger gesture (which stays embody-only).

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

This is the *runtime* tier — the same source asset scaled per device. Where that source
art comes from, how it's graded into tiers in the cook step, and the open-source sourcing
+ license policy behind it are covered in [`content-pipeline.md`](content-pipeline.md).

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
  [`architecture.md`](architecture.md) to a build matrix (all Rust/LLVM, varying
  target triple): `{x86_64-pc-windows-msvc, x86_64-unknown-linux-gnu,
  aarch64-linux-android, aarch64-apple-ios}`.
- iOS caveat for any scripting VM: **no JIT on iOS** — run Lua (or similar) in
  interpreter mode; LuaJIT's JIT is unavailable. Affects the scripting-hot-reload dev
  convenience (roadmap), not shipping determinism.

## 8. Build & distribution

**`cargo`** is the meta-build; each platform is a target triple plus the store
wrapper its ecosystem requires (`cargo-ndk` → Gradle for Android, `cargo` → Xcode for
iOS).

| Platform | Build | Package | Store / channel | Notes |
|---|---|---|---|---|
| Linux | `cargo` (gnu triple) | AppImage or **Flatpak** | Flathub, itch, Steam | Easiest desktop bring-up; your dev OS |
| Windows | `cargo` (msvc triple) | `.exe` + installer | Steam, itch | wgpu picks D3D12; PIX still works for profiling |
| Android | `cargo-ndk` + Gradle | `.aab` | Google Play | Existing primary target |
| iOS | `cargo` + Xcode | `.ipa` | App Store | **Needs a macOS build host**, code signing, review |

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
- **Phase 4 — Add iOS.** `wgpu`'s native Metal backend, Apple toolchain,
  signing, review. Last because it carries the most external friction.

## 10. Risks specific to going cross-platform

| Risk | Mitigation |
|---|---|
| Platform code leaks into the core, killing portability | Enforce the PAL boundary from Phase 0; core has zero platform `#include`s; CI builds the core standalone |
| Cross-arch sim divergence → silent desync | Fixed-point only; checksum-diff CI across the full platform/compiler/arch matrix from day one |
| iOS toolchain/process shock | Plan for a macOS build host + signing + review early; don't discover it at ship time |
| Renderer rewritten N times | One RHI — `wgpu` provides the native per-device backend; drop to a raw backend only for a profiled hotspot |
| Controls good on one input class, bad on another | Phase 0 prototypes **both** touch and mouse+kbd; intent layer keeps the core input-agnostic |
| Maintenance multiplies | Shared core is non-negotiable; only the thin PAL is per-platform |
