# Going Dark *(working title)*

A mobile-first **RTS / FPS hybrid**. Command and grow your camps from above like
*Company of Heroes* — economy, territory, army-building, cover-and-suppression
tactics — then **drop into a single tank or trooper and fight in first person**.
The catch: while you're embodied, *the world goes dark*. You lose all sight of
the battlefield except what your unit can see. Stay in as long as you dare.

This repo holds the design, architecture, and roadmap. **There is no engine code
yet** — this is pre-production.

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
| [`docs/infrastructure.md`](docs/infrastructure.md) | Local dev (clone-and-run via Docker), config/env files, Terraform infra, sops secrets |
| [`docs/roadmap.md`](docs/roadmap.md) | Build phases, milestones, and the top risks |
| [`docs/decisions.md`](docs/decisions.md) | Decision log — the choices we locked in and the reasoning |
| [`docs/open-questions.md`](docs/open-questions.md) | Unresolved design forks still on the table |

## Status

**Pre-production / design.** Target platforms: **Windows, Linux, Android, iOS** — one
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

Production secrets are KMS-encrypted (sops) in `infra-secrets/` and cloud infra is
Terraform in `infra/` — neither is touched for local work. Full details in
[`docs/infrastructure.md`](docs/infrastructure.md).
