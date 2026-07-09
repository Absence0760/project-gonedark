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

## See also

- [`positioning/positioning.md`](../positioning/positioning.md) — the graveyard table and the
  "escape by construction" this mode must not undo
- [`modes.md`](../modes.md) §4 — the PvP subsystems this extends; §4a army selection ([D130](../decisions.md))
- [`factions.md`](../factions.md) — the army roster (and the deferred third army the draft needs)
- [`game-design.md`](../game-design.md) §8/§9 — the literal-executor pillar and the multiplayer
  skill model
- [D130](../decisions.md), [D131](../decisions.md); invariants #3, #5, #6, #7 (CLAUDE.md)
