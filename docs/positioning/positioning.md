# Positioning — *Going Dark* vs. the field

> Status: living strategy doc. Who we overlap with, where the moat is, where we're
> exposed, and the concrete roadmap work that closes each gap. This is the
> *product-strategy* counterpart to [`game-design.md`](../game-design.md) (the design) and
> [`decisions.md`](../decisions.md) (the why). It picks no fights with the invariants — every
> "reach parity here" item below is bounded by them, especially #1 (determinism), #3
> (literal-executor AI), #5 (embodiment is not a character system), and #6 (the dark
> stays fair).

> **This doc leans mobile / storefront** (Delta Force, CoD Mobile, the gunsmith,
> free-to-play) — that's the most crowded fight. Two companion docs cover the other fronts:
> [`positioning-pc.md`](positioning-pc.md) (the PC fight — Company of Heroes, Battlefield,
> Halo, Destiny, the strategy lineage) and
> [`positioning-cross-platform.md`](positioning-cross-platform.md) (keeping *one game* fair
> across phone/PC/console). The moat thesis (§3) and the scorecard (§6) below are
> platform-agnostic and hold across all three.

---

## 1. The one-paragraph thesis

We are **not a shooter, and not an RTS.** We are a single-player-driven **divided-attention
hybrid**: you command an army top-down, and when you possess one unit to fight it in first
person, *the strategic map goes dark.* No competitor occupies this square. The shooters we
share a storefront with (Delta Force, CoD Mobile) have no strategic layer; the RTS lineage
we share a pedigree with (Company of Heroes) has no embodiment; and every prior FPS/RTS
*hybrid* split the two jobs across **different players** and died on the resulting
role-imbalance. Our wedge is that **one brain does both jobs, sequentially** — which is
simultaneously the design's whole identity *and* the reason it dodges the genre's historical
cause of death. The risk is not "is the idea distinct" (it is); it's "can a small team reach
**good-enough FPS fidelity** next to billion-dollar incumbents while keeping the hybrid the
star." That's what §6–7 are about.

---

## 2. The competitive map

No single competitor overlaps us on more than one axis. The danger is that we're compared
**piecemeal** — our FPS to Delta Force, our gunsmith to CoD Mobile, our command layer to CoH
— and judged on each in isolation, where a focused incumbent always wins. The defense is to
keep the *intersection* legible, because the intersection is empty.

```
                    FPS / embodiment fidelity  →  (twitch, gunfeel, production values)
                  low                                                   high
  high ┌───────────────────────────────────────────────────────────────────────┐
   ▲   │                                              Delta Force · CoD Mobile   │
   │   │                                              Warzone Mobile · PUBG-M    │
 RTS / │                                              (NO strategic layer)       │
command│                                                                         │
 depth │        Company of Heroes · StarCraft                                    │
   │   │        Bad North (mobile RTS)            ┌───────────────────────────┐  │
   │   │        (NO embodiment)                   │   ★ GOING DARK            │  │
   │   │                                          │   the empty square:       │  │
   │   │   Natural Selection 2 · Savage           │   command depth ×         │  │
   │   │   Nuclear Dawn · Eximius · Silica        │   embodiment, ONE player, │  │
   │   │   Battlezone · Foxhole                   │   vision as the cost      │  │
   │   │   (hybrid, but SPLIT across players →     └───────────────────────────┘  │
  low  │    role-imbalance death — see §3)                                        │
       └───────────────────────────────────────────────────────────────────────┘
```

We sort competitors into four cohorts. Each gets a table: **what they nail · why they don't
threaten the square · what we take · what we must match.**

---

## 3. Cohort A — FPS/RTS hybrids *(the direct concept comparables, and the graveyard)*

These are the games people will say we "remind them of." Every one splits command and combat
across **different players** in PvP — and the genre has a perfect failure record because of it.
This cohort is the single most important evidence in this doc: it tells us exactly which wall
to not walk into.

| Game | What it proved can work | How / why it stayed niche |
|---|---|---|
| **Natural Selection 2** | A commander in an RTS overhead view *and* players in FPS, one shared world, genuinely beloved | One player commands for a whole team; if that player is weak or absent the match collapses. Two mindsets, one of them "niche." |
| **Savage / Savage 2** | The original commander-plus-grunts MMO-FPS/RTS; cult adoration | Same role-split; tiny audience; never escaped niche. |
| **Nuclear Dawn** | FPS + a single RTS commander on Source; slick presentation | Died on population — the commander role can't fill in matchmaking. |
| **Eximius: Seize the Frontline** | 4 squad officers + 1 commander, 5v5; "Mostly Positive" | Reviewers: *"a core imbalance… due to the lack of players filling the Commander role,"* clunky gunplay, thin progression. The cautionary tale, verbatim. |
| **Silica** (2023, EA) | Modern, photoreal; commander *or* FPS, **with an AI-commander fallback** | "Mostly Positive" (76%) but *"both types of gameplay feel like watered-down versions of the genres."* The AI fallback exists precisely because the human-commander slot won't reliably fill. |
| **Battlezone** (1998 / 2018) | The granddaddy: drive a tank in first person *and* command a base. Ahead of its time | Control-scheme friction; never broke out of cult status. |
| **Foxhole** | Persistent hybrid war, logistics as gameplay; devoted community | Hardcore, massive-MP, glacial-pace — a different planet from mobile sessions, but proof the fantasy has pull. |

