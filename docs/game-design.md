# Game Design — Going Dark *(working title)*

> Status: living design doc. Captures the design as agreed in pre-production.
> Open forks live in [`open-questions.md`](open-questions.md); the reasoning
> behind locked decisions lives in [`decisions.md`](decisions.md); how this design
> stacks up against the field (Delta Force, the FPS/RTS-hybrid graveyard, the CoH
> lineage) lives in [`positioning.md`](positioning.md).

## 1. Concept

A real-time tactics game in the mold of *Company of Heroes* — squad combat, cover,
suppression, territory control, base/camp building and upgrading — with one
defining twist: **the commander can possess an individual unit and fight it in
first person, and doing so blinds them to the rest of the battlefield.**

You are never *not* the commander. Embodiment is a temporary lens. Death in FPS
isn't a game-over screen — it's a demotion back to the command view, from which you
pick another unit to drive.

## 2. Design pillars

1. **Divided attention is the skill.** Not delegation. You can't command and embody
   at the same time, so every dive is a bet on what you're *not* watching.
2. **Information is the currency.** Embodiment costs sight, not just actions. The
   meta-skill is reading the board well enough to know when it's safe to go dark.
3. **The army obeys, it doesn't think.** Unit AI is a literal executor of your last
   orders. Smart autopilot is banned — it would let the game play itself.
4. **The cost must always feel fair.** Every loss should read as *"I stayed too
   long,"* never *"the game robbed me."*
5. **More buildable, not less.** Every design choice here was checked against the
   engine; the good ones simplify the tech (see §10).

## 3. The two layers

### Command layer (RTS, top-down)
- Build and upgrade camps; manage economy and production.
- Capture and hold territory; manage fog of war.
- Train armies; issue orders and **stances** to units and squads.
- Full strategic vision of everything you've scouted — **only while not embodied.**

### Embodiment layer (FPS, first/third person)
- Possess any one living unit (tank, trooper, etc.).
- Your manual skill is now live: precise aim, cover peeking, dodging — things the
  unit AI cannot do.
- **The strategic map goes dark.** You see only what your unit sees.

These two layers are mutually exclusive in time. That exclusivity *is* the game.

## 4. Core loop

```
   ┌─────────────── COMMAND (RTS, full vision) ───────────────┐
   │  build / upgrade camps · economy · train army            │
   │  set orders + stances · capture territory · scout        │
   └───────────────┬───────────────────────────▲─────────────┘
                   │ embody a unit              │ surface (by choice)
                   ▼                            │ or death → ejected
   ┌─────────────── EMBODIED (FPS, WORLD DARK) ─┴─────────────┐
   │  fight one unit by hand · win the local engagement       │
   │  blind to the macro · thin alerts only · "stay as I dare"│
   └──────────────────────────────────────────────────────────┘
```

## 5. The embodiment (possession) mechanic

- **Anyone, (probably) anywhere.** You can drop into any living unit you own.
  Whether possession is instant-and-global or leashed (cooldown / must be near a
  controlled camp) is an [open question](open-questions.md) — starting unconstrained
  and adding a leash only if it feels too slippery.
- **The switch is cheap; the absence is expensive.** Entering/leaving a unit must be
  fast and smooth. The cost is *time away*, never fumbling UI. Clunky menus would be
  fake difficulty.
- **Embodiment must be mechanically *better* at its job.** If driving the tank by
  hand isn't clearly stronger than letting the AI drive it, players won't accept the
  blindness and the whole FPS layer becomes a novelty nobody uses. Manual control
  has to win the local fight in ways the AI can't.

**The embodied control scheme** ([D14](decisions.md) prototype → [D51](decisions.md) shipping;
mobile-first, COD-Mobile-shaped). On touch: a floating **left move stick**, a **right drag-to-look**
region (no visible stick), and floating **Fire / Crouch / Reload / Surface** buttons. On desktop:
WASD + mouse-look + click-fire, with C/R-style keys for crouch/reload. Because two fingers are
*always* down in twin-stick play (move + look), **ejecting back to command is the on-screen Surface
button**, not a two-finger gesture. The three combat mechanics those buttons drive are deterministic
sim state (so they stay bit-identical in lockstep):

- **Ammo + reload.** The embodied weapon has a magazine; running it dry leaves you dry-clicking until
  you tap **Reload** (a real timed reload). This is the moment-to-moment FPS resource — *don't get
  caught reloading.* Auto-combat (AI units) ignores ammo entirely; it is a first-person-only pressure.
