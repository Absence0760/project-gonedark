# Team mode plan — cooperative multi-commander PvP (design pass)

> **Status: DESIGN PASS — not built, deliberately reopening a scoped assumption.** Until now PvP
> was implicitly **1v1** (the whole "one player does both jobs" pillar; [Q29](../open-questions.md)
> names Glicko-2 "1v1"). This doc opens a team mode — target **5v5**, built up from smaller sizes —
> at the player's explicit direction. It is the one PvP surface that touches the *core premise*, so
> it is a full design pass with a stance, not a feature ticket. Direction recorded in
> [D131](../decisions.md); the per-queue army-selection refinement it rides on is
> [D130](../decisions.md). Nothing here is built; the build ramp is §9.

---

## 1. The wall we are not allowed to hit

[`positioning/positioning.md`](../positioning/positioning.md) is blunt about it, and it is the
single most important sentence in the whole corpus for this doc:

> *Going Dark deletes the entire failure mode by construction: **one player does both jobs, never
> simultaneously** (invariants #3 and #5). There is no commander seat to go unfilled, no grunts to
> ignore orders.*

Every game in the FPS/RTS-hybrid graveyard — Natural Selection 2, Savage, Nuclear Dawn, **Eximius
(the cautionary tale, verbatim: "a core imbalance… due to the lack of players filling the Commander
role"), Silica — died the same death: **they split the two jobs across different kinds of player.**
The RTS player can't trust the FPS players; the commander seat never fills in matchmaking; the
soldiers do the glamorous work and the unglamorous work (holding ground, logistics) goes undone.

**A naive 5v5 — one commander + four embodied soldiers — *is* Eximius.** Speccing that would throw
away the game's entire competitive moat. This doc exists to spec a team mode that does **not** do
that.

---

## 2. The stance — multi-commander, never split-role

**Every player in a team is a full "does both jobs" player.** Nobody is a dedicated commander;
nobody is a dedicated grunt. Each player:

- commands their **own sub-force** from the top-down view (their camp, their production, their
  orders — literal executors of *their* last command, invariant #3), **and**
- **embodies their own units** and fights them in first person, going dark on their own strategic
  view while they do (invariants #5, #6).

The team is `N` such players sharing one battlefield and one win condition. The pillar is not
reopened — it is **multiplied**. The positioning sentence still holds verbatim, because *there is
still no commander seat to go unfilled*: **everyone is their own commander.** The divided-attention
tension that is the whole game survives per-player, and gains a second axis on top — *team*
coordination under *individual* blindness.

```
        SPLIT-ROLE (Eximius — the wall)          MULTI-COMMANDER (our stance)
        ┌───────────────────────────┐            ┌───────────────────────────┐
        │  1 commander (RTS only)    │            │  P1  cmd+embody  sector A  │
        │  4 soldiers  (FPS only)    │            │  P2  cmd+embody  sector B  │
        │                            │            │  P3  cmd+embody  sector C  │
        │  seat #1 goes unfilled;    │            │  P4  cmd+embody  sector D  │
        │  soldiers ignore orders    │            │  P5  cmd+embody  sector E  │
        └───────────────────────────┘            └───────────────────────────┘
        one job per player                        BOTH jobs per player, ×N
        the genre's cause of death                the pillar, scaled
```

This is also the only model consistent with invariant #3: a unit obeys *one* commander's last
order. In split-role, whose order does a soldier's squad follow — the human in the seat, or the
human piloting it? Multi-commander has no such ambiguity: **your units are yours**; a teammate can
*see* them (§5) but never *command* them.

---

## 3. Anatomy of a team match

- **One battlefield, `N` sectors.** The map is sized and seeded so each player owns a **deploy
  sector** (their camp + spawn), the way a 1v1 skirmish gives each side one base. Sectors are
  starting anchors, not fences — forces range across the whole map.
- **`N` independent sub-forces per side.** Each player runs their own camp economy and production
  (`core::economy`), issues their own orders, embodies their own units. No shared unit pool, no
  shared purse — coordination is *social/positional*, not a shared spreadsheet (that would recreate
  the "who harvests?" incentive clash).
- **Shared win condition.** The objective is the team's, not the player's — hold the control
  points, eliminate the enemy team, seize the ground (the [`modes.md`](../modes.md) §4b map + the
  campaign objective vocabulary in [`pve-campaign.md`](../pve-campaign.md), reused). A player can be
  wiped and the team fights on; a match ends on the *team* condition.
- **The 200-unit power budget is now a *team* budget.** Phase 3's per-device unit-count ceiling
  (CLAUDE.md "honest caveat") was scoped for one commander's army on mid-range silicon. Ten
  sub-forces cannot each field 200 units on a phone. The budget is split — a per-player cap sized so
  the *whole battlefield* stays inside the device envelope (a real perf fork, §8/§11). This is the
  hardest engineering constraint in the mode and it caps team size as much as netcode does.

---

## 4. What the draft/ban actually drafts

The request was a Mobile-Legends-style **pick/ban** before the match. But Going Dark has **no
heroes** — a MOBA hero is a bundle of a kit + a role, and this game's "kit" is spread across the
*army* (US/FR — [`factions.md`](../factions.md)), the *unit roster* (Rifleman/Heavy/Tank/Medic), and
the *gunsmith loadout* (horizontal sidegrades, [`customization.md`](../customization.md)). A literal
hero-select doesn't map. What *does* map, and gives the draft the texture the player wants:

| Draft layer | Picked | Banned | Notes |
|---|---|---|---|
| **Army** | Per-player, from the roster | — | Rides [D130](../decisions.md): **random in ranked** (see §7), player-picked in quick/custom. Meaningful only once the roster exceeds two ([`factions.md`](../factions.md) deferred armies). |
| **Doctrine** *(new, gated)* | One per player — an *emphasis*, not a power: e.g. **armor / infantry / recon / logistics / support** | Team bans M doctrines from the pool | Forces team-comp diversity ("no all-armor team") the way a MOBA ban forces hero diversity — the on-theme replacement for hero pick/ban. Must stay **fairness-bounded** ([D71](../decisions.md)): a doctrine reshapes your sub-force's *rhythm/identity*, never its power ceiling. |
| **Map** | Existing per-queue policy | Ranked veto | Unchanged — [`modes.md`](../modes.md) §4b already bans/vetoes maps; the CT-G symmetry lint gate applies to team maps too (and gets harder: an `N`-sector map must be fair across *all* sectors, not just mirror-x). |

**Doctrine is a new system and it is the gating dependency for a meaningful draft.** With today's
two armies and no doctrine layer, a "draft" is one coin-flip of army and nothing to ban — see §11.
Doctrine is deferred behind its own design pass; it must clear the same no-strict-domination proof
the gunsmith pools already carry (`no_pool_build_strictly_dominates_another`).

---

## 5. Going dark in a team — invariant #6 under `N` eyes

The sharpest design question. In 1v1, going dark costs *you* the strategic map. In a team, does a
teammate who can see the whole map become your eyes — and if so, does going dark still cost
anything?

**Stance: the team shares *vision*, but never shares *command agency*.**

- **Shared team fog (genre-standard).** The team sees the union of its members' vision — teammates,
  their sight radii, and detection tells. This is expected and good: it is how RTS/MOBA teams
  function, and it lets a teammate *warn* you ("armor massing on your flank").
- **But going dark still costs the diver the thing that matters: control.** While embodied, your
  sub-force is a literal executor of your *last* order (invariant #3) and you cannot re-command it —
  you are blind to the strategic layer *as an actor*, not merely as a viewer. A teammate can *see*
  your sector collapsing and *ping* it, but **cannot drive your units** (they are yours, §2). So the
  cost of diving is unchanged in substance: *you* gave up the ability to adapt your own force
  mid-fight. The team can shout; only you can act, and you're not there.
- **Alerts stay alerts, not intel, for the diver** (invariant #6): your embodied HUD still gets the
  directional flash + audio, never a map reveal — teammate pings arrive on that same *alert*
  channel (a bearing + a tag), not as a restored strategic view. The blindness stays visceral.

This keeps every loss reading as *"I dove at the wrong moment"* / *"we didn't cover each other,"*
never *"the game robbed me."* It is a genuine fork with a second option (independent per-player fog,
no shared team vision) captured in §11 — but shared-vision-without-shared-command is the lean,
because it makes the team a team without softening the personal cost of going dark.

---

## 6. Determinism & netcode — the same core, a wider matrix

Nothing here bends invariants #1/#2/#4/#7. The sim stays fixed-point and deterministic; a team match
is still a seed + a per-tick command log ([D89](../decisions.md)). What changes is **scale and
topology**:

- **`N`-peer lockstep, not 2.** `core::lockstep` is sans-I/O and already exchanges per-tick command
  sets + cross-client checksums ([D27](../decisions.md)); the model generalizes to `2N` peers, but
  the transport topology (relay vs mesh), the per-tick input-delay budget under the slowest of
  `2N` links, and desync *isolation* (which peer diverged, in a 10-way match) are real Phase-3+
  work. The [D27](../decisions.md) checksum-agreement protocol becomes the tool that names the
  guilty peer.
- **Result verification scales for free.** The re-simulate-any-disputed-match verifier
  ([`modes.md`](../modes.md) §4c) is topology-independent — a 10-command-stream log re-sims exactly
  like a 2-stream one.
- **The CI checksum matrix (invariant #7) does not change shape**, only the scenarios it runs: add
  an `N`-peer team scenario to `net-sim-runner` so a team-mode desync fails CI, not a player's
  ranked match.

---

## 7. Ranked army selection — [D130], generalized

This mode is where the player's first two asks land, and they slot cleanly into the existing
**per-queue policy** structure ([`modes.md`](../modes.md) §4b already makes *map* selection a
per-queue policy; [D130](../decisions.md) makes *army* selection one too):

- **Quick / Custom** — you pick your army (mirror matchups legal, [D71](../decisions.md) keeps them
  fair). Unchanged.
- **Ranked** — **army is assigned randomly**, not picked. Rationale: at the competitive tier the
  test is *can you command and fight*, not *did you pre-pick the flavour you drill best*; random
  assignment also denies pre-game army-targeting the meta.
- **Anti-mirror guard on random assignment.** A random ranked assignment must not hand both
  duelists (1v1) the same army. Generalized to teams, the guard becomes a **distribution rule** over
  `2N` seats — the exact rule (no dupes at all? balanced per side? mirror the two sides for
  fairness?) is a §11 fork, but the 1v1 case is settled: **never US-vs-US by random draw.**
- **Honest caveat — degenerate today.** With a two-army roster, "random + anti-mirror" in 1v1
  collapses to *always* US-vs-FR (the only non-mirror pairing). The guard is correct and
  forward-looking; it only becomes genuinely *random* when a third army lands
  ([`factions.md`](../factions.md) deferred). We build the guard now, as a **pure, seed-driven,
  unit-tested seam** (the assignment must be identical on every peer — it rides the match-setup
  handshake seed, the same path skirmish config uses), so the day a third army ships it Just Works.

---

## 8. Team size — a ramp, not a cold 5v5

5v5 is the target, but it is the *hardest* size on every axis at once (10-peer net, 10-way unit
budget on a phone, 10-way desync isolation, a map fair across 10 sectors). Cold-starting there is
how you ship a mode that only works on a LAN of gaming PCs. **Ramp it:**

| Step | Size | Proves |
|---|---|---|
| T-1 | **2v2** | The whole model end-to-end: shared win condition, shared team fog, `N`-peer lockstep (4 peers), team unit budget, sector maps. Smallest thing that is genuinely "team." |
| T-2 | **3v3** | Net topology + input-delay budget past the 4-peer trivial case; map fairness across 3 sectors; the doctrine draft becomes interesting (bans matter at 3). |
| T-3 | **5v5** | The target. Only attempt once T-2's perf + net envelope is measured on mid-range silicon, not a flagship. |

Each step ships playable and is its own sign-off, exactly like the phase plans. 5v5 is a
destination, not a v1.

---

## 9. Build order (dependency-sequenced)

Sequenced so each step is playable and nothing waits on ambition:

1. **[D130] per-queue army policy** — ranked randomizes (anti-mirror), quick/custom pick. This is a
   1v1 change, ships *now* with the PvP staging screen; needs no team mode. **(The near-term, real
   deliverable.)**
2. **Team match engine (T-1, 2v2)** — sector maps, team win condition, shared team fog +
   command-agency rule (§5), `N`-peer lockstep (`core::lockstep` generalization + `net-sim-runner`
   scenario). Depends on the Phase 3 transport + the team unit budget (§3).
3. **Doctrine system** — its own fairness-bounded design pass (§4), the prerequisite for a
   meaningful pick/ban. Independent of net.
4. **Draft/ban UI + flow** — army (per D130) + doctrine pick/ban, team-level bans. Needs (3) and a
   roster/doctrine pool worth banning from.
5. **Scale to 3v3 → 5v5 (T-2, T-3)** — gated on measured perf/net envelopes.
6. **Ranked team queue** — rating for teams (Glicko-2 is a *1v1* model; team rating is a
   [Q29](../open-questions.md) extension), seasons, verification. Ships last, by nature.

---

## 10. What this does NOT do

- **Does not reopen invariants #3 or #5.** Every player still does both jobs, never simultaneously;
  units are still literal executors of *their* commander's last order; there is still no respawn and
  no separate player-character.
- **Does not add a dedicated commander or dedicated soldiers.** The moment a design draft tries to,
  it has become Eximius — reject it at review.
- **Does not share unit control across players.** Shared vision, never shared command (§2, §5).
- **Does not give doctrine or army any power advantage** (invariant / pillar 4, [D71](../decisions.md)).
- **Does not ship before the PvE product.** [D58](../decisions.md) (PvE-first) is unchanged; this is
  PvP fast-follow, and the team variant is the *last* PvP surface.

---

## 11. Open questions (the real sub-forks)

Recorded rather than silently decided — the load-bearing ones are promoted to
[`open-questions.md`](../open-questions.md):

- **[Q31](../open-questions.md#q31--going-dark-team-fairness) — Going-dark team fairness** — shared
  team vision (the §5 lean) vs. independent per-player fog. Does a teammate's map view soften the
  personal cost of diving too much?
- **[Q32](../open-questions.md#q32--team-random-army-distribution) — Random-army distribution rule
  for teams** — the 1v1 anti-mirror is settled; the `2N`-seat rule (no dupes / balanced per side /
  sides mirrored) is open.
- **Doctrine vocabulary & fairness** — what the doctrines *are*, and the no-strict-domination proof
  they must pass. (Its own design pass; parked here until team mode is greenlit past T-1.)
- **Team rating model** — Glicko-2 is 1v1; team rating (per-player vs per-team MMR, carry
  detection) extends [Q29](../open-questions.md).
- **Net topology at `2N`** — relay vs mesh, the input-delay budget under the slowest link, desync
  isolation in a 10-way match (§6).
- **Team unit budget on mid-range silicon** — the per-player cap that keeps the whole battlefield
  inside the device envelope (§3); the constraint that most tightly caps team size.

---

## 12. Build tickets & gates

The §9 build order says *what*, in what sequence, and *why*. This section is the *how* —
shovel-ready tickets a builder can pick up the moment each gate clears. Each is scoped to the
real files, seams, and checksum surface the scout pass confirmed.

**The dependency spine.** T-a is the keystone: a per-unit owner id is the one sim change every
other team-mode workstream leans on. It is a **pure, net-independent** change and can land *now*
as a checksum-versioned foundation, ahead of transport and doctrine. T-b/T-c build headlessly on
top of it (validated in-process like `net-sim-runner`/objective host tests). T-d is the hard net
gate. T-e/T-f are design-gated. Read the flags column before starting anything.

**Shipped prerequisite — do not re-ticket.** [D130](../decisions.md) (per-queue army policy:
ranked randomizes with a seed-driven anti-mirror guard; quick/custom player-pick + legal mirrors)
is the §9-step-1 deliverable and is **already built** — the `PvpQueue`/`PVP_QUEUES` policy in
[`app/src/shell/pvp.rs`](../../app/src/shell/pvp.rs) and the seed-driven assignment seam are in.
Team mode *consumes* it (T-f draft, §7); it is not a team-mode ticket. [Q32](../open-questions.md#q32--team-random-army-distribution)
generalizes its anti-mirror guard to `2N` seats — that lives in T-f, not here.

| Ticket | Workstream | Net | Gate | /safe-edit |
|---|---|---|---|---|
| **T-a** | Multi-commander ownership model | **INDEPENDENT** | nothing | **yes** (sim core + fold) |
| **T-b** | Team scenario seeders + `N`-sector maps | **INDEPENDENT** | nothing (skeleton); T-a for per-owner | yes (touches `core`) |
| **T-c** | Shared team fog + team win condition | **INDEPENDENT** (host-testable) | roster>2 to *ship*; buildable now | no (pure derivation + host-side) |
| **T-d** | `N`-peer lockstep + desync isolation | **GATED** | Phase-3 transport | **yes** (netcode + PAL) |
| **T-e** | Doctrine layer | **INDEPENDENT** | its own design pass | tbd (per design) |
| **T-f** | Draft/ban UI + flow | **INDEPENDENT** (UI) | T-e + roster>2 | no (shell/UI) |

Build order: **T-a → (T-b ∥ T-c) → T-d → T-e → T-f**. T-a first, always.

---

### T-a — Multi-commander ownership model  ·  NET-INDEPENDENT  ·  /safe-edit

The keystone. Today ownership is *implicit*: owner == `Faction`, and the sim **trusts** the host
to only ever build commands for the local player's own units — [`core::sim::apply`](../../core/src/sim.rs)
gates on `is_alive(entity)` and nothing else. Multi-commander needs a real per-unit owner and a
one-line apply-time gate. This is a pure sim change with **no** blocker; land it as a
checksum-versioned foundation before anything else.

| | |
|---|---|
| **Files** | [`core/src/components.rs`](../../core/src/components.rs) (add `CommanderId(u8)` newtype, parallel to `Faction`/`Army`, zero-default); [`core/src/ecs.rs`](../../core/src/ecs.rs) (add `pub commander: Vec<CommanderId>` column beside `faction`, defaulted in `spawn`/`from_parts`/`WorldComponents`); [`core/src/sim.rs`](../../core/src/sim.rs) (thread issuer into the command stream; add gate in `apply`; fold the tag; bump `SNAPSHOT_VERSION`); [`core/src/lockstep.rs`](../../core/src/lockstep.rs) (map `PeerId`→`CommanderId` at the merge boundary; clone the `put_faction`/`get_faction` codec if the id must ride individual commands); [`core/src/economy.rs`](../../core/src/economy.rs) (stamp producing owner onto built buildings + produced units); [`core/src/snapshot.rs`](../../core/src/snapshot.rs) (add `commander` to `UnitView` + persist walk so the UI can grey out un-commandable teammate units). **No change to [`core::orders`](../../core/src/orders.rs)** — enforcement is upstream at apply, not in the hot executor loop (do not add a redundant gate there). |
| **Change** | (a) Widen the command entry from `Command` to `(CommanderId, Command)` at [`Sim::step`](../../core/src/sim.rs) — the point that currently flattens away issuer identity. (b) In [`Sim::apply`](../../core/src/sim.rs), the single authoritative command sink, add one gate — `world.commander[i] == issuer` (and same-faction) — before any mutation; reject silently otherwise (**SEE-but-not-command**, invariant #3). One check covers every unit-targeting variant. (c) At the [`lockstep`](../../core/src/lockstep.rs) merge, the issuing `PeerId` is already on the wire (`Frame::Command(PeerId, …)`) but dropped when frames flatten — preserve it as the `CommanderId` issuer. |
| **Determinism / checksum** | **Sim state — checksum-FOLDED.** The owner id gates which commands mutate which units, so two peers that disagree on a unit's owner apply different commands and diverge; folding it makes invariant #7 catch that immediately. Add **one tag byte per slot** in [`Sim::fold`](../../core/src/sim.rs) right after `faction_tag`, mirror in `serialize`/`deserialize`, bump `SNAPSHOT_VERSION`. Default `CommanderId 0` on every existing entity makes the appended byte a constant zero, but appending it still shifts every scene's per-tick checksum value — **golden checksums must be re-baselined** (same treatment as the D55 shell/dispersion/armour appends). The gate itself is a pure function of folded state + the deterministic per-peer `PeerId`, so it is identical on every peer. `u8` id — no float; `core` stays platform-free (invariants #1/#2 intact). **Watch:** `commander.rs` is the enemy-AI planner despite the name, not player ownership; `[_; FACTION_COUNT]` arrays are unaffected — `CommanderId` nests *within* a faction. |
| **Tests** | `core` unit tests, **both profiles** (dev + release), **float-free**: the apply gate accepts owner-matched + rejects owner-mismatched commands (SEE-but-not-command); default-zero back-compat (single-owner scenes unchanged); fold/serialize/deserialize round-trip with the new byte; a re-baselined golden checksum. |
| **Gate** | **Nothing.** Pure sim change, buildable and testable headlessly today. |
| **/safe-edit** | **Yes** — touches sim core, `fold`, and the lockstep merge. Run `/safe-edit` + determinism-auditor; expect the golden-checksum re-baseline. |

---

### T-b — Team scenario seeders + `N`-sector maps  ·  NET-INDEPENDENT  ·  /safe-edit (touches `core`)

A shared-faction 2v2 seeder skeleton has **nothing** blocking it — it composes the existing
byte-exact spawn primitives and inherits the shared per-side win condition for free (T-c).
A *faithful* per-player economy is gated on T-a's owner dimension (`Resources` is a
checksum-pinned `[i64; FACTION_COUNT]` — no per-player split today).

| | |
|---|---|
| **Files** | [`core/src/scenario.rs`](../../core/src/scenario.rs) (add sibling seeder `seed_team_skirmish`, mirroring `seed_positioned_skirmish`); [`engine/src/map_format.rs`](../../engine/src/map_format.rs) (one named `SpawnZoneSpec` per sector; seeder reads `spawn_zone(name)`/zone centers; **the CT-G symmetry lint gets harder** — fair across `N` sectors, not just mirror-x); [`core/src/economy.rs`](../../core/src/economy.rs) (per-player unit-cap clamp — **flagged as its own sub-ticket**; no consumer exists in core yet). |
| **Change** | New API in `scenario.rs`, composing only existing primitives (`build_camp`/`spawn_rifleman`/`skirmish_troop_post`/`set_income`/`set_army`/`set_purse`): `struct SectorDeploy { base_pos, faction, loadout }`, `struct TeamSkirmish { sectors: Vec<TeamSector> }`, `fn seed_team_skirmish(sim, deploys: &[SectorDeploy], per_player_unit_cap: u32) -> TeamSkirmish`. Body: `set_income`; `set_army` per distinct faction; **pass 1** — `build_camp` for every deploy in slice order; `set_purse`; **pass 2** — one `spawn_rifleman` per deploy, facing via `skirmish_troop_post` against the nearest enemy sector (or map center). `MapSpec::apply` runs *before* (caller order = `seed_positioned_skirmish`'s contract). `N=2` for T-1; signature is already `N`-general. |
| **Determinism / checksum** | **Split.** Setup levers (sector base positions, per-sector army/loadout, income period, `per_player_unit_cap`) are **host-side SETUP** — they follow the existing serialize-*wrapper*-not-`fold` precedent in [`sim.rs`](../../core/src/sim.rs) (where `income_period`/armies already live outside `fold`) and are **not** folded. **But the seeded entities themselves ARE folded sim state** — so the cross-sector spawn stream must be a pure deterministic function of the map's `SpawnZoneSpec` slice order: spawn **all camps in zone order, then all troops** (mirroring `seed_positioned_skirmish`'s camp-then-troop discipline). If sector iteration order or facing derivation differs across peers, the spawn stream diverges and the cross-arch matrix catches it. **Caveats:** (a) `per_player_unit_cap` is **dead SETUP** until an economy production clamp consumes it — include it in the signature to fix the wire/handshake shape, but enforcement is a separate ticket; **if** it later gates production it becomes a folded *effect* and, like the ranked army draw, must be identical setup on every peer (shared-seed derivation). (b) Shared-faction seeder gives shared-win fidelity but **shared-purse** behavior (contradicts §3) — true per-player economy needs T-a first. (c) **Do not** add a `Faction` variant to widen sides — `Faction`/`FACTION_COUNT`/`Resources` are checksum-pinned; that is a high-blast-radius netcode change, not a seeder edit. |
| **Tests** | `core` tests **both profiles**, float-free: the canonical 2-sector case reduces byte-for-byte to the oracle-pinned `seed_positioned_skirmish`/`skirmish_troop_post` 1v1 bytes (reuse the existing determinism-test discipline); spawn-order stability across sector count; a golden checksum for the 2v2 seed. |
| **Gate** | **Skeleton: nothing.** Faithful per-player economy: **T-a** (owner dimension + per-owner `Resources`). The per-player cap has no enforcement point in core today (the 200 budget is a doc caveat, not a constant). Doctrine/roster do **not** block the seeder. |
| **/safe-edit** | **Yes** — new spawn stream in `core` with a golden-checksum surface; run determinism-auditor on the seed order. |

---

### T-c — Shared team fog + team win condition  ·  NET-INDEPENDENT (host-testable)  ·  no /safe-edit

Both halves are already the **excluded / host-side** side of the sim/render decoupling — they fold
**nothing** and carry no desync risk by construction. The code edits are small pure-seam widenings,
buildable and host-testable headlessly *ahead* of transport. Shipping a real >2-side match is gated
on the roster type, but these edits are not blocked from being built and tested now.

| | |
|---|---|
| **Files** | [`core/src/fog.rs`](../../core/src/fog.rs) (add sibling `team_visibility(world, terrain, members)` beside `command_visibility`); [`engine/src/lib.rs`](../../engine/src/lib.rs) (two fog call sites + the `decide_match_end` feed); [`engine/src/objectives.rs`](../../engine/src/objectives.rs) (aggregate `FactionForces` over a side before `is_eliminated`; generalize `opposing()` off the hardcoded Player↔Enemy map); [`engine/src/session_shell.rs`](../../engine/src/session_shell.rs) (generalize `evaluate_outcome`/`decide_match_end`/`is_eliminated` from the 2-combatant `(player, enemy)` pair to per-side integer aggregates). **No change to [`render/src/fog.rs`](../../render/src/fog.rs)** — `visible_instances` takes an opaque `&Visibility`; a team mask is just a larger union. |
| **Change** | **Fog:** widen the single faction filter (`if world.faction[i] != faction { continue }`) to a side-membership test; `reveal_from` already ORs bools, so the `N`-faction union is the same sweep — no math, no float, no new checksum surface. **Leave `embodied_visibility` exactly as-is** (invariant #6 — a diver never gets the team map; do **not** route team vision into the embodied frame). **Win:** a side is eliminated when **all** its member factions are; the timeout tiebreak sums territory/resources per side. Pure integer reads. |
| **Determinism / checksum** | **Neither subsystem is folded.** Fog (`Visibility`) is a checksum-**EXCLUDED** pure derivation (README + `fog.rs` doc) — computed on demand, never mutates the world. Objectives are host-side off the `SimEvent` stream (`ObjectiveSet` owns no `Sim`). Team fog (union) and a per-team verdict fold **nothing** and add zero desync surface (invariants #1/#7 untouched). **The one determinism obligation is at SETUP:** team membership (who is on which side) is match-setup config like `Army`/[D68](../decisions.md) — carried on the wire + persist wrapper, **not** folded — and must be bit-identical on every peer (a shared-seed derivation for any randomized draft, per [D130](../decisions.md)), never derived from per-host state. [Q31](../open-questions.md#q31--going-dark-team-fairness) (going-dark team fairness) is a **presentation/HUD gating rule**, not a determinism one. |
| **Tests** | `engine` host tests (these seams live above `core`, so not the `core` both-profiles gate, but still float-free where they touch sim reads): `team_visibility` unions correctly across `N` members and equals `command_visibility` for `N=1`; `embodied_visibility` unchanged under team membership; per-side elimination fires only when **all** member factions are gone; per-side timeout tiebreak sums correctly. |
| **Gate** | **Roster>2 to *ship* a real >2-side match** (`Faction` is Player/Enemy/Neutral only; `FACTION_COUNT=3` is baked into `[_; FACTION_COUNT]` arrays; the win evaluator is a hardcoded `(player, enemy)` pair — a >2-combatant identity or a side-grouping-over-factions must land first). The fog + objective edits **themselves** are buildable and host-testable headlessly *now*. Secondary: Phase-3 transport to actually *play*; [Q31](../open-questions.md#q31--going-dark-team-fairness)/[Q32](../open-questions.md#q32--team-random-army-distribution) open. |
| **/safe-edit** | **No** — pure derivation + host-side observer, no fold surface. Standard `/check` before commit. |

---

### T-d — `N`-peer lockstep + desync isolation  ·  NET-GATED  ·  /safe-edit

The `core::lockstep` **protocol is already `N`-peer-safe** — pass `peer_count=2N` and the slot, ready
gate, fixed peer-order merge, wire codec + bound check, desync attribution/dedup, and delay-change
tiebreak all scale unchanged. The gate is entirely **host-side transport**: no real `N`-peer
transport exists.

| | |
|---|---|
| **Files** | [`core/src/lockstep.rs`](../../core/src/lockstep.rs) (**no change beyond the caller passing `peer_count=2N`**); [`net-sim-runner/src/main.rs`](../../net-sim-runner/src/main.rs) (add an `N`-peer scenario — 2N sessions/scene slices/scripts — so a team-mode desync fails the CI checksum matrix, not a ranked match); [`engine/src/net_tuning.rs`](../../engine/src/net_tuning.rs) (`RttDelayEstimator` models ONE link — feed it the **worst-case (max) RTT across all `2N-1` peers**, or run per-peer meters and max, then call the peer-count-agnostic `propose_delay`); [`engine/src/lib.rs`](../../engine/src/lib.rs) (`drive_lockstep` pump is 1:1 — **fan out**: send each outbound frame to every peer, poll every peer); [`pal/src/lib.rs`](../../pal/src/lib.rs) (the `Transport` trait is point-to-point — define `N`-peer semantics: broadcast or per-peer send); [`pal-desktop/src/transport.rs`](../../pal-desktop/src/transport.rs) + [`pal-desktop/src/pingpong.rs`](../../pal-desktop/src/pingpong.rs) (all point-to-point / single-inner — need a real `N`-peer transport + one RTT meter per link); [`server/src/lib.rs`](../../server/src/lib.rs) (no relay/matchmaking/multi-peer socket code exists — the Phase-3 relay lands here per [D27](../decisions.md)). |
| **Change** | Do **not** touch the merge/gate/codec — already correct for `N`. Caller passes `peer_count=2N` to the unchanged `Lockstep`; host fans `drain_outbound` out to all peers and maxes RTT across links; add a `2N` `net-sim-runner` scenario. The `Transport` trait + a concrete `N`-peer transport (relay vs mesh — an open topology decision, §6/[Q31 neighbourhood]) is the real build. |
| **Determinism / checksum** | **Folded state is the per-tick merged command stream + each peer's post-tick `Sim::checksum()`** (broadcast as `Checksum` frames). Going to `2N` adds **essentially zero** determinism risk inside `core` — `BTreeMap` key-order iteration, little-endian fixed-point codec, no floats, fixed peer-order merge, and the shipped-as-data `(effective_tick, new_delay)`+tiebreak delay change keep every peer bit-identical regardless of peer count. The real risk is host-side and is a **STALL, not a desync**: (1) a fan-out transport that fails to deliver every peer's frame to every peer stalls the `all(is_some)` ready gate rather than diverging; (2) the RTT→delay float math (correctly **out** of `core`, invariants #1/#2) must feed the **slowest** of `2N` links or a slow peer's gate stalls — still not a divergence, since the delay change applies from shipped data. Desync **isolation** already survives: the [D27](../decisions.md) checksum-agreement compare in `deliver()` names the specific guilty peer among all `2N` (`Desync { tick, peer, local, remote }`). |
| **Tests** | **`net-sim-runner` `N`-peer scenario is the load-bearing deliverable** — a 2N-session lockstep run whose per-tick checksums agree across the arch matrix (invariant #7); a seeded-loss/jitter variant that **stalls** (not diverges) when a link drops frames; a deliberate one-peer divergence that the desync compare attributes to the correct peer id. Plus `engine` tests for the worst-of-`2N` RTT aggregation. |
| **Gate** | **Phase-3 transport (hard).** No real `N`-peer transport exists — every impl is point-to-point/2-endpoint; the server has no relay/mesh/matchmaking. Secondary: relay-vs-mesh topology + worst-of-`2N` input-delay budget are undesigned (§6, [Q-net-topology] in §11); roster>2 doctrine/seat design open. The `core::lockstep` **protocol is NOT gated.** Rollout starts at **3v3** (T-2), not 10-way. |
| **/safe-edit** | **Yes** — netcode + the PAL `Transport` boundary. `/safe-edit` + determinism-auditor mandatory; a checksum-matrix desync here is a real bug, never narrow the matrix to pass (`/fix-ci` discipline). |

---

### T-e — Doctrine layer  ·  design-GATED  ·  /safe-edit tbd

The gating dependency for a *meaningful* draft (§4). Deferred behind **its own fairness-bounded
design pass** — not shovel-ready until that lands. Ticketed here as a placeholder so T-f knows what
it consumes.

| | |
|---|---|
| **Files** | TBD by the design pass. Likely a new doctrine enum/data table in `core::components` (parallel to `Army`, match-setup config, **not** folded) + a per-sub-force emphasis applied at seed/production time in [`core::economy`](../../core/src/economy.rs) / [`core::scenario`](../../core/src/scenario.rs). |
| **Change** | A doctrine is an **emphasis, not a power** (armor / infantry / recon / logistics / support) that reshapes a sub-force's *rhythm/identity*, never its power ceiling. Must clear the same no-strict-domination proof the gunsmith pools carry (`no_pool_build_strictly_dominates_another`, [D71](../decisions.md)). |
| **Determinism / checksum** | Expected **host-side match-setup config** (like `Army`) — carried on the wire + persist wrapper, seed-consistent across peers, **not** checksum-folded — *if* it only reshapes setup. If a doctrine changes spawned units/stats it becomes a folded *effect* fed pre-tick as identical setup on every peer. To be pinned by the design pass. |
| **Tests** | The `no_pool_build_strictly_dominates_another`-style fairness proof, extended to the doctrine pool (both profiles if it touches `core`). |
| **Gate** | **Its own design pass** (vocabulary + fairness proof — §11 "Doctrine vocabulary & fairness"). Independent of net. |
| **/safe-edit** | TBD — `yes` if it reaches into `core` spawn/production; `no` if it stays match-setup config. |

---

### T-f — Draft/ban UI + flow  ·  NET-INDEPENDENT (UI)  ·  no /safe-edit

The front-end over the picks. Needs a pool worth banning from — so it is gated on **T-e** (doctrine)
and **roster>2** (with two armies and no doctrine, a "draft" is one army coin-flip and nothing to
ban, §4/§11).

| | |
|---|---|
| **Files** | [`app/src/shell/pvp.rs`](../../app/src/shell/pvp.rs) (extend the `PvpQueue` staging flow with the draft/ban phase; **consumes** the shipped [D130](../decisions.md) per-queue army policy); [`app/src/shell/army.rs`](../../app/src/shell/army.rs) (`SELECTABLE_ARMIES` — the roster the army layer draws from; only `[Us, Fr]` today). |
| **Change** | The three-layer draft of §4: **Army** (per-player; random in ranked per [D130](../decisions.md), player-pick in quick/custom), **Doctrine** (per-player pick + `M` team bans — the on-theme hero-pick/ban replacement, from T-e), **Map** (existing per-queue policy + ranked veto, unchanged). Generalize [D130](../decisions.md)'s 1v1 anti-mirror guard to the `2N`-seat distribution rule ([Q32](../open-questions.md#q32--team-random-army-distribution) — no-dupes / balanced-per-side / sides-mirrored, still open). |
| **Determinism / checksum** | **Host-side match-setup only** — no sim state, no fold. The **one** obligation: any randomized draw (ranked army, `2N`-seat distribution) must be a **pure deterministic function of the shared match seed** and applied identically before the first tick (else per-unit `CommanderId`/army tags spawn differently per peer and desync) — it rides the match-setup handshake seed, the same path skirmish config uses. |
| **Tests** | `app`/shell unit tests: the seed-driven `2N`-seat assignment is deterministic and identical for a given seed; the anti-mirror/distribution rule holds; ban phase removes exactly `M` from the pool. |
| **Gate** | **T-e** (a doctrine pool to ban from) **and roster>2** (a third army for the draft to be non-degenerate — [`factions.md`](../factions.md) deferred armies). UI itself is net-independent. |
| **/safe-edit** | **No** — shell/UI + seed-driven setup; standard `/check`. (The seed-driven determinism seam is worth a determinism-auditor pass even so.) |

---

## See also

- [`positioning/positioning.md`](../positioning/positioning.md) — the graveyard table and the
  "escape by construction" this mode must not undo
- [`modes.md`](../modes.md) §4 — the PvP subsystems this extends; §4a army selection ([D130](../decisions.md))
- [`factions.md`](../factions.md) — the army roster (and the deferred third army the draft needs)
- [`game-design.md`](../game-design.md) §8/§9 — the literal-executor pillar and the multiplayer
  skill model
- [D130](../decisions.md), [D131](../decisions.md); invariants #3, #5, #6, #7 (CLAUDE.md)
