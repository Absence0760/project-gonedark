# PvE campaign plan — the first shippable product

> **Status: IN PROGRESS — WS-A, WS-C, WS-D shipped and live-wired; WS-E shipped; WS-B
> host-side only.** WS-A (mission/objective core — `engine/src/objectives.rs`,
> `core::scenario::seed_seize_mission`, `render::objective_hud`) and WS-D (HUD layout editor —
> `engine/src/hud_layout.rs`) are built, tested, and wired into the live host. WS-E (difficulty /
> modifiers / briefing — `core/src/mission_tuning.rs`) is built and threaded into
> `core::commander`. **WS-C is now live-wired:** the gunsmith sim model + fairness/checksum proofs
> (`core/src/gunsmith.rs`) and pre-match UI seam (`engine/src/loadout_ui.rs`) feed
> `core::scenario::seed_seize_mission_with_loadout` / `engine` `new_scene_with_loadout`, so the
> chosen loadout is applied to every player troop's weapon **at match start** and folded into the
> per-tick checksum (`STANDARD` stays a byte-identical no-op; same-loadout peers agree every tick,
> different loadouts diverge only as expected), and the **desktop gunsmith UI is now wired**
> (egui Title→Gunsmith→Deploy flow in `app`, calling `new_scene_with_loadout`). The mobile-native
> gunsmith screen remains [D32](../decisions.md)-blocked like the other native shells. WS-B's
> Operations-hub host model + persistence (`core/src/campaign.rs`, via `core::shell`) is built and
> tested, but its **mission-select/briefing native shell is BLOCKED on [D32](../decisions.md)** and
> the `MissionId→mission` registry is unbuilt. Design: [`pve-campaign.md`](../pve-campaign.md) +
> [`customization.md`](../customization.md); decisions [D58](../decisions.md)–[D61](../decisions.md).

---

## Why this exists

The engine is systems-complete and end-to-end playable ([D31](../decisions.md), D37–D40): you can
command a squad, possess a unit and shoot, fight a scripted commander, capture points, and win or
lose to a readable summary. What it has **no** of is **content a player progresses through** — a
reason to play a second time, a curve that teaches the blindness cost, a place to earn and try
loadouts. That's the gap this plan closes.