- **Crouch — the marksman stance.** Crouching halves your move speed but tightens your aim and extends
  your range: a deliberate *"set up the precise long shot, but you can't reposition"* trade, paid for
  with the mobility you'd want when the world is dark around you.
- Manual fire stays sim-authoritative (the hit is resolved on every peer, [D51](decisions.md)), so
  embodiment "winning the local fight" is real mechanical advantage, not a client-side fudge.

## 6. Going dark — the vision model

**Locked decision: while embodied, the world goes dark.** Fog of war reverts to
*avatar-only vision*. You do not see the rest of the map.

- **Thin thread back, not a blackout-and-pray.** You receive **alerts, not intel** —
  a directional flash + audio cue ("Commander, taking fire on the east camp") tells
  you *something* is wrong, but not what or how bad. This is the tension engine: the
  agonizing *"pull out now, or push for one more kill?"* decision.
- **Audio is a primary system, not polish.** When the screen goes dark on your
  empire, sound is your only remaining link to it — distant explosions, panicked
  callouts, armor you can't see. The embodied mix must bleed strategic-layer audio
  in. You *hear* your empire when you can't see it.
- **The cost must be visceral and constant.** Embodying should visibly clamp the
  world down — vignette, darkened edges, a steady "you are blind right now" signal.
  If the cost is always *felt*, the player owns every death.

Exactly **how thin the thread is** (total blackout / alerts-only / minimap-survives) is
an [open question](open-questions.md) (Q1; current lean: alerts-only with strong audio).
**Whether the enemy can tell you've gone dark** is resolved ([Q2 → D33](decisions.md)): a
tunable tell (`Hidden|Subtle|Marked`), default the soft, LoS-gated, aging `Subtle` tell.

## 7. Death & re-entry

- **Death is a demotion, not a game-over.** Dying ejects you back to the command
  view. There is no FPS respawn timer and no spectator downtime — you land straight
  into a fully active RTS.
- **The unit you were driving is gone for good.** After death you pick a *different*
  living unit. This makes the embodied unit genuinely precious and ties the risk
  straight into the army economy — overstaying costs you a unit *and* dumps you back
  into a base you've been neglecting. The greed is self-punishing; no artificial
  timer needed.
- **Re-entry is its own skill.** Surfacing into a changed map and rapidly
  reassessing — what's the threat, what did I lose, re-issue orders — is a learnable,
  trainable move. The UI should support fast re-orientation (alert markers, a brief
  "while you were away" summary) **without** auto-fixing anything, which would defeat
  the blindness.
- **No army left = stripped to the general.** If your whole army dies you're locked
  into pure base-builder mode until you produce a new unit to embody. That's the
  natural low point and comeback loop; your first new embodiable unit is a real
  moment.

## 8. Unit AI philosophy — the literal executor

The AI must be **dumb and obedient on purpose.**

- Units hold their **last order** and a simple **stance** (e.g. aggressive / hold
  position / hold fire / fall back at X% health). They do *exactly* that — no more.
- A unit told to advance will walk into an ambush if you don't come back to react.
  **That is the point.** The punishment for camping in FPS is your literal-minded
  army getting taken apart while you were heads-down.
- This is what keeps the game skill-based rather than "whose autopilot is smarter."

**Depth goes into the order vocabulary, not the AI brain.** The richer the things you
can pre-program before you dive — patrol routes, engagement ranges, retreat triggers,
trigger zones, queued build/production orders — the more skill lives in the *setup*.
That is the intended home for "smart play": the human's planning, not the machine's
reactions.

## 9. Multiplayer & skill expression

- **PvP is where this sings.** Two humans both face the same dilemma, so a mind game
  emerges: *read when your opponent has gone dark and punish it* — a flank, a base
  poke, an expansion grab timed for when you guess they're blind. Whether blindness
  is detectable by the enemy is an [open question](open-questions.md); even if it
  isn't directly visible, it can be *inferred* (their units stopped getting new
  orders; one unit is suddenly moving with superhuman precision).
- **Single-player / PvE** simulates the same pressure: the AI runs its own attack
  timing and *happens* to punish you when you've overstayed and left an angle
  undefended. It should **not** be omnisciently "you're embodied, attack now" — that
  feels cheap. Emergent punishment from honest AI timing feels fair.

