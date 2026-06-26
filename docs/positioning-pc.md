# Positioning — *Going Dark* on PC

> **The one-sentence version:** on PC we're measured against the best strategy games
> *and* the best shooters at the same time — but no PC game lets one player command an
> army and then drop into a single soldier's eyes while the map goes black, so we win by
> being the only thing in that space, not by out-gunning *Call of Duty* or out-strategising
> *StarCraft*.

This is the **PC** companion to the positioning family:

- [`positioning.md`](positioning.md) — the overview + the **mobile / storefront** fight
  (Delta Force, CoD Mobile, the gunsmith, free-to-play).
- **`positioning-pc.md`** *(this doc)* — the **PC** fight (Company of Heroes, Battlefield,
  Halo, Destiny, the strategy lineage).
- [`positioning-cross-platform.md`](positioning-cross-platform.md) — how we keep *one game*
  fair and coherent across phone, PC, and console.

Nothing here reopens a [locked invariant](../CLAUDE.md). This is strategy, not design.

---

## 1. Why PC is a different fight than mobile

A phone player and a PC player want different things. If we treat them the same, we lose
both.

| | **Mobile player** | **PC player** |
|---|---|---|
| Session | Short, on the move, interrupted | Long, seated, focused |
| Input | Thumbs on glass | Mouse precision + a full keyboard |
| Money | Free-to-play, small purchases | Often pays up front; expects *value* |
| Taste | "Is this fun in 5 minutes?" | "Does this have depth I can master?" |
| Knows the genres? | Casually | **Deeply** — they've played the classics |

The headline: **PC players are genre-literate and unforgiving.** They will judge our
command layer against *Company of Heroes* and *StarCraft*, and our shooting against *Call
of Duty* and *Battlefield* — the best in the world at each. We can't beat those games at
their own game. We don't have to. **We're playing a different game that happens to contain
both.**

---

## 2. The PC field, in three groups

We don't have one competitor on PC — we have three *kinds* of competitor, each judging a
different half of us. Here's the whole field at a glance, then a closer look at each.

```
            ┌─────────────────────────────────────────────────────────┐
            │                  WHO JUDGES WHICH HALF OF US             │
            ├──────────────────────┬──────────────────────────────────┤
            │  Our COMMAND layer    │  Company of Heroes 3 · StarCraft │
            │  (the RTS half)       │  II · Total War · World in       │
            │                       │  Conflict                        │
            ├──────────────────────┼──────────────────────────────────┤
            │  Our EMBODIMENT layer │  Call of Duty · Battlefield 6 ·  │
            │  (the FPS half)       │  Halo                            │
            ├──────────────────────┼──────────────────────────────────┤
            │  Our CAMPAIGN + the   │  Destiny 2 · Halo · (Helldivers) │
            │  "play it for months" │                                  │
            ├──────────────────────┴──────────────────────────────────┤
            │  The PC games that ALMOST do what we do — and split it    │
            │  across players instead:  Hell Let Loose · Squad ·        │
            │  Planetside 2  (see §6 — the "so close" cohort)           │
            └───────────────────────────────────────────────────────────┘
```

---

## 3. Group A — The strategy lineage *(who judges our command layer)*

These set the bar for **how good it feels to command an army.** None of them has
embodiment, so none threatens our core — but they decide whether a strategy veteran
respects our RTS half.

### Company of Heroes 3 — *our literal model*
Squad combat, cover, suppression, territory control. This **is** the game our command
layer is built in the image of ([`game-design.md`](game-design.md) §1). The bar it sets:
tactical depth that reads instantly. The lesson hiding in its history: even *Relic*, the
masters, shipped CoH3 to a rough reception in 2023 and had to win players back with
updates. **Pure tactical RTS is hard and a little niche on its own** — which is exactly why
strapping it to an FPS gives it a reason to reach a bigger audience.

### StarCraft II — *the skill-ceiling benchmark*
The gold standard for "mastery is the whole point," and still the face of RTS as an
e-sport. We do **not** compete on lightning-fast macro/micro and 300-actions-per-minute.
But SC2 sets the expectation a PC strategy player carries in the door: **the game should
reward getting better at it.** Our depth lives in the *order-and-stance* vocabulary and in
*when you dare to go dark* — a different skill, but it must be a real one.

### Total War — *the closest thing to "two layers" that exists*
This is the most important comparison in this doc. Total War is the one mainstream series
that already lets you **zoom from a strategy map down into a real-time battle.** It proves —
across two decades and millions of sales — that **players love commanding the big picture
and the boots on the ground.**

But look at *where it stops*, because the gap is our entire product:

- Its two layers are **separate phases** (a turn-based campaign, *then* a battle). Ours is
  **one continuous moment** — you dive mid-fight and surface mid-fight.
- You command **thousands**; you never *become* one of them.
- **You never go blind.** Total War never charges you *sight* for getting closer.

