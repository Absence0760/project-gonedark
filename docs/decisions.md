# Decision Log

Lightweight ADR-style record of the design choices locked in during pre-production,
and *why*. Newest decisions at the bottom. Open forks live in
[`open-questions.md`](open-questions.md).

---

## D1 — One player does both roles (not asymmetric commander + soldiers)

**Decision:** A single player is both the RTS commander and the FPS avatar, switching
between them — *not* the *Eximius* / *Natural Selection* model where the commander and
the soldiers are different people.

**Why:** The whole intended tension — opportunity cost, divided attention — only
exists if the *same* person owns both jobs. Splitting them across players removes the
core decision.

---

## D2 — Divided attention is the skill; the AI does not delegate

**Decision:** Command and embodiment are mutually exclusive in time. You cannot do
both at once.

**Why:** This is the game's primary skill expression — the macro/micro attention
split of high-level RTS, cranked to the extreme (embodying means *zero* command for
that whole duration).

---

## D3 — Unit AI is a literal executor, not a smart autopilot

**Decision:** Units hold their last order + a simple stance and do exactly that —
nothing more. No autonomous strategic decisions.

**Why:** Smart autopilot quietly lets the game play itself and makes "whose AI is
better" the contest instead of player skill. Dumb, obedient AI means leaving your
army is a *real* risk, which is the point. Bonus: it's cheaper and more deterministic
to build than smart LOD AI.

**Consequence:** Design depth moves into the **order/stance vocabulary** (patrol
routes, engagement ranges, retreat triggers, trigger zones, queued production) — that
is the intended home for "smart play": the human's planning, not the machine's
reactions.

---

## D4 — Switching is cheap; the absence is expensive

**Decision:** Entering/leaving a unit is fast and smooth. The cost of embodiment is
*time away from command*, never UI friction.

**Why:** Clunky menus are fake difficulty. The intended cost is opportunity cost; the
transition itself should never be the obstacle.

**Corollary:** Embodiment must be *mechanically better* at the local fight than AI
control, or players won't accept the cost and the FPS layer becomes a dead novelty.

---

## D5 — Pure opportunity cost: stay as long as you dare

**Decision:** No hard timer, fuel gauge, or cooldown forcing you out of a unit. You
stay embodied as long as you choose.

**Why:** It's the most elegant and most punishing option. The bound is emergent (risk
of death + neglect), not an artificial mechanic.

---

## D6 — Death is a demotion, not a game-over

**Decision:** Dying while embodied ejects you back to the command view. No FPS respawn
timer, no spectator downtime. You then pick a *different* living unit to embody. The
unit you were driving is gone for good.

**Why:**
- Zero dead time — death drops you into a fully active RTS, never an idle wait.
- Self-balancing — overstaying gets you killed, which costs you the unit *and* dumps
  you into a neglected base. Greed is auto-punished.
- Makes the embodied unit precious (you lose it permanently), tying the risk into the
  army economy.
- **Big tech win:** you always respawn as the commander, so there is **no FPS respawn
  system to build** — the avatar is just whichever entity you're currently driving,
  not a persistent character with lives. Deletes a pile of state/netcode complexity.

**Consequence — comeback loop:** if your whole army dies you're stripped to pure
base-builder until you produce a new embodiable unit. That's the intended low point.

---

## D7 — While embodied, the world goes dark (blind, not informed)

**Decision:** Embodiment reverts the player's vision to *avatar-only*. You do **not**
keep strategic map vision. (Chosen over the "informed dread" alternative where you
keep passive map sight and watch helplessly.)

**Why:** Turns information into the game's real currency. The central skill becomes
*"can I afford to be blind right now?"* — reading the board before you dive. The blind
version makes "stay as long as you dare" genuinely tense because you don't even know
what you're sacrificing.

