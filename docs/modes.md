# Game modes — Campaign, Skirmish, and PvP

> **Status: design (build pending).** This doc defines the **mode taxonomy** and the
> **match-setup surfaces** the roadmap scopes as Phase 4 "Match setup" and "Lobby &
> matchmaking" ([`roadmap.md`](roadmap.md)). The campaign's own design lives in
> [`pve-campaign.md`](pve-campaign.md) (+ the conflict atlas — desktop ships the D104
> navigable globe; remaining forks in [Q28](open-questions.md#q28--conflict-atlas)); this doc covers what sits *beside* it:
> free-pick **Skirmish**, and the PvP meta — army selection, map selection, and ranking.
> PvP timing is locked by [D58](decisions.md): **PvE-first, PvP fast-follow**; nothing here
> reorders that.

---

## 1. One match engine, three front doors

The organizing principle: **modes differ in what wraps the match, never in what the match
is.** Every mode boots the same deterministic `Sim` through the same lockstep session core
(single-player is literally a 1-peer, delay-0 lockstep session — [D27](decisions.md)), on
the same single measured balance baseline ([D30](decisions.md)). What varies is the *front
door*: who you fight, how the scenario is chosen, and what persists afterward.

```
                        ┌───────────────── TITLE ─────────────────┐
                        │    CAMPAIGN      SKIRMISH       PVP     │
                        └───────┬─────────────┬────────────┬──────┘
                                │             │            │
        the mission owns ──►  node        MAP pick      QUEUE pick (quick/ranked/custom)
        the setup             graph       ARMY pick     ARMY pick   (pre-queue)
                                │         DIFFICULTY    MAP pool / veto / lobby pick
                                │         + MODIFIERS   matchmaking + rating
                                │             │            │
                                ▼             ▼            ▼
                        ┌─────────────────────────────────────────┐
                        │   the SAME match: seeded Sim + lockstep │
                        │   session, one balance baseline (D30)   │
                        └─────────────────────────────────────────┘
```

| | **Campaign** (PvE) | **Skirmish** (PvE) | **PvP** |
|---|---|---|---|
| What it is | Authored, gated missions | Free single match vs the AI | Live humans over lockstep |
| Opponent | Honest commander at the *briefing's* tier ([D39](decisions.md)/[D83](decisions.md)) | Honest commander at the *player's chosen* tier | A matched human |
| Map | The mission owns it (authored in its data, [D76](decisions.md)) | **Player-picked** from the map library | **Queue policy** — pool / veto / lobby pick (§4b) |
| Army | Authored per mission (campaign is US-side, [D58](decisions.md)) | Player-picked (both sides) | Each player picks independently (§4a) |
| Loadout | Gunsmith carries in ([D60](decisions.md)) | Gunsmith | Gunsmith |
| Stakes / persistence | Node unlocks, best-tier badges (`core::campaign`) | None — it's the sandbox | Rating, rank tier, leaderboards (§4c) |
| Backend | None (local progress) | None | Matchmaker, relay, accounts, ratings ([`infrastructure.md`](infrastructure.md)) |
| Status | Functionally complete (PvE WS-B) | Match exists ([D64](decisions.md) `seed_skirmish`); **entry screen + map library landed, both shells** ([D102](decisions.md); picker preview/metrics + baked/generated maps still target-model — §3) | Queues blocked on Phase 3 net layer; **staging door landed, both shells** ([D101](decisions.md) — honest, nothing joinable) |

Keeping the three *distinct* is deliberate product design, not just code hygiene: campaign
is where a stranger learns the going-dark cost (invariant #6), skirmish is where a player
rehearses a map or an army with zero stakes, PvP is where the divided-attention mind game
actually sings ([`game-design.md`](game-design.md) §2). Each gets its own surface and its
own persistence; none leaks its rules into the others.

---

## 2. Campaign — pointer only

Fully designed elsewhere: the Operations hub node graph ([`pve-campaign.md`](pve-campaign.md),
[D58](decisions.md)/[D59](decisions.md)), difficulty/replay ([D83](decisions.md)), and the
conflict-atlas presentation (desktop: the D104 navigable globe; [Q28](open-questions.md#q28--conflict-atlas)). The one
rule this doc adds: **the campaign never grows a map picker or an opponent picker** — a
mission's map, factions, and commander tier are authored content. If a player wants to
choose, that's what the other two doors are for.

---

## 3. Skirmish — pick your battle

Skirmish is the **free-configuration PvE match**: the sandbox where a new map, a new army
tilt, or a difficulty tier gets rehearsed without campaign gating. The setup flow, in the
order a player thinks:

1. **Map** — any map in the library that passes the content lint
   ([`maps.md`](maps.md) § Diagnostics). The library is everything the content model
   already produces: baked real-world maps ([D80](decisions.md)), authored `*.map.ron`
   battlefields ([D76](decisions.md)), and eventually generated ones (CT-G). Maps are
   content-addressed ([D77](decisions.md)), so the picker lists manifest entries — no
   registry recompile per map. The picker shows the lint preview PNG + the balance
   metrics the baker already emits (cover density, asymmetry) so a player can see what
   they're getting into.

   > **Shipped v1 vs the target model ([D102](decisions.md)).** The picker's library is
   > live on both shells (`engine::map_library::BATTLEFIELDS` — the standing battles +
   > the authored maps, one embedded `include_str!` table, the D80-bridge delivery), and
   > a picked map boots a real skirmish (`seed_positioned_skirmish` in its spawn zones).
   > Still target-model, not shipped: the preview PNG + balance metrics in the picker
   > UI; baked (D80) and generated (CT-G) maps joining the library (blocked on the D77
   > content-hash loader — their grids aren't reachable through `Terrain::from_map_id`);
   > and more than the one authored map (Crossroads).
2. **Army** — US or FR via the existing army-select seam ([D71](decisions.md); the
   native screens already landed on both platforms). Pick the enemy's army too.
3. **Opponent** — the honest commander's difficulty tier (the 3-tier
   `core::mission_tuning` knob) **plus scenario modifiers** (force size, reinforcement
   cadence, fog rules — the WS-E machinery). Same [D83](decisions.md) philosophy as
   campaign replay: difficulty reshapes the *situation*, never the balance numbers.
4. **Loadout** — the gunsmith ([D60](decisions.md)/[D85](decisions.md)), as everywhere.

Win/lose is the standard `evaluate_outcome` (elimination + territory). Objective *presets*
(play a Hold or a Push on any map) are a natural later extension — the `ObjectiveSet`
machinery is mode-agnostic — but the first skirmish ships with the plain match.

**What exists / what's owed.** The match itself is live ([D64](decisions.md): the two-base
`seed_skirmish` boots by default and is winnable end-to-end); army select and the gunsmith
are landed. The **desktop skirmish-entry / match-setup screen has now landed**
(`app/src/shell/skirmish.rs`, behind the title's SKIRMISH door): the battlefield pick, both
armies — the enemy commander's roster too, through the same `SelectArmy` seam the player pick
rides — and the opponent tier, the D83 campaign `Difficulty` whose `combat_tuning` carries
both step-3 axes (commander band + situation modifiers) via `Game::apply_campaign_tuning`;
DEPLOY fields the persisted loadout, and the post-match REMATCH re-boots the same configured
fight. Proof of the "chrome over landed seams" claim: it needed **zero** engine work. The
**Android Compose twin landed the same day**
([`compose-shell-parity.md`](plans/compose-shell-parity.md) §12 item 6: `SkirmishSetup.kt` +
`SkirmishSetupScreen.kt`, with the enemy army + configured-skirmish flag riding new
`earmy`/`skirm` wire keys — a skirmish win records no campaign clear). The **map library
landed next** ([D102](decisions.md)): the [D34](decisions.md) presentation-safe manifest
listing is `engine::map_library::BATTLEFIELDS` (standing battles + embedded authored
`*.map.ron`, one table both shells render), and a picked map boots a real skirmish through
`MapSpec::apply` + `core::scenario::seed_positioned_skirmish` in its spawn zones. What's
still target-model: the picker's preview PNG / balance metrics, and baked/generated maps
joining the library (blocked on the D77 content-hash loader — see the step-1 note above).

Skirmish is also the **content proving ground**: a map or faction enters the PvP pool only
after it has been playable in skirmish (same spirit as faithful-then-balance-pass,
[`maps.md`](maps.md)).

---

## 4. PvP — three separate subsystems

PvP is not one "multiplayer screen." It is three systems with different owners, different
backends, and different failure modes — designed separately, composed at the queue:

> **The door exists before the queues do.** The title's PvP button opens a dedicated
> **staging screen** ([D101](decisions.md)): the three queues below in §5 build order,
> the §4a pre-queue identity line, and *nothing joinable* until the Phase 3 transport
> exists — the honesty rule is a tested seam (`queue_joinable`), not copy. The old shared
> PvE/PvP mode picker is retired; no door leaks another's content (§1).

### 4a. Army selection

The *mechanism* is done: [D71](decisions.md) locked fairness-bounded soft asymmetry
(logistics rhythm, not gun stats), verified swap-invariant, with native army-select on both
platforms feeding `Game::select_army`. The PvP layer on top is thin and should stay thin:

- **Pick before queueing**, alongside loadout — the queue matches *players*, not army
  matchups, and mirror matches (US vs US) are legal precisely because [D71](decisions.md)
  keeps armies inside the fairness band. No draft/ban phase while the roster is two
  armies; revisit only if a third army ([`factions.md`](factions.md) § deferred) lands.
- The picks travel in the match-setup handshake and seed the deterministic sim start —
  the same path skirmish uses. Nothing army-specific enters matchmaking.

### 4b. Map selection — a policy per queue

One map library, three selection policies:

| Queue | Policy | Why |
|---|---|---|
| **Custom lobby** | Host picks any lint-passing map; both ready-up | It's a scrim; freedom is the point |
| **Quick match** | Curated **rotation pool**, random pick | Zero-decision entry; rotation keeps it fresh |
| **Ranked** | Seasonal pool + **veto** (each side bans N, random from the rest) | Competitive integrity + player agency without stale map memorization |

The load-bearing gate: **a map enters any matchmade PvP pool only if it passes the CT-G
PvP-symmetry lint** (`lint.py --pvp mirror-x|mirror-y|point` — already built, an ERROR-level
check). Faithful real-world maps that fail symmetry ([`maps.md`](maps.md): Pointe du Hoc's
cliff edge) stay PvE/custom-only until a balance-passed symmetric variant exists. This makes
map fairness **structural** — a pool entry is a lint artifact, not a judgement call.

### 4c. Matchmaking & ranking

The competitive spine, and the only part of PvP that is genuinely *new* system design:

- **Two queues.** **Quick** (unranked — wide skill tolerance, fast matches, where the
  Q17 aim-assist lean can absorb input mismatch) and **Ranked** (rating-gated, stricter).
  This composes with [Q17](open-questions.md#q17--crossplay-input-fairness)'s input-based
  pools — and that composition is the population risk: queues × input pools fragments the
  player base. Lean: strict input pools in **ranked only**; quick match runs mixed-input.
- **Rating.** Hidden matchmaking rating + **visible rank tiers** mapped over it, placement
  matches to seed, seasonal soft-reset. The rating *model* (Elo vs Glicko-2 vs
  TrueSkill-style) is a real fork —
  [Q29](open-questions.md#q29--pvp-rating--ranked-season-design), lean **Glicko-2** (1v1,
  rating deviation handles sparse mobile play patterns). Leaderboards ride the Postgres
  schema already scoped in [`infrastructure.md`](infrastructure.md).
- **Seasons.** Align season turnover with content drops — if the conflict atlas
  ([Q28](open-questions.md#q28--conflict-atlas)) lands, a season and a conflict drop are
  naturally the same beat (new maps → new ranked pool → soft reset).
- **Rewards are cosmetic-only.** Rank rewards obey [D13](decisions.md) exactly as the
  store does — titles, skins, calling-card cosmetics; never power, never gunsmith
  exclusives with sim effect.
- **Integrity — determinism is the anti-cheat asset.** Lockstep has no authoritative
  server world, so *result reporting* is the trust problem (win-trading, result spoofing,
  rage-disconnects). But invariants #1/#7 hand us a verifier no float engine gets: a match
  is fully described by its seed + per-tick command log (the [D89](decisions.md) replay
  artifact), so the backend can **re-simulate any disputed match bit-exactly** and settle
  it. Concretely: both peers submit result + final checksum; the relay (already scoped in
  [`infrastructure.md`](infrastructure.md)) arbitrates the command log and the disconnect
  timeline; a checksum disagreement flags the match. Ranked verification only needs
  *same-build* replay, so [Q26](open-questions.md#q26--replay-compatibility)'s disposable
  lean suffices. Leaver policy gets a real reconnect window for free —
  `core::reconnect`/[D28](decisions.md) already exists — then a forfeit after grace.

---

## 5. Build order

Sequenced by dependency, not ambition — each step ships something playable:

1. **Skirmish entry screen** (both native shells). No net dependency; every seam is
   landed. This is the Phase 4 "Match setup" row and closes the release-readiness
   checklist item. **Landed on both shells** (§3;
   [`compose-shell-parity.md`](plans/compose-shell-parity.md) §12 item 6).
2. **Custom PvP lobby** — direct connect / invite over the Phase 3 transport, host picks
   the map, both ready-up. First two-human match; unblocks the PvP mind-game tuning the
   roadmap has been waiting on. No matchmaker, no rating.
3. **Quick match** — the matchmaker service (Redis queue state per
   [`infrastructure.md`](infrastructure.md)), rotation pool, unranked.
4. **Ranked** — rating + tiers + seasons + veto + result verification. Decide
   [Q29](open-questions.md#q29--pvp-rating--ranked-season-design) before this build
   starts; it needs real queue population, so it ships last by nature as well as by plan.

Steps 2–4 are the [D58](decisions.md) "PvP fast-follow" made concrete; step 1 belongs to
the PvE product and shouldn't wait for any of them.

---

## See also

- [`pve-campaign.md`](pve-campaign.md) — the campaign pillar this doc deliberately does not restate
- [`factions.md`](factions.md) — the army model behind §4a
- [`maps.md`](maps.md) — the map library + the symmetry lint behind §4b
- [`infrastructure.md`](infrastructure.md) — the backend services §4c rides on
- [Q17](open-questions.md#q17--crossplay-input-fairness), [Q26](open-questions.md#q26--replay-compatibility),
  [Q28](open-questions.md#q28--conflict-atlas), [Q29](open-questions.md#q29--pvp-rating--ranked-season-design)
- [D13](decisions.md), [D27](decisions.md), [D30](decisions.md), [D58](decisions.md),
  [D64](decisions.md), [D71](decisions.md), [D77](decisions.md), [D89](decisions.md)
