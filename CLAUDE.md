# CLAUDE.md — project-gonedark

Working title **"Going Dark"**: a mobile-first **RTS / FPS hybrid**. You command and
grow camps from a top-down view like *Company of Heroes*, then **possess a single unit
and fight it in first person — while the strategic map goes dark.** One player does
both jobs; the tension is divided attention.

**Current state: Phase 1 in progress — the engine spine is real through build-order step 5,
compile-verified but not yet device-validated (D10).** The design corpus in `docs/` is still
the product of record, but engine code now exists: the Cargo workspace (`core/ pal/ render/
pal-desktop/ pal-android/ app/ sim-runner/ server/`) with a deterministic fixed-point `core`
(Q16.16 [D17], hand-rolled SoA ECS [D18]). **Steps 3–5 done & verified:** a real deterministic
**flow field** (`core::flow_field` — integer Dijkstra over a 128×128 fixed grid) driving the
`movement_system`, with `sim-runner` bit-identical run-to-run and debug==release; a real
`wgpu` 29 + `winit` 0.30 desktop renderer and PAL backend that interpolate prev→curr snapshots
(invariant #4); and an `app` run loop (fixed-tick accumulator, tap-to-move, embody/surface
input swap with "world goes dark"). Per [D19], `core`+`pal` stay GPU-free; `render`/
`pal-desktop`/`app` carry wgpu. **Caveat: steps 4–5 are compile-verified only — not run on a
GPU/display here.** Step 6 (`pal-android` + `android/` Gradle) now **compiles + links for
real arm64** via `cargo-ndk` (`libgonedark_pal_android.so`); it is **not yet APK-packaged or
run on a device**, and the shared sim/render game-loop wiring inside `android_main` is Phase
2. Step 7 CI is **extended**: a
blocking `graphics-build` job + an `android-build` tripwire (`continue-on-error` until the
backend is real); the determinism checksum matrix is unchanged. Two decide-first gates are
locked (D17/D18); **sim rate (Q10) is still open**, parameterized as `core::sim::TICK_HZ`
(provisional 60), to be profiled on real arm64 before locking. Step 8 (on-device validation)
is **not done**, so the Phase 1 exit criterion (one unit, commandable + embodiable, on real
arm64 with the checksum matrix green) is **not yet met** — keep the Unity/Godot fallback live
until it is. The two **throwaway Godot
prototypes** in `prototypes/` (`phase0-controls/` → D14, `phase0.5-netfeel/` → D15, both
2026-06-23) are disposable feel-test scaffolding — *not* engine code, carry none of the
invariants below, and can be deleted. Don't grow a prototype into the game; build behind the
invariants instead.

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

4. **Sim and render are decoupled.** The sim runs a **fixed deterministic tick**; render
   runs at a variable rate and interpolates. The sim never touches Vulkan/Metal/D3D12; the
   renderer never mutates sim state. (The **tick rate** is being finalized in Phase 1 — 30 Hz
   proved too coarse for embodied combat, target ~60 Hz; see `decisions.md` D16 and Q10. The
   decoupling + fixed-deterministic-tick core of this invariant is rate-independent and still
   load-bearing.) (`architecture.md`.)

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

8. **Clone-and-run locally; never commit a plaintext secret — and no secrets in *this*
   repo at all.** Local dev runs against Docker (`compose.yaml`) using committed,
   non-secret defaults in `.env.development` — keep it working with zero setup. Real
   secrets are KMS-encrypted (sops) in the **separate private estate repo**
   (`~/github/infra-secrets/gonedark/`, a sibling of this repo), *not* in this
   potentially-public game repo — Terraform reads them via the `carlpett/sops` provider
   at `../../infra-secrets/gonedark/prod.sops.yaml`. All cloud infra is Terraform in
   `infra/`; no click-ops. Never put a real secret in `.env*`, code, or any tracked
   file. (`docs/decisions.md` D12, `docs/infrastructure.md`.)

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
- **Git — work on `main`, commit completed work.** A normal session works **directly on
  `main`**; do *not* open a feature branch (this overrides the harness "branch first on
  the default branch" default). Branches exist only for isolated/parallel runs via
  `claude --worktree <name>` (see `.claude/README.md`). When a logical unit of work is
  finished and self-consistent, **commit it** — don't leave the tree dirty waiting to be
  asked (this overrides the global "commit only when the user asks" default *for this
  repo*).
- **Git — keep every commit path-scoped.** Stage and commit only the paths you actually
  changed: `git add <path>` then `git commit -m "…" -- <path1> <path2>`. **Never**
  `git add -A`/`.`/`-u`, a bare `git commit`, or `git commit -a` — `git-scope-guard.py`
  denies them, and for good reason: concurrent sessions and worktrees share this
  checkout, so a whole-tree stage would sweep up another session's in-flight work. One
  commit = one workstream. Follow the user's global commit rules (no attribution
  footers/trailers of any kind).

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
