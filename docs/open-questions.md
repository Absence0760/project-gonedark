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
tension. **Reaffirmed at Phase-2 close ([decisions.md](decisions.md) D31, high confidence):** the
only option satisfying both pillar 2 and invariant #6, and already the shipped mechanism — but the
lock is gated on a *real-audio* playtest (the load-bearing half has never been validated by ear).

---

## Q2 — Can the enemy tell when you've gone dark? — RESOLVED ([D33](decisions.md): tunable tell, default Subtle)

**Resolved in [D33](decisions.md): ship a tunable three-mode mechanism (`Hidden | Subtle |
Marked`), default `Subtle`.** Rather than lock one design, `core::detection` ships all three behind
a `DetectionConfig`, defaulting to the **soft tell** — a line-of-sight-gated, *aging* marker on the
embodied unit that an observer earns only by having a unit in range with a sightline, and that fades
after sight is lost. The derivation is a pure, checksum-excluded view (same footing as fog/alerts),
so it can never desync lockstep, and in `Hidden` it returns nothing — making the no-omniscient-AI
invariant (#3) structural. The default is a starting point to tune from play, not a frozen lock;
`Hidden`/`Marked` stay one config field away for A/B. The Phase-2-close lean (below) leaned the other
way (no-signal default); D33 takes the *soft-tell-default* fork instead, shipped as a tunable
mechanism so the lean can be validated rather than assumed. Original analysis retained below.

---

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

**Resolved to a tunable mechanism, default Subtle ([D33](decisions.md)).** The Phase-2-close review
(D31, medium confidence) leaned the *other* way — *no-signal / pure inference* as the default,
soft-tell held as a deferred knob. D33 instead **ships all three modes** (`Hidden`/`Subtle`/`Marked`)
behind `core::detection::DetectionConfig` and **defaults to `Subtle`** (the soft tell, now built and
LoS-gated/aging), so the "most interesting but needs playtesting" option is shipped ON and validated
from play rather than assumed — with `Hidden` (the old lean) one config field away for A/B. The final
lock still needs live PvP; this resolves *what to build and default*, not the frozen design.

---

## Q3 — Is possession instant-and-global, or leashed?

Can you drop into *any* living unit *anywhere*, instantly?

- **Unconstrained** — your "presence" teleports to wherever the fight is; your skill
  always shows up where needed. Most fun, most slippery.
- **Leashed** — a short cooldown between possessions, or you can only embody units
  near a camp you control. More tactical, less god-like.

**Current lean:** start unconstrained; add a leash *only* if it feels too slippery in
testing. **Reaffirmed at Phase-2 close ([decisions.md](decisions.md) D31, medium confidence):** ship
unconstrained — the D7 blindness cost is already the leash, and a cooldown would fight D4/D5's
"no artificial friction" stance. If testing proves it too slippery, prefer a *camp-proximity* leash
(diegetic, ties to territory) over a cooldown. Locking needs the loop played at speed (ideally PvP).

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

## Q5 — Single-player, multiplayer, or both — and in what order? — RESOLVED ([D58](decisions.md): PvE-first)

**Resolved in [D58](decisions.md): PvE-first, PvP fast-follow.** The first shippable product is
the single-player **Operations campaign** ([`pve-campaign.md`](pve-campaign.md)) — the onboarding
surface for the going-dark mechanic (invariant #6) and the lower-risk way to prove the core loop
is *fun* before proving it holds up *over the wire* (Phase 3). PvP rides the same
deterministic-lockstep core as a fast-follow; single-player runs the existing `core::lockstep`
loop as a 1-peer delay-0 session ([D27](decisions.md)), so no new netcode is in the critical path.
This takes the pre-production lean (below) and locks it. The PvP-specific forks
([Q1](#q1--how-thin-is-the-thread-back-to-command-while-embodied), [Q3](#q3--is-possession-instant-and-global-or-leashed))
stay open — PvE-first defers their *lock* to when live PvP exists, it doesn't resolve them.
Opens [Q14](#q14--co-op-pve). Original analysis retained below.

---

The design supports both, and the tech (deterministic lockstep) is multiplayer-ready,
but the *first shippable* target wasn't decided.

- PvP is where the attention mind-game sings (Q2).
- PvE/campaign is a lower netcode risk and a better onboarding surface for the
  blindness mechanic.

**Pre-resolution lean (now locked by [D58](decisions.md)):** PvE-first to derisk onboarding and
skip netcode until the core loop is proven, with multiplayer as a fast-follow given the
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
the only acceptable rate — and this held *even with* avatar-local prediction ([Q7](#q7--what-netcode-model-carries-embodied-fps-combat--resolved-d15decisionsmd)/D15) on,
because prediction kills input *latency* but not the *granularity* of hit/aim resolution. The
embodied layer needs the higher rate.

**The follow-on — how to deliver it — was [Q10](#q10--how-to-deliver-the-60-hz-embodied-rate-global-vs-dual-rate--resolved-d21-global-60), now resolved in [D21](decisions.md): a single global 60 Hz tick for Phase 1.**

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

## Q10 — How to deliver the 60 Hz embodied rate: global vs dual-rate? — RESOLVED ([D21](decisions.md): global-60)

**Resolved in [D21](decisions.md): a single global 60 Hz tick for Phase 1** (`core::sim::TICK_HZ
= 60`). [D16](decisions.md) settled that embodied combat needs ~60 Hz but deferred the *delivery
mechanism* to real-arm64 profiling. With Phase 1's **one** unit running on real arm64 (an Adreno
750), a global 60 Hz tick has enormous headroom, so the dual-rate machinery (two
lockstep-deterministic clocks) is unjustified complexity here — exactly D16's lean ("start
global-60; fall to dual-rate only if the 200-unit projection forces it").

**Dual-rate is deferred, not killed.** The 200-unit power/thermal question that motivates a split
is a **scale** concern → it belongs to **Phase 3** (200-unit stress + thermal profiling), not
Phase 1. `TICK_HZ` stays a single named constant so the rate is trivially re-tunable if Phase-3
profiling reopens the split. Invariants #1/#4 hold at any rate (a faster tick admits no floats).

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

---

## Q12 — Does the meta-UI / app shell render in-engine, or as native per-platform shells? — RESOLVED ([D32](decisions.md): native shells, in-engine in-session)

**Resolved in [D32](decisions.md): native per-platform shells for the out-of-match app shell
(option b), with the in-session shell kept in-engine** because it renders under avatar-only fog
while embodied (invariant #6). Native toolkits (SwiftUI / Jetpack Compose / a desktop shell) win
exactly where the fork bites — store/billing sheets ([Q9](open-questions.md)) and accessibility
for the going-dark alert channel (invariant #6) — and the per-platform fork is *chrome*, not game
logic, so invariant #2 holds: the sim/netcode/order vocab stay single-sourced in `core`, reached
through a narrow GPU-free, logic-free shell↔sim seam. Original analysis retained below.

---

The in-match UI is already in-engine (`wgpu`/`render`, D24/D25). The **app shell** — title,
onboarding, settings, lobby, store, profile (scoped in [`roadmap.md`](roadmap.md) Phase 4) — is
unbuilt, and *how* it renders is a real fork. Invariant #2 (one shared core, thin PAL) pushes
toward one shared UI; store/OS integration pushes toward native.

| Option | Upside | Cost / risk |
|---|---|---|
| **(a) In-engine** (wgpu-drawn, one shared shell) | One UI across all four platforms — matches invariant #2; no per-platform UI fork to maintain; consistent look; reuses the renderer already shipped | Rebuilds what the OS gives free (text input, scroll, accessibility tree, IME); native store/account sheets (StoreKit, Play Billing) still must be hosted; weaker OS-native feel |
| **(b) Native shells** (SwiftUI / Jetpack Compose / desktop egui-or-native, per platform) | Best OS integration — accessibility, IME, store/billing sheets, deep links, back-stack; fastest path to platform store compliance | A UI fork *per platform* (the thing invariant #2 exists to avoid), ×4 maintenance; the shared core must expose a clean shell↔sim boundary; look drifts across platforms |
| **(c) Hybrid** | In-engine for the *in-session* shell (pause/summary — must match the game's look + fairness fog); native for *out-of-match* shells where store/OS/accessibility integration pays | Two UI stacks to maintain; a clear seam needed for which surface lives where |

**Why it matters:** the store (Q9 billing rails) and accessibility (the colorblind/HoH
equivalent for the going-dark alert channel, invariant #6) are exactly where native shells earn
their keep — and exactly where an in-engine shell has to *re-earn* what the OS gives for free.
But a per-platform UI fork is the precise cost invariant #2 was written to avoid, so this isn't
a free "just use native" call.

**Constraint either way:** whichever renders, the **in-session** shell (pause, reconnect,
post-match) must obey invariant #6 — it renders under avatar-only fog while embodied and leaks
no strategic intel. That argues the in-session shell stays in-engine regardless, which is why
(c) is more than a fence-sit.

**Resolved to (b) + the forced in-session carve-out ([D32](decisions.md)).** The analysis
above leaned (c) hybrid; the decision lands on **native out-of-match shells** with the in-session
shell in-engine because invariant #6 forces it — which is the (c)-shaped carve-out folded into a
(b) choice. The shell↔sim boundary it gates is fixed before Phase 4 shell work begins.

---

## Q13 — Tank gunnery: hitscan-with-penetration, or true ballistic shell flight? — RESOLVED ([D55](decisions.md): full ballistic, projectile-local height) <a id="q-tank-ballistics"></a>

**Resolved to (b) — full fixed-point ballistic shell flight as a core phase**, not the hitscan
MVP. Travel time + leading is War Thunder's soul and resolving facing *at impact* avoids a
hitscan-then-projectile rework (D55 P3→P4). The one thing **not** taken from option (b): a true
world z-axis. Drop is delivered by **localizing verticality to the projectile** — units stay 2D at
a per-kind hull height; only the shell carries `height`+`vz` and integrates gravity (plan §6a). So
the remaining sub-fork is narrow: *do units ever need real elevation (multi-storey cover, hills)?*
— parked as a later call ([D55](decisions.md) deferrals); the projectile-local model holds until
level design demands more. Original analysis retained below.

The tank-embodiment plan ([D55](decisions.md), [`tank-embodiment-plan.md`](tank-embodiment-plan.md))
makes **shell flight** — travel time, drop, leading — a core phase. War Thunder's signature is
exactly this: **shells with travel time and drop**, so you *lead* moving targets and *arc* over
cover. The fork below was *which fidelity*; it landed on real projectiles.

| Option | Upside | Cost / risk |
|---|---|---|
| **(a) Hitscan + penetration only** (the original pre-resolution lean) | Simplest; reuses the cone/LoS machinery; cheap on the 200-unit mobile budget; no projectile entities | No lead/drop skill; long-range gunnery feels "lasery", not ballistic |
| **(b) Fixed-point projectile** (gravity per tick, hitscan-on-impact) | Real travel time + drop = the War Thunder lead-the-target skill; still float-free (Q16.16 kinematics) | New per-shot projectile entities (sim state + checksum surface); a 200-unit fight firing shells multiplies entity count; aim UX must teach leading |
| **(c) Instant ray, simulated "drop" as a range falloff** | A middle path: no projectile entity, but penetration/damage taper with range to *fake* the long-shot tax | Cosmetically ballistic, not mechanically — no real leading; may read as neither |

**Why it matters:** travel-time gunnery is the deepest part of the embodied-tank skill ceiling
and the clearest "embodiment beats the AI" lever (§5) at range — but projectile entities are the
first thing in the sim that *spawns per shot*, which hits both the checksum surface (invariant #7)
and the 200-unit power budget Phase 3 still has to prove. **Resolved to (b)** with two risk
mitigations baked into D55: a **bounded projectile ring** (a hard shell-count cap, `log`-ed if
hit) keeps the budget honest, and **projectile-local height** delivers drop without the cost/blast
of a world z-axis. Option (c) was rejected as "neither" (no real leading); (a) was rejected because
hitscan tank guns read as lasers and undercut the whole reference feel.

---

## Q14 — Co-op PvE? <a id="q14--co-op-pve"></a>

The Operations campaign ([`pve-campaign.md`](pve-campaign.md), [D58](decisions.md)/[D59](decisions.md))
is designed single-commander first, but deterministic lockstep already supports N peers — so co-op
is *architecturally* cheap-ish and a natural fast-follow alongside PvP. The fork is **design, not
tech.**

- **Shared command** — two players co-command one army (split duties, both can embody). Highest
  coordination, but *whose fog is whose* when one goes dark and the other doesn't? The going-dark
  cost (invariant #6) is built around **one** attention being divided; two attentions change the
  core tension fundamentally.
- **One commander + others embody** — one player runs the macro, others live permanently in units.
  Cleaner fairness model (one fog of war), but it splits the *one player does both jobs* pillar
  ([`game-design.md`](game-design.md) §1) across people — arguably a different game.
- **Separate armies, shared objective** — two commanders, two economies, one mission. Closest to
  PvP-with-a-shared-enemy; least novel but least invariant risk.

**Why it matters:** the entire design rests on *one* divided attention. Co-op doesn't break the
tech, it potentially dilutes the pillar — so this needs a deliberate call, not a default.

**Current lean:** single-commander campaign first ([D58](decisions.md) ships that); revisit co-op
after the solo loop and PvP both exist, leaning *separate armies, shared objective* as the variant
that least disturbs invariant #6.

---

## Q15 — Mission authoring format: Rust builders or external data files? <a id="q15--mission-authoring-format"></a>

Missions are **data** ([D59](decisions.md)) — a parameterized scenario + an objective set. *Where
that data lives* is open.

| Option | Upside | Cost / risk |
|---|---|---|
| **Rust scenario builders** (like [`sim-runner`](../sim-runner/src/main.rs) today) | Type-safe, no parser, zero new deps; the pattern already exists | Every mission edit is a recompile; designers need Rust; missions ship in the binary |
| **External data file** (RON / Lua) | Hot-reloadable, designer-editable, fits the dev-workflow scripting lane ([`roadmap.md`](roadmap.md)); missions become content, not code | A schema + loader to build and validate; must stay deterministic (no float leakage from the data into the sim — invariant #1) |

**Why it matters:** missions are the campaign's *content volume* — the thing we'll author the most
of. A recompile-per-mission loop throttles that; a data format is the primary mitigation for Rust's
weak engine hot-reload ([D10](decisions.md) tradeoff, [`roadmap.md`](roadmap.md) dev workflow).

**Current lean:** external data file (RON for a first pass — serde-native, no scripting VM), so
mission design iterates without a recompile. Validate the loader rejects any non-fixed-point input
so authored data can't smuggle a float into the sim.

---

## Q16 — Campaign narrative depth: light framing or an authored arc? <a id="q16--narrative-depth"></a>

The Operations hub ships with **light briefings** (who/where/why per node — [D59](decisions.md)).
How far past that to go is open.

- **Light framing** — short briefings + connective text; the hub is a mission delivery system, not
  a story. Cheapest; keeps focus on the loop.
- **Authored arc** — a Halo-style throughline with characters, set-piece reveals, scripted beats.
  Stronger identity and retention; the most expensive content per hour and needs writing/VO/cutscene
  pipeline none of which exists.

**Why it matters:** narrative is a retention lever but a deep cost sink, and it's easy to over-invest
before the loop is proven fun. The hub structure ([D59](decisions.md)) supports growing from the
former into the latter **without restructuring**, so this can stay deferred safely.

**Current lean:** light framing for the first shippable campaign; revisit an authored arc once the
core loop and difficulty curve are validated by play.
