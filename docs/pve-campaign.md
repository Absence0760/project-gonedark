# PvE — the Operations campaign *(working design)*

> Status: living design doc. The **first shippable product** is single-player PvE
> ([`decisions.md`](decisions.md) D58 resolves [Q5](open-questions.md) → *PvE-first, PvP
> fast-follow*). This doc is the design of that pillar; the build sequencing lives in
> [`pve-campaign-plan.md`](plans/pve-campaign-plan.md). The *why* behind the locked choices is
> [D58](decisions.md)/[D59](decisions.md); why PvE-first is also the right *competitive*
> move (ship a strong PvE product without winning the PvP-fidelity arms race) is
> [`positioning.md`](positioning/positioning.md) §5.

## 1. Why PvE exists — it teaches *going dark*

The north star is the divided-attention bet: *can I afford to be blind right now?*
([`game-design.md`](game-design.md) §2). PvP is where that mind game ultimately sings,
but a stranger's first match cannot be against another human — they have to **learn the
blindness cost in a place that punishes it honestly, not cruelly**.

That place is the campaign. PvE is not a tutorial bolted on the side; it is the onboarding
surface for invariant #6 — *every loss must read as "I stayed too long," never "the game
robbed me."* A mission is a controlled environment where we can *script the temptation*:
put a kill within reach while a timer runs on a base the player can't see, and let the
honest-AI commander (§4) collect the debt if they overstay. The campaign is the going-dark
mechanic with training wheels and a difficulty dial.

It is also the **lowest-risk first ship**: no netcode in the critical path (single-player
runs the same `core::lockstep` loop as a 1-peer, delay-0 session — [D27](decisions.md)),
so we prove the core loop is *fun* before we prove it holds up *over the wire* (Phase 3).

## 2. The Operations hub — structure

Borrowed from **Company of Heroes**' meta-campaign map and **Delta Force**'s replayable
*Operations*: the campaign is a **node graph of missions**, not a fixed linear reel.

```
        ┌──────────────────────── OPERATIONS HUB ────────────────────────┐
        │                                                                 │
        │   [M1]───[M2]───┬──[M4 ✦]──────[M6]                             │
        │     ✓     ✓     │     ▲          ▲   ✦ = set-piece / boss beat  │
        │                 └──[M3]──[M5]────┘                              │
        │                       ▲                                         │
        │   clearing a node unlocks its successors · replay any cleared  │
        │   node at a higher difficulty for better cosmetic/content drops │
        └─────────────────────────────────────────────────────────────────┘
```

- **Nodes unlock as you clear them.** Progress is *content* gating — a cleared node opens
  the next and may unlock a new unit type, a map, or a gunsmith attachment to *try*
  ([`customization.md`](customization.md)). It never unlocks raw power ([D60](decisions.md) —
  unlocks are sidegrades + content, the no-pay-to-win line from [D13](decisions.md) holds in PvE
  too).
- **Replayable, with difficulty tiers + modifiers.** Any cleared node replays at a higher
  difficulty. **Modifiers** (a Destiny-2 / weekly-rotation idea) change the *scenario
  parameters* — starting force size, enemy reinforcement cadence, fog rules, a time limit —
  **never the balance numbers**. That distinction is load-bearing: combat/economy tuning
  stays the single measured baseline ([D30](decisions.md)); a modifier reshapes the
  *situation*, so fairness and determinism are untouched.
- **Light narrative glue.** A short briefing frames each node (who, where, why); we are
  *not* committing to a hand-authored Halo story arc up front (depth is [Q16](open-questions.md)).
  The structure supports growing into one without rework.

## 3. Mission archetypes — the verbs

Each archetype is a **parameterized scenario** (a starting world) plus an **objective set**
(win/lose rules). Both ride seams that already exist — missions are *data*, not new engine.

| Archetype | The fantasy | Built from |
|---|---|---|
| **Seize** | *"Take 10 troops and capture the enemy base."* Fixed starting force, little/no production — a pure tactics puzzle. The user's example, and **mission 1**. | data-driven spawn ([`core/src/sim.rs`](../core/src/sim.rs) `Sim::new`) + a capture-or-eliminate objective |
| **Hold** | Defend a point against escalating waves for N ticks (a Halo set-piece). | `territory_system` ownership + a survive-to-timeout objective |
| **Assassinate / Extract** | Eliminate a specific enemy VIP, or escort a friendly one off the map alive. | a `SimEvent::Killed` listener keyed to one entity |
| **Push** | Capture a chain of control points down a lane, CoH-style. | sequential `territory_system` captures |

The list is open — new verbs are new objective evaluators, not new subsystems.

### Mission 1 — *Seize* (the worked example)

> *"Commander — ten troops, one enemy garrison. The base is yours by dusk. We can't see
> what's behind the ridge; you'll have to go in and look."*

- **Setup:** 10 player Riflemen at a staging point, production disabled; an enemy camp
  (`Faction::Enemy`) with a small garrison + the honest commander on a low difficulty tier.
- **Objective:** *capture or destroy the enemy camp* (a `Captured`-flip on its control
  point **or** an elimination of its buildings — reuses `evaluate_outcome`'s elimination
  rule, [`engine/src/session_shell.rs`](../engine/src/session_shell.rs)).
- **Fail:** lose all ten troops (the natural low point — [`game-design.md`](game-design.md)
  §7) before the camp falls.
