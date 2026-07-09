# Factions — real modern armies (US Army vs French Army)

**Status: BUILT (modern + WW2 core).** The per-army roster is real engine code: the `Army` tag
(`Neutral`/`Us`/`Fr`, plus the WW2 `UsWw2`/`Germany` — D120) threads the persist + lockstep codecs,
and `economy::unit_stats_for(army, kind)` draws each side's tilted loadout. **Two fairness axes now
ship.** The modern **US vs French** matchup (D68/D71) is *soft, power-neutral* asymmetry — identity
lives in the logistics rhythm (magazine depth, reload cadence, turret slew) while every combat stat
stays identical, proven swap-invariant by the `cross_faction_*` metrics. The WW2 **US(Sherman) vs
Germany(Panther/Tiger)** matchup (D120) is *cost-vs-power quality-vs-quantity* — a unit may be
individually better but costs more, balanced at **equal budget** (not equal count) by the
`equal_budget_quality_vs_quantity` harness. Both report side-by-side under `sim-runner --metrics`.
Remaining specifics track in [Q19](open-questions.md); build sequencing in
[`plans/factions-plan.md`](plans/factions-plan.md). **Still deferred (D120 content stage):** bespoke
per-faction models/silhouettes, a WW2 gunsmith pool, a WW2 campaign conflict, and WW2 infantry tilts.

> *"the goal is to have a USA army vs the French army."* — the north star this doc serves.

---

## 1. The vision

The two sides are **asymmetric factions modelled on real modern armies**, the first matchup the
**US Army vs the French Army**. A side is not a palette swap of one generic roster — it has its own
infantry, vehicles, and support, its own silhouettes, weapons, and feel, drawn from the real-world
force it depicts. This is the concrete destination for the "modern-army" framing the lethality
([D66](decisions.md)) and all-unit-ammo + logistics ([D67](decisions.md)) passes already set in
motion: a hit kills like a real round, an army runs on finite ammo and resupply, and *which* army you
command actually means something.

Factions are the **identity** layer over the existing systems — they do not change *how* the game
plays (command-and-grow, embody-and-go-dark), they change *who* you play as.

---

## 2. The fairness bound (non-negotiable)

Asymmetry of **flavour and feel — never of power.** This is pillar 4 (*the cost must always feel
fair*) extended from the embodiment cost to the roster: a player must never lose because they picked
the "wrong" army. Cross-play parity ([Q17](open-questions.md)) makes this doubly load-bearing —
US-vs-FR must be balanced across mouse, thumb, and controller at once.

Concretely: every faction fields the same **archetype skeleton** (a rifleman-equivalent, a
heavy/bruiser-equivalent, a vehicle, a support unit), so no side lacks an answer to a role. Asymmetry
lives in **tilts within a measured band** — slightly different stats, a signature ability, a distinct
gunsmith pool — not in one side having a tool the other can't counter. The band is set against the
`--metrics` harness, the same objective signal the combat re-tune is measured against.

---

## 3. How it maps onto the engine (the architecture seam)

Today the deterministic core has two relevant concepts, and a faction *identity* is a **third** layered
over them — not a rename of either:

| Concept | Today | Role under factions |
|---|---|---|
| `Faction` enum | `Player` / `Enemy` / `Neutral` — **allegiance** (who fights whom; `combat::is_enemy`) | **Unchanged.** Stays the allegiance tag the sim resolves combat against. |
| `UnitKind` | one shared roster (Rifleman / Heavy / Tank / Medic) | Becomes a **per-faction roster** — US and FR each get their own archetype set. |
| *(new)* faction identity | — | A US/FR tag chosen at match/loadout time that selects which roster + cosmetics + gunsmith pool a side draws from. |

Determinism (invariants #1/#7) is the build constraint: per-faction stats must come from the same
fixed-point `unit_stats`-style table on every peer, and any new identity tag must be encoded
identically across the three codecs that already carry `UnitKind`/`BuildingKind` — the checksum/persist
fold (`sim.rs`) **and** the lockstep wire codec (`lockstep.rs`) — exactly as [D65](decisions.md) did
for Tank/Medic/Barracks. A faction is **content + a table**, not a fork of game logic (invariant #2:
one shared core).

---

## 4. Roster sketch (illustrative, not locked — see [Q19](open-questions.md))

The shared archetype skeleton, with the kind of real-platform mapping the two armies suggest:

| Archetype | US Army | French Army |
|---|---|---|
| Rifleman | M4-pattern carbine | FAMAS / HK416F |
| Heavy / support weapon | M249 / M240 gunner | Minimi / AANF1 gunner |
| Main battle tank | M1 Abrams | Leclerc |
| Support | Combat medic | Auxiliaire sanitaire |

These are **flavour anchors**, not a stat spec — the exact tilts (and whether asymmetry is a reskin, a
soft per-stat tilt, or a hard StarCraft-style divergence) are the open fork in [Q19](open-questions.md).
**Current lean: soft asymmetry** — shared archetypes with per-faction tilts inside a fairness band.

---

## 5. Interactions

- **Gunsmith ([D60](decisions.md)).** The horizontal sidegrade gunsmith is per-weapon; a faction roster
  gives each side a *different weapon pool* to gunsmith, which is a natural identity lever **and**
  stays fairness-bounded because the gunsmith is already sidegrade-only (no power creep). The two
  systems compose cleanly.
- **PvE campaign ([D58](decisions.md)).** The first shippable product is single-player PvE. The clean
  framing: the campaign is played **US-side**, with the **French Army as one OPFOR** among the PvE
  threats — so factions debut in PvE (no cross-play fairness pressure yet, [Q17](open-questions.md))
  and graduate to PvP later. The PvP army-selection surface built on this roster is
  [`modes.md`](modes.md) §4a — a **per-queue policy** ([D130](decisions.md)): player-pick in
  quick/custom (mirrors legal), **random assignment in ranked** (1v1 anti-mirror guard). A
  draft/ban surface exists only for the specced **team mode** (§4d,
  [`plans/team-mode-plan.md`](plans/team-mode-plan.md)), over army + a future doctrine layer — and a
  meaningful army draft still needs a roster larger than two (the deferred third army above).
- **Balance ([Q18](open-questions.md)).** Do the **lethal-speed re-tune of the shared archetypes
  first**, *then* tilt them per faction — balance the skeleton once, against the harness, before adding
  per-faction variance on top. Re-tuning twice (before and after factions) is wasted measurement.

---

## 6. Built vs still deferred

**Built:** the `Army` identity tag + its persist/lockstep codecs; the per-faction `unit_stats_for`
tables (modern logistics tilt, D71; WW2 cost-vs-power tilt, D120); `unit_cost_for`; the per-army
gunsmith pool (WS-E); the modern army-select UI; and the `--metrics` fairness harnesses for both axes.

**Still deferred** (the D120 content stage / open specifics): bespoke per-faction
models/silhouettes/voicelines (WW2 tanks currently share the greybox mesh); a dedicated WW2 gunsmith
pool (WW2 armies share the baseline pool); a WW2 campaign conflict that fields `UsWw2`/`Germany`; WW2
infantry tilts (only the tank is tilted so far); how (or whether) faction interacts with progression;
and any faction beyond the US/FR/WW2 set. This doc is the place those land when they do.
