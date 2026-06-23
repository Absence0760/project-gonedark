# CLAUDE.md — project-gonedark

Working title **"Going Dark"**: a mobile-first **RTS / FPS hybrid**. You command and
grow camps from a top-down view like *Company of Heroes*, then **possess a single unit
and fight it in first person — while the strategic map goes dark.** One player does
both jobs; the tension is divided attention.

**Current state: pre-production / design-only. There is NO engine code yet.** All work
so far is the markdown design corpus in `docs/`. Treat changes as documentation work
until the first code phase begins (roadmap Phase 0/1).

---

## Read these first

| Doc | What it is |
|---|---|
| `docs/game-design.md` | The design — pillars, embodiment, the going-dark vision model, unit-AI philosophy |
| `docs/architecture.md` | Engine & systems reference — native core, deterministic sim, Vulkan, lockstep netcode |
| `docs/platforms.md` | Cross-platform plan — Windows/Linux/Android/iOS, shared core + native backends |
| `docs/roadmap.md` | Build phases, dev workflow, top risks |
| `docs/decisions.md` | Decision log (ADR-style, D1…Dn). The *why* behind every locked choice |
| `docs/open-questions.md` | Unresolved design forks (Q1…Qn) with current leans |

When the user settles a design question, **record it**: append a `Dn` entry to
`docs/decisions.md` and move the resolved item out of `docs/open-questions.md`. Keep the
README repo-map and inter-doc links in sync.

---

## Non-negotiable invariants

These are the load-bearing decisions. Violating any of them silently breaks the game.
Do not "improve" past them without the user explicitly reopening the decision.

1. **No floats in the simulation. Ever.** The sim is **fixed-point** so it is
   bit-identical across devices, CPU architectures, and compilers. Floats live *only*
   in rendering. No `libm` transcendentals in sim (use fixed-point / LUTs); fast-math
   disabled. Floats leaking into the sim desync lockstep **silently** — there is no
   error, just divergence. (`decisions.md` D7 context, `architecture.md` determinism
   checklist.)

2. **One shared deterministic core; the platform layer never leaks into it.** Game
   logic (ECS, sim, systems, AI, netcode) is identical on all four platforms. Only the
   thin **PAL** (GPU/audio/input/window/storage) is per-platform. The core carries
   **zero** platform `#include`s. Never fork game logic per platform — it kills
   cross-play and multiplies maintenance. (`decisions.md` D9.)

3. **Unit AI is a literal executor, not a strategist.** Units hold their last order +
   a simple stance and do *exactly* that. Never give units autonomous "smart"
   decision-making — it lets the game play itself and destroys the core skill. Design
   depth goes into the **order/stance vocabulary**, not the AI brain. (`decisions.md`
   D3, `game-design.md` §8.)

4. **Sim and render are decoupled.** Sim runs fixed 30 Hz; render runs at a variable
   rate and interpolates. The sim never touches Vulkan/Metal/D3D12; the renderer never
   mutates sim state. (`architecture.md`.)

5. **Embodiment is an input-source swap + a vision toggle — not a character system.**
   Possessing a unit swaps that ECS entity's input from AI/orders to live player input,
   and flips the local player's fog to avatar-only. There is **no FPS respawn system**
   and no separate player-character object: death ejects you back to command and you
   pick another unit. Don't reintroduce respawn/character-lives machinery.
   (`decisions.md` D6, D7.)

6. **"World goes dark" must stay fair.** While embodied: alerts, not intel (directional
   flash + audio, not map reveal); audio is a primary system; the blindness is visceral
   and constant. Every loss must read as *"I stayed too long,"* never *"the game robbed
   me."* (`game-design.md` §6.)

7. **Cross-platform lockstep needs a full CI matrix.** When netcode/sim code exists,
   per-tick checksum diffing must run across
   `{Windows/MSVC, Linux/Clang, Android/Clang-arm64, iOS/Clang-arm64}` — not one
   platform. (`platforms.md` §7.)

---

## How to work in this repo

- **Docs are the product right now.** Match the existing markdown voice: tight,
  opinionated, reasoned. Prose wraps at ~88 columns. Tables and fenced ASCII diagrams
  are used heavily — follow suit.
- **Decision-log discipline.** `docs/decisions.md` is append-only, newest at the bottom,
  every entry has a **Why**. Use `/decision` (see `.claude/commands/`) to add one.
- **Don't silently decide open questions.** If you resolve a `Qn`, say so and migrate it
  to a `Dn`. If you hit a *new* fork, add it to `open-questions.md` rather than picking
  for the user.
- **Names:** game working title is "Going Dark" (placeholder, `open-questions.md` Q6);
  repo/dir is `project-gonedark`. Keep them distinct.
- **Git:** commit only when the user asks. Follow the user's global commit rules (no
  attribution footers/trailers of any kind).

## When code eventually starts (not yet)

- **Language is undecided** — C++20 or Rust (`decisions.md` D8). Cross-platform leans
  Rust because `wgpu` gives the four native backends nearly free (`platforms.md` §3).
  Don't assume one until the user picks.
- **Build:** CMake meta-build, per-platform toolchain files; Android via CMake+Gradle.
- **The PAL boundary goes in from the first commit of engine code** — retrofitting
  portability is far costlier than building to it.
- Workstation toolchain conventions (Android NDK, Rust, etc.) live in the user's global
  `~/CLAUDE.md`, not here.

## Glossary

- **Embodiment** — possessing one unit to control it in first person.
- **Going dark** — losing strategic vision while embodied (avatar-only fog).
- **Literal-executor AI** — units obey the last order/stance, no autonomous strategy.
- **Command layer / Embodiment layer** — the RTS view and the FPS view; mutually
  exclusive in time.
- **PAL** — Platform Abstraction Layer (the thin per-platform backend boundary).
- **RHI** — Render Hardware Interface (one renderer API over Vulkan/Metal/D3D12).
- **Lockstep** — clients exchange orders, not world state; relies on determinism.