The north star ([`game-design.md`](../game-design.md) §2) is the divided-attention bet. PvP is where
it ultimately sings, but a stranger's first match cannot be against a human — they have to learn
*"can I afford to be blind right now?"* somewhere that punishes overstaying **honestly, not
cruelly** (invariant #6). The Operations campaign is that somewhere.

**Scope decisions (locked with the user up front, [D58](../decisions.md)–[D61](../decisions.md)):**

- First target = **PvE-first**, PvP a fast-follow on the same lockstep core.
- Campaign shape = a **CoH/Delta-Force Operations hub** (replayable mission node-graph), not a
  linear Halo reel or a pure roguelite.
- Progression = **horizontal** — gunsmith sidegrades + content unlocks, **never** raw power
  ([D13](../decisions.md) holds; pillar 4 holds).

---

## The load-bearing architecture call

**Objectives are host-side, not sim state.** This is the call that keeps the whole content pillar
cheap and safe:

- An `ObjectiveSet` is evaluated **after `Sim::step`**, reading the per-tick deterministic
  `SimEvent` stream ([`core/src/event.rs`](../../core/src/event.rs)) + already-derived faction reads —
  the **exact footing** the win/lose evaluator already stands on
  (`evaluate_outcome`/`FactionForces`, [`engine/src/session_shell.rs`](../../engine/src/session_shell.rs),
  [D38](../decisions.md)) and the same footing as fog/alerts/tell ([D23](../decisions.md)/[D33](../decisions.md)).
- Because objectives **observe** the sim and never **change** it, folding them into the checksum
  would add desync surface for zero benefit. Keeping them host-side means missions are authored,
  tuned, and reshuffled with **no lockstep risk** (invariant #7) and **no new cross-arch coverage**
  for the objective layer itself.

The **one** customization that *does* touch the sim — gunsmith stat deltas (WS-C) — gets the full
fixed-point/checksum treatment precisely *because* it changes sim outcomes. Cosmetics and the HUD
editor never reach the sim at all.

---

## Workstreams

Each owns a new module where possible, following the repo's "extract a pure testable seam" pattern
(`engine`'s `command_ui`/`selection`/`tuning`; `render`'s `fog`/`hud`). Edits to shared hub files
(`core/sim.rs`, `engine/lib.rs`) stay small, additive, region-disjoint.

### WS-A — Mission/objective core *(the spine)* — **DONE**

The first playable mission proves the loop. Everything else wraps this.

- **`core` (or a host-side `engine` module) — `Objective`/`ObjectiveSet`:** `Objective { kind,
  target, progress, state }`; `kind ∈ {Capture, Eliminate(entity|faction), Survive(ticks), Escort,
  Reach}`. An `observe(events, faction_reads)` pass folds a tick's events into `progress` and may
  flip `state` → Completed/Failed, emitting `ObjectiveCompleted/Failed` for the summary + HUD.
  Generalizes `evaluate_outcome`'s elimination/territory/timeout rules; reuses them, doesn't replace.
- **The *Seize* archetype + mission 1** ("10 troops, take the base"): a parameterized scenario via
  the data-driven `Sim::new` + spawn path ([`core/src/sim.rs`](../../core/src/sim.rs)) — 10 player
  Riflemen, production disabled, an enemy camp + garrison + the honest commander on a low tier;
  objective = capture-or-eliminate the enemy camp; fail = lose all ten.
- **In-match objective HUD:** a thin presentation surface showing the current objective + progress
  (reuses the in-match text pass from the playability push).

**Tests (must ship in the same commit):** unit tests for each `Objective.kind` evaluator against
synthetic `SimEvent` streams (capture flips, VIP-killed, survive-to-timeout, lose-all-units); an
`engine`-level integration test driving mission 1 to both a win and a loss. Green **dev + release**;
the `determinism.yml` cross-arch matrix stays green (the objective layer is host-side, so it adds no
checksum surface — confirm the sim it observes is unchanged). This is the WS that most needs
[`/check`](../../.claude/commands) + the test-gap-checker before commit.

### WS-B — Operations hub — **PARTIAL (host model built; native shell BLOCKED on [D32](../decisions.md))**

- Node-graph meta-progression: a `Campaign`/`OperationNode` model (host/shell-side), unlock state
  (clearing a node opens successors), replay-at-higher-difficulty.
- Mission-select + briefing surface in the **native shell** ([D32](../decisions.md)) reached through
  the `core::shell` seam ([D34](../decisions.md)); progress persisted **outside** the checksum fold
  (campaign metadata alongside `Sim::serialize`, or a separate host file — [Q15](../open-questions.md)
  informs which).

**Tests:** unlock-graph transitions; persistence round-trip of campaign progress.

### WS-C — Gunsmith loadout — **DONE on desktop (sim model + UI seam + match-start application + egui gunsmith screen); mobile-native screen D32-blocked**

- **Fixed-point attachment-delta model in `core`** (Q16.16, [D17](../decisions.md)): an integer
  attachment table applied to the weapon component **at match start** as match-setup input; folded
  into the per-tick checksum via `Sim::fold` ([D28](../decisions.md)). **Sidegrades only** — enforce
  *no strictly-dominant build* the way [D30](../decisions.md)'s balance harness enforces unit parity.
- **Pre-match loadout UI** on the command layer; the cosmetic catalogue (skins/paint) stays
  presentation-only and feeds the [D13](../decisions.md)/[Q9](../open-questions.md) store (not this WS).

**Tests:** the delta table is float-free (the determinism guard greps it); a property/test that no
attachment combination strictly dominates another on the tracked stat axes; a checksum test that two
peers with the same loadout stay bit-identical and different loadouts diverge *only* as expected
sim state. This WS touches the sim → it rides the full determinism + 2-peer lockstep runners.

### WS-D — HUD layout editor — **DONE**

- Per-layer (command vs embodied) drag/resize/opacity editor over the **existing** touch seams
  (`engine` `touch_controls` → intents + geometry; `render::touch_controls` screen-space pass,
  [D51](../decisions.md)); multiple saved presets + reset-to-default; stored in local/profile config.
- **Presentation/input-only, never sim** ([D61](../decisions.md)). **Invariant-#6 guard:** the editor
  configures placement, never information — no element that surfaces strategic intel while embodied.
  Accessibility cues are a *separate* settings surface.

**Tests:** the pure seam that maps a saved layout → control geometry + raw-touch→intent mapping
(host-testable, no winit/Android types); a guard test that no editable element exposes
strategic-intel data while embodied.

### WS-E — Difficulty + modifiers + narrative glue — **DONE**

- A deterministic `difficulty` tier threaded into `commander_orders`
  ([`core/src/commander.rs`](../../core/src/commander.rs), [D39](../decisions.md)) — scales
  reserve/unit-mix/cadence/aggression on the **seeded** planner. **Never omniscient** (no "you're
  embodied, attack now" — invariant #6 / §9).
- Scenario-parameter **modifiers** (force size, reinforcement cadence, fog rules, time limit) —
  **never balance-number hacks**, so the [D30](../decisions.md) baseline and determinism hold.
- Light briefing framing per node ([Q16](../open-questions.md) keeps depth deferred).

**Tests:** a given mission + tier + seed replays bit-identically (the commander RNG is
`sim_seed ^ faction`); modifiers change only scenario params, asserted against the checksum being a
deterministic function of (scenario, seed, inputs).

---

## Sequencing

```
   WS-A ──► WS-B ──► (ship the campaign)
    │
    ├─► WS-C  (loadout, parallelizable once A's scenario format exists)
    ├─► WS-D  (HUD editor, independent — touches only existing touch seams)
    └─► WS-E  (difficulty/modifiers, layers onto A's commander + scenarios)
```

WS-A is the spine and the **natural next code slice** after this design lands — a single playable
mission ("10 troops, take the base") that proves the objective system and the going-dark teach beat.
WS-B wraps it into a campaign; C/D/E layer on and are largely independent of each other.

## Determinism & fairness guardrails (apply to every WS)

- **Objectives host-side** → zero checksum/desync surface (WS-A/B/E).
- **Loadout deltas fixed-point + checksum-folded**, no floats (WS-C, invariants #1/#7).
- **Cosmetics + HUD editor presentation-only**, never sim (WS-C cosmetics, WS-D).
- **AI honest, never omniscient** (WS-E, invariant #6 / §9).
- **HUD editor: placement not information** (WS-D, invariant #6).
- **Modifiers reshape the situation, never the balance numbers** (WS-E, [D30](../decisions.md)).

## Verification

- Per WS: unit/integration tests **in the same commit**, green `cargo test` **dev + release**
  (CLAUDE.md floor); the determinism + 2-peer lockstep runners agree; run [`/check`](../../.claude/commands)
  before each non-trivial commit, [`/safe-edit`](../../.claude/commands) for the sim-touching WS-C.
- End-to-end: a person plays mission 1 start→finish on desktop — commands ten troops, scouts the
  ridge, takes (or loses) the base, sees the objective HUD + summary. The by-hand feel pass is the
  human-confirmation layer, carried not faked (same honesty bar as D31/playability-plan).
