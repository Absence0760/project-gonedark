# Going Dark *(working title)*

A mobile-first **RTS / FPS hybrid**. Command and grow your camps from above like
*Company of Heroes* — economy, territory, army-building, cover-and-suppression
tactics — then **drop into a single tank or trooper and fight in first person**.
The catch: while you're embodied, *the world goes dark*. You lose all sight of
the battlefield except what your unit can see. Stay in as long as you dare.

This repo holds the design, architecture, and roadmap, and — as of Phase 1 — the
**Rust engine workspace** (a deterministic core with a real flow field, plus a
compile-verified desktop renderer + command/embodiment run loop; see Status). The
disposable Phase 0/0.5 prototypes in [`prototypes/`](prototypes/) are feel-test
scaffolding, not the engine.

---

## The pitch in one line

> You are always the commander. Embodiment is a lens you put on — powerful, and
> blinding. The skill isn't whose AI plays better; it's *yours*: how well you set
> your army up before you dive, and whether you can read the board well enough to
> know when it's safe to go dark.

## The core loop

1. **Command** (top-down RTS) — build and upgrade camps, manage economy, train an
   army, capture territory, issue orders and stances to your units.
2. **Embody** (FPS) — possess any one of your living units. Your skill with that
   tank or soldier is now in play: precise aim, peeking cover, clutch moments the
   AI literally cannot do.
3. **Go dark** — the moment you embody, the strategic map blacks out. You see only
   what your unit sees. Thin alerts ("base under attack") are your one thread back.
4. **Surface** — pull out when you dare, or die and get ejected back to command.
   Re-read the changed board fast, re-issue orders, pick your next moment.

## What makes it different

Most RTS/FPS hybrids either split the two roles across different players
(*Eximius*, *Natural Selection 2*) or make the unit AI smart enough that leaving
your army alone is safe (which quietly lets the game play itself). **Going Dark
does neither.** One player does both jobs, the AI is a deliberately *literal*
order-executor, and embodiment costs you your sight. That turns information into
the game's real currency and makes "can I afford to be blind right now?" the
central, skill-based decision.

See [`docs/game-design.md`](docs/game-design.md) for the full design and
[`docs/decisions.md`](docs/decisions.md) for *why* each rule is the way it is.

## Repo layout

| Path | What's in it |
|---|---|
| [`docs/game-design.md`](docs/game-design.md) | The game design doc — concept, mechanics, the going-dark rule, unit AI philosophy |
| [`docs/architecture.md`](docs/architecture.md) | Engine & systems architecture (native core, deterministic sim, Vulkan, netcode) |
| [`docs/platforms.md`](docs/platforms.md) | Cross-platform plan — Windows/Linux/Android/iOS, one shared core with platform-optimized backends |
| [`docs/content-pipeline.md`](docs/content-pipeline.md) | Asset production — quality tiers, open-source sourcing, license hygiene, the two-view filter, what Claude can/can't build |
| [`docs/infrastructure.md`](docs/infrastructure.md) | Local dev (clone-and-run via Docker), config/env files, Terraform infra, sops secrets |
| [`docs/roadmap.md`](docs/roadmap.md) | Build phases, milestones, and the top risks |
| [`docs/phase-0.5-plan.md`](docs/phase-0.5-plan.md) | Plan + record of the embodiment-over-network latency spike (resolved Q7/Q8 → D15/D16) |
| [`docs/phase-1-plan.md`](docs/phase-1-plan.md) | Detailed plan for the **next** build — the Phase 1 Rust engine vertical slice |
| [`docs/decisions.md`](docs/decisions.md) | Decision log — the choices we locked in and the reasoning |
| [`docs/open-questions.md`](docs/open-questions.md) | Unresolved design forks still on the table |
| [`prototypes/phase0-controls/`](prototypes/phase0-controls/) | **Throwaway** Godot control prototype — proved the Phase 0 touch-feel gate (D14); deleted after Phase 0.5. Not the engine |
| `Cargo.toml` + `core/ pal/ render/ pal-desktop/ pal-android/ app/ sim-runner/ server/` | **The Rust engine workspace** (Phase 1). `core` = deterministic fixed-point sim incl. a real flow field (zero platform deps); `pal` = platform traits; `render` = real `wgpu` instanced renderer; `pal-desktop` = real `winit`+`wgpu` backend; `app` = the winit run loop (command + embodiment); `pal-android` = JNI/cargo-ndk backend (builds an arm64 `.so`); `sim-runner` = headless checksum driver; `server` = backend placeholder. See [`docs/phase-1-plan.md`](docs/phase-1-plan.md) |

## Status

**Phase 1 — in progress.** **Phase 0** (control prototype, D14) and **Phase 0.5**
(embodiment-over-network latency spike, D15) both **passed** (2026-06-23): the
embody↔command touch loop feels good in hand, and embodied combat feels good over the
lockstep netcode with **avatar-local prediction**. That retired the two biggest risks
(touch controls; embodied feel over the wire). **Phase 1 is underway and the spine is
real through build-order step 5 — compile-verified, not yet device-validated:** a
deterministic fixed-point `core` (Q16.16, [D17](docs/decisions.md); hand-rolled SoA ECS
[D18](docs/decisions.md)) with a real **flow field** moving one unit; a real `wgpu` 29 + `winit`
0.30 desktop renderer + PAL that interpolate between snapshots; and an `app` run loop wiring
tap-to-move command + embodiment (the "world goes dark" input swap). Per [D19](docs/decisions.md),
`core`/`pal` stay GPU-free; `render`/`pal-desktop`/`app` carry wgpu. The `core` tests and the
`sim-runner` determinism check (bit-identical run-to-run **and** debug==release) pass locally;
the per-tick checksum CI matrix ([invariant #7](docs/phase-1-plan.md)) is green and CI now also
builds the graphics crates. **Caveats:** the renderer/app are compile-verified only (no
GPU/display in the build env, so not run); the **Android backend compiles + links for arm64**
via `cargo-ndk` and **assembles an installable arm64 debug APK** (Gradle wrapper committed),
but is **not yet run on a device**; and sim rate
([Q10](docs/open-questions.md)) is still open
(`core::sim::TICK_HZ`, provisional 60). **Not yet met:** the Phase 1 exit criterion (one unit,
commandable + embodiable, on **real mid-range arm64 hardware** with the cross-arch checksum
matrix green) — build-order step 8 (on-device validation) is pending hardware. The Unity/Godot
fallback stays live until it passes.

Target platforms: **Windows, Linux, Android, iOS** — one
shared deterministic core with platform-optimized backends (D3D12/Vulkan, Vulkan,
Vulkan, Metal), developed on Linux desktop first and shipping Android-first. See
[`docs/platforms.md`](docs/platforms.md). Engine: **custom native in Rust** (renderer
via `wgpu`) — see [`docs/decisions.md`](docs/decisions.md) D10 for the reasoning, and
the architecture doc for the viable fallbacks (Unity DOTS, Godot + GDExtension) if the
custom path is ever abandoned.

## Local development

A fresh clone runs against local Docker services with committed, non-secret defaults —
no cloud access or secrets needed:

```
docker compose up -d        # Postgres + Redis (backend deps)
cargo run                   # loads .env.development   (once engine code exists)
```

Production secrets are KMS-encrypted (sops) in the separate private estate repo
(`~/github/infra-secrets/gonedark/`, **not** in this repo — see D12) and cloud infra is
Terraform in `infra/` — neither is touched for local work. Full details in
[`docs/infrastructure.md`](docs/infrastructure.md).
