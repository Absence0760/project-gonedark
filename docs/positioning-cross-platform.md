# Positioning — *Going Dark* across platforms

> **The one-sentence version:** the dream is *one game everywhere* — start a mission on your
> phone on the bus, finish it on your PC at home — and we're built for it from the ground up,
> because the same deterministic core runs bit-for-bit identically on every device. The one
> genuinely hard part is fairness: **a thumb on glass should never have to out-aim a mouse.**

This is the **cross-platform** companion to the positioning family:

- [`positioning.md`](positioning.md) — the overview + the **mobile / storefront** fight.
- [`positioning-pc.md`](positioning-pc.md) — the **PC** fight (strategy + shooter heavyweights).
- **`positioning-cross-platform.md`** *(this doc)* — keeping *one game* fair and coherent across
  phone, PC, and console.

---

## 1. The promise, in plain words

"Cross-platform" is two separate promises, and people mix them up. Keeping them distinct keeps
us honest:

| Promise | What it means | Everyday example |
|---|---|---|
| **Cross-play** | Players on *different devices* in the *same match* | Your PC friend and your phone both in one battle |
| **Cross-progression** | *One account* — your stuff follows you everywhere | Unlock a sight on mobile, it's there on PC |

The fantasy that sells both: **your game is wherever you are.** Commute on your phone, come home
to your PC, same army, same progress, no restart. That's the bar Fortnite set and everyone now
chases.

---

## 2. Why we're *built* for this (our quiet superpower)

Here's the thing most people miss. **Most games bolt cross-play on afterwards** — it's painful,
because their game logic runs differently on different machines and they have to reconcile it over
the network. We have the opposite problem solved already, by accident of good architecture:

