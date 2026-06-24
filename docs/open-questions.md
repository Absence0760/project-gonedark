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

## Q4 — Touch control scheme (the real product risk)

Not a design-fork so much as the hardest unsolved problem. How do *CoH*-style
selection/orders, a competent FPS scheme, and an instant swap between them all coexist
on a small touchscreen?

**Status:** unsolved by design — this is **Phase 0** in the roadmap. Prototype before
committing to any systems. If this isn't fun in hand, the concept reworks or dies
here.

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

## Q7 — What netcode model carries *embodied* (FPS) combat?

The RTS half is settled: **deterministic lockstep + input delay** (see
[`architecture.md`](architecture.md) §Netcode, D9) — bandwidth scales with players, not
units. But input-delay lockstep deliberately executes orders *several ticks in the
future*, which is exactly wrong for twitch first-person aim. The FPS half rides the same
wire and this tension is not yet resolved. (See the "Embodied combat over lockstep"
analysis in [`architecture.md`](architecture.md).)

| Option | Feel | Cost / risk |
|---|---|---|
| **Pure lockstep + tuned input delay** | Reuses everything; one netcode path | Fixed input lag on aim/fire; may feel mushy in a PvP gunfight |
| **Avatar-local prediction** — predict only *your* embodied unit locally, reconcile against the tick; the other ~200 units stay pure lockstep | Crisp local aim without abandoning lockstep | Reconciliation/mispredict handling on one entity; must not leak into deterministic state |
| **Rollback on the embodied layer** (GGPO-style, local fight only) | Best twitch feel | Heavy; rollback over a 200-unit deterministic sim is expensive and complex |
| **Server-arbitrated FPS hits** | Authoritative, cheat-resistant | Breaks the pure-P2P-lockstep model; needs a server in the loop |

**Current lean:** undecided — likely **lockstep + input delay first, add avatar-local
prediction if embodied combat feels laggy.** This is the primary thing the **Phase 0.5
spike** ([`roadmap.md`](roadmap.md)) exists to answer *before* the full engine is built.

---

## Q8 — Is a 30 Hz sim tick enough for embodied combat?

The sim runs **fixed 30 Hz** ([`architecture.md`](architecture.md) Targets / Sim loop). Render
interpolates to 60/120, but *hits, ballistics, and aim resolution happen in the sim* —
they mutate deterministic state, so they cannot live in the render layer. 30 Hz
hit-registration is low next to the 60–128 Hz competitive shooters target.

| Option | Feel | Cost / risk |
|---|---|---|
| **Hold 30 Hz everywhere** | Simplest; one tick rate; cheapest sim | Aim/hitreg may feel coarse; leans hard on interpolation + the Q7 answer |
| **Raise the global sim rate** (e.g. 60 Hz) | Crisper FPS | ~2× sim cost for 200 units; tighter net budget; revisits the frame/sim budgets |
| **Aim sampled at render rate, committed at tick** | Smooth aim without a faster sim | Added complexity; care needed so the committed result stays deterministic |

**Determinism constraint either way:** embodied aim, recoil, raycasts, and projectile
ballistics run *inside* the sim and so must be **fixed-point with LUT trig** — the "no
floats in the sim" invariant applies to first-person combat math too. The "embodiment is
cheap" finding in [`architecture.md`](architecture.md) is about *state plumbing* (input-
source swap, vision toggle), not about combat-resolution math, which is real work.

**Current lean:** hold 30 Hz, lean on interpolation, and let the **Phase 0.5 spike**
decide alongside Q7 whether it's enough.

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