**Summary of where skill lives:** pre-dive setup (order/stance quality) ·
board-reading ("can I afford to be blind?") · manual combat while embodied ·
fast re-entry and reassessment · (PvP) reading the opponent's attention.

## 10. Failure modes we're designing against

| Failure mode | Guard |
|---|---|
| FPS mode is a novelty nobody uses | Embodiment must be *mechanically better* at the local fight than AI control (§5) |
| Blindness feels like robbery | Thin alert thread + killer audio + visceral, constant "you're blind" feedback (§6) |
| Game plays itself | Literal-executor AI; depth in the order vocabulary, not the AI brain (§8) |
| Switching feels clunky | Fast, smooth transitions; cost is time-away, never UI friction (§5) |
| Death feels like dead time | Death = instant return to an active command view; no respawn timer (§7) |
| New players bounce | Teach and telegraph the blindness; make the cost legible from minute one |

## 11. The biggest *non*-engine risk

**Touch controls.** *CoH*-style selection, cover orders, and camera control were
built for mouse + keyboard. Translating that to a small touchscreen — *and*
layering a competent FPS control scheme on top, *and* making the swap between them
feel instant — is harder than any engine problem in this project. **Prototype the
controls before committing to the systems.** (See the roadmap — this is Phase 0.)

## 12. Monetization — cosmetic-only

**Locked: the game sells only cosmetics — weapon skins and player/unit skins. No
pay-to-win, ever.** Nothing purchasable touches stats, balance, or capability; skins
sell *identity, not advantage*. ([`decisions.md`](decisions.md) D13.)

This is the only revenue model that doesn't contradict pillar 4 (*the cost must always
feel fair*). The whole game is a fairness argument — a purchasable edge would detonate
it. Skins are **presentation-layer only**, so they ride the decoupled sim/render split
for free and can't affect determinism, hitboxes, silhouette readability, or the
embodied-unit tell (the hard guardrails live in D13). The open fork is *billing rails*
per platform — store IAP on mobile vs Stripe/Steam on desktop —
[`open-questions.md`](open-questions.md) Q9.

## 13. Progression, loadout & customization

The game has a **horizontal** progression: playing *widens* your options, it never raises
your power ceiling. This is the only progression model that survives pillar 4 and
[D13](decisions.md), and it covers three surfaces — full design in
[`customization.md`](customization.md).

- **Gunsmith (weapon function).** A Call-of-Duty-Mobile-style attachment system where every
  attachment is a **trade, not an upgrade** — long barrel buys range with ADS speed, a grip
  buys recoil control with handling. There is no strictly-dominant build (the anti-degeneracy
  discipline of [D30](decisions.md)); a loadout is a *playstyle*, not a *tier*. Loadout stat
  deltas are the one customization that touches the sim, so they are **fixed-point and
  checksum-folded** (invariants #1/#7) as deterministic match-setup input
  ([D60](decisions.md)).
- **Cosmetics (identity).** Skins, paint, charms — strictly presentation-layer, the only
  purchasable goods ([D13](decisions.md)). Identity, never advantage.
- **HUD layout editor.** A CoD-Mobile / Mobile-Legends layout editor for the touch controls
  — drag/resize/opacity, per-layer presets, presentation-only, and **bounded by invariant #6**
  (it configures placement, never information; it can never reveal map intel while embodied)
  ([D61](decisions.md)).

**Unlocks grant content, not power:** the campaign opens new units, maps, and attachment
*options* — a wider palette a new player and a veteran field at equal power.

## 14. PvE — the Operations campaign

**The first shippable product is single-player PvE** ([D58](decisions.md) resolves
[Q5](open-questions.md): *PvE-first, PvP fast-follow*). The campaign is a **CoH/Delta-Force
Operations hub** — a node-graph of replayable missions with difficulty tiers and
scenario-parameter modifiers — and it exists to **teach *going dark*** to a player who can't
yet face a human: it scripts the temptation to overstay and lets the honest-AI commander
(§9) collect the debt *fairly* (invariant #6). Mission archetypes (Seize / Hold / Assassinate
/ Push) are each a parameterized scenario plus a **host-side objective set** read off the
deterministic event stream — *not* sim state, so missions add **zero desync surface** (the
same footing as the win/lose evaluator, fog, and alerts). Full design:
[`pve-campaign.md`](pve-campaign.md); build sequencing: [`pve-campaign-plan.md`](pve-campaign-plan.md).