**Guardrails (so it's fair, not robbery):**
- **Alerts, not intel** — a thin thread back: directional flash + audio ("base under
  attack"), telling you *something* is wrong but not what/how bad.
- **Audio is a primary system** — strategic-layer sound bleeds into the embodied mix;
  you *hear* your empire when you can't see it.
- **Visceral, constant feedback** — vignette / darkened edges so the cost is always
  *felt*; the player must own every death as "I stayed too long."

**Tech note:** this is a vision-culling toggle in the presentation layer, not a sim
change — it cannot cause desyncs and needs no special pause machinery.

---

## D8 — Pre-production is design-only; engine direction is custom-native with a live fallback

> **Language resolved in [D10](#d10--engine-language-rust): Rust.** The "C++ or Rust"
> openness below is superseded; the rest of D8 stands.

**Decision:** Target Android-first (arm64-v8a, Vulkan 1.1). Lead direction is a custom
native engine (C++20 or Rust — a performance wash; decide on engineering values).
Keep Unity DOTS / Godot+GDExtension as a live fallback until the vertical slice is
validated on real hardware.

**Why:** The RTS ceiling (deterministic many-agent sim in a fixed budget) needs
cache-line control that managed runtimes abstract away — but the build cost is real,
so we don't burn the fallback until Phase 1 proves the slice. See
[`architecture.md`](architecture.md) and [`roadmap.md`](roadmap.md).

---

## D9 — Four platforms: one shared deterministic core, platform-optimized backends

**Decision:** Ship on **Windows, Linux, Android, and iOS**. The game architecture (ECS,
deterministic fixed-point sim, game systems, AI, netcode) is **identical on every
platform**; only the **platform backend** (GPU API, audio, windowing, input, storage)
and the **presentation tier** (resolution, effects, refresh, controls) are optimized
per device — native paths: **D3D12/Vulkan on Windows, Vulkan on Linux, Vulkan 1.1 on
Android, Metal on iOS.** Develop on Linux desktop first; ship Android-first.

**Why:** "Optimized per device" must mean *native backends*, not *forked game logic*.
Forking the core would (1) kill cross-platform lockstep — deterministic play requires
every client to run the *same* sim — and (2) turn one game into four to maintain. The
float-free fixed-point sim already produces bit-identical results across x86-64 and
arm64, so cross-play across all four platforms is achievable *for free* — provided the
per-tick checksum CI runs across the whole platform/compiler/arch matrix from day one.

**Consequences:**
- A **Platform Abstraction Layer (PAL)** boundary is enforced from Phase 0; the core
  carries zero platform dependencies (`winit` collapses most of the non-GPU PAL).
- The renderer talks to one **RHI** — `wgpu` (native Vulkan/D3D12/Metal per device)
  hits all four targets with no per-platform renderer. *(This was a decisive factor in
  later choosing Rust as the engine language — see [D10](#d10--engine-language-rust).)*
- iOS carries the most external friction (macOS build host, signing, review, Metal) and
  is sequenced last.

See [`platforms.md`](platforms.md) for the full plan.

---

## D10 — Engine language: Rust

**Decision:** Build the engine in **Rust**, not C++. Resolves the language question left
open in [D8](#d8--pre-production-is-design-only-engine-direction-is-custom-native-with-a-live-fallback).
Renderer via **`wgpu`** (native Vulkan/D3D12/Metal, auto-selected per device); ECS via a
mature crate (Bevy/hecs/legion) or hand-rolled; FFI to Kotlin/JNI (Android) and
Swift/Obj-C (iOS) for platform services.

**Why:** Performance is a wash (both are LLVM-native), so the call is on engineering
values over a long horizon — and for *this* project (greenfield, small/AI-assisted team,
custom-native, mobile+desktop, determinism-critical) Rust's strengths hit the load-
bearing needs while its weaknesses miss them:

- **Cross-platform GPU is solved by `wgpu`** — native backends per device, which is
  exactly the D9 goal. C++ would mean hand-rolling an RHI or adopting The Forge/bgfx.
- **Fearless concurrency.** The deterministic lockstep sim is heavily threaded (job
  system, parallel culling/pathfinding/AI). Its worst bugs are silent, non-reproducible
  data races and determinism leaks. Rust eliminates that entire class at compile time —
  a correctness guarantee C++ cannot give. This is the decisive factor.
- **Determinism is more enforceable** via the type system (newtype fixed-point, no
  implicit float conversions, no UB, no uninitialized reads).
- **Toolchain simplicity** — `cargo` vs CMake + vcpkg/Conan + per-platform toolchains;
  compounds across four platforms over years.

**The maturity question, answered:** Rust's remaining gaps don't intersect this project.
- *Engine-code hot-reload* (no stable ABI) — the one real cost; mitigated by doing
  game-feel iteration in scripting/data + the automated build loop (roadmap). Architect
  a reloadable module only if it hurts.
- *Commercial middleware / hiring pool / console toolchains* — not load-bearing for a
  custom-native, mobile+desktop, small-team project; platform services cross into
  Kotlin/Swift via FFI regardless of engine language.

**When this should be revisited:** if priorities shift to shipping in ~6 months, hiring a
C++-veteran team, targeting consoles, or building on a commercial engine/middleware
stack — none currently true.

**Consequences:**
- The risk to actively manage is **engine-iteration speed** — lean on scripting/data hot-
  reload and the automated edit→build→test loop.
- iOS scripting caveat stands: no JIT, so any embedded VM (e.g. Lua) runs interpreted.
- Architecture/platforms docs now treat Rust as the language; C++ remains a noted
  fallback only if D10 is ever reversed.
