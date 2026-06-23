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
