# Playability plan — from "deep but minimal" to a playable match

> **Status: LANDED (2026-06-25).** A focused, parallel-worker push to make the game *play*
> and *read* like a game. The engine is structurally deep (deterministic fixed-point sim,
> ECS, economy, territory, fog, combat, orders, alerts, sans-I/O lockstep) but in mid-2026 it
> neither looked nor played like a game: you couldn't shoot while embodied, no match ever
> ended, the enemy was inert, and everything in-match was flat untextured quads with **no
> text**. All six workstreams below shipped — built in isolated git worktrees and merged in a
> fixed order (W4 → W1 → W2 → W3 → W5 → W6), each with tests green dev+release, the
> determinism + 2-peer lockstep runners agreeing, and the viz-runner real-pixel assertions
> passing. The four design forks are logged as
> **[D37](../decisions.md) (embodied combat) / [D38](../decisions.md) (win-lose) /
> [D39](../decisions.md) (enemy AI) / [D40](../decisions.md) (embodied world)**. It spans the
> unfinished gameplay tail of Phase 2 and the in-match-readability slice of Phase 4. **Honest
> caveat:** verified by the automated suite (unit tests, determinism matrix, offscreen pixel
> assertions); the by-hand "play a full match on desktop" feel pass is still owed, and the
> art is procedural placeholder (no asset pass).

---

## Why this exists

Two confirmed problems, from a code audit:

**It plays minimal — three gameplay verbs missing/broken:**

- **No embodied shooting.** `core/combat.rs:164-166` skips embodied units and there is no
  `Command::Fire` in `core/sim.rs`. An embodied player can move, look, and *die*, but deals
  no damage — the FPS half of the hybrid is non-functional.
- **No match ever ends.** `engine/lib.rs:522-524` notes there is no win-condition evaluator;
  outcome is always `Draw`.
- **The enemy is inert.** Enemy units get one `AttackMove` at spawn (`engine/lib.rs:454-465`)
  then stop forever. No AI, no reinforcements.

**It looks bad — flat quads, no text:**

- All renderables are flat colored quads; no meshes, terrain, ground, skybox, or lighting.
- **No in-match text rendering at all** — radial-menu wedges, buttons, and the post-match
  summary have no labels/numbers (position + color only).
- The embodied (FPS) view is a **literal black void** + the avatar quad — no world.

**Scope decisions (locked with the user when this plan was approved):**

- Scope = **full match loop + readable UI** (both halves above).
- Embodied view = **render a real FPS world (ground/sky/weapon) while keeping enemy & map
  intel dark.** "World goes dark" (invariant #6) means losing *intel*, not a black void.

**Goal:** a person can play a full match — command a squad top-down, possess a unit and
shoot in first person, fight an opponent that fights back, capture points, and win/lose to a
readable summary — with in-match text and a real embodied world.

---

## The six workers

Each worker mostly **owns a new module**, following this repo's established "extract a pure
testable seam" pattern (`engine`'s `command_ui`/`selection`/`audio`/`tuning`; `render`'s
`fog`/`hud`/`marquee`/`overlay`/`radial`). Edits to the shared hub files (`core/sim.rs`,
`engine/lib.rs`, `render/lib.rs`) are kept **small, additive, and region-disjoint** so
parallel branches collide minimally.

### Gameplay (touch `core`/sim — determinism-critical)

**W1 — Embodied combat.** *(HIGH blast radius — `/safe-edit` + determinism-auditor.)*
Add `Command::Fire { entity, dir: Vec2 }` to `core/sim.rs` (enum tail + one `apply` arm —
both appends). New `core/combat.rs::resolve_fire(...)`: a fixed-point **cone hitscan** —
nearest hostile (ties → lowest index) with `dir·(target−pos) ≥ cos_half·|target−pos|` (Fixed dot, **no
normalize/sqrt**), within `range²`, passing `terrain.line_of_sight`; reuse
`cover_at().damage_multiplier()` + the existing damage/suppression/cooldown writes. Keep the
embodied-skip at `combat.rs:164` — embodied units fire *only* via the command. The firing
direction crosses into `core` float-free **exactly like tap targets** (`world_to_fixed`,
`engine/lib.rs:202-204`): the host quantizes `cos/sin(yaw)` to `Fixed` bits at the boundary
(`yaw` is host-only, `engine/lib.rs:345`). New pure seam
`engine/src/fire.rs::fire_command(...)`, unit-tested without a GPU; one push in `frame()`.
Writes only already-checksummed fields → **no `fold()` change**, matrix stays comparable.
**Logs a decision:** embodied firing model.

**W2 — Win/lose.** *(Medium — host-side, no sim mutation.)* New pure
`engine/session_shell.rs::evaluate_outcome(...) -> MatchOutcome`: elimination (a faction at 0
alive units **and** 0 buildings loses; sole survivor wins), territory/score tiebreak at a
timeout, else `Draw`. Host reads per-faction alive counts from `sim.world` in stable index
order; swap the `Draw` placeholder at `engine/lib.rs:546`. Zero `sim.rs` edits, no `fold`
change → a pure function of already-checksummed state, can't desync. Wires into the existing
`MatchSummary`/`session_shell` end-state machine. **Logs a decision:** match-end condition.

