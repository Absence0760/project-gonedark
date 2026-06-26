# Tank embodiment plan — the War Thunder-flavoured tank

**Status: PLAN (D55).** Phasing for turning the embodied tank from "infantry-FPS-in-a-tank-
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
| **Module/crew damage** | tracks, breech, ammo rack, optics | *deferred* — see §7 |
| **Shell drop / ballistic flight** | projectile with gravity, lead the target | *deferred* — see §7 |
| **Sniper/zoom gun-sight** | a second, zoomed aim view | render + HUD |

The two **load-bearing, can't-fake-it** pieces are *hull≠turret heading* and *armour facing*.
Everything else is depth layered on top of those. So they go first.

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
shot direction onto the hull heading; the sign and magnitude bucket it into front/side/rear by
fixed `cos` thresholds — the **same squared-cosine trick** `resolve_fire`'s aim cone already uses.

A new **dedicated `UnitKind::Tank`** carries the real armour block (so we don't overload the
`Heavy` infantry-bruiser balance that D30 tuned). `Heavy`'s render mapping to the tank mesh stays
as-is for now; the playable embodied tank is the new kind.

---

## 4. New sim state (all `Fixed`/`Angle`/integer — checksummed)

```
World (new dense arrays, indexed by entity)
  hull_heading : Angle      // way the chassis points; movement turns it
  turret_yaw   : Angle      // independent gun bearing (absolute world angle)
Weapon (new fields)
  penetration  : Fixed      // vs armour facets
  shell        : ShellKind  // AP | APHE | HE  (enum, repr-stable)
  dispersion   : Fixed      // current aim bloom (shrinks at rest, grows moving/traversing)
  turret_speed : u16        // max turret slew, angle-units/tick (0 = fixed mount / infantry)
Armor (new component, default all-zero = unarmoured)
  front, side, rear : Fixed
```

`turret_speed == 0` ⇒ "no turret" ⇒ every infantry unit keeps a turret locked to hull heading,
i.e. exactly current behaviour. Like `mag_size == 0` for the magazine (D51), the new system is
**opt-in by a zero default**, so non-tank entities cost nothing and move the checksum by nothing.

---

## 5. New / changed commands (lockstep stream, host-quantized)

- `DriveHull { entity, dir }` — replaces/extends `Locomote` for vehicles: `dir` turns the **hull
  heading** toward the stick (rate-limited) and drives forward along it. (Infantry keep
  `Locomote`'s instant strafe; tanks turn-then-move — the tank "feel".)
- `AimTurret { entity, dir }` — slews `turret_yaw` toward the look-stick bearing at
  `turret_speed`/tick. Embodied-only intent; AI sets the same field toward its target.
- `SelectShell { entity, shell }` — cycles AP/APHE/HE; changes `penetration`/`damage`/splash for
  the *next* shot. A no-op mid-reload.
- `Fire` (existing) gains the dispersion roll: the shot direction is `turret_yaw` perturbed by a
  **deterministic RNG draw bounded by current `dispersion`** (the first real use of `combat`'s
  reserved `&mut Rng`). Hold still → tight; fire on the move → spray.

All payloads stay `Copy` fixed-point/handle data — no float crosses the boundary (#1).

---

## 6. Phasing — each phase is a green, committed, testable slice

```
P1  trig: atan2 + rotate_toward (turret slew math)      ── pure, isolated  ◀ START HERE
P2  hull_heading + turret_yaw state + AimTurret/DriveHull + AI slew
P3  Armor + Weapon.penetration + facing multiplier (ALL-UNIT damage rewrite)
P4  dispersion / aim-time bloom + the RNG dispersion roll on Fire
P5  ShellKind AP/APHE/HE + SelectShell + per-shell pen/damage/splash
P6  render: turret mesh node + hull/turret interpolation (invariant #4)
P7  HUD: hull-relative turret indicator, dispersion reticle, shell selector, reload ring
P8  tank UnitKind archetype + economy stats + sniper/zoom view
─────────────────────────────────────────────────────────────────────────
DEFERRED (own decision later): shell drop / ballistic projectile flight,
module+crew damage (tracks/breech/ammo-rack), commander's-optics third view.
```

P1–P3 are the spine (heading + the combat-model rewrite). P4–P5 are the skill ceiling. P6–P8 make
it legible and playable. Each lands with unit tests in the same commit; P3 additionally must keep
`determinism.yml`'s arch matrix green (invariant #7). High-blast-radius phases (P2, P3, P5) run
through `/safe-edit`.

---

## 7. What's explicitly deferred and why

- **Ballistic shell flight / drop.** War Thunder's signature, but it forces either a projectile
  entity with fixed-point gravity per tick (cheap-ish) *or* lead-the-target gunnery (a big UX
  shift on a 200-unit mobile budget). It rides on P1–P3 cleanly later; hitscan-with-penetration
  is the honest MVP of "tank gunnery." Logged as an open question when P3 lands.
- **Module + crew damage.** Tracks-out/breech-broken/ammo-rack-detonation is the deepest War
  Thunder layer. It multiplies sim state per vehicle and needs its own balance pass; out of scope
  until the facing model has shipped and proven fun.

---

## 8. Test obligations (the floor, per CLAUDE.md)

- **P1** `atan2` round-trips known bearings (±1 angle-unit); `rotate_toward` never overshoots,
  takes the short way around the wrap seam, and is monotone. Pure, `cargo test` dev+release.
- **P2** turret slews toward a target at exactly `turret_speed`/tick; hull turn-rate-limited; an
  AI tank tracks its `acquire_target`; embodied routing verified via the `InputSource` seam.
- **P3** the **load-bearing battery**: unarmoured unit takes *identical* damage to today (golden
  test against current numbers); frontal shot on a tank bounces; flank shot pens; facet selection
  correct at the front/side and side/rear boundaries; **a determinism test that runs an armoured
  duel and checksums it** — added to the cross-arch matrix.
- **P4** dispersion grows under motion/traverse and shrinks at rest; the `Fire` RNG draw is
  bounded by dispersion and is bit-identical across two `Sim` instances on the same seed.
- **P6/P7** pure render seams tested like `interpolate_instances` / `map_input_commands`
  (turret-node transform math, reticle-from-dispersion mapping) — platform glue excepted, stated
  explicitly where skipped.

---

## 9. First commit (this session)

P1 only: `trig::atan2` + `trig::rotate_toward`, fully tested, no other file touched. It's pure,
isolated, and unlocks P2's turret slew without risking the combat resolver. Then P2, then the
P3 rewrite under `/safe-edit`.