> **Take-away:** the "strategy + the soldier's-eye view" fantasy is proven and huge. Nobody
> has pushed the zoom all the way down to **one soldier's actual eyes, mid-battle, at the
> cost of your vision of everything else.** That last step is us.

### World in Conflict — *RTS without the busywork*
A fast, tactical RTS with **no base-building** — you call in reinforcements and fight. Worth
naming because our command layer is more *tactical* than *economic*, and WiC is the proof
that a streamlined, combat-forward RTS can feel great. A reference for keeping the command
layer punchy, not fiddly.

---

## 4. Group B — The big shooters *(who judges our embodiment layer)*

These set the bar for **how good it feels to be the soldier.** This is our weaker half on
raw production values, and we're honest about it (same as the mobile doc, §4). The goal is
**"a Battlefield player doesn't wince at our gunplay,"** not "we out-shoot Battlefield."

### Call of Duty (PC) — *the gunfeel benchmark*
The most polished moment-to-moment shooting in the business: fast time-to-kill, butter-smooth
movement, instant feedback. **We will never out-twitch CoD, and we shouldn't try.** Our
shooting is *tactical and consequential* — every death drops you back to command and costs
you the unit — which is a deliberately different feel from CoD's instant-respawn arcade.

### Battlefield 6 — *the closest mainstream FPS to our feeling*
Of all the big shooters, Battlefield's fantasy is nearest to ours: **you're one soldier in a
huge combined-arms battle** — infantry, tanks, vehicles, things blowing up around you. That
"small part of a big war" feeling is the emotional cousin of embodiment.

Two things make it *not* our competitor, and both are gifts to us:

1. In Battlefield the "big war" around you is **other players and scripting** — not *your*
   army that *you* set up. We give you the part Battlefield can't: the battle is **yours,
   because you built and ordered it** before you dove in.
2. Even **Battlefield 6** — the best-selling game of 2025 — was criticised at launch for maps
   *too small and too infantry-focused* to deliver the large-scale combined-arms chaos people
   actually wanted. **The fantasy is in demand and hard to satisfy.** That's the gap we aim at.

### Halo — *the campaign and the sandbox*
Halo wrote the book on **handcrafted FPS encounters** and the "thirty seconds of fun" combat
loop, and the series is moving to Unreal Engine 5 under the new Halo Studios. We won't match
its fidelity — but its *encounter design* is the model for our PvE missions
([`pve-campaign.md`](pve-campaign.md)): pressure, then relief, then a memorable set-piece.

---

## 5. Group C — The "play it for months" shooters *(who judges our campaign + longevity)*

### Destiny 2 — *the decade-long shooter*
Destiny is the proof that **a shooter can be a hobby you keep for years** — deep build-crafting,
seasonal content, a PvE endgame people return to. Its weapon/build depth is a north star for
our gunsmith ([`customization.md`](customization.md)) and its content cadence for our live-ops
(CP-8 in [`roadmap.md`](roadmap.md)).

The cautionary half: even Destiny's *best* and most acclaimed expansion (The Final Shape, 2024,
which wrapped its decade-long story) **made less money than the ones before it.** Live-service
is brutal and crowded. **Lesson for us: earn retention with the thing only we have (the
divided-attention bet), don't try to out-grind the grind-masters.**

---

## 6. The "so close" cohort — PC games that almost do what we do

This is the most flattering and most instructive group. A handful of PC games have reached for
**war with both a strategy brain and boots on the ground** — and every one splits the two jobs
across **different players.**

