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

The tank-embodiment plan ([D55](decisions.md), [`tank-embodiment-plan.md`](plans/tank-embodiment-plan.md))
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

## Q15 — Mission authoring format: Rust builders or external data files? — RESOLVED ([D76](decisions.md)) <a id="q15--mission-authoring-format"></a>

> **Resolved by [D76](decisions.md):** **external RON data files** (the standing lean), behind a
> **host-side loader in `engine`** that drives a new **serde-free `ScenarioBuilder` in `core`** — the
> same `core`-primitive / `engine`-content split the objective system already uses
> ([D59](decisions.md)). `core` keeps **no serde dependency** (invariant #2). The loader is the
> **float airlock**: every numeric field parses as an integer → `Fixed` (no `f32`/`f64` path into the
> sim), with a `deny_unknown_fields` range-validated schema that fails loudly at load, so a mission
> file **cannot** smuggle a float into the sim (invariant #1) or add checksum surface (only the
> seeded `Sim` enters the per-tick checksum, exactly as a hand-written seeder does — invariant #7).
> **Battlefields** factor out into reusable `*.map.ron` files a mission references (one map → many
> missions). Lua/scripting is the documented **second pass**, deferred until a scenario genuinely
> needs control flow ([Q16](#q16--narrative-depth)). Build sequencing:
> [`content-tooling-plan.md`](plans/content-tooling-plan.md).

Missions are **data** ([D59](decisions.md)) — a parameterized scenario + an objective set. *Where
that data lives* was the open fork; the chosen option (external data file) is the second row below.

| Option | Upside | Cost / risk |
|---|---|---|
| **Rust scenario builders** (like [`sim-runner`](../sim-runner/src/main.rs) today) | Type-safe, no parser, zero new deps; the pattern already exists | Every mission edit is a recompile; designers need Rust; missions ship in the binary |
| **External data file** (RON / Lua) ← **chosen (RON), [D76](decisions.md)** | Hot-reloadable, designer-editable, fits the dev-workflow scripting lane ([`roadmap.md`](roadmap.md)); missions become content, not code | A schema + loader to build and validate; must stay deterministic (no float leakage from the data into the sim — invariant #1) |

**Why it matters:** missions are the campaign's *content volume* — the thing we'll author the most
of, and (via `*.map.ron`) the gate on *extensive* battlefields for PvE and PvP. A recompile-per-mission
loop throttles that; a data format is the primary mitigation for Rust's weak engine hot-reload
([D10](decisions.md) tradeoff, [`roadmap.md`](roadmap.md) dev workflow).

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

---

## Q17 — Cross-play input fairness: how does embodied PvP handle a thumb vs. a mouse? <a id="q17--crossplay-input-fairness"></a>

The engine is cross-play-native by construction — the deterministic core runs bit-identically on
phone and PC ([D22](decisions.md), invariants #1/#2), so a touch player and a mouse player in the
**same** match is technically the normal case, not a bolt-on. The *command* layer is naturally fair
across inputs (issuing orders isn't a twitch contest). The **embodied** (FPS) layer is not: a mouse
out-aims a thumb. How embodied **PvP** handles that mismatch is open. (Full framing:
[`positioning-cross-platform.md`](positioning/positioning-cross-platform.md) §4.)

| Option | Upside | Cost / risk |
|---|---|---|
| **Input-based matchmaking** (Warzone-style pools — match by *how you hold the game*, not device) | The cleanest fairness story; mouse-vs-mouse, touch-vs-touch | Splits the matchmaking population; needs enough players per pool to keep queues healthy |
| **Aim assist for the slower input**, tuned per mode | Keeps one shared pool; industry-standard | A perennial balance headache; the "is aim-assist unfair?" argument never ends |
| **Lean on the command-heavy balance** (accept some embodied asymmetry because most of the match is input-fair commanding) | No population split; plays to our structural difference | Leaves a real edge on the table during the embodied beats; may feel unfair in close PvP |

**Why it matters:** get it wrong and cross-play PvP feels rigged to whoever lost the aim duel — a
direct hit to pillar 4 (*the cost must always feel fair*) extended to input. Designing it in up front
is far cheaper than patching it after it has already alienated a platform's players (the way it bit
Destiny and others). **Crucially, this blocks nothing now:** the first shippable product is
single-player PvE ([D58](decisions.md)), where there's no opponent to be unfair to and cross-play is
pure upside. The question only switches on when **embodied PvP** does (Phase 3 netcode / the PvP
fast-follow).

**Current lean:** **input-based matchmaking** for embodied PvP (the cleanest fairness model), with the
command-heavy balance as a natural cushion and aim assist held in reserve for casual modes. Defer the
lock until embodied PvP is actually being built — but decide it *before* that, not after. Tracked as
build item **XP-2** ([`roadmap.md`](roadmap.md)).

---

## Q18 — Inter-unit balance at lethal speed: how do we restore RPS + suppression? — RESOLVED ([D69](decisions.md) + [D70](decisions.md)) <a id="q18--lethal-speed-retune"></a>

**Resolved by the combat-rebalance plan, both workstreams landed.** [D69](decisions.md) (WS-A) re-tuned
the Heavy (HP 280→300, damage 90→100) to restore the range-dependent Rifleman/Heavy rock-paper-scissors
at lethal speed; [D70](decisions.md) (WS-B) added **area (fire-and-maneuver) suppression** + lowered
`SUPPRESSION_PIN` to 3/8 so concentrated fire pins a cluster before it is wiped, while a lone shooter
never pins. Both were dialed against `sim-runner --metrics`, and the metrics tests now assert the
*intended* properties (reversing the D66 regression-locks). The chosen fork below was **both the stat
re-tune and the suppression rework** — exactly the lean.

[D66](decisions.md) scaled damage ×5 for modern lethality (~1.5 s rifle TTK). Uniform scaling keeps
the D30 DPS *ratios* on paper, but at 1–2-volley kill speed the *emergent* balance collapses: the
equal-cost **Rifleman-vs-Heavy rock-paper-scissors** flattens (rifle mass wins at every range), and
per-*hit* **suppression stops pinning before the kill** (the target dies first). Both are measured
facts now locked in the `--metrics` tests, not predictions.

| Fork | For the RPS | For suppression |
|---|---|---|
| **Re-tune unit stats at lethal speed** (buff Heavy durability/close-range punch until it wins close again; iterate against `--metrics`) | Restores the intended matchup with no model change | Doesn't address suppression |
| **Rework suppression to per-near-miss** (fire *near* a unit suppresses it, not only hits that land) | — | Makes suppression the *modern* fire-and-maneuver lever it should be — you're pinned by rounds that *miss*, exactly the doctrine the US-vs-France fantasy wants |
| **Accept faster, deadlier, less-tactical combat** (lethality over depth) | No work; embrace the new feel | No work |

**Why it matters:** suppression + maneuver *is* modern infantry doctrine — leaning into it ([D68](decisions.md))
while it's currently vestigial is a missed pillar. And a flat "rifles always win" roster undercuts the
army-building depth (pillar: *Company of Heroes* economy/composition). **Current lean:** do **both** the
stat re-tune *and* a per-near-miss suppression rework, measured against the harness — but as a focused
balance pass, not blocking the lethality/ammo changes that shipped. Likely bundled with the
[faction rosters](#q19--faction-roster-specifics) (re-tune once, against the real armies).

**Plan:** [`combat-rebalance-plan.md`](plans/combat-rebalance-plan.md) — **both workstreams landed**:
WS-A restored the RPS ([D69](decisions.md): Heavy HP 280→300, damage 90→100); WS-B added area
suppression + `SUPPRESSION_PIN` 1/2→3/8 ([D70](decisions.md)). Harness-confirmed; question closed.

---

## Q19 — Faction roster specifics: how asymmetric is US Army vs French Army? — RESOLVED ([D71](decisions.md)) <a id="q19--faction-roster-specifics"></a>

> **Resolved by [D71](decisions.md):** **soft asymmetry**, with the tilt confined to **logistics rhythm**
> (magazine / reload / reserve) — combat-power axes (damage, cadence, range, HP, penetration) stay shared,
> because the equal-cost mass trade is a Lanchester square-law snowball that no gun-stat tilt survives. US =
> deep-mag/long-reload; FR = shallow-mag/snappy-reload; tank identity is turret-slew only; Neutral ==
> baseline. Campaign is played US-side with France as OPFOR ([D58](decisions.md)). Verified swap-invariant
> against `sim-runner --metrics`. **Residual specifics still tracked in
> [`factions-plan.md`](plans/factions-plan.md):** per-faction **gunsmith pools** (WS-E, layers on
> [D60](decisions.md)) and army-tilting the pre-placed scenario starting troops (WS-C/WS-D follow-up). The
> fork below is retained for the record.

[D68](decisions.md) locks the **direction** — asymmetric factions modelled on real modern armies, US
vs FR first, fairness-bounded — and [`factions.md`](factions.md) holds the design. The *specifics* are
open:

| Fork | Upside | Risk |
|---|---|---|
| **Reskin parity** (same roster, different art/names/voicelines) | Trivially fair; cheap; ships fast | Barely a "faction" — wastes the fantasy |
| **Soft asymmetry** (shared archetypes, per-faction stat/ability *tilts* within a fairness band) | Distinct feel, tractable balance, cross-play-safe | Needs a real balance budget; the band is a judgement call |
| **Hard asymmetry** (genuinely different rosters/mechanics per army, à la StarCraft races) | Deepest identity; highest replay | Balance + cross-play fairness ([Q17](#q17--crossplay-input-fairness)) get much harder; large build |

Sub-questions: which real platforms map to which `UnitKind` slots (M1 Abrams vs Leclerc; M4/HK416 vs
FAMAS/HK416F)? How does a faction roster interact with the horizontal **gunsmith** ([D60](decisions.md))
and the **PvE campaign** ([D58](decisions.md): is the campaign US-side, with France as the OPFOR)? Is
faction a **cosmetic** choice or a **strategic** one? **Current lean:** **soft asymmetry** — shared
archetypes (rifleman/heavy/vehicle/support) with per-faction tilts inside a measured fairness band,
campaign played US-side first. Defer the lock until after the [lethal-speed re-tune](#q18--lethal-speed-retune)
(balance the shared archetypes first, *then* tilt them per faction).

**Plan:** [`factions-plan.md`](plans/factions-plan.md) (WS-0 = the rebalance prerequisite; WS-A
identity tag + codecs; WS-B per-faction rosters; C/D/E cosmetics, selection + PvE OPFOR, gunsmith
pools). The asymmetry fork above is the design gate on WS-B.

---

## Q20 — AI-controlled ballistic fire — does a produced/AI tank's gun travel, or stay hitscan? — RESOLVED ([D72](decisions.md): option (ii) — AI tanks also fire traveling projectiles) <a id="q20--ai-controlled-ballistic-fire--does-a-producedai-tanks-gun-travel-or-stay-hitscan"></a>

When a produced (and thus sometimes-AI-controlled) unit carries `muzzle_vel > 0` — the armoured
ballistic Tank of P9's remaining scope in
[`tank-embodiment-plan.md`](plans/tank-embodiment-plan.md) — its shot is ballistic **only while
embodied**. `core::sim`'s `Command::Fire` path calls `projectile::fire_ballistic` only when
`muzzle_vel > 0` and `InputSource::Embodied`; the AI auto-resolver `combat::combat_system`
resolves fire as instant hitscan and never reads `muzzle_vel` (`core/src/combat.rs`), and
aim-bloom (dispersion) is grown only at embodied sites. So the **same gun is hitscan when
AI-driven and ballistic when embodied**.

This fork is currently **dormant**: the only produced `UnitKind::Tank` is deliberately
`muzzle_vel == 0` + unarmoured ([D65](decisions.md)), so neither path fires a projectile. It
manifests the moment P9's ballistic + armoured produced tank lands. Two options:

| Option | For | Against |
|---|---|---|
| **(i) Ballistics embodied-only** — AI tank gun stays hitscan | Literal-executor friendly (invariant #3): AI tanks have no skill at leading targets, which is honest. No new sim writes. | Physically inconsistent: the same barrel is a laser or a cannon depending on who pulls the trigger. Visible in combat once AI tanks fire at range. |
| **(ii) AI tanks also spawn projectiles** — teach `combat_system` to fire `Projectile`s for `muzzle_vel > 0` | Physically consistent: identical gun, identical shell, regardless of driver. Emergent: AI tanks can now be hit by return fire *in flight*. | Determinism-sensitive: new sim writes (projectile spawns from the AI resolver) must keep the lockstep checksum matrix green (invariant #7). More new code in a hot path. |

Note that **armour facing is already consistent across all three fire paths** (AI hitscan, embodied
hitscan, shell impact) — the `facing_penetration_multiplier` is applied at damage resolution in all
cases (P4, `dc8ce4e`). This fork is strictly about **projectile travel**, not damage/armour.

Resolve this **before** P9's produced-tank ballistic gun ships (cross-link:
[`tank-embodiment-plan.md`](plans/tank-embodiment-plan.md) §9). The relevant invariants are #3
(literal-executor AI — skill lives in the *order/stance vocabulary*, not autonomous unit cleverness)
and #7 (lockstep checksum matrix must stay green across the arch matrix).

**Resolved — [D72](decisions.md): option (ii).** A produced tank's gun fires a real traveling
projectile whether AI-driven or embodied (`combat::combat_system` spawns a `Projectile` for
`muzzle_vel > 0` via `projectile::fire_ballistic`, hitscan only for `muzzle_vel == 0`). Physically
consistent and emergent without making the AI a strategist (it fires along current aim, does not lead);
the new sim writes are index-ordered, fixed-point, checksum-folded, and must keep the arch matrix green.

## Q21 — Campaign replay tier → commander aggression: how do the 4 progression tiers map onto the 3 commander tiers? <a id="q21--replay-tier-to-commander-tier"></a>

There are **two distinct `Difficulty` enums**, and nothing yet bridges them:

- `core::campaign::Difficulty` — the **4-tier progression/replay** coordinate
  (Recruit / Regular / Veteran / Elite) the Operations hub records when you clear or replay a node.
- `core::mission_tuning::Difficulty` — the **3-tier commander-aggression** knob
  (Recruit / Veteran / Elite) that scales the seeded planner's reserve / unit-mix / cadence /
  aggression ([D39](decisions.md) honest AI, never omniscient — invariant #6).

The new `engine::mission_registry` (PvE WS-B) launches each mission at the **briefing's authored
commander tier** and exposes `LaunchedMission::commander_difficulty` as the plug point, but it does
**not** yet scale the commander by the player's chosen *replay* tier. So replay-at-higher-difficulty
currently records a best-tier badge while the **actual fight is unchanged** — the progression
coordinate is inert until a mapping consumes it.

| Option | For | Against |
|---|---|---|
| **(i) Collapse 4→3** — map Recruit→Recruit, {Regular,Veteran}→Veteran, Elite→Elite | Simplest; reuses the shipped 3-tier commander knob unchanged | Two progression tiers feel identical in-fight — replay reward is cosmetic for one step |
| **(ii) Widen commander to 4 tiers** — add a `Regular` aggression band | Each replay tier is a distinct fight; cleanest player-facing meaning | New sim-tuning surface to balance + keep checksum-folded; re-measures the [D30](decisions.md) bands |
| **(iii) Layer modifiers, not just the tier** — replay tier also tightens WS-E scenario modifiers (force size / cadence / fog), commander tier stays 3 | Richer difficulty without inflating the AGGRESSION enum; modifiers already exist (WS-E) | More knobs interacting; must stay "reshape the situation, never the balance numbers" ([D30](decisions.md)) |

Resolve before campaign replay ships as a player-facing feature. Relevant invariants: #1 (any
tier→tuning mapping stays fixed-point + checksum-folded), #3/#6 (the commander stays honest, never
omniscient, at every tier). Cross-link: [`pve-campaign-plan.md`](plans/pve-campaign-plan.md) WS-B/WS-E.

---

## Q22 — Terrain representation: a built-in map-id registry or embedded terrain data? — RESOLVED ([D77](decisions.md)) <a id="q22--terrain-representation"></a>

> **Resolved by [D77](decisions.md):** option **(iii) content-addressed terrain** (the lean). Terrain
> becomes authorable/generatable data — a map carries its cover/LoS grid, identified by a **deterministic
> content hash of the canonical fixed-point bytes**; `MapId` widens from a `u16` registry index to that
> digest. `persist`/reconnect ([D28](decisions.md)) keeps serializing **only the id, not the grid** (the
> resuming/joining peer rebuilds it from the shared content set [D76](decisions.md) already introduces),
> so reconnect snapshots stay lean **and** terrain is authorable. A missing/mismatched id is a hard
> match-setup failure, never a silent desync; the hash is arch-independent (integer bytes only —
> invariants #1/#7). This **unblocks CT-G's terrain half**.

Surfaced by the procedural map generator ([`content-tooling-plan.md`](plans/content-tooling-plan.md)
CT-G): generating *novel battlefields* wants novel **terrain**, but terrain is the one piece of a map
that is **not yet data**. `core::terrain` is a `MapId` (`u16`) **registry** — `Terrain::from_map_id`
reconstructs the cover/LoS grid from an id, and the reconnect snapshot ([D28](decisions.md)) serializes
**only the map-id, not the grid**. So a generated `*.map.ron` can place cover-props / control-points /
spawns over an *existing* terrain id, but cannot define a *new* terrain layout without either new code
or a format change. (Cover, control points, and spawns are already pure placement data —
[D76](decisions.md); this question is **only** about the terrain heightfield/cover grid itself.)

| Option | For | Against |
|---|---|---|
| **(i) Registry of built-in ids** — each new terrain is a new `Terrain::from_map_id` arm | No `persist` change; smallest blast radius; reuses what ships today | Every terrain is a **recompile** and ships in the binary — defeats the data-file goal *for terrain* (the generator can't make new ground) |
| **(ii) Embed the grid in the map data** — `MapSpec` carries the fixed-point cover grid; `persist` serializes it **by value** | Terrain becomes authorable/generatable data like everything else; maps are self-contained | Bigger reconnect snapshots; `persist`/reconnect ([D28](decisions.md)) must serialize the whole grid; more determinism surface to keep folded (invariant #7) |
| **(iii) Content-addressed terrain** — map data carries the grid, `persist` serializes a **content-hash id**, peers rebuild from a shared content set | Reconnect snapshots stay lean (id only, like today) **and** terrain is authorable; rides the content set [D76](decisions.md) already introduces | Needs a content-distribution assumption (both peers hold the terrain file); a missing/mismatched id is a hard fail to handle |

**Why it matters:** it's the gate on *novel* generated terrain (vs. reusing a handful of base fields),
and it touches reconnect/persist determinism ([D28](decisions.md), invariant #7) and the fixed-point
airlock (invariant #1) — so it can't be picked casually. It blocks **nothing else**: CT-G's placement
half (props/CP/spawns over an existing terrain id) ships without resolving this.

**Current lean:** **(iii) content-addressed** — it keeps reconnect snapshots lean *and* makes terrain
authorable, and the shared content set is already a thing [D76](decisions.md)'s data-file model
introduces. Decide **before CT-G's terrain half ships**; the placement half needs none of it.
Cross-link: [`content-tooling-plan.md`](plans/content-tooling-plan.md) CT-G, [D28](decisions.md),
[D76](decisions.md).

---

## Q23 — Sim-side terrain elevation: does the simulation get a height layer? <a id="q23--sim-elevation"></a>

`core::terrain` is **flat**: a `GRID×GRID` grid of `Cover` (None/Light/Heavy) with **no height**.
Real-world map ingestion ([`maps.md`](maps.md)) pulls a real DEM, but that float elevation feeds
**only the render mesh** (`tools/maps/terrain_mesh.py`) — the sim treats every battlefield as a
plane. For a game whose worked example is *Pointe du Hoc* (a cliff assault), "the sim doesn't know
the cliff exists" is a real gap: no high-ground sightlines, no slope cost, no defilade.

Whatever lands must be **fixed-point** (invariant #1) — a real DEM is float metres, so it is
quantised to integer height-per-cell at bake time, never sampled as float in the sim.

| Option | For | Against |
|---|---|---|
| **(i) Stay flat — elevation is render-only** | Zero sim change; keeps the grid + LoS DDA as-is; ships today | The map's defining feature (the cliff, the ridge) is cosmetic; no tactical high ground |
| **(ii) Fixed-point height per cell, folded into LoS + flow field** | High ground blocks/opens sightlines and slows movement — real terrain tactics; content-addressable like the cover grid ([D77](decisions.md)) | New sim state + a 3D-ish LoS (height into the supercover DDA); slope→cost in `flow_field`; more determinism surface |
| **(iii) Coarse height *tiers*** (a few discrete levels, e.g. low/mid/high) | Most of the tactical payoff (defilade, high ground) at a fraction of (ii)'s complexity; still integer | An authoring/quantisation choice (how many tiers, thresholds); still touches LoS + cost |

**Current lean:** **(iii) coarse tiers** first — the cliff/ridge tactics without a full continuous
heightfield — with (ii) reserved if playtests want finer relief. Blocks nothing shipping now
(ingest already produces the render height); decide before elevation is claimed as a *gameplay*
feature. Ties to [Q24](#q24--terrain-traversal-cost). Cross-link: [`maps.md`](maps.md),
[`architecture.md`](architecture.md), [D77](decisions.md).

---

## Q24 — Terrain traversal cost / true impassability: where does "you can't walk here" live? <a id="q24--terrain-traversal-cost"></a>

`Cover` has only None/Light/Heavy — there is **no "impassable"**. Baked maps ([`maps.md`](maps.md))
map water and cliff edges to `Cover::Heavy` (a wall: blocks sight, and units path around it *because*
combat treats it as a wall) — but that is a stand-in. The flow field (`core::flow_field`) is
Phase-1 obstacle-free; its own docs note obstacles are "the Phase-2 generalisation, by raising
per-cell entry costs" — not yet implemented. So "impassable water", "slow mud", and "a wall you
route around" all collapse onto one `Heavy` today.

| Option | For | Against |
|---|---|---|
| **(i) Keep `Heavy` as the de-facto wall** | No new state; already works for the coarse case | Can't express *slow* (marsh) vs *blocked* (water) vs *cover* (wall); overloads one enum |
| **(ii) Add a per-cell entry-cost layer to `flow_field`** (the noted Phase-2 generalisation) | Real traversal cost (mud slows, water blocks) independent of cover; the pathing home for it | New fixed-point grid folded into map content ([D77](decisions.md)-style hash) + the airlock; more determinism surface |
| **(iii) Derive cost from cover + elevation ([Q23](#q23--sim-elevation))** | One source of truth; steep slope / heavy cover imply cost — fewer authored layers | Couples pathing to cover/height; less direct authorial control; waits on Q23 |

**Current lean:** **(ii) an explicit per-cell cost layer** in `flow_field` (the generalisation
already anticipated), authored/baked alongside the cover grid and content-addressed the same way.
Decide with [Q23](#q23--sim-elevation) (slope is a cost source). Cross-link: [`maps.md`](maps.md),
[`architecture.md`](architecture.md), [D77](decisions.md).

---

## Q25 — Destructible terrain: can walls and cover be destroyed mid-match? <a id="q25--destructible-terrain"></a>

Terrain is **static**: set once at scenario build, deliberately **not folded into the per-tick
checksum** ([D28](decisions.md)/[D77](decisions.md)), and content-addressed by the hash of its
**initial** grid. A destructible battlefield (blow a hole in a wall, crater the ground, collapse a
building) is a common expectation — but it collides head-on with that design: destructible terrain is
**mutable per-tick state**, so it **must** enter the checksum (invariant #7) or lockstep desyncs
silently, and the content-hash id ([D77](decisions.md)) would then identify only the *starting*
state.

| Option | For | Against |
|---|---|---|
| **(i) Fully static (today)** | Terrain stays out of the checksum; lean snapshots; content-hash is the whole story | No destruction — a real expressive/immersion gap; "that wall should have blown up" |
| **(ii) Destructible via *entity* cover-props only** ([D50](decisions.md) crate/tree/rock/barricade/turret) | Props are **already** ECS entities in the checksum — destroying them is *already* deterministic; no terrain-grid change; "buildings" become destructible prop clusters | Grid-baked `Heavy` walls (from OSM footprints) stay indestructible; two classes of "wall" (prop vs grid) to reconcile |
| **(iii) Mutable terrain overlay folded into the checksum** | True grid destruction (breach any wall/cover); initial grid still content-addressed, per-tick **deltas** ride the fold | New per-tick fold surface + delta serialization in `persist`; the biggest determinism/perf surface of the three; re-opens D28's lean-snapshot posture |

**Current lean:** **(ii) entity-prop destruction** — it is the cheapest *and* already-deterministic
path (props are in the ECS/checksum today), so "destructible buildings" become destructible prop
clusters over a static grid, with (iii) held for genuine grid-breaching if the game demands it. This
is exactly the kind of thing to verify in the map **debug mode** ([`maps.md`](maps.md) §Diagnostics:
the `MapInspect` scene + cover overlay). Relevant invariants: #1 (any mutable terrain stays
fixed-point), #4 (render never mutates sim terrain), #7 (mutable terrain **must** be checksum-folded).
Cross-link: [`maps.md`](maps.md), [D28](decisions.md), [D50](decisions.md), [D77](decisions.md).