**The pattern — and our escape from it.** The genre's cause of death is *not* "two genres
don't mix." It's that **splitting the two jobs across two kinds of player creates an
unfixable incentive clash**: the RTS player can't trust FPS players to follow orders or to
do unglamorous work (harvesting, holding ground), and the "niche" commander seat never
fills in matchmaking. *Going Dark deletes the entire failure mode by construction:* **one
player does both jobs, never simultaneously** (invariants #3 and #5). There is no commander
seat to go unfilled, no grunts to ignore orders — your army is a literal executor of *your*
last command (invariant #3), and you're the one who dives. The thing that killed the genre
is a multiplayer-role problem we don't have.

> **Take:** the fantasy is real and under-served — people *want* command-and-embody. **Avoid:**
> never split the two roles across players. The PvP pillar (Phase 3) must stay
> *symmetric* — each player is their own commander-and-avatar — not asymmetric commander-vs-grunts.
> Co-op ([Q14](../open-questions.md)) is the one place the role-split temptation returns; if we ever
> build it, each player keeps both jobs.

---

## 4. Cohort B — Mobile-first tactical shooters *(the storefront, the wallet, our hardest flank)*

This is where we actually overlap on *features* — F2P, gunsmith, embodied FPS, cross-platform
— and where the incumbents are strongest and we're thinnest. We do **not** try to out-shoot
them. We try to be *fair-and-good-enough* on the FPS so the hybrid can be the reason to stay.

| Game | What they nail | Why they don't own our square |
|---|---|---|
| **Delta Force** (Team Jade / TiMi) | UE4 (MP) + UE5 (campaign); unified PC/mobile assets; bespoke terrain/anim tooling; extraction + Warfare; a real studio | Pure server-authoritative shooter. **No strategic layer, no command-an-army, no vision-as-cost.** They can't add it without becoming us — and their netcode (no deterministic sim) is the wrong substrate for 200-unit lockstep. |
| **CoD: Mobile / Warzone Mobile** | The gunsmith that *defined* mobile weapon customization; omnimovement; true cross-progression with the PC/console CoD wallet | Twitch BR/MP only. The gunsmith is the bar we're measured against — but theirs is a *vertical-power* grind; ours is **fairness-bounded sidegrades** ([D60](../decisions.md)) so it can fold into a deterministic checksum. |
| **PUBG Mobile** | Massive reach; the template for mobile FPS controls + monetization | Same: no command layer. Reach/serviceability lessons, not a design threat. |

**The honest exposure.** Their gunsmith is table-stakes now — "mobile gamers crave and deserve
complex, meaningful systems." Their gunfeel, animation, and texture-streaming are the product
of studios and years. We will **lose any head-to-head on raw FPS fidelity**, and we must be
candid that we're not trying to win it. Our embodiment is deliberately *constrained and
visceral* (invariant #6) — the FPS exists to make the strategic bet *feel* dangerous, not to be
a standalone CoD-killer. The bar we must clear is **"a Delta Force player doesn't bounce off our
gunplay in the first ten seconds,"** not "we out-gun Delta Force." §6–7 size that bar.

> **Take:** unified cross-platform assets; gunsmith depth as *expectation*; mobile control polish.
> **Avoid:** pay-to-win vertical progression (corrodes invariant #6 and breaks checksum fairness —
> [D60](../decisions.md)); chasing photoreal fidelity we can't staff.

---

## 5. Cohort C / D — the pedigree and the PvE neighbors

**Cohort C — RTS lineage (the command layer's parentage).** *Company of Heroes* is our explicit
model (cover, suppression, territory, squads); *StarCraft* is the macro/micro skill ceiling;
*Bad North* is the proof a legible RTS can live on a phone. None has embodiment, so none
threatens the square — but they set the bar for **command-layer depth and readability** that a
shooter audience won't have patience to learn slowly. Lesson: the command layer must teach itself
fast (PvE mission 1, [`pve-campaign.md`](../pve-campaign.md) §3) and read at a glance on a small screen.

**Cohort D — PvE / objective shooters (the campaign neighbors).** *Helldivers 2* (the recent proof
that co-op PvE with a strategic frame can be a breakout), *Destiny 2* (weekly modifiers, pursuit
goals — borrowed in [`pve-campaign.md`](../pve-campaign.md) §6), *Deep Rock Galactic* (co-op identity),
and *Halo* (handcrafted set-pieces — also borrowed). Our **PvE-first** decision ([D58](../decisions.md))
puts us in this neighborhood at launch; the lesson is that a strong PvE shooter can ship and thrive
*without* winning the PvP-fidelity arms race — which is exactly the lane our FPS-fidelity honesty (§4)
needs.

---

## 6. The scorecard — where we lead, sit at par, and lag

Brutally honest, feature by feature. **Lead** = no incumbent has it. **Par** = we can credibly
match. **Lag** = an incumbent is materially better and we must close or consciously concede.

| Capability | Us | Best incumbent | Verdict | Closing item (→ §7 / roadmap) |
|---|---|---|---|---|
| Command-an-army + embody, one player | ✅ unique | — | **LEAD** | Protect it; don't dilute |
| Vision-as-cost ("going dark") | ✅ unique | — | **LEAD** | Teach it (PvE M1) |
| Deterministic sim built for 200-unit lockstep | ✅ substrate landed; on-device scale pending (Phase 3) | — (DF has no sim) | **LEAD** | Hold determinism gates; finish Phase 3 mobile profile |
| Symmetric hybrid PvP (no dead commander seat) | ⏳ designed | hybrids: all asymmetric | **LEAD (potential)** | Phase 3 netcode → PvP |
| Strategic/command depth | ⏳ systems-complete | Company of Heroes | **PAR-ish** | **CP-9** command-layer readability pass |
| Gunsmith / loadout | ⏳ designed (WS-C) | CoD Mobile | **LAG** | **CP-1** gunsmith to mobile-expected depth |
| Raw FPS gunfeel / gunplay | ⏳ hitscan + tank | Delta Force / CoD | **LAG** | **CP-2** embodied game-feel bar |
| Animation / character fidelity | ⏳ floor slice landed (WS-B/D84) | Delta Force (UE5) | **LAG (conceded tier)** | **CP-3** "not jarring" floor, not parity |
| Mobile control feel / HUD | ⏳ editor designed (WS-D) | CoD Mobile | **PAR (closing)** | **CP-4** HUD editor + touch polish |
| Cross-platform progression | ⏳ persist exists | Warzone (CoD wallet) | **LAG** | **CP-5** unified entitlement layer |
| Audio fidelity (a primary system, #6) | ⏳ procedural placeholders | Delta Force | **LAG** | **CP-6** audio identity pass |
| Onboarding / teach-the-twist | ⏳ PvE M1 designed | (none need it) | **AT-RISK** | **CP-7** the most important screen |
| Live-ops / content cadence | ⏳ server scaffolding | all of them | **LAG (post-launch)** | **CP-8** modifier/content engine |
| Net infra robustness (reconnect, handoff) | ⏳ designed | all of them | **LAG (Phase 3)** | Phase 3 reconnect/handoff |

**Read:** we **lead on the four things that define the product** and **lag on the
production-value table-stakes** every mobile shooter has had years to polish. That's the right
shape — lead on what's defensible, reach parity on what's expected, *consciously concede*
photoreal fidelity (CP-3) we can't staff. The fatal version of this table would be lagging on
the LEAD rows; we're not.

---

## 7. The plan — closing each gap to parity-or-better

The gaps above map to roadmap items, tagged **CP-n** (Competitive Parity), tracked in
[`roadmap.md`](../roadmap.md) → *"Competitive parity — reaching the incumbents' bar."* The
principle for every one: **reach the incumbents' bar on table-stakes, then let the hybrid be
the reason to choose us.** None reopens an invariant.

- **CP-1 — Gunsmith to mobile-expected depth.** Extend the WS-C sidegrade model
  ([D60](../decisions.md)) to the attachment-category breadth a CoD-Mobile player expects (optics,
  barrel, stock, mag, grip, muzzle), *staying horizontal* (sidegrades, fixed-point,
  checksum-folded — never vertical power). Parity on *feel*, deliberate divergence on *fairness*.
- **CP-2 — Embodied game-feel bar.** A focused gunplay pass so a Delta Force player doesn't bounce
  in ten seconds: hit-feedback (impact, hitmarker, damage direction), weapon kick/recoil readability,
  responsive ADS, audio-coupled firing. Presentation/feel layer only — never sim state (#4). Define a
  written "good-enough floor" and playtest against it.
- **CP-3 — Animation/fidelity floor (conceded tier).** *Not* UE5 parity — a "not jarring" floor:
  coherent locomotion/fire/death anims on the greybox so the eye-level view reads as a place, via the
  scripted asset pipeline ([`content-pipeline.md`](../content-pipeline.md), D41/D46). Explicitly a
  conceded tier; we compete on the hybrid, not the polygon count. *WS-B floor slice landed
  ([D84](../decisions.md)): clip-selection seam + procedural pose (troopers animate) + rig authoring;
  runtime skeletal playback + WS-F mesh fidelity still owed.*
- **CP-4 — Mobile HUD + touch polish.** Ship the per-layer HUD layout editor (WS-D, [D61](../decisions.md))
  and a touch-target/rebind pass so controls feel CoD-Mobile-class. Closes the one PAR row we can
  actually win.
- **CP-5 — Unified cross-platform entitlement.** A single account/entitlement layer so progression,
  loadouts, and cosmetics follow the player across Android/iOS/desktop — the cross-progression
  Warzone Mobile trained the market to expect ([Q9](../open-questions.md) billing rails feed this).
- **CP-6 — Audio identity pass.** Replace procedural placeholders ([D26](../decisions.md)/[D29](../decisions.md))
  with a deliberate sound identity — *load-bearing*, not polish, because audio is the going-dark alert
  channel (invariant #6). Use the scripted Csound/SoX pipeline; keep the accessibility-equivalent cue.
- **CP-7 — Onboarding that teaches the twist.** The highest-leverage screen in the project: a new
  player must read their first death as *"I stayed too long"* (invariant #6). Built into PvE mission 1
  ([`pve-campaign.md`](../pve-campaign.md) §3, WS-A). No incumbent needs this because no incumbent has the
  twist — so we can't borrow it; we have to nail it.
- **CP-8 — Live-ops / content cadence engine.** Wire the `server` scaffolding (telemetry/consent/
  live-ops) into the rotating scenario-parameter modifier system (PvE WS-E) so we can sustain a
  post-launch cadence — *modifiers and content, never balance-number or power hacks* (keeps #1/#6).
- **CP-9 — Command-layer readability + teach-fast pass.** The closing item for the **PAR-ish**
  *Strategic/command depth* row above: the RTS half must read *at a glance on a small screen* and
  *teach itself fast* — a shooter-first audience won't learn it slowly, and a CoH/StarCraft veteran
  must respect it (§5, Cohort C). Information architecture + glanceability of selection/orders/
  economy/territory, broader than the pure visual-design HUD pass. Depth stays in the order/stance
  vocabulary (invariant #3), no intel leaks while embodied (#6). *Launch-important.*

**Sequencing principle:** CP-7 (onboarding) and CP-2 (game feel) are launch-critical — they gate
whether a stranger *gets* and *enjoys* the core. CP-1/CP-4/CP-9 are launch-important (table-stakes
for the shooter and command audiences). CP-3/CP-5/CP-6/CP-8 are parity-over-time, fine to ramp
after the PvE product proves the loop. The LEAD rows need *protection*, not new work — the
determinism gates and the single-player-both-jobs symmetry must not erode as we chase parity.

---

## 8. One-line answer to "what's our plan against Delta Force?"

**Don't fight on their axis.** Delta Force is the best mobile shooter; we are the only
command-and-embody hybrid where going dark is the gamble. We reach *good-enough* on the shooter
table-stakes (CP-1…CP-8) so the FPS never embarrasses us, and we win on the square no incumbent —
not Delta Force, not the dead hybrids, not Company of Heroes — actually occupies. The hybrid is the
moat; the parity work is the price of admission to be allowed near the wallet.

---

### Sources

Competitive facts in this doc are grounded in public reporting as of June 2026:

- Delta Force engine/cross-platform — [Wikipedia](https://en.wikipedia.org/wiki/Delta_Force_(2025_video_game)),
  [TechPowerUp](https://www.techpowerup.com/331106/team-jade-discusses-delta-force-franchises-modern-reboot),
  [GDC Vault terrain talk](https://gdcvault.com/play/1035606/-Delta-Force-Performant-High)
- FPS/RTS hybrid failure modes — [Eximius reviews (Metacritic)](https://www.metacritic.com/game/eximius-seize-the-frontline/),
  [Silica (Steam)](https://store.steampowered.com/app/1494420/Silica/),
  [genre retrospective](https://procasualgaming.com/games/best-rts-fps-hybrid-games/)
- Mobile gunsmith / cross-progression bar — [Warzone Mobile gunsmith](https://www.oneesports.gg/call-of-duty/gunsmith-in-warzone-mobile/),
  [CoD Mobile gunsmith impact](https://codmobilecentral.com/blog/call-of-duty-mobile-the-revolutionary-impact-of-the-gunsmith-customization-system)