- **One shared core, identical everywhere** (invariant #2). The game's brain — the sim, the
  rules, the netcode — is the *same code* on phone, PC, and console. Only a thin platform layer
  (screen, speakers, touch/mouse) differs.
- **It runs bit-for-bit identically** (invariant #1). We already *proved* this: the simulation on a
  Galaxy S24 phone produced a checksum **identical** to the desktop, tick for tick, over a 300-tick
  run ([`decisions.md`](decisions.md) D22). Same input → same exact result, on an ARM phone and an
  x86 PC alike.

Why that matters for cross-play: our netcode is **lockstep** — players exchange *orders*, not world
state, and each device computes the identical world from them. That **only works if every device
agrees to the last bit** — which is exactly the determinism we built and verified (the lockstep loop
is already proven correct in-process, over a lossy/jittery channel — [D27](decisions.md); the
real-socket transport that carries it between machines is Phase 3 work, not yet shipped). So for us,
**cross-play isn't a feature we bolt on later — it's what the engine is *built to be* from the first
commit.** Once that transport lands, a phone and a PC in the same match is the *natural* case, not a
special one — because the hard part (every device computing the identical world) is already done.

> **In one line:** everyone else fights their engine to bolt on cross-play. Ours was cross-play from
> the first commit — the determinism we needed for the *gameplay* (invariant #1) hands us the
> *platform* story for free.

---

## 3. The gold standards — who set the bar

These are the games players will (consciously or not) expect us to match on the *one game
everywhere* promise.

| Game | What they nail | What we learn |
|---|---|---|
| **Fortnite** | The benchmark: true cross-play **and** cross-progression across PC, console, mobile, Switch — one account, everywhere | This is the bar. Crossplay on by default; your stuff follows you. |
| **Call of Duty: Warzone** | Cross-play with **input-based lobbies** (mouse players matched separately from controller) + shared progression with the main CoD wallet | The cleanest answer to the fairness problem — *sort players by input, not just device* (§4). |
| **Genshin Impact** | One account, one world across PC / mobile / console / cloud; seamless handoff | The "put it down, pick it up elsewhere" handoff done beautifully. (PvE, so no input-fairness fight — note that.) |
| **Minecraft (Bedrock)** | The mass-market proof that phone + console + PC can share one world for a *decade* | Longevity through universal access. |
| **Destiny 2** | Cross-play **and** cross-save bolted onto a live shooter years in | Possible, but it was *hard* precisely because it came late — the opposite of our situation. |
| **Delta Force** | Our shooter rival, also PC + mobile + console with shared progression | Even the incumbent treats cross-platform as table-stakes now. We can't *not* do it. |

The takeaway: **cross-platform is no longer a bonus — it's expected.** A new game that traps your
progress on one device feels broken in 2026. The interesting question isn't *whether*; it's *how we
stay fair while doing it* — which is the rest of this doc.

---

## 4. The hard problem: a thumb vs. a mouse in a gunfight

Here is the one genuinely difficult thing, and we should say it plainly.

We have **two layers**, and they have *very different* fairness profiles across input types:

```
   THE COMMAND LAYER (RTS)              THE EMBODIMENT LAYER (FPS)
   ─────────────────────────           ──────────────────────────
   Tap/click to select & order.        Aim a weapon under pressure.
   A few decisions per second.         Dozens of micro-corrections/sec.

   A thumb and a mouse are             A mouse is simply faster and more
   ROUGHLY EVEN here. Touch is         precise than a thumb here. This is
   even arguably native.               the classic cross-play unfairness.

   ✅  Fair across inputs by nature.    ⚠️  NOT fair across inputs by nature.
```

So our **command** half is naturally fair between a phone and a PC — issuing orders isn't a
twitch contest. But our **embodied** half is a shooter, and **a mouse out-aims a thumb**, full
stop. This is the exact problem the whole industry wrestles with (the endless "is aim-assist
unfair?" argument), and we don't get a free pass on it.

**The industry's answers — and ours:**

1. **Input-based matchmaking** (Warzone's approach): sort players into pools by *how they're
   holding the game* — mouse players with mouse players, touch with touch — regardless of device.
   This is the cleanest fix and our most likely path for any embodied PvP.
2. **Aim assist** for the slower input, tuned per mode. Standard, but a perennial balance headache.
3. **Our structural ace — PvE-first sidesteps it entirely.** Our [first shippable product is
   single-player PvE](pve-campaign.md) ([D58](decisions.md)). Against the AI, **there's no human on
   the other end to be unfair to** — so a phone player and a PC player can each enjoy the campaign at
   their own input's comfort level, cross-progression and all, with *zero* fairness tax. The hard
   problem only switches on when **embodied PvP** does — and that's later (Phase 3 netcode).
4. **Our second structural cushion — the battle is mostly command.** Even in PvP, far more of the
   game is *commanding* (input-fair) than *embodied twitch-aiming* (input-sensitive). The thing that
   most decides a match — reading the board, setting your army up, timing your dive — is the *fair*
   skill, not raw aim. That softens the mismatch in a way a pure shooter can't lean on.

> **This is a real, open decision, not a solved one.** Exactly how embodied PvP handles
> mixed-input fairness — separate pools, aim assist, or lean on the command-heavy balance — is logged
> as **[Q17](open-questions.md)**. We flag it now so it's designed in deliberately, not patched in
> after launch (the way it bit Destiny and others). The good news: **nothing about it blocks the PvE
> launch**, where cross-play is pure upside.

---

## 5. One account, everything follows you

The *cross-progression* half is less philosophically tricky but still real work: a single account
and entitlement layer so your unlocks, loadouts, and cosmetics live on a server, not a device.

- It's already tagged as **CP-5** in [`roadmap.md`](roadmap.md) (unified cross-platform entitlement).
- It leans on the per-platform **billing rails** question ([Q9](open-questions.md)) — buying a
  cosmetic on iOS vs. Android vs. PC goes through different stores, but must land in *one* wallet.
- Our cosmetics are **presentation-only and fairness-bounded** ([D13](decisions.md)/[D60](decisions.md)),
  which keeps this clean: nothing that follows you across devices can grant power, so syncing it can
  never create a pay-or-platform-to-win problem. Two players with different skins (or different
  phones) compute the **same** world (invariant #1).

---

## 6. The cross-platform scoreboard

**Lead** = our architecture gives us an edge · **Par** = we can match the standard ·
**Lag / open** = work or a decision remains.

| Capability | Us | The bar | Verdict |
|---|---|---|---|
| Engine *built* for cross-play (not bolted on) | deterministic core, proven phone≡PC (D22) | most bolt it on late | **LEAD** |
| Phone + PC in the same match, technically | lockstep already device-agnostic | Fortnite / Warzone | **PAR (substrate done)** |
| Cross-progression / one account | persistence exists; entitlement layer pending (CP-5) | Fortnite / Genshin | **LAG (build it)** |
| Fairness: command layer across inputs | even by nature | — | **LEAD** |
| Fairness: embodied PvP across inputs | **open — [Q17](open-questions.md)** | Warzone input pools | **OPEN DECISION** |
| Pick-up-and-continue handoff | shared core makes state portable | Genshin | **PAR (needs cloud-save wiring)** |
| Per-platform billing into one wallet | open — [Q9](open-questions.md) | every live game | **LAG (decision + build)** |
| Cross-play with *no pay/platform-to-win* | guaranteed by fairness invariants | a constant struggle for others | **LEAD** |

**Read it like this:** our **architecture hands us the hard technical half for free** (the engine is
cross-play-native and provably identical across devices), and our **fairness invariants hand us the
"no platform-to-win" half for free.** What's left is *product plumbing* (one account, billing, cloud
save) and *one real design decision* (how embodied PvP handles a thumb-vs-mouse fight, Q17). That's a
genuinely strong position — most studios would trade a lot to start here.

---

## 7. What this means for the build (cross-platform work)

Tagged **XP-n** in [`roadmap.md`](roadmap.md) → *Competitive parity*.

- **XP-1 — Cross-save & handoff.** Match/campaign state and progress live server-side so you can
  stop on one device and resume on another. The "commute on your phone, finish on your PC" promise.
- **XP-2 — Input-based matchmaking policy (decide [Q17](open-questions.md) first).** For embodied PvP:
  separate input pools, aim-assist tuning, or lean on the command-heavy balance. **Decide before
  building PvP, not after.** PvE needs none of this.
- **XP-3 — Unified entitlement / one wallet** *(= CP-5)*. One account; unlocks and cosmetics follow
  the player; per-platform purchases ([Q9](open-questions.md)) all resolve into it.
- **XP-4 — Control parity without forking the game.** Each platform gets a native-feeling control
  scheme (touch / mouse+keyboard / controller) **over the same shared core** (invariant #2) — never a
  forked ruleset. The *controls* differ; the *game* doesn't.

> **The guardrail that makes all of this safe:** invariants #1 and #2. Because every device runs the
> identical deterministic core, cross-play can never desync and cross-progression can never smuggle in
> a power advantage. We get to chase the *one-game-everywhere* dream without the usual fear that it
> quietly breaks fairness — the fairness was load-bearing in the engine long before it was a
> cross-platform feature.

---

## 8. The one-line answer to "what's our cross-platform story?"

**We're cross-play-native by construction** — the same deterministic core runs identically on every
device (proven phone≡PC, [D22](decisions.md)), so once the network transport ships (Phase 3) a phone
and a PC sharing a battle is a *natural* case, not a bolted-on miracle. We ship the
*one-account-everywhere* plumbing as product work, and we **start where cross-play is pure upside
(PvE)**, deliberately solving the one genuinely hard part — a thumb shouldn't have to out-aim a mouse
— *before* we turn on embodied PvP, instead of patching it in after it hurts someone.

---

### Sources

Grounded in public reporting as of June 2026:

- Cross-play / input-based matchmaking / aim-assist fairness —
  [Fortnite crossplay guide (Eneba)](https://www.eneba.com/hub/games/game-guides/is-fortnite-cross-platform/),
  [input-based matchmaking discussion (ResetEra)](https://www.resetera.com/threads/input-based-matchmaking-needs-to-be-standard-in-every-fps-multiplayer-game-from-here-on-out.1187616/)
- Warzone Mobile cross-progression with the CoD wallet —
  [oneEsports](https://www.oneesports.gg/call-of-duty/gunsmith-in-warzone-mobile/)
- Internal evidence the core is provably identical phone≡PC — [`decisions.md`](decisions.md) D22.