**W3 — Enemy commander AI.** *(HIGH blast radius — `/safe-edit` + determinism-auditor.)* New
`core/src/commander.rs::commander_orders(...) -> Vec<Command>` emitting **only existing order
commands** (AttackMove / SetOrder / Build / QueueProduction) — units stay literal executors
(**invariant #3**; this is a *commander*, not unit smarts). Determinism: the commander gets
its **own** seeded `core::rng::Rng` owned by `Game` (seed = sim seed ⊕ faction) — it must
**not** draw from `sim.rng()` (that stream is checksummed; a host draw would desync). On a
`tick % PERIOD == 0` gate its commands are pushed into the same `commands` Vec **before**
`drive_lockstep`, so they enter the lockstep stream like player commands → bit-identical on
all peers. Replaces the one-shot spawn order at `engine/lib.rs:454-465`. **Logs a decision:**
enemy = commander-level scripted AI via the lockstep stream.

### Presentation (touch `render`/engine glue — never mutate sim)

**W4 — In-match text rendering.** *(Foundational; render-only.)* New `render/src/text.rs`
screen-space LOAD pass (lightest viable approach — a baked bitmap-glyph atlas or a thin crate
like `wgpu_text`/`glyphon`; avoid heavy deps), same shape as `hud.rs`/`overlay.rs`. Pure
layout math extracted and unit-tested. Feeds labels to radial-menu wedges
(`radial.rs:20-22` notes labels are unplumbed), pause/summary buttons, and the
resource/kill/territory **numbers** on the summary. The single highest-leverage fix for "the
UI is really bad"; other workers consume this API after it merges.

**W5 — Embodied FPS world.** *(Fairness-sensitive — invariant #6; render + engine camera.)*
New `render/src/world.rs` (+ shader): ground plane, skybox/gradient, and a **weapon
viewmodel** in the embodied pass, while enemy units, buildings, and control points stay
filtered out (intel dark; keep `fog.rs` / `render/lib.rs:21-26` filtering). The black void
becomes a real first-person space that is still fair. Adds a muzzle-flash/impact cue tied to
W1's `Fire` event. No sim writes. **Logs a decision:** embodied-world rendering & its fairness
boundary.

**W6 — Command-view polish.** *(Render-only; consumes W4 text.)* New `render/src/terrain.rs`
(or extend ground): a readable top-down ground/terrain grid instead of flat slate; clearer
control-point rings, selection rim, health bars; resource/unit-count readouts via W4's text
API. Pure layout/color math unit-tested; offscreen pixel assertions via `viz-runner`.

---

## Orchestration & merge order

All six run in **isolated git worktrees** branched from `main`. Hub-file edits are additive
and region-disjoint, so they merge in a fixed order, each later branch rebased onto the merged
result:

- **Wave 1 (foundations):** **W4** (text API), **W1** (`Command::Fire` spine), **W2**
  (win/lose) — largely disjoint (render-only / core+engine / engine-host).
- **Wave 2 (build on Wave 1):** **W3** (rebased onto W1's `sim.rs` appends), **W5** (uses
  W1's `Fire` event), **W6** (uses W4's text API).

Merge order: **W4 → W1 → W2 → W3 → W5 → W6**. Expected conflicts are limited to one
`mod`/`pub mod` line per new module in each crate hub plus the disjoint `engine/lib.rs`
regions (W1 in `frame()`, W3 a `commander` field + a push before `drive_lockstep`, W2 the
`:546` placeholder swap) — trivial to resolve.

**Decision log.** `docs/decisions.md` is append-only; the latest at plan time is **D36**. The
four new decisions (W1, W2, W3, W5) become **D37–D40**, numbered in merge order via the
`/decision` discipline.

---

## Guardrails (from `CLAUDE.md`)

- **Invariants:** #1 no floats in sim (W1/W3 fixed-point only, LUTs via `core::trig`, no
  sqrt/normalize); #2 `core` takes no GPU/platform dep; #3 units stay literal executors (W3 is
  a *commander*); #4 sim/render decoupled (W4/W5/W6 never mutate sim); #6 no intel leak while
  embodied (W5).
- **Tests ship in the same commit and pass**, dev **and** release; `core` logic float-free and
  in the checksum where it is sim state. Logic behind un-constructible platform glue → extract
  a pure seam and test that.
- **`/safe-edit`** for W1 & W3 (sim/determinism); **`/check`** before every non-trivial commit.
- **Commits path-scoped** (`git add <path>` then `git commit -m … -- <paths>`); never
  `git add -A`/`.`/`-u` or `git commit -a`. No attribution footers/trailers.

---

## Verification

Per worker and again after each merge:

- **Tests/lint:** `pnpm test` (workspace, dev) + `cargo test --workspace --release` (W1/W3
  release determinism); `pnpm lint`.
- **Determinism (W1, W3):** `pnpm desktop:sim` (per-tick checksum stream) and
  `pnpm desktop:sim:net` (2-peer lockstep agreement); the cross-arch matrix is enforced by
  `determinism.yml` in CI. A desync is a real bug — never narrow the matrix.
- **Render (W4, W5, W6):** `pnpm desktop:viz` offscreen pixel-readback assertions; `pnpm play`
  to eyeball.
- **End-to-end (after Wave 2):** `pnpm play` and play a full match — band-select a squad →
  issue move/attack/stance orders → **embody a unit and shoot** an enemy to death → the
  **commander AI counter-attacks / reinforces** → capture a control point → drive a faction to
  elimination → **match ends with a readable summary** (kills/territory/resources as real
  numbers). Confirm the embodied view shows a real ground/sky/weapon world with **no enemy/map
  intel** visible.
