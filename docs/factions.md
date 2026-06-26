# Factions — real modern armies (US Army vs French Army)

**Status: DESIGN-ONLY ([D68](decisions.md)).** This doc records the *direction* and the model. No
engine code implements per-faction rosters yet — the game still fights with one shared `UnitKind`
roster across `Faction::Player`/`Faction::Enemy`. The unresolved specifics are [Q19](open-questions.md).

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
  and graduate to PvP later.
- **Balance ([Q18](open-questions.md)).** Do the **lethal-speed re-tune of the shared archetypes
  first**, *then* tilt them per faction — balance the skeleton once, against the harness, before adding
  per-faction variance on top. Re-tuning twice (before and after factions) is wasted measurement.

---

## 6. Deliberately deferred

Out of scope until [Q19](open-questions.md) resolves and the build is greenlit: the per-faction
`unit_stats` tables and the identity tag + its codecs; per-faction cosmetics/voicelines/silhouettes;
faction selection UI; how (or whether) faction interacts with progression; any third faction beyond
US/FR. This doc is the place those land when they do.