| Game | What it has | The catch (and our edge) |
|---|---|---|
| **Hell Let Loose** | 50-v-50 WWII shooter with a real **Commander role** + an RTS-style resource meta-game (manpower / munitions / fuel) | The commander is *one human*; the soldiers are *other humans* who may ignore the plan. We put both jobs in **one head** — your army can't disobey, because it's a literal executor of *your* orders (invariant #3). |
| **Squad** | Large tactical FPS with squad leaders + a command layer, heavy on coordination | Same split, same dependence on strangers cooperating. |
| **Planetside 2** | Massive persistent combined-arms war; outfits and leaders coordinate | Strategy emerges from *crowds*, not a single commander's plan. Gloriously chaotic, but not *your* battle. |

> **Why this matters:** these games prove the appetite for our fantasy is real and specifically
> *PC-shaped* — players will happily sink hundreds of hours into "strategy + boots on the
> ground." But they all live or die on **other people filling the unglamorous roles.** Our
> design removes that dependency entirely: **one player, both jobs, never at the same time.**
> The exact thing these games struggle with is the thing we deleted by design — see the FPS/RTS
> hybrid history in [`positioning.md`](positioning.md) §3.

---

## 7. The PC scoreboard — where we stand, honestly

**Lead** = nobody on PC has it · **Par** = we can credibly hold our own · **Lag** = a PC
heavyweight is clearly better and we either close the gap or knowingly concede it.

| What PC players care about | Us | The PC bar | Verdict |
|---|---|---|---|
| Command **and** embody, one continuous battle | unique | — | **LEAD** |
| Vision as the price of getting closer ("going dark") | unique | — | **LEAD** |
| Your army is *yours* and obeys (no teammate roulette) | unique | Hell Let Loose splits it | **LEAD** |
| Tactical RTS depth & readability | systems-complete | Company of Heroes 3 | **PAR-ish** |
| RTS skill ceiling / mastery | designed | StarCraft II | **LAG** (different skill, must prove it's real) |
| Raw gunfeel | hitscan + tank | Call of Duty | **LAG** (concede twitch; aim for "feels right with a mouse") |
| Combined-arms spectacle | greybox, growing | Battlefield 6 | **LAG** (but BF can't make it *your* battle) |
| Handcrafted campaign encounters | PvE designed | Halo | **LAG** (closing via PvE) |
| Build depth / long-term chase | gunsmith designed | Destiny 2 | **LAG** (fairness-bounded by choice) |
| Mouse + keyboard precision controls | desktop host exists | every PC game | **PAR (must finish)** |
| Mods / data-driven content | scenarios are data | StarCraft / Total War workshops | **OPPORTUNITY** |
| Replays / spectating / e-sports potential | determinism makes it cheap | SC2 | **OPPORTUNITY** |

**Read it like this:** we **lead on the things that make us us**, sit at **par on tactical
RTS**, and **lag on the production-value polish** PC heavyweights have spent years and huge
teams on. That's the right shape — *lead where it's defensible, concede the photoreal arms race
on purpose.* The two **OPPORTUNITY** rows are the happy surprise: our deterministic core
(invariant #1) makes **replays, spectating, and modding cheap** — things PC audiences love and
mobile rarely gets.

---

## 8. What this means for the build (PC-facing work)

These map to roadmap items tagged **PC-n** in [`roadmap.md`](roadmap.md) → *Competitive
parity*. They're about meeting PC expectations *without* betraying the cross-platform plan or
the invariants.

- **PC-1 — Mouse-and-keyboard combat feel.** Our shooting has to feel right with a *mouse*, not
  just thumbs. Precise aim, sensible defaults, FOV control. PC players notice instantly. (Pairs
  with CP-2's game-feel bar.)
- **PC-2 — The PC control & options surface.** Full rebinds, graphics options, ultrawide /
  high-refresh / high-DPI support — the table-stakes a PC player expects in the settings menu.
- **PC-3 — Replays & spectating (our free win).** The deterministic core means a match is just
  a seed + an input log; replays and spectator view are *cheap* and a genuine PC/e-sports
  differentiator. Low cost, high signal.
- **PC-4 — Mods / data-driven content.** Missions and scenarios are already **data**
  ([`pve-campaign.md`](pve-campaign.md)). Exposing that as moddable content is how *StarCraft*
  and *Total War* stayed alive for decades — a longevity lever unique to PC.

> **The guardrail:** none of this is allowed to fork the game. The PC build is the *same shared
> core* (invariant #2) with a richer control-and-options skin on top. PC-specific features
> (replays, mods) sit at the seams, never inside the sim. How PC and mobile players actually meet
> in the same match is [`positioning-cross-platform.md`](positioning-cross-platform.md)'s job.

---

## 9. The one-line answer to "how do we stand on PC?"

**We don't beat *Company of Heroes* at strategy or *Battlefield* at shooting — we make the game
neither of them can: command your own army, then become one of its soldiers and feel the battle
go dark around you.** On PC we reach *good-enough* on the polish a seated, genre-literate player
expects, then win on the one experience the whole PC field — from *Total War* to *Hell Let
Loose* — has circled but never landed.

---

### Sources

Grounded in public reporting as of June 2026:

- Battlefield 6 scale/reception — [EA](https://www.ea.com/games/battlefield/battlefield-6),
  [GamesRadar vehicles/maps](https://www.gamesradar.com/games/battlefield/battlefield-6-vehicles/)
- Halo → Unreal Engine 5 / Halo Studios — [GamingTrend](https://gamingtrend.com/news/343-industries-is-now-halo-studios-all-future-halo-games-will-be-made-with-unreal-engine-5/),
  [Halo: Campaign Evolved (Wikipedia)](https://en.wikipedia.org/wiki/Halo:_Campaign_Evolved)
- Company of Heroes 3 reception/recovery — [Wikipedia](https://en.wikipedia.org/wiki/Company_of_Heroes_3),
  [PCGamesN](https://www.pcgamesn.com/company-of-heroes-3/dlc-new-update)
- Destiny 2: The Final Shape — [Wikipedia](https://en.wikipedia.org/wiki/Destiny_2:_The_Final_Shape)
- Hell Let Loose commander + RTS meta — [Wikipedia](https://en.wikipedia.org/wiki/Hell_Let_Loose),
  [Steam](https://store.steampowered.com/app/686810/Hell_Let_Loose/)
