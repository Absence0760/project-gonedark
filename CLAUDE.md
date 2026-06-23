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
| `docs/infrastructure.md` | Local dev (Docker), env/config files, Terraform infra, sops secrets |
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
   bit-identical across devices, CPU architectures, and compilers. No `f32`/`f64` in
   sim/core types or math; floats live *only* in rendering. No `std`/libm
   transcendentals in sim (use fixed-point / LUTs). Floats leaking into the sim desync
   lockstep **silently** — there is no error, just divergence. (`decisions.md` D7
   context, `architecture.md` determinism checklist.)

2. **One shared deterministic core; the platform layer never leaks into it.** Game
   logic (ECS, sim, systems, AI, netcode) is identical on all four platforms. Only the
   thin **PAL** (GPU/audio/input/window/storage) is per-platform. The core crate
   depends on **no** platform/windowing/GPU crates (`wgpu`, `winit`, JNI, etc.). Never
   fork game logic per platform — it kills cross-play and multiplies maintenance.
   (`decisions.md` D9.)

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
   `{x86_64-pc-windows-msvc, x86_64-unknown-linux-gnu, aarch64-linux-android,
   aarch64-apple-ios}` — not one
   platform. (`platforms.md` §7.)

8. **Clone-and-run locally; never commit a plaintext secret.** Local dev runs against
   Docker (`compose.yaml`) using committed, non-secret defaults in `.env.development` —
   keep it working with zero setup. Real secrets are KMS-encrypted (sops) in
   `infra-secrets/` (only `*.sops.yaml` is committable — `.gitignore` blocks plaintext
   there). All cloud infra is Terraform in `infra/`; no click-ops. Never put a real
   secret in `.env*`, code, or any tracked file. (`docs/infrastructure.md`.)

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

- **Language: Rust** (`decisions.md` D10). Renderer via `wgpu` (native
  Vulkan/D3D12/Metal per device); ECS via Bevy/hecs/legion or hand-rolled; windowing
  via `winit`; FFI to Kotlin/JNI (Android) and Swift/Obj-C (iOS) for platform services.
  C++ is only a fallback if D10 is ever reversed.
- **Build:** `cargo` meta-build; `cargo-ndk` + Gradle (Android), `cargo` + Xcode (iOS).
- **The PAL boundary goes in from the first commit of engine code** — retrofitting
  portability is far costlier than building to it.
- **Mind the one Rust tradeoff:** weaker engine-code hot-reload (no stable ABI). Lean on
  scripting/data hot-reload and the automated build loop; reach for `hot-lib-reloader`/
  `dexterous_developer` only if those aren't enough.
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
