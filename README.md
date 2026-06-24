# Going Dark *(working title)*

A mobile-first **RTS / FPS hybrid**. Command and grow your camps from above like
*Company of Heroes* — economy, territory, army-building, cover-and-suppression
tactics — then **drop into a single tank or trooper and fight in first person**.
The catch: while you're embodied, *the world goes dark*. You lose all sight of
the battlefield except what your unit can see. Stay in as long as you dare.

This repo holds the design, architecture, and roadmap, and — as of Phase 1 — the
**Rust engine workspace** (validated end-to-end on real arm64, D22; see Status). The
Phase 0/0.5 throwaway Godot prototypes that proved touch-feel and netcode feel have
been deleted on Phase 1 completion.

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
| [`docs/phase-1-plan.md`](docs/phase-1-plan.md) | Detailed plan + sign-off record for the Phase 1 Rust engine vertical slice (DONE — exit criterion met, Galaxy S24, D22) |
| [`docs/decisions.md`](docs/decisions.md) | Decision log — the choices we locked in and the reasoning |
| [`docs/open-questions.md`](docs/open-questions.md) | Unresolved design forks still on the table |
| `prototypes/` *(deleted)* | The two throwaway Godot prototypes (`phase0-controls/` → D14, `phase0.5-netfeel/` → D15) proved touch-feel and embodied netcode feel; deleted on Phase 1 completion (D22). Not the engine |
| `Cargo.toml` + `core/ pal/ render/ engine/ pal-desktop/ pal-android/ app/ sim-runner/ server/` | **The Rust engine workspace** (Phase 1). `core` = deterministic fixed-point sim incl. a real flow field (zero platform deps); `pal` = platform traits; `render` = real `wgpu` instanced renderer; `engine` = the platform-agnostic game loop (sim+render+fixed-tick+cameras+command/embodiment) that both hosts drive ([D20](docs/decisions.md)); `pal-desktop` = real `winit`+`wgpu` backend; `app` = thin winit desktop host; `pal-android` = JNI/cargo-ndk backend whose `android_main` drives the same `engine` loop (builds an arm64 APK); `sim-runner` = headless checksum driver; `server` = backend placeholder. See [`docs/phase-1-plan.md`](docs/phase-1-plan.md) |

## Status

**Phase 1 — DONE (D22). Phase 2 (game systems) is active.** Phase 0 (D14) and Phase 0.5
(D15) both passed (2026-06-23): touch-feel and embodied-combat-over-lockstep risks retired.
**Phase 1 exit criterion met on Galaxy S24, Adreno 750:** `pnpm android:checksum` confirmed
the device sim-runner checksum stream **bit-identical** to desktop over 300 ticks
(`4c34c6b5951edf57`); the `adb logcat` FPS heartbeat showed **120 fps** sustained at the locked
**60 Hz** sim tick — demonstrating sim/render decoupling (invariant #4) live on hardware. One
unit moves via a real deterministic flow field; tap-to-move works; the two-finger embody toggle
flips the world dark. The Rust engine workspace carries: a deterministic fixed-point `core`
(Q16.16 [D17](docs/decisions.md), hand-rolled SoA ECS [D18](docs/decisions.md)), the PAL trait
boundary, a real `wgpu` 29 + `winit` 0.30 renderer + `pal-desktop`/`pal-android` backends
([D19](docs/decisions.md)), and the shared `engine::Game` loop ([D20](docs/decisions.md)) that
both hosts drive. All three decide-first gates locked — sim rate closed by
[D21](docs/decisions.md): **global 60 Hz** (`core::sim::TICK_HZ = 60`; dual-rate deferred to
Phase 3). The **Unity/Godot fallback ([D8](docs/decisions.md)) is retired**; the custom Rust
engine is committed. **Honest caveat:** validated on a flagship; frame-rate/thermal on mid-range
silicon and the 200-unit power budget are Phase 3.

Target platforms: **Windows, Linux, Android, iOS** — one shared deterministic core with
platform-optimized backends (D3D12/Vulkan, Vulkan, Vulkan, Metal), developed on Linux desktop
first and shipping Android-first. See [`docs/platforms.md`](docs/platforms.md). Engine:
**custom native in Rust** (renderer via `wgpu`) — see [`docs/decisions.md`](docs/decisions.md)
D10 for the reasoning.

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
