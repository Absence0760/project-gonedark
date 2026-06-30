# Factions plan — US Army vs French Army

> **Status: IN PROGRESS — full faction system landed; only the D32-blocked native army-select screen
> remains.** Direction locked in [D68](../decisions.md); design in [`factions.md`](../factions.md); the
> WS-B stat-budget fork resolved by [D71](../decisions.md) (soft asymmetry on logistics rhythm, not gun
> stats). **WS-0 prerequisite met** ([`combat-rebalance-plan.md`](combat-rebalance-plan.md) is COMPLETE).
> **WS-A/B/C/E built; WS-D's seam + scenario seeding built (only its native army-select screen is
> D32-blocked):** **WS-A** (`Army` tag + persist/lockstep codecs — `Sim::army_of`,
> codec round-trips), **WS-B** (per-faction rosters via `economy::unit_stats_for`, logistics-rhythm
> tilt — [D71](../decisions.md)), **WS-C** (per-faction cosmetic identity — US/FR
> silhouettes/viewmodels/names via `render::model_for_unit`), **WS-D** (army selection through the
> `core::shell` seam + `core::shell::resolve_select_army`; PvE US-vs-FR OPFOR scenario seeding via
> `core/src/scenario.rs` `set_army`/`spawn_rifleman` per-army loadout; army-tilted pre-placed starting
> troops via `core::scenario`), **WS-E** (per-faction gunsmith pools — `gunsmith::pool_for`). **The
> one remaining item** is WS-D's **native army-select screen** (D32-blocked — the in-engine seam is
> built and ready, no native UI project exists yet).

---

## Why this exists

The player's north star: *"the goal is to have a USA army vs the French army."* Today the sim has only
`UnitKind` (one shared roster) and `Faction` (an allegiance tag — `Player`/`Enemy`/`Neutral`). A
*faction identity* (US/FR) is a **third** concept layered over those ([`factions.md`](../factions.md)
§3): per-faction rosters, silhouettes, and feel, **asymmetric but fairness-bounded** (pillar 4; never
power, only flavour). [D68](../decisions.md) locked that direction and left the build here.

It also gives the modern-combat-realism work a destination: [D66](../decisions.md) lethality and
[D67](../decisions.md) logistics make *a* modern army feel right; this makes *which* army you command
mean something.

---

## The load-bearing call

**A faction is content + a table, not a fork of game logic** (invariant #2 — one shared deterministic
core). Concretely:

- The `Faction` allegiance enum is **unchanged** — it stays the thing `combat::is_enemy` resolves.
  Faction identity is a **new tag** (`Army { Us, Fr }` — name TBD) chosen at match setup, selecting
  which roster + cosmetics + gunsmith pool a side draws from.
