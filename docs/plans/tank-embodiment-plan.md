# Tank embodiment plan — the War Thunder-flavoured tank

**Status: IN PROGRESS (D55) — P1–P8 landed and tested (see §6); P9 PARTIAL.** The
`UnitKind::Tank` archetype + economy-stats half of P9 shipped via [D65](../decisions.md), but as
an *unarmoured, hitscan* production unit (`penetration == 0`, `muzzle_vel == 0`, no `Armor`).
What remains of P9: the produced tank's **armour block + ballistic gun**, and the
**sniper/zoom gun-sight view** (not built anywhere in render/engine yet). Landing the ballistic
gun will expose the latent AI-fire design fork documented in
[Q20](../open-questions.md#q20--ai-controlled-ballistic-fire--does-a-producedai-tanks-gun-travel-or-stay-hitscan)
— resolve it before the produced-tank ballistic gun ships.
Phasing for turning the embodied tank from "infantry-FPS-in-a-tank-
shaped-token" into a real **vehicle**: independent hull + turret control, a reticle that
blooms when you move and settles when you hold, and a penetration-vs-armour-facing combat
model that applies to **every unit**, not just the one you're driving. The reference feel is
**War Thunder (sim)**; the hard constraint is that every gram of it stays **fixed-point and
lockstep-identical** (invariants #1, #7).

> Read first: `game-design.md` §5 (embodiment), `architecture.md` (determinism), `combat.rs`
> (the resolver this plan rewires), `decisions.md` D51 (the infantry FPS scheme this extends).

---

## 1. The gap, stated honestly

What exists today (D50–D52): `UnitKind::Heavy` renders as a **tank mesh**, and embodiment
gives it the same scheme as a rifleman — a left move-stick, a drag-look aim cone, Fire /
Crouch / Reload (`combat::resolve_fire`, D51). The "tank" is a costume on an infantryman.

What a tank game *is*, mechanically, and where each piece lives here:

| War Thunder trait | What it demands | Where it lands |
|---|---|---|
| **Hull ≠ turret** | two independent headings per vehicle | new sim state (none today) |
| **Slow turret traverse** | angular-velocity-limited slew toward aim | `systems` + new `trig` math |
| **Gun dispersion / aim time** | reticle blooms on the move, settles at rest | new `Weapon` fields + fire math |
| **Penetration vs armour angle** | shot may *bounce* or *fail to pen* by facing | rewrite of the damage step |
| **Shell types (AP/APHE/HE)** | pen/damage trade per shell, selectable | `Weapon` + a select command |
| **Shell flight: travel time + drop + leading** | a real fixed-point projectile, impact-resolved | **core phase** (D55 update) — §6, §6a |
| **Module/crew damage** | tracks, breech, ammo rack, optics | *deferred* — see §7 |
| **Sniper/zoom gun-sight** | a second, zoomed aim view | render + HUD |

The **load-bearing, can't-fake-it** pieces are *hull≠turret heading*, *armour facing*, and — per
the D55 update — *ballistic shell flight* (travel time is War Thunder's soul; committing to it now
means armour facing resolves at **projectile impact**, with no hitscan-then-projectile rework).

> **Design stance (D55 update):** the tank is the project's **deep** embodiment, deliberately
> richer than D51's quick infantry (move/aim/crouch/reload). That asymmetry is *intended* — the
> tank is the unit you commit to and master. The pillar tension it creates (a sticky, rewarding
> embodiment vs. the "cost is time away" rule, §5/§6 of `game-design.md`) is held in check by the
> existing levers — **going-dark blindness** and the **unit-economy precious-unit cost** — not by
> making the tank shallow. If playtest shows tanks over-reward camping, the dial is the going-dark
> cost, not the tank's depth.

---

## 2. Non-negotiables this must not break

- **#1 No floats in the sim.** Turret angles are **binary-radian `Angle`** (`trig.rs`, full
  turn = `1<<16`, wrapping is a mask). Bearings come from a new fixed-point **`atan2`**;
  penetration curves are **integer/`Fixed` LUTs**, never `powf`/`expf`. Shell "drop", when it
  lands, is fixed-point kinematics, not `f32` physics.
- **#3 Literal-executor AI.** An AI-driven tank keeps its hull/turret pointed per its *order +
  stance* (turret tracks the same target `acquire_target` already picks). It never gains
  autonomous aim micro. The new aim *skill* is an **embodied-only** advantage, exactly as
  ammo/crouch are (D51).
- **#5 Embodiment is an input swap.** No tank "vehicle object." The hull/turret headings are
  plain components on the same ECS entity; `InputSource::Embodied` just routes the sticks to
  them instead of the AI slew. Death still ejects to command — no vehicle respawn.
- **#7 Lockstep matrix.** Armour facing changes **core combat resolution for all units**, so it
  is the highest-risk determinism change in the project to date. Every step ships with the
  cross-platform per-tick checksum coverage green; build the slice behind a neutral default so
  the checksum stream only moves when armoured units are actually present.

---

## 3. The combat-model decision: all-unit armour facing, neutral by default

The user chose **all-unit armour facing** over an embodied-only bonus. The trap is that a naive
rewrite re-balances every infantry fight and breaks every combat test. The design that satisfies
"all units" *and* keeps the existing balance intact:

```
damage_applied = base_damage
              × cover_multiplier            (unchanged, terrain)
              × facing_penetration_multiplier(NEW)
```

`facing_penetration_multiplier` derives from **(a)** the incoming shot's `penetration` vs **(b)**
the defender's `armor` on the *facet the shot hits* (front / side / rear), where the facet is
chosen by the angle between the **shot direction** and the **defender's hull heading**:

- **Unarmoured unit** (`armor = {0,0,0}`, the **default** for Rifleman/Heavy/buildings):
  penetration always ≥ 0 armour → multiplier **1.0** → *identical to today*. Existing tests and
  balance are untouched. This is what makes "all-unit" safe.
- **Armoured unit** (the Tank): a frontal shot meets the thickest facet and may **bounce**
  (multiplier 0, a hard zero — the War Thunder "non-penetration") or get **reduced**; a flank or
  rear shot meets thin armour and lands full or amplified damage. *Angle the hull at the enemy;
  flank to kill.* This is the new strategic texture, and it applies whether the tank is
  AI-driven or embodied.

Facet selection uses the existing dot-product machinery (no new transcendental): project the
**projectile's travel direction at impact** onto the defender's hull heading; the sign and
magnitude bucket it into front/side/rear by fixed `cos` thresholds — the **same squared-cosine
trick** `resolve_fire`'s aim cone already uses. Resolving at *impact* (not at fire time) is why
ballistic flight is promoted to a core phase below: a shell fired at a tank's front that catches
it mid-turn hits the side it has rotated into — emergent, exactly the War Thunder moment.

**Honest scope of "all-unit."** The facet model is only *meaningful* for entities whose facing is
sim-authoritative — i.e. **vehicles with a maintained `hull_heading`**. Infantry facing is
render-derived from velocity today and they default to `armor = {0,0,0}`, so the multiplier is a
constant `1.0` for them: the damage *path* is unified across all units (one resolver, no
embodied-only fork — the "all-unit" the user asked for), but armour *texture* only appears where a
hull heading is actually driven. Giving infantry a real rear-vulnerability later is then a pure
data change (nonzero `armor` + maintaining their `hull_heading`), not a new code path.

A new **dedicated `UnitKind::Tank`** carries the real armour block (so we don't overload the
`Heavy` infantry-bruiser balance that D30 tuned). `Heavy`'s render mapping to the tank mesh stays
as-is for now; the playable embodied tank is the new kind.

---

## 4. New sim state (all `Fixed`/`Angle`/integer — checksummed)

```
World (new dense arrays, indexed by entity)
  hull_heading : Angle      // way the chassis points; movement turns it (rate-limited)
  turret_yaw   : Angle      // independent gun bearing, ABSOLUTE world angle (HUD shows yaw − hull)
  hull_speed   : Fixed      // current forward speed; accel/brake toward target (tank inertia)
Weapon (new fields)
  penetration  : Fixed      // vs armour facets
  shell        : ShellKind  // AP | APHE | HE  (enum, repr-stable)
  dispersion   : Fixed      // current aim bloom (shrinks at rest, grows moving/traversing)
  turret_speed : u16        // max turret slew, angle-units/tick (0 = fixed mount / infantry)
  muzzle_vel   : Fixed      // shell speed; 0 = hitscan (infantry), >0 = ballistic projectile
Armor (new component, default all-zero = unarmoured)
  front, side, rear : Fixed
Projectile pool (NOT main entity arrays — a bounded ring, checksummed)
  pos2d, vel2d : Vec2       // ground-plane flight
  height, vz   : Fixed      // localized verticality for DROP (see §6a)
  owner, faction, penetration, shell, damage : …   // carried from the firing weapon
```

`turret_speed == 0` ⇒ "no turret" (locked to hull) and `muzzle_vel == 0` ⇒ "hitscan, no flight":
**every infantry unit keeps exactly today's behaviour**. Like `mag_size == 0` for the magazine
(D51), each new system is **opt-in by a zero default**, so non-tank entities cost nothing and move
the checksum by nothing. The tank gun itself reuses D51's magazine as **`mag_size = 1` + a long
reload** (one shell, then re-load); an autoloader is just a small `mag_size` — no new reload code.
Hull motion gains **inertia** (`hull_speed` accelerates/brakes toward the stick rather than
snapping to `MOVE_SPEED`) — that weight is core tank feel and is new work, not a free lump.

---

## 5. New / changed commands (lockstep stream, host-quantized)

- `DriveHull { entity, dir }` — replaces/extends `Locomote` for vehicles: `dir` turns the **hull
  heading** toward the stick (rate-limited) and drives forward along it. (Infantry keep
  `Locomote`'s instant strafe; tanks turn-then-move — the tank "feel".)
- `AimTurret { entity, dir }` — slews `turret_yaw` toward the look-stick bearing at
  `turret_speed`/tick. Embodied-only intent; AI sets the same field toward its target.
- `SelectShell { entity, shell }` — cycles AP/APHE/HE; changes `penetration`/`damage`/splash for
  the *next* shot. A no-op mid-reload.
- `Fire` (existing) **spawns a projectile** (when `muzzle_vel > 0`) along `turret_yaw`, perturbed
  by current `dispersion`. **Skill-honest dispersion (refinement over the first draft):** a
  *fully settled* gun fires dead-on `turret_yaw` — **zero scatter** — and only an unsettled gun
  (just fired / moving / mid-traverse) scatters, the offset scaling with `dispersion`. So mastery
  = wait for the reticle to settle, then the shot is exact; it never feels like an RNG bullet
  robbed a perfect aim. The bounded scatter still uses `combat`'s reserved `&mut Rng` (integer
  draw), so it's deterministic in lockstep.

All payloads stay `Copy` fixed-point/handle data — no float crosses the boundary (#1).

---

## 6. Phasing — each phase is a green, committed, testable slice

```
P1  trig: atan2 + rotate_toward (turret slew math)      ── DONE (committed, fully tested)
P2  hull_heading + turret_yaw + hull inertia + AimTurret/DriveHull + AI slew   ── DONE (c1e4059)
P3  ballistic projectile pool: flight + drop + impact detection (muzzle_vel > 0)   ── DONE (4fbe31b)
P4  Armor + Weapon.penetration + facing multiplier, resolved AT IMPACT (ALL-UNIT rewrite)   ── DONE (dc8ce4e)
P5  dispersion / aim-time bloom: settle-to-center scatter on Fire   ── DONE
P6  ShellKind AP/APHE/HE + SelectShell + per-shell pen/damage/splash   ── DONE
P7  render: turret mesh node + shortest-arc angle interp + tracer/projectile draw ── DONE
    (turret mesh + hull/turret yaw in snapshot, shortest-arc interp; shell tracers via a
    `tracer` mesh extrapolated from the projectile snapshot, embodied pass) (inv #4)
P8  HUD: hull-relative turret indicator, dispersion reticle, LEAD pip, shell selector,
    reload ring   ── DONE
P9  tank UnitKind archetype + economy stats + armour block + ballistic gun + sniper/zoom view
    ── DONE (D65 archetype; wave-1 W1 armour block, W4 ballistic gun + AI projectile fire per
    D72/Q20, W2 sniper/zoom gun-sight view)
─────────────────────────────────────────────────────────────────────────
DEFERRED (own decision later): module+crew damage (tracks/breech/ammo-rack),
commander's-optics third view.
```

P2–P4 are the spine: heading, then the projectile, then armour facing resolved on its impact (this
order is why ballistics-first avoids hitscan-then-projectile rework). P5–P6 are the skill ceiling.
P7–P9 make it legible and playable. Each lands with unit tests in the same commit; **P3 and P4
must keep `determinism.yml`'s arch matrix green** (invariant #7 — projectiles and the damage
rewrite are both checksummed sim state). High-blast-radius phases (P2, P3, P4, P6) run through
`/safe-edit`. **P1–P9 done.** P7 landed early (turret mesh node + shortest-arc yaw interp, then shell
tracers) — a pure render seam, no dependency on P5/P6. P5 (dispersion bloom — settle-to-zero
scatter), P6 (ShellKind AP/APHE/HE + SelectShell), and P8 (hull-relative turret indicator,
dispersion reticle, LEAD pip, shell selector, reload ring) followed. **P9 landed in a 3-worker
wave-1** (armour block, ballistic gun + AI projectile fire per D72, sniper/zoom view) — see §9.

---

## 6a. Ballistic flight — travel time + drop in a 2D sim (the new core piece)

The sim is 2D top-down (`Vec2` ground plane); War Thunder *drop* is vertical. Rather than make the
whole world 3D (a huge, invasive change to `pos`/spatial/everything), **verticality is localized to
the projectile**: units stay 2D at a known per-kind hull *height*; a shell carries its own
`height` + vertical velocity `vz`, and only the projectile integrates gravity.

```
each tick, per live projectile (fixed-point, index-ordered → peer-identical):
  pos2d += vel2d                       // ground-plane travel  (finite muzzle_vel ⇒ travel time)
  height += vz ;  vz -= GRAVITY        // the arc / drop
  on crossing a unit's (x,y) footprint at a height within its hull  → IMPACT:
      facet = facing(vel2d vs defender.hull_heading)      // §3, resolved here
      apply penetration vs armour[facet] → cover-mitigated damage   // the unified resolver
  despawn on impact, on leaving the map, or on a max-lifetime cap
```

This gives the two things that *make* it a tank game — **leading a moving target** (finite travel
time) and **arcing fire** (drop) — while keeping units, terrain, fog, and the spatial index 2D.
The projectile pool is a **bounded ring** (a hard cap, `log()`-ed if hit) so a 200-unit firefight
can't unbounded-spawn shells against the Phase-3 thermal budget. Whether units ever need a true
z-axis (multi-storey cover, real elevation) is the remaining sub-fork, parked in
[Q13](../open-questions.md) — the projectile-local height model is the answer until something needs
more.

---

## 7. What's explicitly deferred and why

- **Module + crew damage.** Tracks-out/breech-broken/ammo-rack-detonation is the deepest War
  Thunder layer. It multiplies sim state per vehicle and needs its own balance pass; out of scope
  until the facing model + ballistics have shipped and proven fun.
- **A true world z-axis** (unit elevation, multi-storey cover). The projectile-local height (§6a)
  covers shell drop without it; promote only if level design demands real verticality (Q13).

---

## 8. Test obligations (the floor, per CLAUDE.md)

- **P1** ✅ `atan2` exact on cardinals/diagonals + round-trips sin/cos under tolerance;
  `rotate_toward` never overshoots, takes the short way around the wrap seam, is monotone. Pure,
  green dev+release (12 tests, committed).
- **P2** turret slews toward a target at exactly `turret_speed`/tick; hull turn-rate-limited and
  inertia accelerates/brakes (no snap); an AI tank tracks its `acquire_target`; embodied routing
  verified via the `InputSource` seam.
- **P3** a projectile travels at `muzzle_vel` (a near target is hit later than a far gun would
  hitscan); drop arcs `height` down by `GRAVITY`; impact detection picks the right unit/tick;
  the pool ring-caps and never leaks; **two `Sim` instances on one seed agree** (cross-arch).
- **P4** ✅ the **load-bearing battery**: unarmoured unit takes *identical* damage to today (golden
  test vs current numbers); frontal shot on a tank bounces; flank/rear pens; facet correct at the
  front/side and side/rear boundaries; **a shot that lands mid-turn hits the rotated-in facet**;
  an armoured duel checksums identically — added to the cross-arch matrix. Green dev+release (301
  core tests, 8 new), 2-peer lockstep agrees over 300 ticks (`dc8ce4e`).
- **P5** dispersion grows under motion/traverse, shrinks at rest, and a **fully-settled gun fires
  dead-on (zero scatter)**; the bounded scatter is bit-identical across two `Sim`s on one seed.
- **P7/P8** pure render/HUD seams tested like `interpolate_instances` / `map_input_commands`
  (shortest-arc angle interp across the seam, lead-pip + reticle-from-dispersion math) — platform
  glue excepted, stated explicitly where skipped.

---

## 9. Status & next step

**P1–P8 are committed and green.** P1 (`trig::atan2`/`rotate_toward`, `a5812fb`); **P2** hull/turret
heading + inertia + `AimTurret`/`DriveHull` + AI `heading_system` (`c1e4059`); **P3** the
fixed-point ballistic projectile pool — travel time + drop via projectile-local height, impact
applies the existing cover-mitigated damage, embodied-only (`4fbe31b`); **P4** the all-unit armour
rewrite — `Armor{front,side,rear}` + `Weapon.penetration` + a shared `facing_penetration_multiplier`
applied at all three damage sites (AI hitscan, embodied hitscan, shell impact), resolved **at
impact** (`dc8ce4e`). Built via a fan-out workflow (build → 3-lens adversarial review → fix); the P2
review caught a real `WIRE_VERSION` desync-on-skew bug, and the P4 review caught a latent i32
overflow in the facet compare (now i64) + a fold-coverage gap (now perturbing every armour facet)
before commit. Verified: **301 core tests** green dev+release, the **2-peer lockstep runner agrees
over 300 ticks** with no desync, `WIRE_VERSION` 6 (unchanged — P4 adds no command),
`SNAPSHOT_VERSION` 5→6. **P5** (dispersion bloom — settle-to-zero scatter, bounded draw from the
`combat` `&mut Rng`), **P6** (ShellKind AP/APHE/HE + SelectShell + per-shell pen/damage/splash),
and **P8** (HUD: hull-relative turret indicator, dispersion reticle, LEAD pip, shell selector,
reload ring) followed — each committed and tested.

**P9 is done** (a 3-worker wave-1). The `UnitKind::Tank` archetype + economy-stats half landed via
[D65](../decisions.md) as an *unarmoured, hitscan* production unit; the remaining scope then landed:
the produced tank's **armour block** (`Armor{front:40,side:16,rear:8}` via `economy::unit_armor`,
golden + facing tests — wave-1 W1), its **ballistic gun** (`muzzle_vel:2`, `penetration:18`; the AI
auto-resolver `combat::combat_system` now spawns a traveling `Projectile` for `muzzle_vel > 0` per
[D72](../decisions.md)/[Q20](../open-questions.md), hitscan only for `muzzle_vel == 0`; 2-peer
lockstep agrees over 300 ticks — wave-1 W4), and the **sniper/zoom gun-sight view** (RMB
aim-down-sight narrows the embodied FOV 60°→20° with a scope reticle; render + input seam only, no
sim writes — wave-1 W2; an on-screen touch aim-down-sight button reached mobile parity in wave-2 W6).
The full War-Thunder-capable produced tank this plan specifies now exists, and its infantry **anti-tank
counter** shipped as a new dedicated AT unit ([D73](../decisions.md), wave-2 W8) — so the armoured tank
is no longer immune to a properly-equipped infantry force. The AI-fire fork is now **resolved — [D72](../decisions.md)
([Q20](../open-questions.md#q20--ai-controlled-ballistic-fire--does-a-producedai-tanks-gun-travel-or-stay-hitscan),
option ii):** the produced tank's gun fires a real traveling projectile whether AI-driven or embodied
— `combat::combat_system` spawns a `Projectile` for `muzzle_vel > 0` (hitscan stays the path only for
`muzzle_vel == 0`). The P9 ballistic-gun work builds against that contract and must keep the arch
checksum matrix + 2-peer lockstep runner green (invariant #7).
