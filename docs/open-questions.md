# Open Questions

Design forks still on the table. Resolve these into [`decisions.md`](decisions.md) as
they're settled. Current leans are noted but not locked.

---

## Q1 — How thin is the thread back to command while embodied?

The "world goes dark" rule is locked (D7), but *how much* gets through is not.

| Option | Feel | Risk |
|---|---|---|
| **Total blackout** — no minimap, no alerts | Purest, harshest, highest nerve | Can feel like robbery; hard to make fair |
| **Alerts only** — directional flash + audio, no detail | Tense but fair; "something's wrong, but what?" | Needs excellent audio to carry it |
| **Minimap survives** — blips/fog on a corner map | Softest, most strategic | Bleeds away the dread; may undercut the whole point |

**Current lean:** *alerts-only with killer audio.* Keeps it fair without killing the
tension.

---

## Q2 — Can the enemy tell when you've gone dark?

Does an opponent get any signal that you're currently embodied (and therefore blind)?

- **No signal, pure inference** — they must *read* it: your units stopped getting new
  orders; one unit is suddenly moving with superhuman precision (that's your avatar).
  Rewards game sense.
- **Soft tell** — the embodied unit is visually marked to enemies (a hero-unit
  standout), so killing it specifically is a high-value play. Risk/reward of being the
  star.
- **No tell at all** — blindness is fully private.

**Why it matters:** this is the heart of the PvP mind game — *read when your opponent
is blind and punish it.* In PvE, the AI simulates the same pressure by punishing
undefended angles when you've overstayed (but should NOT be omnisciently "you're
embodied, attack now" — that feels cheap).

**Current lean:** undecided. The "soft tell / marked hero unit" option is the most
interesting risk/reward but needs playtesting.

---

## Q3 — Is possession instant-and-global, or leashed?

Can you drop into *any* living unit *anywhere*, instantly?

- **Unconstrained** — your "presence" teleports to wherever the fight is; your skill
  always shows up where needed. Most fun, most slippery.
- **Leashed** — a short cooldown between possessions, or you can only embody units
  near a camp you control. More tactical, less god-like.

**Current lean:** start unconstrained; add a leash *only* if it feels too slippery in
testing.

---

## Q4 — Touch control scheme (the real product risk) — RESOLVED ([D14](decisions.md))

**Resolved in [D14](decisions.md):** the Phase 0 control prototype passed — the
embody↔command loop (tap-to-move command layer + drag-pan/pinch-zoom, instant embody
swap, left-stick/right-look/FIRE embodied scheme) feels good in hand, validated on real
hardware (Galaxy S24). The existential risk this question carried — that the scheme
couldn't be made fun on a touchscreen — is retired.

What remains is *downstream design work, not this fork*: the detailed shipping touch UI
(multi-unit selection, the full order/stance vocabulary on a small screen) is a Phase 2
concern. Two Phase-0-adjacent caveats are logged in D14: **audio** is still faked
(D7/§6 makes it primary for going-dark) and must be validated with real audio, and
embodied feel **over the network** is unproven — that is the Phase 0.5 spike (see Q7/Q8).

---

## Q5 — Single-player, multiplayer, or both — and in what order?

The design supports both, and the tech (deterministic lockstep) is multiplayer-ready,
but the *first shippable* target isn't decided.

- PvP is where the attention mind-game sings (Q2).
- PvE/campaign is a lower netcode risk and a better onboarding surface for the
  blindness mechanic.

**Current lean:** undecided; likely PvE-first to derisk onboarding and skip netcode
until the core loop is proven, with multiplayer as a fast-follow given the
lockstep-ready architecture.

---

## Q6 — Working title

`Going Dark` is a placeholder chosen for the signature mechanic. Not locked; the repo
and directory (`project-gonedark`) are trivial to rename.

---

## Q7 — What netcode model carries *embodied* (FPS) combat? — RESOLVED ([D15](decisions.md))

**Resolved in [D15](decisions.md): avatar-local prediction.** The Phase 0.5 latency spike
proved that embodied combat over deterministic lockstep + input delay feels good **when the
player's own embodied entity is predicted locally and reconciled against the authoritative
tick** (everything else stays pure lockstep) — and feels laggy with raw lockstep alone.
Validated hands-on over real Wi-Fi up to a simulated "cellular, worst" connection.

**Hard rule carried to Phase 1:** the prediction lives in the **presentation/input path
only** and must never feed back into deterministic sim state (or it desyncs lockstep);
authoritative hit resolution still happens at tick T+D. See
[`architecture.md`](architecture.md) §"Embodied combat over lockstep" (now a settled
approach) and D15 for the full caveats (audio still faked; not a determinism test).

---

## Q8 — Is a 30 Hz sim tick enough for embodied combat? — RESOLVED ([D16](decisions.md): NO)

**Resolved in [D16](decisions.md): 30 Hz is too coarse for embodied combat — target 60 Hz.**
The Phase 0.5 A/B was decisive: 30 Hz felt "chunky/bad" for first-person aim/fire, 60 Hz was
the only acceptable rate — and this held *even with* avatar-local prediction ([Q7](decisions.md)/D15) on,
because prediction kills input *latency* but not the *granularity* of hit/aim resolution. The
embodied layer needs the higher rate.

**The follow-on — how to deliver it — is now [Q10](#q10--how-to-deliver-the-60-hz-embodied-rate-global-vs-dual-rate).**

---

## Q9 — Billing rails for cosmetic purchases (per platform)

Monetization is **cosmetic-only** ([`decisions.md`](decisions.md) D13). *How and where
players pay* is open, and it's platform-constrained — not a free choice.

| Rail | Platforms | Notes |
|---|---|---|
| **Platform IAP** | iOS, Android | StoreKit / Play Billing are **mandatory** for in-app digital goods per store policy; ~30% cut (15% small-business tier). Non-negotiable on mobile. |
| **Stripe / own checkout** | desktop/web only | Viable for a Linux/Windows direct build or a web store; **not allowed** for in-app digital goods on mobile. |
| **Steam** | Windows (+Linux via Steam) | If shipping on Steam, its wallet + ~30% cut apply; separate entitlement source again. |

**The real tension:** a player who buys a skin on one platform should ideally **own it
everywhere** — but unified cross-platform entitlement layered on top of three different
storefronts (Apple, Google, Steam/direct), each with its own rules, receipts, and
refunds, is real work. Mobile *must* use store IAP; desktop *can* use Stripe/Steam.

**Current lean:** undecided; likely **hybrid** — mandatory store IAP on mobile, plus
Stripe/Steam on desktop — behind a **unified entitlement service** keyed to the account
(ties to the accounts/entitlements backend in [`infrastructure.md`](infrastructure.md)).
The cross-store reconciliation cost needs scoping before this locks.

---

## Q10 — How to deliver the 60 Hz embodied rate: global vs dual-rate?

[D16](decisions.md) settled that embodied combat needs **~60 Hz** (30 Hz felt chunky). *How*
to provide it — without wrecking the mobile power/thermal budget at ~200 units — is open. The
Phase 1 sim loop is coded against a single parameterized rate (`core::sim::TICK_HZ`, provisional
60), so the loop runs, but **the rate is not locked**: it must be decided by **profiling on real
mid-range arm64 hardware** (build-order step 8) before it can be fixed.

| Option | Upside | Cost / risk |
|---|---|---|
| **(a) Global 60 Hz** — whole sim runs at 60 | One tick rate; simplest; no dual-clock complexity | ~2× total sim CPU and ~2× battery/heat for 200 units; tighter net budget. Per-tick work (~8.5 ms) still fits a 16.6 ms tick, so feasible — just power-hungry |
| **(b) Dual-rate** — RTS/unit sim 30 Hz, embodied combat 60 Hz | Far cheaper at scale (combat is a tiny slice of the 200-unit work) | **Two** deterministic clocks that both must stay lockstep-deterministic; careful rate-interaction; more netcode/checksum complexity |
| **(c) Aim@render, commit@tick** | — | **Insufficient alone** (D16): the chunkiness *is* the 30 Hz commit granularity |

**Constraints either way:** the sim/render decoupling + fixed deterministic tick (invariant
#4) and fixed-point combat math (invariant #1) hold at whatever rate — a faster tick admits
no floats. **Current lean:** undecided; the user will *accept the perf cost* (favouring (a))
if (b) proves too complex, but (b) is the elegant mobile answer if the determinism stays
clean. Profiling decides.

---

## Q11 — How to source the *hero* asset tier: CC0-curated, commissioned, or AI-generated?

The content pipeline ([`content-pipeline.md`](content-pipeline.md)) settles the *mechanism*
— one high-quality source, cooked down into low/mid/high tiers, license-checked in CI — and
the *low/mid* tiers are clearly CC0-curated + procedural greybox. What's open is the **hero
tier** (§2): the rationed, eye-level art the embodied camera lingers on (the player's weapon,
their own unit, signature structures). Three ways to get it, each a different cost/identity
bet.

| Option | Upside | Cost / risk |
|---|---|---|
| **(a) CC0-curated only** | Cheapest, zero attribution burden, ships today | Generic look; hero assets shared with every other CC0 game; hard to build a distinct art identity |
| **(b) Commissioned / bought** | Distinct identity; full rights; best eye-level quality | Real money + lead time; needs an art director; the per-hero-asset bill is the budget risk |
| **(c) AI-generated** (text-to-3D) | Fast, cheap, on-brief iteration | Quality still uneven at eye-level FPS range; **license/ownership terms vary by tool and are unsettled**; output still must pass the two-view filter and the cook |

**Why it matters:** the two-view constraint ([`architecture.md`](architecture.md)) means hero
assets carry real eye-level scrutiny — exactly where (a)'s generic look and (c)'s uneven
quality show worst, and where (b)'s cost concentrates. The low/mid tiers don't force this
call; the hero tier does.

**Constraints either way:** whatever the source, it passes the same license hygiene
([`content-pipeline.md`](content-pipeline.md) §3) and two-view filter (§4) and goes through
the same cook (§1) — the pipeline is source-agnostic. This fork is about *spend and identity*,
not plumbing.

**Current lean:** undecided. Likely a **hybrid** — CC0/procedural for low/mid (most of the
game), a small *commissioned* hero set for the handful of things the camera lingers on, with
AI-generation used for *iteration/greyboxing* hero candidates rather than final output until
its license terms and eye-level quality firm up. Scope the hero-asset count and budget before
locking.