- Per-faction unit stats come from the **same fixed-point table on every peer** — a `unit_stats`
  keyed by `(Army, archetype)` instead of bare `UnitKind` (invariant #1).
- The identity tag must be **encoded identically across the three codecs** that already carry
  `UnitKind`/`BuildingKind` — the checksum/persist fold (`core/src/sim.rs`) **and** the lockstep wire
  codec (`core/src/lockstep.rs`) — exactly as [D65](../decisions.md) added Tank/Medic/Barracks. A
  `Build`/match-setup command must decode to the same army on every peer (invariant #7).
- **Fairness is measured, not asserted:** per-faction tilts live inside a band dialed against
  `sim-runner --metrics` (the [D30](../decisions.md) harness), and cross-play parity ([Q17](../open-questions.md))
  means the band must hold across mouse/thumb/controller at once.

---

## Workstreams

### WS-0 — Balance the shared archetypes first *(prerequisite)*

Land [`combat-rebalance-plan.md`](combat-rebalance-plan.md) (restore the Rifleman/Heavy RPS + make
suppression bite at lethal speed, [Q18](../open-questions.md)) **before** adding per-faction variance.
Rationale ([Q19](../open-questions.md) lean): balance the skeleton once, against the harness, then tilt
per faction — re-tuning twice (before and after factions) is wasted measurement.

**Exit:** the `--metrics` RPS + pin-before-kill properties hold on the shared roster.

### WS-A — Faction identity model + codecs *(the spine)* — **DONE**

- **`core` — an `Army` tag** (`Us`/`Fr`, with a `Neutral`/none for non-aligned scenes), distinct from
  `Faction`. A per-side selection carried as match-setup state (each `Faction` in a match maps to one
  `Army`), reachable through the `core::shell` seam ([D34](../decisions.md)).
- **Codec parity:** add `Army` to the persist/checksum fold (`sim.rs`) **and** the lockstep wire codec
  (`lockstep.rs`), same field-order discipline as [D67](../decisions.md)'s `reserve` fields and
  [D65](../decisions.md)'s kind tags. Plumb it through `scenario` seeding.

**Tests (same commit):** `Army` codec round-trip (fold↔deserialize, wire encode↔decode); a 2-peer
lockstep test where both sides pick armies and the checksum streams agree; a no-army (legacy) scene
stays byte-unchanged. Green **dev + release**; `determinism.yml` matrix green. This WS touches the sim
codecs → [`/safe-edit`](../../.claude/commands).

### WS-B — Per-faction rosters — **DONE**

- **`economy::unit_stats` keyed by `(Army, archetype)`** — US and FR archetype sets (rifleman / heavy /
  vehicle / support, the shared skeleton from [`factions.md`](../factions.md) §2) with **tilts inside a
  fairness band**, fixed-point. Every army fields every archetype (no missing role). *(The roster gained
  a fifth archetype — the dedicated **anti-tank** infantry unit, [D73](../decisions.md) — routed through
  the same `unit_stats_for`; it carries no logistics tilt, held shared across armies inside the band.)*
- **Measure parity:** extend the `--metrics` harness with a cross-faction equal-cost trade; assert no
  army wins the mirror-of-roles trade outside the band (the per-faction analogue of [D30](../decisions.md)'s
  unit-parity check).

**Tests (same commit):** per-faction stat tables are float-free (determinism guard greps them) +
checksum-folded; a metrics test that US-vs-FR equal-cost stays within the fairness band; 2-peer
lockstep agreement with mismatched armies. Sim-touching → full determinism + lockstep runners.

### WS-C — Cosmetic identity (presentation-only) — **DONE**

- Per-faction meshes/silhouettes/names via the asset pipeline ([D41](../decisions.md)/[D46](../decisions.md),
  script-not-binary) — M1 Abrams vs Leclerc, FAMAS vs M4 viewmodels, faction names/voicelines. `render`
  maps `(Army, kind)` → mesh, exactly as it maps `UnitKind` → mesh today.
- **Never sim** — silhouettes/names/voicelines are pure presentation; they never reach `core` and add
  no checksum surface.

**Tests:** the `(Army, kind)` → render-asset mapping (host-testable); an asset-manifest entry per new
model (`source`/`license`/`sha256`).

### WS-D — Faction selection + PvE integration — **PARTIAL (seam + scenario seeding built; native screen BLOCKED on [D32](../decisions.md))**

- **Army-select UI** in the native shell ([D32](../decisions.md)) through the `core::shell` seam; the
  choice flows into WS-A's match setup.
- **PvE framing** ([D58](../decisions.md)): the Operations campaign is played **US-side**, with the
  **French Army as one OPFOR** — so factions debut in PvE (no cross-play fairness pressure yet,
  [Q17](../open-questions.md)) and graduate to PvP later.

**Tests:** the selection → match-setup mapping; a campaign mission seeded US-vs-FR drives to a result.

### WS-E — Per-faction gunsmith pools *(layers on [D60](../decisions.md))* — **DONE**

- Each army gunsmiths a **different weapon pool**; the gunsmith stays **sidegrade-only** ([D60](../decisions.md)),
  so this is identity without power creep. Composes with the gunsmith loadout WS in
  [`pve-campaign-plan.md`](pve-campaign-plan.md) (WS-C there).

**Tests:** no per-faction attachment build strictly dominates (the [D60](../decisions.md) no-dominant-build
property, per pool); checksum parity for identical loadouts across peers.

---

## Sequencing

```
   WS-0 (shared-archetype balance, combat-rebalance-plan)
     │
     ▼
   WS-A (Army tag + codecs)  ─►  WS-B (per-faction rosters)  ─►  (ship factions in PvE)
     │                              │
     ├─► WS-C (cosmetic identity, parallel once A's tag exists)
     ├─► WS-D (selection + PvE OPFOR, after A)
     └─► WS-E (per-faction gunsmith pools, after B + D60 gunsmith)
```

WS-0 gates everything (balance the skeleton first). WS-A is the spine; B/C/D/E layer on. Resolving the
[Q19](../open-questions.md) asymmetry fork (reskin vs soft-tilt vs hard-asymmetry — **lean: soft tilt**)
is the design gate on WS-B's stat budget.

## Determinism & fairness guardrails (apply to every WS)

- **One shared core** — faction = content + a table, never a logic fork (invariant #2).
- **`Army` tag encoded identically across fold + wire codecs** ([D65](../decisions.md)/[D67](../decisions.md)
  discipline; invariant #7).
- **Per-faction stats fixed-point + checksum-folded** (WS-B, invariant #1); cosmetics presentation-only
  (WS-C).
- **Fairness measured against `--metrics`**, held across cross-play inputs ([Q17](../open-questions.md));
  asymmetry of feel, never power (pillar 4).
- **Gunsmith stays sidegrade-only** per faction (WS-E, [D60](../decisions.md)).

## Verification

- Per WS: tests **in the same commit**, green `cargo test` **dev + release**; determinism + 2-peer
  lockstep runners agree; [`/check`](../../.claude/commands) before commit, [`/safe-edit`](../../.claude/commands)
  for the sim-touching WS-A/WS-B.
- End-to-end: a person picks the US Army, plays a campaign mission against a French OPFOR, and the two
  armies *read* as distinct (silhouettes, weapons, feel) while the match stays fair — the
  human-confirmation layer (same honesty bar as D31 / playability-plan).