- **The teach:** the ridge hides the garrison's size. The player *must* embody a scout to
  see it — and the first time they linger in that scout for one kill too many, a flanking
  enemy squad chews into their ten-stack while the map is dark. Lesson delivered, fairly.

## 4. Difficulty & the honest commander

The enemy is the existing scripted commander ([`core/src/commander.rs`](../core/src/commander.rs)
`commander_orders`, [D39](decisions.md)) — a **literal planner that issues player-equivalent
orders** through the normal command path; the units it commands stay literal executors
(invariant #3). PvE extends it with a **deterministic difficulty tier**, never with cheating.

- **What difficulty scales:** reserve thresholds, unit-mix bias (more Heavies sooner),
  re-plan cadence (`COMMANDER_PERIOD`), capture aggression, reinforcement size. All are
  knobs on the *honest* planner.
- **What difficulty must NEVER do:** become omniscient. The AI may *not* read "the player
  is embodied, attack now" — that is the cheap punisher [`game-design.md`](game-design.md)
  §9 explicitly forbids, and it would detonate invariant #6. Emergent punishment from
  honest attack *timing* (it happened to push an undefended angle while you were heads-down)
  feels fair; scripted clairvoyance feels like robbery.
- **Determinism:** difficulty is a parameter into the seeded commander RNG
  (`sim_seed ^ faction`), so a given mission + tier + seed plays out bit-identically — the
  cross-arch checksum matrix (invariant #7) covers PvE missions exactly as it covers
  skirmish.

## 5. The objective system — host-side, zero desync surface

The single most important architecture call in this pillar: **objectives are not sim
state.** They are evaluated **host-side, after `Sim::step`**, by reading the per-tick
deterministic `SimEvent` stream ([`core/src/event.rs`](../core/src/event.rs)) plus
already-derived faction reads — exactly the footing the win/lose evaluator
(`evaluate_outcome`) already stands on, and the same footing as fog / alerts / the
gone-dark tell (all checksum-*excluded* derivations, [D23](decisions.md)/[D33](decisions.md)).

```
   Sim::step(commands)  ──► SimEvent stream (Killed/Captured/Damaged/Produced…)
            │                          │
            │ (authoritative,          ▼
            │  checksum-folded)   ObjectiveSet::observe(events, faction_reads)
            │                          │   ── pure, host-side, reads never &World
            ▼                          ▼
     per-tick checksum          Objective{ kind, target, progress, state }
     (UNCHANGED by objectives)        │
                                      ▼
                          ObjectiveCompleted / ObjectiveFailed
                                      │
                                      ▼
                       in-match objective HUD  +  MatchSummary
```

- **Shape:** `Objective { kind, target, progress, state }`; an `ObjectiveSet` per mission.
  `kind` ∈ {Capture, Eliminate(entity|faction), Survive(ticks), Escort, Reach}. Each
  `observe` pass folds the tick's events into `progress` and may flip `state` to
  Completed/Failed, emitting an event the summary + HUD consume.
- **Why host-side, not in `core`:** objectives read state but never *change* sim outcomes,
  so putting them in the checksum fold would add desync surface for zero benefit. Keeping
  them host-side means a mission can be authored, tuned, and reshuffled with **no risk to
  lockstep** — and the existing `determinism.yml` matrix needs no new coverage for the
  objective layer itself (it covers the sim the objectives observe).
- **Reuse first:** the Capture/Eliminate verbs are the rules `evaluate_outcome` already
  encodes (elimination + territory); the objective system *generalizes* that function
  rather than replacing it. `Survive` reuses its `timeout_ticks` path.

## 6. What we borrowed — and the one concrete thing from each

| Source | What players loved | How it lands here |
|---|---|---|
| **Halo** | Handcrafted set-piece encounters; pacing of pressure → relief | The `✦` boss/set-piece nodes; mission 1's scripted "scout the ridge" temptation beat |
| **Company of Heroes** | The operations meta-map; cover/territory as objectives | The hub node-graph; the *Push* and *Hold* archetypes off `territory_system` |
| **Delta Force** | Replayable *Operations*; bring-your-loadout-into-a-mission | Replayable nodes at scaling difficulty; the gunsmith loadout ([`customization.md`](customization.md)) carries into missions |
| **Destiny 2** | Weekly modifiers, escalation, pursuit goals worth chasing | Rotating scenario-parameter modifiers; content/cosmetic drops for higher-difficulty clears |

The throughline: **every borrowed idea is expressed as a scenario parameter or a host-side
objective** — none of them reopen a locked invariant or add sim/desync surface.

## 7. Caveats & deferred forks

- **Co-op?** Lockstep already supports N peers, so co-op PvE (shared command, or one
  commander + others embodying) is *architecturally* free-ish — but it's a real design fork
  (whose fog? whose orders?). Parked as [Q14](open-questions.md); single-commander first.
- **Mission authoring format.** Missions are data; whether that data is Rust scenario
  builders (like [`sim-runner`](../sim-runner/src/main.rs) today) or an external
  hot-reloadable file (RON/Lua, the dev-workflow scripting lane) is [Q15](open-questions.md).
  Lean: data-file, so design iterates without a recompile.
- **Narrative depth.** Light briefings now; a full authored arc is [Q16](open-questions.md),
  expandable without restructuring the hub.
- **Not yet built.** This is the *design*; the first code slice (the objective evaluator +
  mission 1, with tests + the determinism matrix green) is [`pve-campaign-plan.md`](plans/pve-campaign-plan.md)
  WS-A.
