//! Combat, suppression, cover, and death (invariants #1, #3 — fixed-point, literal AI).
//!
//! `combat_system` is the deterministic per-tick weapons resolver. For each living, armed
//! unit it acquires a target (nearest enemy in weapon range with line of sight), respecting
//! the unit's [`Stance`](crate::components::Stance), fires on cooldown, applies
//! cover-mitigated damage, accumulates **suppression** on the target, decays suppression
//! over time, and despawns anything reduced to zero health — emitting [`SimEvent`]s for the
//! alert/audio channel as it goes.
//!
//! Determinism rules it must hold (the determinism guard greps this file):
//! - Fixed-point only; no floats, no `std`/`libm` transcendentals (use `trig`/`Fixed`).
//! - Iterate entities in **stable index order**; break target ties to the lowest index.
//! - All randomness via the passed `&mut Rng` (integer draws only), never wall-clock.
//!
//! The literal-executor rule (invariant #3) still binds: combat acts on the *stance* the
//! player set, it does not invent targets the stance forbids or chase beyond weapon range.
//!
//! IMPLEMENTATION OWNER: worker 2. A real generational [`Entity`] handle for the
//! shooter/target (needed by `last_attacker` and the `SimEvent`s) comes from the O(1)
//! [`World::entity`] accessor.

use crate::components::{Armor, EntityKind, Faction, InputSource, Posture, Stance, Vec2};
use crate::ecs::World;
use crate::event::SimEvent;
use crate::fixed::Fixed;
use crate::rng::Rng;
use crate::spatial::SpatialHash;
use crate::terrain::Terrain;
use crate::trig::{self, Angle};

/// Suppression at or above this fraction of [`SUPPRESSION_MAX`] pins a unit: it may not fire
/// and (per `orders`) moves at reduced speed.
///
/// 3/8 (D70, lowered from the D30 1/2, itself down from 3/4). The threshold dropped when the
/// [D66](../docs/decisions.md) ×5 lethality made kills outrun suppression: at 1/2 a focused unit
/// died on the same volley it would have pinned, so "concentrate fire to pin" was vestigial again
/// (the metric honestly locked pin-at-0). With **area suppression** ([`SUPPRESSION_SPLASH_PER_HIT`])
/// a 4-shooter volley splashes a whole cluster, and at 3/8 that splash crosses the pin line *before*
/// the kill — a directly-hit unit needs 3 hits (it dies on the 4th), and a *neighbour* of the impact
/// pins from one direct hit plus the volley's splash. A lone shooter still never pins: one hit per
/// cooldown (1/8) decays (1/64 a tick) before the next, never reaching 3/8, so a clean 1v1 resolves
/// by damage. (combat-rebalance-plan WS-B; tuned against `sim-runner --metrics`.)
pub const SUPPRESSION_PIN: Fixed = Fixed::from_ratio(3, 8);

/// Ceiling for accumulated suppression; it decays toward zero each tick.
pub const SUPPRESSION_MAX: Fixed = Fixed::ONE;

/// Suppression removed per tick when not taking fire.
pub const SUPPRESSION_DECAY: Fixed = Fixed::from_ratio(1, 64);

/// Suppression added to the unit a shot directly lands on.
pub const SUPPRESSION_PER_HIT: Fixed = Fixed::from_ratio(1, 8);

/// World-unit radius of a shot's **area** suppression: every hostile unit within this distance of
/// the impact (not just the body hit) accrues [`SUPPRESSION_SPLASH_PER_HIT`]. This is the
/// fire-and-maneuver model (combat-rebalance-plan WS-B, D70): rounds cracking past a position pin
/// the soldiers near them, so concentrated fire pins a *cluster* before it is wiped one-by-one —
/// the doctrine the D66 lethal speed had broken. Squared-compared in [`combat_system`] (no sqrt).
pub const SUPPRESSION_RADIUS: Fixed = Fixed::from_int(4);

/// Suppression added to each hostile unit within [`SUPPRESSION_RADIUS`] of a shot's impact, *on
/// top of* the full [`SUPPRESSION_PER_HIT`] on the directly-hit body. Strictly **less** than the
/// per-hit value (1/16 < 1/8): a near-miss suppresses half as much as a direct hit. Tuned (with
/// [`SUPPRESSION_PIN`]) so a 4-shooter cluster volley pins before the kill while a lone shooter never
/// pins — and kept low enough that area suppression does not let a rifle blob trivially pin-and-wipe
/// a cost-equal Heavy force (the [D69](../docs/decisions.md) RPS still holds at the canonical points:
/// heavy wins close at 500, rifle kites at range; a larger rifle mass trading up close stays a real
/// ~3 s fight, not a blowout). (combat-rebalance-plan WS-B, D70; measured against `--metrics`.)
pub const SUPPRESSION_SPLASH_PER_HIT: Fixed = Fixed::from_ratio(1, 16);

/// Is `(attacker, defender)` a hostile pair? Combat engages only across distinct factions and
/// never involves `Neutral` on either side (invariant #3 keeps it literal — no friendly fire,
/// no neutral aggression).
#[inline]
pub(crate) fn is_enemy(attacker: Faction, defender: Faction) -> bool {
    attacker != defender && attacker != Faction::Neutral && defender != Faction::Neutral
}

/// Can `shooter_idx` currently land a shot on `target_idx`? Target must be alive, a different
/// hostile faction, within `range` (squared compare — never a sqrt), and in line of sight.
///
/// There is deliberately **no `EntityKind` filter on the target**: a unit may engage an enemy
/// *building* (so an attack-moving squad razes a hostile camp), and `is_enemy` still spares
/// friendly buildings. Only the *attacker* is restricted to `EntityKind::Unit` (in the engage
/// pass) — buildings never shoot.
fn can_engage(world: &World, terrain: &Terrain, shooter_idx: usize, target_idx: usize) -> bool {
    if shooter_idx == target_idx || !world.is_index_alive(target_idx) {
        return false;
    }
    if world.health[target_idx].is_dead() {
        return false;
    }
    if !is_enemy(world.faction[shooter_idx], world.faction[target_idx]) {
        return false;
    }
    let my_pos = world.pos[shooter_idx];
    let target_pos = world.pos[target_idx];
    let range = world.weapon[shooter_idx].range;
    let dist_sq = (target_pos - my_pos).len_sq();
    if dist_sq > range * range {
        return false;
    }
    terrain.line_of_sight(my_pos, target_pos)
}

/// Apply **area suppression** for a shot that landed on `target_idx` at `target_pos`, fired by a
/// unit of `shooter_faction`, to one candidate slot `j` (yielded by the per-tick spatial scan or a
/// plain index scan). `j` accrues [`SUPPRESSION_SPLASH_PER_HIT`] iff it is a *living hostile unit*
/// within [`SUPPRESSION_RADIUS`] of the impact and is **not** the directly-hit body (which already
/// took the full [`SUPPRESSION_PER_HIT`]). Same-faction friendlies are excluded (invariant #3: no
/// friendly suppression); buildings are excluded (only soldiers near the impact pin). Squared-distance
/// compare — never a sqrt. The saturating add is independent per slot, so applying it across the
/// candidate set in any order yields the identical result (determinism, invariants #1/#7).
#[inline]
fn splash_suppress(
    world: &mut World,
    shooter_faction: Faction,
    target_idx: usize,
    target_pos: Vec2,
    j: usize,
) {
    if j == target_idx || !world.is_index_alive(j) {
        return;
    }
    if world.kind[j] != EntityKind::Unit {
        return;
    }
    if world.health[j].is_dead() {
        return;
    }
    if !is_enemy(shooter_faction, world.faction[j]) {
        return;
    }
    if (world.pos[j] - target_pos).len_sq() > SUPPRESSION_RADIUS * SUPPRESSION_RADIUS {
        return;
    }
    world.suppression[j] = (world.suppression[j] + SUPPRESSION_SPLASH_PER_HIT).min(SUPPRESSION_MAX);
}

/// Pick the target slot for `shooter_idx` under its stance, or `None` to hold fire.
/// `FireAtWill` takes the nearest valid enemy, ties broken to the lowest index. `ReturnFire`
/// engages only its recorded `last_attacker` (and only if that attacker is still a valid
/// target). `HoldFire` never fires.
///
/// `FireAtWill` queries the per-tick [`SpatialHash`] instead of scanning all units (O(n²) →
/// near-O(n)), but the result is **bit-identical** to the old brute-force scan: the hash's
/// `(dist_sq, idx)` lexicographic comparator reproduces the same min-distance/lowest-index pick,
/// and `can_engage` remains the sole authoritative range/LoS/hostility filter.
fn acquire_target(
    world: &World,
    terrain: &Terrain,
    spatial: &SpatialHash,
    shooter_idx: usize,
) -> Option<usize> {
    match world.stance[shooter_idx] {
        Stance::HoldFire => None,
        Stance::ReturnFire => {
            let attacker = world.last_attacker[shooter_idx]?;
            let target_idx = attacker.index as usize;
            // The stored handle must still be the live occupant of that slot, and a valid
            // target right now (in range / LoS / hostile). Otherwise: do nothing.
            if world.is_alive(attacker) && can_engage(world, terrain, shooter_idx, target_idx) {
                Some(target_idx)
            } else {
                None
            }
        }
        Stance::FireAtWill => {
            let my_pos = world.pos[shooter_idx];
            let range = world.weapon[shooter_idx].range;
            spatial.nearest_within(
                my_pos,
                range,
                |idx| can_engage(world, terrain, shooter_idx, idx),
                |idx| (world.pos[idx] - my_pos).len_sq(),
            )
        }
    }
}

/// Resolve one tick of combat over `world`, using `terrain` for cover + line of sight and
/// `rng` for any deterministic rolls, pushing facts into `events`.
///
/// Three index-ordered passes for clean, peer-identical ordering:
/// 1. **Upkeep** — decay suppression toward zero; tick weapon cooldowns down.
/// 2. **Engage** — armed, non-embodied, non-pinned units acquire a target by stance and fire
///    on a ready cooldown, applying cover-mitigated damage + suppression.
/// 3. **Deaths** — anything at zero health emits `Killed` and is despawned.
pub fn combat_system(
    world: &mut World,
    terrain: &Terrain,
    rng: &mut Rng,
    events: &mut Vec<SimEvent>,
) {
    let _ = rng; // No stochastic combat yet; reserved for future spread/crit rolls.
    let n = world.capacity();

    // --- Pass 1: upkeep (every alive entity) ---
    for i in 0..n {
        if !world.is_index_alive(i) {
            continue;
        }
        let s = world.suppression[i];
        world.suppression[i] = (s - SUPPRESSION_DECAY).max(Fixed::ZERO);
        if world.weapon[i].cooldown_left > 0 {
            world.weapon[i].cooldown_left -= 1;
        }
        // Reload upkeep (all-unit ammo, D67). Count an in-progress reload down; on completion draw
        // a fresh magazine from carried `reserve` (a partial reserve tops the mag only as far as it
        // reaches; an empty reserve loads nothing). When no reload is running, an AI unit whose
        // magazine has run dry *auto-starts* one if rounds remain in reserve — the embodied player
        // instead reloads manually via `Command::Reload`. A `mag_size == 0` weapon never reloads.
        // Index-ordered with the cooldown tick above, so the pass stays peer-identical (invariant #7).
        let is_embodied = world.input_source[i] == InputSource::Embodied;
        let w = &mut world.weapon[i];
        if w.reload_left > 0 {
            w.reload_left -= 1;
            if w.reload_left == 0 {
                let draw = w.mag_size.saturating_sub(w.ammo).min(w.reserve);
                w.ammo += draw;
                w.reserve -= draw;
            }
        } else if w.mag_size > 0 && w.ammo == 0 && w.reserve > 0 && !is_embodied {
            // Literal-executor auto-reload (invariant #3): reloads the held weapon, never strategy.
            w.reload_left = w.reload_ticks;
        }
    }

    // --- Build the per-tick spatial index for target acquisition (A5) ---
    // Positions are fixed for the whole engage pass (pass 2 mutates only health/suppression/
    // cooldown/last_attacker, never `pos`), so one build serves every shooter this tick. It is
    // NOT sim state — never folded into the checksum, exactly like `flow_field::FlowFieldCache`.
    let spatial = SpatialHash::build(world);

    // --- Pass 2: engage (armed, order-driven, un-pinned units) ---
    for i in 0..n {
        if !world.is_index_alive(i) {
            continue;
        }
        // Only units attack — buildings never acquire targets or fire (they are damageable
        // TARGETS only; see `can_engage`). This is the sole attacker-side kind filter.
        if world.kind[i] != EntityKind::Unit {
            continue;
        }
        if world.input_source[i] == InputSource::Embodied {
            continue;
        }
        if world.weapon[i].range <= Fixed::ZERO {
            continue;
        }
        // Pinned by suppression: may not fire (orders also slows it elsewhere).
        if world.suppression[i] >= SUPPRESSION_PIN {
            continue;
        }

        let target_idx = match acquire_target(world, terrain, &spatial, i) {
            Some(t) => t,
            None => continue,
        };

        // Cooldown gates the rate of fire; a target may be held but not shot this tick.
        if world.weapon[i].cooldown_left != 0 {
            continue;
        }

        // All-unit ammo gate (D67), mirroring `resolve_fire`: a magazine weapon cannot fire while
        // reloading or with an empty magazine. Upkeep auto-starts the reload, so a dry unit simply
        // holds here until it finishes (or stays silent if its reserve is also spent).
        {
            let w = world.weapon[i];
            if w.mag_size > 0 && (w.reload_left > 0 || w.ammo == 0) {
                continue;
            }
        }

        let shooter = match world.entity(i) {
            Some(e) => e,
            None => continue,
        };
        let target = match world.entity(target_idx) {
            Some(e) => e,
            None => continue,
        };

        let mult = terrain.cover_at(world.pos[target_idx]).damage_multiplier();
        // All-unit armour facing (D55 P4): shot direction is target − shooter. Unarmoured targets
        // (the default) return the multiplier as one, so this is byte-neutral for existing balance.
        let facing = facing_penetration_multiplier(
            world.pos[target_idx] - world.pos[i],
            world.hull_heading[target_idx],
            world.weapon[i].penetration,
            world.armor[target_idx],
        );
        let damage = world.weapon[i].damage * mult * facing;

        world.health[target_idx].cur -= damage;
        world.last_attacker[target_idx] = Some(shooter);
        world.suppression[target_idx] =
            (world.suppression[target_idx] + SUPPRESSION_PER_HIT).min(SUPPRESSION_MAX);
        // Area (fire-and-maneuver) suppression (WS-B, D70): the same shot pins the hostiles near the
        // impact, not just the body it hit. Reuse the per-tick spatial index (the candidates are a
        // superset; `splash_suppress` owns the precise radius/hostility/kind filter). Index-ordered,
        // float-free; the per-slot saturating add is order-independent. Applied during pass 2, so a
        // splash that pushes a not-yet-processed unit over SUPPRESSION_PIN can keep it from firing
        // this same tick — exactly the within-fight bite the lethal speed had erased (deterministic:
        // the engage pass is index-ordered identically on every peer, invariant #7).
        let sf = world.faction[i];
        let target_pos = world.pos[target_idx];
        spatial.for_each_within(target_pos, SUPPRESSION_RADIUS, |j| {
            splash_suppress(world, sf, target_idx, target_pos, j);
        });
        world.weapon[i].cooldown_left = world.weapon[i].cooldown_ticks;
        // Spend a round (magazine weapons only); the gate above guarantees ammo > 0 here, so the
        // subtraction never underflows. AI auto-reload (upkeep) refills from reserve when it empties.
        if world.weapon[i].mag_size > 0 {
            world.weapon[i].ammo -= 1;
        }

        events.push(SimEvent::Damaged {
            entity: target,
            faction: world.faction[target_idx],
            source: shooter,
            amount: damage,
            pos: world.pos[target_idx],
        });
    }

    // --- Pass 3: deaths (anything reduced to zero) ---
    for i in 0..n {
        if !world.is_index_alive(i) {
            continue;
        }
        if !world.health[i].is_dead() {
            continue;
        }
        let dead = match world.entity(i) {
            Some(e) => e,
            None => continue,
        };
        // The killer is the last entity to hit it, when that handle is still live.
        let source = match world.last_attacker[i] {
            Some(a) if world.is_alive(a) => a,
            _ => dead, // self-attributed when the attacker is gone (e.g. mutual kill).
        };
        events.push(SimEvent::Killed {
            entity: dead,
            faction: world.faction[i],
            source,
            pos: world.pos[i],
        });
        world.despawn(dead);
    }
}

/// Cosine of the embodied weapon's half-cone angle, as a Fixed in `[0, 1]`. `cos(30°) ≈
/// 0.8660` — a 60°-wide aim cone, generous enough that a hip-fired shot reads as "I pointed at
/// him and hit," tight enough that you must actually face the target. Stored as the exact
/// rational `866/1000` so it is float-free and bit-identical on every peer (invariant #1). The
/// cone test squares this (never a sqrt), so only `cos²` ever enters the comparison.
pub const FIRE_CONE_COS_HALF: Fixed = Fixed::from_ratio(866, 1000);

/// Cosine of the half-cone angle while **crouched** — `cos(18°) ≈ 0.951`, a ~36°-wide cone
/// (tighter than the ~60° standing cone). Crouch is a *marksman* stance: the narrower cone
/// **demands more precise aim** (an off-axis target a standing hip-fire would clip is now a miss),
/// which together with the extended [`CROUCH_RANGE_BONUS`] range and the slower
/// [`systems::CROUCH_MOVE_SPEED`](crate::systems::CROUCH_MOVE_SPEED) makes crouch a deliberate
/// "aim true, reach further, can't reposition" trade. The exact rational `951/1000` keeps it
/// float-free (invariant #1); the cone test squares it, so only `cos²` ever enters the compare.
pub const FIRE_CONE_COS_HALF_CROUCHED: Fixed = Fixed::from_ratio(951, 1000);

/// Weapon-range multiplier while **crouched** — a steady stance reaches `5/4` (25%) further.
/// This is the tangible upside that pays for the crouch movement penalty
/// ([`systems::CROUCH_MOVE_SPEED`](crate::systems::CROUCH_MOVE_SPEED)): crouch to set up a
/// precise, longer shot; stand to reposition. Exact rational keeps it float-free (invariant #1).
pub const CROUCH_RANGE_BONUS: Fixed = Fixed::from_ratio(5, 4);

/// The armour facet an incoming shot strikes — chosen by the angle between the shot's travel
/// direction and the defender's hull heading (tank embodiment P4, D55).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Facet {
    /// The shot arrives within the head-on arc — the thickest armour.
    Front,
    /// The shot arrives from the flank — thinner.
    Side,
    /// The shot catches the tail — the thinnest armour.
    Rear,
}

/// Cosine of the half-angle of the front (and, mirrored, rear) armour arc. `1/2 = cos(60°)`: the
/// front facet covers shots arriving within 60° of head-on (a 120°-wide frontal arc), the rear
/// facet the mirror 120° tail arc, and the two 60° wedges between are the sides — a clean partition
/// of the full turn. The facet test squares this (never a sqrt), exactly like
/// [`FIRE_CONE_COS_HALF`], so only `cos²` ever enters the compare. Exact rational, no float
/// (invariant #1).
pub const FACET_ARC_COS_HALF: Fixed = Fixed::from_ratio(1, 2);

/// Pick the [`Facet`] a shot travelling along `shot_dir` strikes on a defender whose chassis points
/// along `hull_heading` (tank embodiment P4, D55). Float-free: the hull direction is the LUT
/// `(cos, sin)` of the heading and the bucket is decided by the **squared cosine** of the angle
/// between `shot_dir` and that direction — the same sqrt-free trick the aim cone uses
/// ([`resolve_fire`]). `proj = shot_dir·hull_dir`; a shot opposing the hull (`proj < 0`) within the
/// front arc is `Front`, one trailing it (`proj > 0`) within the rear arc is `Rear`, otherwise
/// `Side`. A zero `shot_dir` is degenerate (no travel direction → `proj` and `mag_sq` both zero, so
/// the arc test is trivially satisfied and the result is unspecified between `Front`/`Rear`);
/// callers only ever reach here with a real, non-zero shot.
#[inline]
pub fn shot_facet(shot_dir: Vec2, hull_heading: Angle) -> Facet {
    let hull_dir = Vec2::new(trig::cos(hull_heading), trig::sin(hull_heading));
    let proj = shot_dir.dot(hull_dir);
    let mag_sq = shot_dir.len_sq();
    // Inside an arc when proj² ≥ cos²·|shot_dir|² (hull_dir is ~unit, so |hull_dir|² ≈ 1). Squaring
    // keeps it sqrt-free; the sign of `proj` then splits front (opposing the hull) from rear. The
    // compare is done in i64 on the raw Q16.16 bits — both sides land at the same `2^32` scale, so
    // there is no precision loss and (unlike `Fixed`'s i32-truncating multiply) no overflow even
    // for a large `shot_dir`, keeping facet selection correct as weapon ranges grow.
    let cos_half_sq = FACET_ARC_COS_HALF * FACET_ARC_COS_HALF;
    let proj_bits = proj.to_bits() as i64;
    let lhs = proj_bits * proj_bits;
    let rhs = (cos_half_sq.to_bits() as i64) * (mag_sq.to_bits() as i64);
    if lhs >= rhs {
        if proj < Fixed::ZERO {
            Facet::Front
        } else {
            Facet::Rear
        }
    } else {
        Facet::Side
    }
}

/// The damage multiplier an incoming shot earns against a defender's directional [`Armor`], by
/// **penetration vs the struck facet** (tank embodiment P4, D55 — the all-unit armour model). This
/// is the single shared resolver multiplied into damage at **every** site (AI hitscan, embodied
/// hitscan, and shell impact), so armour facing is genuinely all-unit, not embodied-only.
///
/// The safety property that keeps existing balance intact: an **unarmoured** defender
/// (`armor.is_unarmored()`, the default for every Rifleman/Heavy/building) returns **exactly
/// [`Fixed::ONE`]** with no facet/trig work — so every existing combat/economy test passes
/// byte-for-byte and the checksum only moves where a unit is actually armoured (invariant #7).
///
/// For an armoured defender, `shot_facet` picks the facet, then penetration `p` meets that facet's
/// armour `a`:
/// - `p ≥ a` → full damage ([`Fixed::ONE`]): the shot pens cleanly.
/// - `2·p ≤ a` → a hard **bounce** ([`Fixed::ZERO`]): armour clearly overmatches the shot (the War
///   Thunder non-penetration).
/// - between (`a/2 < p < a`) → a reduced multiplier ramping `(2p − a)/a` from just above zero to
///   just below one as `p` climbs toward `a`.
///
/// All `Fixed`; the division only runs when `a > p ≥ 0` (so `a > 0`, never a divide-by-zero). No
/// float, no transcendental beyond the LUT `cos`/`sin` in `shot_facet` (invariant #1).
#[inline]
pub fn facing_penetration_multiplier(
    shot_dir: Vec2,
    hull_heading: Angle,
    penetration: Fixed,
    armor: Armor,
) -> Fixed {
    // Fast path + the load-bearing safety property: unarmoured ⇒ exactly ONE, identical to today.
    if armor.is_unarmored() {
        return Fixed::ONE;
    }
    let a = match shot_facet(shot_dir, hull_heading) {
        Facet::Front => armor.front,
        Facet::Side => armor.side,
        Facet::Rear => armor.rear,
    };
    let p = penetration;
    if p >= a {
        Fixed::ONE
    } else if p + p <= a {
        Fixed::ZERO
    } else {
        // a/2 < p < a: ramp (2p − a)/a ∈ (0, 1). `a > 0` here, so the divide is safe.
        (p + p - a) / a
    }
}

/// Resolve one embodied shot from `shooter_idx` aimed along `dir` (a unit aim vector in Fixed
/// world space — quantized at the host boundary, invariant #1). A fixed-point **cone hitscan**:
/// the **nearest** living hostile entity — a unit **or** a building — that lies inside the aim
/// cone, within weapon range, and in line of sight takes the same cover-mitigated damage +
/// suppression the auto-resolver applies, and the weapon goes on cooldown. Returns silently (no
/// shot, no cooldown) if the weapon is disarmed, still cooling down, or no target qualifies.
/// Buildings are damageable so an embodied player can shoot down an enemy structure; the
/// `is_enemy` filter still spares friendly buildings (no own-base fire), and a unit screening a
/// structure is hit before the structure (nearest wins).
///
/// Determinism (the guard greps this file): fixed-point only, no sqrt/normalize. The cone test
/// `dir·(t−p) ≥ cos_half·|t−p|` is evaluated by **squaring both non-negative sides** —
/// `proj·proj ≥ cos_half²·|t−p|²` — after rejecting any target behind the aim (`proj < 0`), so a
/// transcendental never enters. Targets are scanned in stable index order and the nearest qualifier
/// (by squared distance) wins, the lowest index breaking a distance tie. Only already-checksummed
/// fields are written, so the per-tick `fold()`/checksum stream is untouched (invariant #7).
pub fn resolve_fire(
    world: &mut World,
    terrain: &Terrain,
    shooter_idx: usize,
    dir: Vec2,
    events: &mut Vec<SimEvent>,
) {
    if !world.is_index_alive(shooter_idx) {
        return;
    }
    // A disarmed (range 0) weapon never fires; a hot weapon must finish its cooldown first. Both
    // mirror the auto-resolver's gates so embodied fire obeys the same rate-of-fire contract.
    if world.weapon[shooter_idx].range <= Fixed::ZERO {
        return;
    }
    if world.weapon[shooter_idx].cooldown_left != 0 {
        return;
    }
    // Embodied magazine gate (opt-in: `mag_size == 0` = no magazine, fires freely). An empty
    // mag or an in-progress reload is a silent no-op — a dry click, no cooldown spent, exactly
    // like an out-of-cone miss — so the player can reload and try again. AI/auto fire never
    // reaches here (combat skips embodied units; this is the embodied-only path), so the
    // mechanic is first-person-only by construction (invariant #3).
    {
        let w = world.weapon[shooter_idx];
        if w.mag_size > 0 && (w.reload_left > 0 || w.ammo == 0) {
            return;
        }
    }

    // Crouch (player posture) tightens the aim cone and extends range — the marksman stance that
    // pays for its slower movement. Posture is `Standing` for every non-embodied unit, so this
    // only ever shifts an embodied player's shot.
    let crouched = world.posture[shooter_idx] == Posture::Crouched;
    let my_pos = world.pos[shooter_idx];
    let base_range = world.weapon[shooter_idx].range;
    let range = if crouched {
        base_range * CROUCH_RANGE_BONUS
    } else {
        base_range
    };
    let range_sq = range * range;
    let cos_half = if crouched {
        FIRE_CONE_COS_HALF_CROUCHED
    } else {
        FIRE_CONE_COS_HALF
    };
    let cos_half_sq = cos_half * cos_half;

    // Pick the **nearest** hostile, living target inside the cone, in range, with LoS — ties broken
    // to the lowest index. This matches what the crosshair promises (the closest enemy you are
    // pointing at takes the shot) and mirrors the AI auto-resolver's nearest-target pick
    // (`acquire_target` FireAtWill); the old lowest-index-in-cone pick let a lower-index enemy (or
    // an enemy building) behind your actual target steal every shot, so the unit under the crosshair
    // never died. A target may be a unit OR a building — embodied fire razes enemy structures (the
    // `is_enemy` test below still excludes friendly buildings), but a unit screening it is hit first.
    // Track the best (nearest) candidate as `(index, dist_sq)` so the nearest-wins invariant is
    // structural — there is no zero sentinel that a future edit could compare against by mistake.
    let mut best: Option<(usize, Fixed)> = None;
    for t in 0..world.capacity() {
        if t == shooter_idx || !world.is_index_alive(t) {
            continue;
        }
        if world.health[t].is_dead() {
            continue;
        }
        if !is_enemy(world.faction[shooter_idx], world.faction[t]) {
            continue;
        }
        let to_target = world.pos[t] - my_pos;
        let dist_sq = to_target.len_sq();
        // Exclude a target sitting exactly on the muzzle (zero range): the cone test divides the
        // half-space by direction, and a zero vector has no direction. Treat it as out of arc.
        if dist_sq <= Fixed::ZERO || dist_sq > range_sq {
            continue;
        }
        // Cone test without a sqrt. `proj = dir·to_target`; reject anything behind the aim, then
        // compare squared: `proj² ≥ cos_half² · |to_target|²`.
        let proj = dir.dot(to_target);
        if proj < Fixed::ZERO {
            continue;
        }
        if proj * proj < cos_half_sq * dist_sq {
            continue;
        }
        if !terrain.line_of_sight(my_pos, world.pos[t]) {
            continue;
        }
        // Nearest wins; a strict `<` keeps the first (lowest-index) candidate on a distance tie.
        if best.is_none_or(|(_, bd)| dist_sq < bd) {
            best = Some((t, dist_sq));
        }
    }

    let target_idx = match best {
        Some((t, _)) => t,
        None => return,
    };

    // Same writes the auto-resolver's engage pass performs: cover-mitigated damage, last-attacker,
    // suppression, and the weapon cooldown. Reusing them keeps embodied and AI fire identical
    // (and keeps every touched field already in the checksum fold).
    let shooter = match world.entity(shooter_idx) {
        Some(e) => e,
        None => return,
    };
    let target = match world.entity(target_idx) {
        Some(e) => e,
        None => return,
    };

    let mult = terrain.cover_at(world.pos[target_idx]).damage_multiplier();
    // All-unit armour facing (D55 P4): shot direction is the aim. Unarmoured targets return the
    // multiplier as one, so embodied infantry fire against infantry/buildings is unchanged.
    let facing = facing_penetration_multiplier(
        dir,
        world.hull_heading[target_idx],
        world.weapon[shooter_idx].penetration,
        world.armor[target_idx],
    );
    let damage = world.weapon[shooter_idx].damage * mult * facing;

    world.health[target_idx].cur -= damage;
    world.last_attacker[target_idx] = Some(shooter);
    world.suppression[target_idx] =
        (world.suppression[target_idx] + SUPPRESSION_PER_HIT).min(SUPPRESSION_MAX);
    // Area (fire-and-maneuver) suppression (WS-B, D70), same as the auto-resolver's engage pass: an
    // embodied player's shot also pins the hostiles near the impact. No per-tick spatial index is
    // built on this single-shot path, so scan index-ordered (mirrors the 0..n targeting scan above);
    // `splash_suppress` owns the precise radius/hostility/kind filter. Float-free, order-independent.
    let sf = world.faction[shooter_idx];
    let target_pos = world.pos[target_idx];
    let n = world.capacity();
    for j in 0..n {
        splash_suppress(world, sf, target_idx, target_pos, j);
    }
    world.weapon[shooter_idx].cooldown_left = world.weapon[shooter_idx].cooldown_ticks;
    // Spend a round (magazine weapons only). The pre-fire gate guarantees `ammo > 0` here, so the
    // subtraction never underflows. A miss spends nothing (we returned before this point).
    if world.weapon[shooter_idx].mag_size > 0 {
        world.weapon[shooter_idx].ammo -= 1;
    }

    events.push(SimEvent::Damaged {
        entity: target,
        faction: world.faction[target_idx],
        source: shooter,
        amount: damage,
        pos: world.pos[target_idx],
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::components::{Faction, Health, Stance, Vec2, Weapon};
    use crate::ecs::Entity;
    use crate::terrain::{Cover, Terrain};

    fn fx(n: i32) -> Fixed {
        Fixed::from_int(n)
    }

    /// Spawn a combat unit at `(x, y)` with the given faction/weapon, full at `hp`.
    fn spawn_unit(
        world: &mut World,
        x: i32,
        y: i32,
        faction: Faction,
        hp: i32,
        weapon: Weapon,
    ) -> Entity {
        let e = world.spawn();
        let i = e.index as usize;
        world.pos[i] = Vec2::new(fx(x), fx(y));
        world.faction[i] = faction;
        world.health[i] = Health::full(fx(hp));
        world.weapon[i] = weapon;
        e
    }

    /// Spawn a building at `(x, y)` with the given faction, full at `hp`. A building carries a
    /// default (range-0) weapon, so it is a valid damage TARGET but never an attacker.
    fn spawn_building(world: &mut World, x: i32, y: i32, faction: Faction, hp: i32) -> Entity {
        let e = world.spawn();
        let i = e.index as usize;
        world.pos[i] = Vec2::new(fx(x), fx(y));
        world.faction[i] = faction;
        world.health[i] = Health::full(fx(hp));
        world.kind[i] = EntityKind::Building;
        e
    }

    fn rifle(range: i32, damage: i32, cooldown: u16) -> Weapon {
        Weapon {
            range: fx(range),
            damage: fx(damage),
            cooldown_ticks: cooldown,
            cooldown_left: 0,
            // No magazine by default — these AI/auto-combat tests fire with infinite ammo, so the
            // ammo gate stays out of their way (it is `mag_size > 0`-gated).
            mag_size: 0,
            ammo: 0,
            reload_ticks: 0,
            reload_left: 0,
            reserve: 0,
            reserve_max: 0,
            turret_speed: 0,
            muzzle_vel: Fixed::ZERO,
            penetration: Fixed::ZERO,
        }
    }

    /// A magazine-armed rifle for the ammo/reload tests: `mag` rounds, `reload` ticks to refill,
    /// no cooldown so successive shots are limited only by ammo. Carries three spare mags in
    /// reserve (D67) so a reload has rounds to draw from.
    fn mag_rifle(range: i32, damage: i32, mag: u16, reload: u16) -> Weapon {
        Weapon {
            range: fx(range),
            damage: fx(damage),
            cooldown_ticks: 0,
            cooldown_left: 0,
            mag_size: mag,
            ammo: mag,
            reload_ticks: reload,
            reload_left: 0,
            reserve: mag * 3,
            reserve_max: mag * 3,
            turret_speed: 0,
            muzzle_vel: Fixed::ZERO,
            penetration: Fixed::ZERO,
        }
    }

    fn run(world: &mut World, terrain: &Terrain, events: &mut Vec<SimEvent>) {
        let mut rng = Rng::new(1);
        combat_system(world, terrain, &mut rng, events);
    }

    #[test]
    fn cover_damage_multiplier_math() {
        assert_eq!(Cover::None.damage_multiplier(), Fixed::ONE);
        assert_eq!(Cover::Light.damage_multiplier(), Fixed::from_ratio(1, 2));
        assert_eq!(Cover::Heavy.damage_multiplier(), Fixed::from_ratio(1, 4));
    }

    #[test]
    fn fire_at_will_open_terrain_full_damage_kills_and_despawns() {
        let mut world = World::new();
        let terrain = Terrain::open();
        let shooter = spawn_unit(&mut world, 0, 0, Faction::Player, 100, rifle(10, 25, 0));
        world.stance[shooter.index as usize] = Stance::FireAtWill;
        let enemy = spawn_unit(&mut world, 3, 0, Faction::Enemy, 100, Weapon::default());
        world.stance[enemy.index as usize] = Stance::HoldFire;

        // 25 dmg/tick at full multiplier (open) -> dead after 4 hits.
        let mut events = Vec::new();
        for _ in 0..3 {
            run(&mut world, &terrain, &mut events);
        }
        assert!(world.is_alive(enemy), "enemy should survive 3 hits of 75");
        assert_eq!(world.health[enemy.index as usize].cur, fx(25));
        // Every hit recorded a Damaged of full 25 from the shooter.
        let damaged: Vec<_> = events
            .iter()
            .filter(|e| matches!(e, SimEvent::Damaged { .. }))
            .collect();
        assert_eq!(damaged.len(), 3);
        if let SimEvent::Damaged { amount, source, .. } = damaged[0] {
            assert_eq!(*amount, fx(25), "open terrain = full damage");
            assert_eq!(*source, shooter);
        }

        // 4th hit kills.
        run(&mut world, &terrain, &mut events);
        assert!(!world.is_alive(enemy), "enemy should be dead + despawned");
        let kills: Vec<_> = events
            .iter()
            .filter(|e| matches!(e, SimEvent::Killed { .. }))
            .collect();
        assert_eq!(kills.len(), 1);
        if let SimEvent::Killed {
            entity,
            source,
            faction,
            ..
        } = kills[0]
        {
            assert_eq!(*entity, enemy);
            assert_eq!(*source, shooter);
            assert_eq!(*faction, Faction::Enemy);
        }
    }

    #[test]
    fn last_attacker_recorded_on_hit() {
        let mut world = World::new();
        let terrain = Terrain::open();
        let shooter = spawn_unit(&mut world, 0, 0, Faction::Player, 100, rifle(10, 10, 0));
        world.stance[shooter.index as usize] = Stance::FireAtWill;
        let enemy = spawn_unit(&mut world, 2, 0, Faction::Enemy, 100, Weapon::default());

        let mut events = Vec::new();
        run(&mut world, &terrain, &mut events);
        assert_eq!(world.last_attacker[enemy.index as usize], Some(shooter));
    }

    #[test]
    fn hold_fire_never_deals_damage() {
        let mut world = World::new();
        let terrain = Terrain::open();
        let shooter = spawn_unit(&mut world, 0, 0, Faction::Player, 100, rifle(10, 25, 0));
        world.stance[shooter.index as usize] = Stance::HoldFire;
        let enemy = spawn_unit(&mut world, 2, 0, Faction::Enemy, 100, Weapon::default());

        let mut events = Vec::new();
        for _ in 0..10 {
            run(&mut world, &terrain, &mut events);
        }
        assert_eq!(world.health[enemy.index as usize].cur, fx(100));
        assert!(events.is_empty());
    }

    #[test]
    fn return_fire_idle_until_attacked_then_fires_back() {
        let mut world = World::new();
        let terrain = Terrain::open();
        // Defender is ReturnFire and armed; attacker is FireAtWill.
        let defender = spawn_unit(&mut world, 0, 0, Faction::Player, 100, rifle(10, 10, 0));
        world.stance[defender.index as usize] = Stance::ReturnFire;
        let attacker = spawn_unit(&mut world, 2, 0, Faction::Enemy, 100, rifle(10, 10, 0));
        world.stance[attacker.index as usize] = Stance::HoldFire; // doesn't shoot yet

        // With the attacker holding fire, the ReturnFire defender has no last_attacker -> idle.
        let mut events = Vec::new();
        run(&mut world, &terrain, &mut events);
        assert_eq!(world.health[attacker.index as usize].cur, fx(100));
        assert!(world.last_attacker[defender.index as usize].is_none());

        // Now the attacker opens fire: it hits the defender, recording last_attacker.
        world.stance[attacker.index as usize] = Stance::FireAtWill;
        run(&mut world, &terrain, &mut events);
        assert_eq!(world.last_attacker[defender.index as usize], Some(attacker));

        // Next tick the defender returns fire against its recorded attacker.
        run(&mut world, &terrain, &mut events);
        assert!(
            world.health[attacker.index as usize].cur < fx(100),
            "ReturnFire defender should now be hitting back"
        );
        assert_eq!(world.last_attacker[attacker.index as usize], Some(defender));
    }

    #[test]
    fn out_of_range_enemy_never_hit() {
        let mut world = World::new();
        let terrain = Terrain::open();
        let shooter = spawn_unit(&mut world, 0, 0, Faction::Player, 100, rifle(5, 25, 0));
        world.stance[shooter.index as usize] = Stance::FireAtWill;
        // Distance 6 > range 5.
        let enemy = spawn_unit(&mut world, 6, 0, Faction::Enemy, 100, Weapon::default());

        let mut events = Vec::new();
        for _ in 0..10 {
            run(&mut world, &terrain, &mut events);
        }
        assert_eq!(world.health[enemy.index as usize].cur, fx(100));
        assert!(events.is_empty());
    }

    #[test]
    fn just_in_range_enemy_is_hit() {
        let mut world = World::new();
        let terrain = Terrain::open();
        let shooter = spawn_unit(&mut world, 0, 0, Faction::Player, 100, rifle(5, 25, 0));
        world.stance[shooter.index as usize] = Stance::FireAtWill;
        // Distance exactly 5 == range 5 -> dist_sq == range*range -> engages.
        let enemy = spawn_unit(&mut world, 5, 0, Faction::Enemy, 100, Weapon::default());

        let mut events = Vec::new();
        run(&mut world, &terrain, &mut events);
        assert_eq!(world.health[enemy.index as usize].cur, fx(75));
    }

    #[test]
    fn same_faction_never_fights() {
        let mut world = World::new();
        let terrain = Terrain::open();
        let a = spawn_unit(&mut world, 0, 0, Faction::Player, 100, rifle(10, 25, 0));
        world.stance[a.index as usize] = Stance::FireAtWill;
        let b = spawn_unit(&mut world, 2, 0, Faction::Player, 100, rifle(10, 25, 0));
        world.stance[b.index as usize] = Stance::FireAtWill;

        let mut events = Vec::new();
        for _ in 0..10 {
            run(&mut world, &terrain, &mut events);
        }
        assert_eq!(world.health[a.index as usize].cur, fx(100));
        assert_eq!(world.health[b.index as usize].cur, fx(100));
        assert!(events.is_empty());
    }

    #[test]
    fn neutral_never_fights() {
        let mut world = World::new();
        let terrain = Terrain::open();
        let player = spawn_unit(&mut world, 0, 0, Faction::Player, 100, rifle(10, 25, 0));
        world.stance[player.index as usize] = Stance::FireAtWill;
        let neutral = spawn_unit(&mut world, 2, 0, Faction::Neutral, 100, rifle(10, 25, 0));
        world.stance[neutral.index as usize] = Stance::FireAtWill;

        let mut events = Vec::new();
        for _ in 0..10 {
            run(&mut world, &terrain, &mut events);
        }
        assert_eq!(world.health[player.index as usize].cur, fx(100));
        assert_eq!(world.health[neutral.index as usize].cur, fx(100));
        assert!(events.is_empty());
    }

    #[test]
    fn cooldown_gates_fire_rate() {
        let mut world = World::new();
        let terrain = Terrain::open();
        // Cooldown of 3 ticks: fire on tick 0, then nothing for 3 ticks, then fire again.
        let shooter = spawn_unit(&mut world, 0, 0, Faction::Player, 100, rifle(10, 10, 3));
        world.stance[shooter.index as usize] = Stance::FireAtWill;
        let enemy = spawn_unit(&mut world, 2, 0, Faction::Enemy, 1000, Weapon::default());

        let mut events = Vec::new();
        // Tick 1: fires, cooldown_left set to 3.
        run(&mut world, &terrain, &mut events);
        assert_eq!(world.health[enemy.index as usize].cur, fx(990));
        // Ticks 2-3: upkeep decrements 3->2 then 2->1; engage sees cd > 0 -> no fire.
        run(&mut world, &terrain, &mut events);
        run(&mut world, &terrain, &mut events);
        assert_eq!(
            world.health[enemy.index as usize].cur,
            fx(990),
            "no fire while on cooldown"
        );
        // Tick 4: upkeep decrements 1->0, engage now sees cd == 0 -> fires again.
        run(&mut world, &terrain, &mut events);
        assert_eq!(
            world.health[enemy.index as usize].cur,
            fx(980),
            "fires exactly cooldown_ticks after the previous shot"
        );
    }

    #[test]
    fn suppression_accumulates_on_target() {
        let mut world = World::new();
        let terrain = Terrain::open();
        let shooter = spawn_unit(&mut world, 0, 0, Faction::Player, 100, rifle(10, 1, 0));
        world.stance[shooter.index as usize] = Stance::FireAtWill;
        let enemy = spawn_unit(&mut world, 2, 0, Faction::Enemy, 1000, Weapon::default());

        let mut events = Vec::new();
        run(&mut world, &terrain, &mut events);
        // One hit: +1/8 suppression, then upkeep next tick would decay; check right after first
        // tick. Pass-1 upkeep runs before pass-2 fire within the same tick, so the freshly
        // applied SUPPRESSION_PER_HIT has not decayed yet this tick.
        let s = world.suppression[enemy.index as usize];
        assert_eq!(s, SUPPRESSION_PER_HIT);

        // Several more hits push suppression up (net of the 1/64 per-tick decay).
        for _ in 0..6 {
            run(&mut world, &terrain, &mut events);
        }
        assert!(
            world.suppression[enemy.index as usize] > SUPPRESSION_PER_HIT,
            "suppression should accumulate over repeated hits"
        );
    }

    #[test]
    fn splash_is_strictly_weaker_than_a_direct_hit() {
        // The WS-B fairness invariant: a near-miss suppresses LESS than a hit (else area suppression
        // would dominate). Cheap, but it pins the relationship the tuning relies on.
        assert!(
            SUPPRESSION_SPLASH_PER_HIT < SUPPRESSION_PER_HIT,
            "splash {SUPPRESSION_SPLASH_PER_HIT:?} must be < per-hit {SUPPRESSION_PER_HIT:?}"
        );
        assert!(SUPPRESSION_SPLASH_PER_HIT > Fixed::ZERO, "splash must actually suppress");
    }

    #[test]
    fn area_suppression_pins_neighbours_by_radius_faction_and_kind() {
        // A Player shooter hits the nearest Enemy A; the SAME shot's area suppression (WS-B) must
        // pin Enemy soldiers NEAR the impact — but only enemy UNITS within SUPPRESSION_RADIUS.
        let mut world = World::new();
        let terrain = Terrain::open();
        // Low damage so A survives the hit (we are measuring suppression, not the kill).
        let shooter = spawn_unit(&mut world, 0, 0, Faction::Player, 100, rifle(12, 1, 0));
        world.stance[shooter.index as usize] = Stance::FireAtWill;
        // A is the directly-hit target (nearest enemy). Big HP so it lives.
        let a = spawn_unit(&mut world, 5, 0, Faction::Enemy, 1000, Weapon::default());
        // B: enemy unit 2 away from A → inside the radius-4 splash.
        let b = spawn_unit(&mut world, 5, 2, Faction::Enemy, 1000, Weapon::default());
        // C: enemy unit 9 away from A → outside the splash radius.
        let c = spawn_unit(&mut world, 5, 9, Faction::Enemy, 1000, Weapon::default());
        // Friendly: a Player unit 1 away from A → never suppressed by Player fire (invariant #3).
        let friendly = spawn_unit(&mut world, 5, 1, Faction::Player, 1000, Weapon::default());
        // An enemy BUILDING 3 away from A → in radius, but only soldiers pin, not structures.
        let building = spawn_building(&mut world, 5, 3, Faction::Enemy, 1000);

        let mut events = Vec::new();
        run(&mut world, &terrain, &mut events);

        let s = |e: Entity| world.suppression[e.index as usize];
        assert_eq!(s(a), SUPPRESSION_PER_HIT, "directly-hit A takes the full per-hit");
        assert_eq!(s(b), SUPPRESSION_SPLASH_PER_HIT, "in-radius enemy B takes splash");
        assert_eq!(s(c), Fixed::ZERO, "out-of-radius enemy C is untouched");
        assert_eq!(s(friendly), Fixed::ZERO, "friendly neighbour is never suppressed");
        assert_eq!(s(building), Fixed::ZERO, "an enemy building is not a suppressible soldier");
    }

    #[test]
    fn area_suppression_is_order_independent() {
        // Two identical builds stepped once yield identical neighbour suppression — the per-slot
        // saturating add must not depend on spatial visitation order (determinism, invariant #7).
        let build = || {
            let mut world = World::new();
            let shooter = spawn_unit(&mut world, 0, 0, Faction::Player, 100, rifle(12, 1, 0));
            world.stance[shooter.index as usize] = Stance::FireAtWill;
            spawn_unit(&mut world, 5, 0, Faction::Enemy, 1000, Weapon::default());
            spawn_unit(&mut world, 5, 2, Faction::Enemy, 1000, Weapon::default());
            spawn_unit(&mut world, 6, 1, Faction::Enemy, 1000, Weapon::default());
            spawn_unit(&mut world, 4, 1, Faction::Enemy, 1000, Weapon::default());
            world
        };
        let mut w1 = build();
        let mut w2 = build();
        let terrain = Terrain::open();
        let mut ev = Vec::new();
        run(&mut w1, &terrain, &mut ev);
        run(&mut w2, &terrain, &mut ev);
        for i in 0..w1.capacity() {
            assert_eq!(w1.suppression[i], w2.suppression[i], "suppression slot {i} must match");
        }
    }

    #[test]
    fn embodied_fire_also_area_suppresses() {
        // The embodied `resolve_fire` path applies the same area suppression as the auto-resolver,
        // so a player's shot pins the cluster around the body it hits.
        let mut world = World::new();
        let terrain = Terrain::open();
        let shooter = spawn_unit(&mut world, 0, 0, Faction::Player, 100, rifle(12, 1, 0));
        let a = spawn_unit(&mut world, 5, 0, Faction::Enemy, 1000, Weapon::default());
        let b = spawn_unit(&mut world, 5, 2, Faction::Enemy, 1000, Weapon::default());
        let far = spawn_unit(&mut world, 5, 9, Faction::Enemy, 1000, Weapon::default());

        let mut events = Vec::new();
        resolve_fire(&mut world, &terrain, shooter.index as usize, Vec2::new(fx(1), fx(0)), &mut events);

        assert_eq!(world.suppression[a.index as usize], SUPPRESSION_PER_HIT, "embodied direct hit");
        assert_eq!(world.suppression[b.index as usize], SUPPRESSION_SPLASH_PER_HIT, "embodied splash");
        assert_eq!(world.suppression[far.index as usize], Fixed::ZERO, "out of radius: no splash");
    }

    #[test]
    fn fully_suppressed_unit_does_not_fire() {
        let mut world = World::new();
        let terrain = Terrain::open();
        let shooter = spawn_unit(&mut world, 0, 0, Faction::Player, 100, rifle(10, 25, 0));
        world.stance[shooter.index as usize] = Stance::FireAtWill;
        // Pin the shooter at max suppression; upkeep decays only 1/64, so it stays >= PIN.
        world.suppression[shooter.index as usize] = SUPPRESSION_MAX;
        let enemy = spawn_unit(&mut world, 2, 0, Faction::Enemy, 100, Weapon::default());

        let mut events = Vec::new();
        run(&mut world, &terrain, &mut events);
        assert_eq!(
            world.health[enemy.index as usize].cur,
            fx(100),
            "pinned unit must not fire"
        );
        assert!(world.suppression[shooter.index as usize] >= SUPPRESSION_PIN);
    }

    #[test]
    fn suppression_decays_toward_zero_and_clamps() {
        let mut world = World::new();
        let terrain = Terrain::open();
        let lone = spawn_unit(&mut world, 0, 0, Faction::Player, 100, Weapon::default());
        // Below one decay step: must clamp to zero, never go negative.
        world.suppression[lone.index as usize] = Fixed::from_ratio(1, 128);

        let mut events = Vec::new();
        run(&mut world, &terrain, &mut events);
        assert_eq!(world.suppression[lone.index as usize], Fixed::ZERO);
    }

    #[test]
    fn embodied_unit_does_not_auto_fire() {
        let mut world = World::new();
        let terrain = Terrain::open();
        let shooter = spawn_unit(&mut world, 0, 0, Faction::Player, 100, rifle(10, 25, 0));
        world.stance[shooter.index as usize] = Stance::FireAtWill;
        world.input_source[shooter.index as usize] = InputSource::Embodied;
        let enemy = spawn_unit(&mut world, 2, 0, Faction::Enemy, 100, Weapon::default());

        let mut events = Vec::new();
        run(&mut world, &terrain, &mut events);
        assert_eq!(
            world.health[enemy.index as usize].cur,
            fx(100),
            "embodied units fire by live input, not the resolver"
        );
    }

    #[test]
    fn fire_at_will_breaks_ties_to_lowest_index() {
        let mut world = World::new();
        let terrain = Terrain::open();
        let shooter = spawn_unit(&mut world, 0, 0, Faction::Player, 100, rifle(10, 25, 0));
        world.stance[shooter.index as usize] = Stance::FireAtWill;
        // Two enemies equidistant (distance 3) — lower index must be chosen.
        let near_low = spawn_unit(&mut world, 3, 0, Faction::Enemy, 100, Weapon::default());
        let near_high = spawn_unit(&mut world, 0, 3, Faction::Enemy, 100, Weapon::default());

        let mut events = Vec::new();
        run(&mut world, &terrain, &mut events);
        assert_eq!(world.health[near_low.index as usize].cur, fx(75));
        assert_eq!(world.health[near_high.index as usize].cur, fx(100));
    }

    #[test]
    fn fire_at_will_ties_lowest_index_across_spatial_buckets() {
        // Three enemies equidistant (distance 5) but in DIFFERENT spatial-hash cells, so the
        // pick must not depend on which bucket the query reaches first — the lowest slot index
        // wins (the spatial query reproduces the brute-force scan's tie-break, A5).
        let mut world = World::new();
        let terrain = Terrain::open();
        let shooter = spawn_unit(&mut world, 0, 0, Faction::Player, 100, rifle(20, 25, 0));
        world.stance[shooter.index as usize] = Stance::FireAtWill;
        let near_low = spawn_unit(&mut world, 5, 0, Faction::Enemy, 100, Weapon::default());
        let near_mid = spawn_unit(&mut world, 0, 5, Faction::Enemy, 100, Weapon::default());
        let near_high = spawn_unit(&mut world, -5, 0, Faction::Enemy, 100, Weapon::default());

        let mut events = Vec::new();
        run(&mut world, &terrain, &mut events);
        assert_eq!(world.health[near_low.index as usize].cur, fx(75), "lowest index hit");
        assert_eq!(world.health[near_mid.index as usize].cur, fx(100));
        assert_eq!(world.health[near_high.index as usize].cur, fx(100));
    }

    // --- Embodied fire: Command::Fire cone hitscan (resolve_fire) -----------------------------

    /// Aim straight along +X as a Fixed unit vector — no float ever constructed (invariant #1).
    fn aim_pos_x() -> Vec2 {
        Vec2::new(Fixed::ONE, Fixed::ZERO)
    }

    fn fire(world: &mut World, terrain: &Terrain, shooter: Entity, dir: Vec2, events: &mut Vec<SimEvent>) {
        resolve_fire(world, terrain, shooter.index as usize, dir, events);
    }

    #[test]
    fn fire_hits_target_inside_cone_in_range() {
        let mut world = World::new();
        let terrain = Terrain::open();
        let shooter = spawn_unit(&mut world, 0, 0, Faction::Player, 100, rifle(10, 25, 0));
        world.input_source[shooter.index as usize] = InputSource::Embodied;
        // Directly ahead on +X, distance 5 (< range 10) — squarely inside the aim cone.
        let enemy = spawn_unit(&mut world, 5, 0, Faction::Enemy, 100, Weapon::default());

        let mut events = Vec::new();
        fire(&mut world, &terrain, shooter, aim_pos_x(), &mut events);
        assert_eq!(world.health[enemy.index as usize].cur, fx(75), "open terrain = full damage");
        assert_eq!(world.last_attacker[enemy.index as usize], Some(shooter));
        assert_eq!(events.len(), 1, "one Damaged event for the hit");
    }

    #[test]
    fn fire_misses_target_outside_the_cone() {
        let mut world = World::new();
        let terrain = Terrain::open();
        let shooter = spawn_unit(&mut world, 0, 0, Faction::Player, 100, rifle(10, 25, 0));
        // Aim +X, but the enemy is at +Y (90° off-axis) — well outside the ~30° half-cone.
        let enemy = spawn_unit(&mut world, 0, 5, Faction::Enemy, 100, Weapon::default());

        let mut events = Vec::new();
        fire(&mut world, &terrain, shooter, aim_pos_x(), &mut events);
        assert_eq!(world.health[enemy.index as usize].cur, fx(100), "off-axis target not hit");
        assert!(events.is_empty());
        // A clean miss must NOT spend the weapon's cooldown — you can re-aim and fire again.
        assert_eq!(world.weapon[shooter.index as usize].cooldown_left, 0);
    }

    #[test]
    fn fire_misses_target_behind_the_shooter() {
        let mut world = World::new();
        let terrain = Terrain::open();
        let shooter = spawn_unit(&mut world, 0, 0, Faction::Player, 100, rifle(10, 25, 0));
        // Enemy directly BEHIND (−X) the +X aim: proj < 0, rejected before any squaring.
        let enemy = spawn_unit(&mut world, -5, 0, Faction::Enemy, 100, Weapon::default());

        let mut events = Vec::new();
        fire(&mut world, &terrain, shooter, aim_pos_x(), &mut events);
        assert_eq!(world.health[enemy.index as usize].cur, fx(100));
        assert!(events.is_empty());
    }

    #[test]
    fn fire_misses_out_of_range_target() {
        let mut world = World::new();
        let terrain = Terrain::open();
        let shooter = spawn_unit(&mut world, 0, 0, Faction::Player, 100, rifle(5, 25, 0));
        // On-axis and centered in the cone, but distance 6 > range 5.
        let enemy = spawn_unit(&mut world, 6, 0, Faction::Enemy, 100, Weapon::default());

        let mut events = Vec::new();
        fire(&mut world, &terrain, shooter, aim_pos_x(), &mut events);
        assert_eq!(world.health[enemy.index as usize].cur, fx(100), "out of range = no hit");
        assert!(events.is_empty());
    }

    #[test]
    fn fire_blocked_by_line_of_sight() {
        let mut world = World::new();
        let mut terrain = Terrain::open();
        let shooter = spawn_unit(&mut world, 0, 0, Faction::Player, 100, rifle(10, 25, 0));
        let enemy = spawn_unit(&mut world, 6, 0, Faction::Enemy, 100, Weapon::default());
        // Wall the cell strictly between the two (world (3,0)) with Heavy cover → blocks sight.
        let (wx, wy) = terrain.cell_of(Vec2::new(fx(3), fx(0)));
        terrain.set_cover(wx, wy, Cover::Heavy);

        let mut events = Vec::new();
        fire(&mut world, &terrain, shooter, aim_pos_x(), &mut events);
        assert_eq!(world.health[enemy.index as usize].cur, fx(100), "LoS-blocked shot misses");
        assert!(events.is_empty());
    }

    #[test]
    fn fire_respects_weapon_cooldown() {
        let mut world = World::new();
        let terrain = Terrain::open();
        let shooter = spawn_unit(&mut world, 0, 0, Faction::Player, 100, rifle(10, 10, 5));
        let enemy = spawn_unit(&mut world, 5, 0, Faction::Enemy, 1000, Weapon::default());

        let mut events = Vec::new();
        // First shot lands and sets cooldown_left = 5.
        fire(&mut world, &terrain, shooter, aim_pos_x(), &mut events);
        assert_eq!(world.health[enemy.index as usize].cur, fx(990));
        assert_eq!(world.weapon[shooter.index as usize].cooldown_left, 5);
        // A second pull while hot does nothing (no damage, no event, cooldown unchanged).
        fire(&mut world, &terrain, shooter, aim_pos_x(), &mut events);
        assert_eq!(world.health[enemy.index as usize].cur, fx(990), "no fire while on cooldown");
        assert_eq!(events.len(), 1);
        assert_eq!(world.weapon[shooter.index as usize].cooldown_left, 5);
    }

    #[test]
    fn fire_skips_dead_and_finds_next_living_target() {
        let mut world = World::new();
        let terrain = Terrain::open();
        let shooter = spawn_unit(&mut world, 0, 0, Faction::Player, 100, rifle(10, 25, 0));
        // Lower-index enemy already at zero health (dead but not yet despawned this tick) must be
        // skipped; the live higher-index enemy on-axis is hit instead.
        let dead = spawn_unit(&mut world, 4, 0, Faction::Enemy, 100, Weapon::default());
        world.health[dead.index as usize].cur = Fixed::ZERO;
        let live = spawn_unit(&mut world, 5, 0, Faction::Enemy, 100, Weapon::default());

        let mut events = Vec::new();
        fire(&mut world, &terrain, shooter, aim_pos_x(), &mut events);
        assert_eq!(world.health[live.index as usize].cur, fx(75), "the living target takes the hit");
        assert_eq!(world.last_attacker[live.index as usize], Some(shooter));
    }

    #[test]
    fn fire_breaks_ties_to_lowest_index() {
        let mut world = World::new();
        let terrain = Terrain::open();
        let shooter = spawn_unit(&mut world, 0, 0, Faction::Player, 100, rifle(10, 25, 0));
        // Two equidistant on-axis enemies; the lowest slot index must take the shot.
        let low = spawn_unit(&mut world, 5, 0, Faction::Enemy, 100, Weapon::default());
        let high = spawn_unit(&mut world, 5, 0, Faction::Enemy, 100, Weapon::default());

        let mut events = Vec::new();
        fire(&mut world, &terrain, shooter, aim_pos_x(), &mut events);
        assert_eq!(world.health[low.index as usize].cur, fx(75), "lowest-index target hit");
        assert_eq!(world.health[high.index as usize].cur, fx(100));
    }

    #[test]
    fn fire_hits_the_nearest_enemy_not_a_lower_index_one_behind_it() {
        // Regression: embodied fire used to pick the LOWEST-INDEX hostile in the cone, so a closer
        // target the player was aiming at went unhit while a lower-index enemy further down the same
        // line (e.g. the enemy base behind a soldier) soaked every shot — "the unit in front of me
        // won't die." It must hit the NEAREST qualifier instead, matching the AI auto-resolver.
        let mut world = World::new();
        let terrain = Terrain::open();
        let shooter = spawn_unit(&mut world, 0, 0, Faction::Player, 100, rifle(20, 25, 0));
        world.input_source[shooter.index as usize] = InputSource::Embodied;
        // The FAR enemy is spawned first → lower index; the NEAR one second → higher index. Both
        // sit dead ahead on +X, inside the cone and range.
        let far_low = spawn_unit(&mut world, 12, 0, Faction::Enemy, 100, Weapon::default());
        let near_high = spawn_unit(&mut world, 4, 0, Faction::Enemy, 100, Weapon::default());

        let mut events = Vec::new();
        fire(&mut world, &terrain, shooter, aim_pos_x(), &mut events);
        assert_eq!(
            world.health[near_high.index as usize].cur,
            fx(75),
            "the nearest enemy takes the shot",
        );
        assert_eq!(
            world.health[far_low.index as usize].cur,
            fx(100),
            "the lower-index enemy behind it is spared",
        );
    }

    #[test]
    fn fire_never_hits_same_faction() {
        let mut world = World::new();
        let terrain = Terrain::open();
        let shooter = spawn_unit(&mut world, 0, 0, Faction::Player, 100, rifle(10, 25, 0));
        let friendly = spawn_unit(&mut world, 5, 0, Faction::Player, 100, Weapon::default());

        let mut events = Vec::new();
        fire(&mut world, &terrain, shooter, aim_pos_x(), &mut events);
        assert_eq!(world.health[friendly.index as usize].cur, fx(100), "no friendly fire");
        assert!(events.is_empty());
    }

    #[test]
    fn fire_kills_then_combat_death_pass_despawns() {
        // Command::Fire applies BEFORE combat_system in a tick: a lethal embodied shot drops the
        // target to 0 health, and the same tick's death pass emits Killed + despawns it.
        let mut world = World::new();
        let terrain = Terrain::open();
        let shooter = spawn_unit(&mut world, 0, 0, Faction::Player, 100, rifle(10, 100, 0));
        world.input_source[shooter.index as usize] = InputSource::Embodied;
        let enemy = spawn_unit(&mut world, 5, 0, Faction::Enemy, 100, Weapon::default());

        let mut events = Vec::new();
        fire(&mut world, &terrain, shooter, aim_pos_x(), &mut events);
        assert!(world.health[enemy.index as usize].is_dead());
        // Now run the auto-resolver's death pass (as Sim::step would, right after apply).
        let mut rng = Rng::new(1);
        combat_system(&mut world, &terrain, &mut rng, &mut events);
        assert!(!world.is_alive(enemy), "lethal embodied shot despawns the target this tick");
        assert!(events.iter().any(|e| matches!(e, SimEvent::Killed { .. })));
    }

    // --- Buildings are damageable / destroyable targets -------------------------------------

    #[test]
    fn embodied_fire_damages_then_destroys_enemy_building() {
        let mut world = World::new();
        let terrain = Terrain::open();
        let shooter = spawn_unit(&mut world, 0, 0, Faction::Player, 100, rifle(10, 50, 0));
        world.input_source[shooter.index as usize] = InputSource::Embodied;
        // Enemy structure dead ahead on +X, distance 5 (< range 10), 100 HP.
        let building = spawn_building(&mut world, 5, 0, Faction::Enemy, 100);

        let mut events = Vec::new();
        // First shot: 50 dmg at full (open) multiplier — building survives, records the attacker.
        fire(&mut world, &terrain, shooter, aim_pos_x(), &mut events);
        assert_eq!(world.health[building.index as usize].cur, fx(50), "building takes fire");
        assert_eq!(world.last_attacker[building.index as usize], Some(shooter));
        assert_eq!(events.len(), 1, "one Damaged event for the hit building");

        // Second shot drops it to zero; the death pass then despawns it (same as a unit).
        fire(&mut world, &terrain, shooter, aim_pos_x(), &mut events);
        assert!(world.health[building.index as usize].is_dead());
        let mut rng = Rng::new(1);
        combat_system(&mut world, &terrain, &mut rng, &mut events);
        assert!(!world.is_alive(building), "a razed building is despawned exactly like a unit");
        assert!(
            events
                .iter()
                .any(|e| matches!(e, SimEvent::Killed { entity, .. } if *entity == building)),
            "destruction emits a Killed event for the building"
        );
    }

    #[test]
    fn embodied_fire_hits_a_unit_screening_a_lower_index_building() {
        // The docstring promise, isolated in one shot: a unit in front of an enemy structure is hit
        // before the structure (nearest wins), even when the structure has the lower index. This is
        // the skirmish case — a soldier in front of the enemy base — distilled to a single call.
        let mut world = World::new();
        let terrain = Terrain::open();
        let shooter = spawn_unit(&mut world, 0, 0, Faction::Player, 100, rifle(20, 25, 0));
        world.input_source[shooter.index as usize] = InputSource::Embodied;
        // The building is spawned first → lower index, and sits farther back on the same +X line.
        let building = spawn_building(&mut world, 12, 0, Faction::Enemy, 100);
        let screen = spawn_unit(&mut world, 4, 0, Faction::Enemy, 100, Weapon::default());

        let mut events = Vec::new();
        fire(&mut world, &terrain, shooter, aim_pos_x(), &mut events);
        assert_eq!(
            world.health[screen.index as usize].cur,
            fx(75),
            "the nearer unit is hit",
        );
        assert_eq!(
            world.health[building.index as usize].cur,
            fx(100),
            "the lower-index building behind it is spared",
        );
    }

    #[test]
    fn attack_moving_unit_razes_enemy_building() {
        // RTS auto-combat: a FireAtWill unit in range of an enemy building destroys it. The
        // AttackMove order documents the scenario (movement is resolved by `orders`, not here);
        // `combat_system` is what applies the damage and despawns the structure.
        let mut world = World::new();
        let terrain = Terrain::open();
        let shooter = spawn_unit(&mut world, 0, 0, Faction::Player, 100, rifle(10, 50, 0));
        world.stance[shooter.index as usize] = Stance::FireAtWill;
        world.order[shooter.index as usize] =
            crate::components::Order::AttackMove(Vec2::new(fx(5), fx(0)));
        let building = spawn_building(&mut world, 3, 0, Faction::Enemy, 100);

        let mut events = Vec::new();
        // Tick 1: 50 dmg lands on the enemy building.
        run(&mut world, &terrain, &mut events);
        assert_eq!(
            world.health[building.index as usize].cur,
            fx(50),
            "a combat unit auto-targets the enemy building"
        );
        // Tick 2: the second 50 dmg razes it and the death pass despawns it the same tick.
        run(&mut world, &terrain, &mut events);
        assert!(!world.is_alive(building), "attack-moving unit razes the enemy building");
        assert!(events.iter().any(|e| matches!(e, SimEvent::Killed { .. })));
    }

    #[test]
    fn friendly_building_is_never_targeted() {
        let mut world = World::new();
        let terrain = Terrain::open();
        let shooter = spawn_unit(&mut world, 0, 0, Faction::Player, 100, rifle(10, 50, 0));
        world.input_source[shooter.index as usize] = InputSource::Embodied;
        world.stance[shooter.index as usize] = Stance::FireAtWill;
        // A friendly structure squarely inside the aim cone and in weapon range.
        let friendly = spawn_building(&mut world, 5, 0, Faction::Player, 100);

        let mut events = Vec::new();
        // Embodied fire must not hit an own-faction building (no friendly fire on the base).
        fire(&mut world, &terrain, shooter, aim_pos_x(), &mut events);
        assert_eq!(
            world.health[friendly.index as usize].cur,
            fx(100),
            "embodied fire spares the friendly building"
        );
        // Auto-combat must not auto-target it either (the shooter goes order-driven for this pass).
        world.input_source[shooter.index as usize] = InputSource::Orders;
        for _ in 0..5 {
            run(&mut world, &terrain, &mut events);
        }
        assert_eq!(
            world.health[friendly.index as usize].cur,
            fx(100),
            "a FireAtWill unit never engages a friendly building"
        );
        assert!(events.is_empty(), "no damage events against a friendly building");
    }

    #[test]
    fn destroyed_building_stops_producing() {
        use crate::components::{Building, BuildingKind, ProductionItem, UnitKind};
        use crate::economy::{economy_system, Resources};
        use crate::territory::Territory;

        let mut world = World::new();
        let terrain = Terrain::open();
        // A built enemy camp with a Rifleman one tick from completion.
        let camp = spawn_building(&mut world, 5, 0, Faction::Enemy, 100);
        let ci = camp.index as usize;
        world.building[ci] = Building {
            kind: BuildingKind::Camp,
            level: 0,
            build_ticks_left: 0,
            queue: vec![ProductionItem {
                kind: UnitKind::Rifleman,
                ticks_left: 1,
            }],
        };

        // An embodied player razes the camp before the unit finishes.
        let shooter = spawn_unit(&mut world, 0, 0, Faction::Player, 100, rifle(10, 100, 0));
        world.input_source[shooter.index as usize] = InputSource::Embodied;
        let mut events = Vec::new();
        fire(&mut world, &terrain, shooter, aim_pos_x(), &mut events);
        assert!(world.health[ci].is_dead());
        let mut rng = Rng::new(1);
        combat_system(&mut world, &terrain, &mut rng, &mut events);
        assert!(!world.is_alive(camp), "razed camp is despawned");

        // The economy must skip the despawned camp: no unit spawns, no UnitProduced event.
        let mut resources = Resources::new(0);
        let territory = Territory::empty();
        let mut eco_events = Vec::new();
        economy_system(
            &mut world,
            &mut resources,
            &territory,
            &mut eco_events,
            &mut rng,
            0,
            1,
        );
        let units = (0..world.capacity())
            .filter(|&i| world.is_index_alive(i) && world.kind[i] == EntityKind::Unit)
            .count();
        assert_eq!(units, 1, "only the shooter remains; the razed camp produced nothing");
        assert!(
            !eco_events
                .iter()
                .any(|e| matches!(e, SimEvent::UnitProduced { .. })),
            "a destroyed camp emits no production"
        );
    }

    #[test]
    fn world_entity_recovers_handle_after_recycle() {
        // combat builds `last_attacker`/event handles via `World::entity`; confirm it returns
        // the live generation for a recycled slot and `None` once the slot is dead.
        let mut world = World::new();
        let a = world.spawn();
        world.despawn(a);
        let b = world.spawn();
        world.despawn(b);
        let c = world.spawn();
        assert_eq!(c.generation, 2);
        assert_eq!(world.entity(c.index as usize), Some(c));
        world.despawn(c);
        assert_eq!(world.entity(0), None);
    }

    // --- Embodied magazine: ammo + reload (resolve_fire, mag_size > 0) -------------------------

    /// Spawn an embodied shooter with a magazine-armed weapon at the origin, plus a fat enemy
    /// dead ahead on +X so each pull either lands a `Damaged` or is a silent dry/blocked click.
    fn mag_scene(range: i32, mag: u16, reload: u16) -> (World, Terrain, Entity, Entity) {
        let mut world = World::new();
        let terrain = Terrain::open();
        let shooter = spawn_unit(&mut world, 0, 0, Faction::Player, 100, mag_rifle(range, 5, mag, reload));
        world.input_source[shooter.index as usize] = InputSource::Embodied;
        let enemy = spawn_unit(&mut world, 4, 0, Faction::Enemy, 10_000, Weapon::default());
        (world, terrain, shooter, enemy)
    }

    #[test]
    fn embodied_fire_decrements_ammo_per_shot() {
        let (mut world, terrain, shooter, _enemy) = mag_scene(10, 3, 90);
        let mut events = Vec::new();
        assert_eq!(world.weapon[shooter.index as usize].ammo, 3);
        fire(&mut world, &terrain, shooter, aim_pos_x(), &mut events);
        assert_eq!(world.weapon[shooter.index as usize].ammo, 2, "one round spent per hit");
        fire(&mut world, &terrain, shooter, aim_pos_x(), &mut events);
        assert_eq!(world.weapon[shooter.index as usize].ammo, 1);
        assert_eq!(events.len(), 2, "both shots landed while ammo remained");
    }

    #[test]
    fn empty_magazine_blocks_fire_no_damage_no_event() {
        let (mut world, terrain, shooter, enemy) = mag_scene(10, 1, 90);
        let mut events = Vec::new();
        // First (and only) round lands.
        fire(&mut world, &terrain, shooter, aim_pos_x(), &mut events);
        assert_eq!(world.weapon[shooter.index as usize].ammo, 0);
        let hp_after_first = world.health[enemy.index as usize].cur;
        // Dry click: no hit, no event, ammo stays 0 (and no cooldown was spent — cooldown is 0).
        fire(&mut world, &terrain, shooter, aim_pos_x(), &mut events);
        assert_eq!(events.len(), 1, "the empty pull emits nothing");
        assert_eq!(world.health[enemy.index as usize].cur, hp_after_first, "no damage on empty");
    }

    #[test]
    fn reload_refills_magazine_after_reload_ticks_in_upkeep() {
        // No enemy needed: drive only the upkeep timer. Start an empty mag with a 3-tick reload
        // in progress (as `Command::Reload` would set), then run combat upkeep three times.
        let mut world = World::new();
        let terrain = Terrain::open();
        let shooter = spawn_unit(&mut world, 0, 0, Faction::Player, 100, mag_rifle(10, 5, 8, 3));
        world.input_source[shooter.index as usize] = InputSource::Embodied;
        let i = shooter.index as usize;
        world.weapon[i].ammo = 0;
        world.weapon[i].reload_left = 3;

        let mut events = Vec::new();
        run(&mut world, &terrain, &mut events); // 3 -> 2
        assert_eq!(world.weapon[i].reload_left, 2);
        assert_eq!(world.weapon[i].ammo, 0, "no refill until the timer expires");
        run(&mut world, &terrain, &mut events); // 2 -> 1
        assert_eq!(world.weapon[i].ammo, 0);
        run(&mut world, &terrain, &mut events); // 1 -> 0: refill
        assert_eq!(world.weapon[i].reload_left, 0);
        assert_eq!(world.weapon[i].ammo, 8, "magazine refilled to mag_size");
    }

    #[test]
    fn cannot_fire_while_reloading_even_with_ammo() {
        let (mut world, terrain, shooter, enemy) = mag_scene(10, 8, 4);
        let i = shooter.index as usize;
        world.weapon[i].reload_left = 4; // reload in progress despite rounds left
        let mut events = Vec::new();
        fire(&mut world, &terrain, shooter, aim_pos_x(), &mut events);
        assert!(events.is_empty(), "a reloading weapon cannot fire");
        assert_eq!(world.health[enemy.index as usize].cur, Fixed::from_int(10_000));
        // Finish the reload (4 upkeep ticks), then the same pull lands.
        for _ in 0..4 {
            run(&mut world, &terrain, &mut events);
        }
        assert_eq!(world.weapon[i].reload_left, 0);
        fire(&mut world, &terrain, shooter, aim_pos_x(), &mut events);
        assert!(!events.is_empty(), "fire resumes once the reload completes");
    }

    // --- All-unit ammo: AI auto-combat rations rounds, auto-reloads, runs dry (D67) -----------

    #[test]
    fn ai_auto_combat_spends_a_round_per_shot() {
        let mut world = World::new();
        let terrain = Terrain::open();
        // mag_rifle has cooldown 0, so the only limit on the rate of fire is ammo.
        let shooter = spawn_unit(&mut world, 0, 0, Faction::Player, 100, mag_rifle(10, 5, 3, 4));
        world.stance[shooter.index as usize] = Stance::FireAtWill;
        let si = shooter.index as usize;
        spawn_unit(&mut world, 4, 0, Faction::Enemy, 10_000, Weapon::default());

        let mut events = Vec::new();
        assert_eq!(world.weapon[si].ammo, 3, "spawns with a full magazine");
        run(&mut world, &terrain, &mut events);
        assert_eq!(world.weapon[si].ammo, 2, "AI auto-combat spends a round per shot (not infinite)");
        run(&mut world, &terrain, &mut events);
        assert_eq!(world.weapon[si].ammo, 1);
    }

    #[test]
    fn ai_dry_magazine_auto_reloads_from_reserve() {
        let mut world = World::new();
        let terrain = Terrain::open();
        // mag 2, 3-tick reload, reserve = mag*3 = 6. No enemy: isolate the upkeep auto-reload from
        // any firing so the timing is unambiguous. Empty the magazine by hand.
        let shooter = spawn_unit(&mut world, 0, 0, Faction::Player, 100, mag_rifle(10, 5, 2, 3));
        world.stance[shooter.index as usize] = Stance::FireAtWill;
        let si = shooter.index as usize;
        world.weapon[si].ammo = 0;

        let mut events = Vec::new();
        // Tick 1 upkeep sees AI + dry + reserve > 0 → auto-arms the reload (does not also tick it).
        run(&mut world, &terrain, &mut events);
        assert_eq!(world.weapon[si].reload_left, 3, "AI auto-starts a reload when dry with reserve");
        assert_eq!(world.weapon[si].ammo, 0, "still empty mid-reload");
        // Drive the 3-tick reload to completion: 3 -> 2 -> 1 -> 0, then the magazine refills.
        run(&mut world, &terrain, &mut events); // 3 -> 2
        run(&mut world, &terrain, &mut events); // 2 -> 1
        run(&mut world, &terrain, &mut events); // 1 -> 0: draw from reserve
        assert_eq!(world.weapon[si].reload_left, 0, "reload complete");
        assert_eq!(world.weapon[si].ammo, 2, "magazine refilled from reserve");
        assert_eq!(world.weapon[si].reserve, 4, "two rounds drawn from the reserve of 6");
    }

    #[test]
    fn ai_unit_with_empty_reserve_goes_combat_ineffective() {
        let mut world = World::new();
        let terrain = Terrain::open();
        // One round loaded, NOTHING in reserve: it shoots once, then can never reload or fire again.
        let mut weapon = mag_rifle(10, 5, 1, 3);
        weapon.reserve = 0;
        weapon.reserve_max = 0;
        let shooter = spawn_unit(&mut world, 0, 0, Faction::Player, 100, weapon);
        world.stance[shooter.index as usize] = Stance::FireAtWill;
        let si = shooter.index as usize;
        let enemy = spawn_unit(&mut world, 4, 0, Faction::Enemy, 10_000, Weapon::default());

        let mut events = Vec::new();
        run(&mut world, &terrain, &mut events); // fires its one round
        assert_eq!(world.weapon[si].ammo, 0);
        let hp_after_one = world.health[enemy.index as usize].cur;
        assert!(hp_after_one < Fixed::from_int(10_000), "the one loaded round landed");
        for _ in 0..30 {
            run(&mut world, &terrain, &mut events);
        }
        assert_eq!(world.weapon[si].reload_left, 0, "no reload can start with an empty reserve");
        assert_eq!(
            world.health[enemy.index as usize].cur, hp_after_one,
            "a fully dry unit deals no further damage (combat-ineffective until resupply)"
        );
    }

    #[test]
    fn embodied_unit_never_auto_reloads() {
        let mut world = World::new();
        let terrain = Terrain::open();
        let shooter = spawn_unit(&mut world, 0, 0, Faction::Player, 100, mag_rifle(10, 5, 2, 3));
        world.input_source[shooter.index as usize] = InputSource::Embodied;
        let si = shooter.index as usize;
        world.weapon[si].ammo = 0; // empty mag, reserve still full
        let mut events = Vec::new();
        for _ in 0..10 {
            run(&mut world, &terrain, &mut events);
        }
        assert_eq!(world.weapon[si].reload_left, 0, "the embodied player reloads manually, never auto");
        assert_eq!(world.weapon[si].ammo, 0, "no auto-refill for the embodied unit");
    }

    // --- Crouch posture: tighter cone + extended range (resolve_fire) --------------------------

    #[test]
    fn crouched_cone_is_tighter_a_standing_shot_lands_a_crouched_one_misses() {
        // Target ~26.6° off the +X aim — inside the ~30° standing half-cone, outside the ~18°
        // crouched one. Same world, only posture differs.
        let mut world = World::new();
        let terrain = Terrain::open();
        let shooter = spawn_unit(&mut world, 0, 0, Faction::Player, 100, rifle(15, 25, 0));
        world.input_source[shooter.index as usize] = InputSource::Embodied;
        let enemy = spawn_unit(&mut world, 10, 5, Faction::Enemy, 100, Weapon::default());

        // Standing: the off-axis target is within the wider cone -> hit.
        let mut events = Vec::new();
        world.posture[shooter.index as usize] = Posture::Standing;
        fire(&mut world, &terrain, shooter, aim_pos_x(), &mut events);
        assert_eq!(world.health[enemy.index as usize].cur, fx(75), "standing hip-fire clips it");

        // Reset health; crouch tightens the cone past the target's bearing -> miss.
        world.health[enemy.index as usize].cur = fx(100);
        events.clear();
        world.posture[shooter.index as usize] = Posture::Crouched;
        fire(&mut world, &terrain, shooter, aim_pos_x(), &mut events);
        assert_eq!(world.health[enemy.index as usize].cur, fx(100), "crouch demands tighter aim");
        assert!(events.is_empty());
    }

    #[test]
    fn crouched_extends_weapon_range() {
        // On-axis target at distance 12: beyond the base range 10, inside the crouched 12.5.
        let mut world = World::new();
        let terrain = Terrain::open();
        let shooter = spawn_unit(&mut world, 0, 0, Faction::Player, 100, rifle(10, 25, 0));
        world.input_source[shooter.index as usize] = InputSource::Embodied;
        let enemy = spawn_unit(&mut world, 12, 0, Faction::Enemy, 100, Weapon::default());

        let mut events = Vec::new();
        world.posture[shooter.index as usize] = Posture::Standing;
        fire(&mut world, &terrain, shooter, aim_pos_x(), &mut events);
        assert_eq!(world.health[enemy.index as usize].cur, fx(100), "out of base range standing");

        world.posture[shooter.index as usize] = Posture::Crouched;
        fire(&mut world, &terrain, shooter, aim_pos_x(), &mut events);
        assert_eq!(world.health[enemy.index as usize].cur, fx(75), "crouch reaches the further target");
    }

    // --- Tank embodiment P4 (D55): all-unit armour facing ------------------------------------

    use crate::components::Armor;

    fn armored(front: i32, side: i32, rear: i32) -> Armor {
        Armor { front: fx(front), side: fx(side), rear: fx(rear) }
    }

    /// An `Angle` for `deg` degrees (binary radians), float-free.
    fn deg(d: i32) -> Angle {
        Angle(trig::ANGLE_FULL * d / 360)
    }

    #[test]
    fn facing_multiplier_unarmored_is_exactly_one() {
        // The load-bearing safety property: an unarmoured defender (the default for every
        // Rifleman/Heavy/building) takes the multiplier as EXACTLY one, for any shot direction and
        // any penetration (even zero) — so existing balance/tests are byte-for-byte unchanged.
        let bare = Armor::default();
        for &(dx, dy) in &[(1, 0), (-1, 0), (0, 1), (0, -1), (3, 4), (-2, 5)] {
            let dir = Vec2::new(fx(dx), fx(dy));
            assert_eq!(
                facing_penetration_multiplier(dir, Angle(0), Fixed::ZERO, bare),
                Fixed::ONE
            );
            assert_eq!(
                facing_penetration_multiplier(dir, deg(137), fx(99), bare),
                Fixed::ONE,
                "unarmoured is unity regardless of facing/penetration"
            );
        }
    }

    #[test]
    fn shot_facet_selects_by_hull_heading_with_correct_boundaries() {
        // A shot travelling +X; the struck facet depends only on where the hull points. The
        // front/rear arcs are 120° wide (±60° of head-on/tail), the sides the 60° wedges between.
        let east = Vec2::new(Fixed::ONE, Fixed::ZERO);
        // Hull facing +X (0°): the +X shot catches it from behind → Rear.
        assert_eq!(shot_facet(east, deg(0)), Facet::Rear);
        // Hull facing -X (180°): the +X shot meets the front head-on.
        assert_eq!(shot_facet(east, deg(180)), Facet::Front);
        // Hull facing +Y (90°): broadside → Side.
        assert_eq!(shot_facet(east, deg(90)), Facet::Side);
        // Side/Rear boundary at 60°: just inside (55°) is Rear, just outside (65°) is Side.
        assert_eq!(shot_facet(east, deg(55)), Facet::Rear, "inside the rear arc");
        assert_eq!(shot_facet(east, deg(65)), Facet::Side, "past the rear arc → side");
        // Front/Side boundary at 120°: just outside (115°) is Side, just inside (125°) is Front.
        assert_eq!(shot_facet(east, deg(115)), Facet::Side, "before the front arc → side");
        assert_eq!(shot_facet(east, deg(125)), Facet::Front, "inside the front arc");
    }

    #[test]
    fn facing_multiplier_bounces_ramps_and_pens() {
        // Hull facing -X so a +X shot strikes the FRONT facet; vary penetration vs front armour.
        let east = Vec2::new(Fixed::ONE, Fixed::ZERO);
        let front_on = deg(180);
        // pen >= armour → full damage.
        assert_eq!(
            facing_penetration_multiplier(east, front_on, fx(10), armored(10, 1, 1)),
            Fixed::ONE,
            "penetration meets armour → full"
        );
        // 2·pen < armour → hard bounce.
        assert_eq!(
            facing_penetration_multiplier(east, front_on, fx(3), armored(10, 1, 1)),
            Fixed::ZERO,
            "armour overmatches → bounce"
        );
        // 2·pen == armour → still a bounce (the boundary is inclusive).
        assert_eq!(
            facing_penetration_multiplier(east, front_on, fx(3), armored(6, 1, 1)),
            Fixed::ZERO,
            "double-armour boundary bounces"
        );
        // a/2 < pen < a → reduced: (2·3 − 4)/4 = 1/2.
        assert_eq!(
            facing_penetration_multiplier(east, front_on, fx(3), armored(4, 1, 1)),
            Fixed::from_ratio(1, 2),
            "close penetration → reduced ramp"
        );
    }

    #[test]
    fn combat_system_armored_target_bounces_frontal_pens_flank_and_rear() {
        // AI hitscan (the engage pass) honours armour facing. An armoured enemy faces +X with a
        // thick front and thin flank/rear; a modest-penetration shooter bounces off the front but
        // pens the flank and rear.
        fn dmg(shooter_x: i32, shooter_y: i32) -> Fixed {
            let mut world = World::new();
            let terrain = Terrain::open();
            let target = spawn_unit(&mut world, 0, 0, Faction::Enemy, 1000, Weapon::default());
            let ti = target.index as usize;
            world.stance[ti] = Stance::HoldFire;
            world.hull_heading[ti] = deg(0); // front faces +X
            world.armor[ti] = armored(100, 1, 1);
            let shooter = spawn_unit(
                &mut world,
                shooter_x,
                shooter_y,
                Faction::Player,
                100,
                Weapon {
                    range: fx(50),
                    damage: fx(20),
                    penetration: fx(10),
                    ..Default::default()
                },
            );
            world.stance[shooter.index as usize] = Stance::FireAtWill;
            let before = world.health[ti].cur;
            let mut events = Vec::new();
            run(&mut world, &terrain, &mut events);
            before - world.health[ti].cur
        }
        assert_eq!(dmg(10, 0), Fixed::ZERO, "frontal shot bounces off the thick front");
        assert_eq!(dmg(-10, 0), fx(20), "rear shot pens for full damage");
        assert_eq!(dmg(0, 10), fx(20), "flank shot pens for full damage");
    }

    #[test]
    fn resolve_fire_applies_armor_facing() {
        // Embodied hitscan also honours armour facing (the same shared resolver). Shot aims +X at a
        // tank 5 ahead; the tank's hull heading decides the struck facet.
        fn dmg(hull: Angle) -> Fixed {
            let mut world = World::new();
            let terrain = Terrain::open();
            let shooter = spawn_unit(
                &mut world,
                0,
                0,
                Faction::Player,
                100,
                Weapon {
                    range: fx(50),
                    damage: fx(20),
                    penetration: fx(10),
                    ..Default::default()
                },
            );
            world.input_source[shooter.index as usize] = InputSource::Embodied;
            let target = spawn_unit(&mut world, 5, 0, Faction::Enemy, 1000, Weapon::default());
            let ti = target.index as usize;
            world.hull_heading[ti] = hull;
            world.armor[ti] = armored(100, 1, 1);
            let before = world.health[ti].cur;
            let mut events = Vec::new();
            resolve_fire(&mut world, &terrain, shooter.index as usize, aim_pos_x(), &mut events);
            before - world.health[ti].cur
        }
        // Hull faces -X → the +X shot meets the front → bounce.
        assert_eq!(dmg(deg(180)), Fixed::ZERO, "embodied frontal shot bounces");
        // Hull faces +X → the +X shot catches the rear → full pen.
        assert_eq!(dmg(deg(0)), fx(20), "embodied rear shot pens");
    }

    /// End-to-end: a real auto-fire shot lights the render snapshot's `firing` flag (so the debug
    /// overlay can draw a muzzle flash), and it clears once the cooldown decays past the flash
    /// window. Couples the presentation flag to actual combat, beyond the pure `weapon_recently_fired`
    /// seam unit-tested in `snapshot`.
    #[test]
    fn snapshot_firing_flag_lights_on_the_shot_tick_then_clears() {
        use crate::snapshot::{Snapshot, MUZZLE_FLASH_TICKS};
        use crate::territory::Territory;

        let mut world = World::new();
        let terrain = Terrain::open();
        let shooter = spawn_unit(&mut world, 0, 0, Faction::Player, 100, rifle(10, 25, 30));
        world.stance[shooter.index as usize] = Stance::FireAtWill;
        // A high-HP target that holds fire: it survives so the shooter keeps a live target, and it
        // never fires itself (so only the shooter's flag should light).
        let target = spawn_unit(&mut world, 3, 0, Faction::Enemy, 1000, Weapon::default());
        world.stance[target.index as usize] = Stance::HoldFire;

        let firing = |w: &World| {
            Snapshot::capture(w, &Territory::default(), &[], 0)
                .units
                .iter()
                .find(|u| u.entity_index == shooter.index)
                .unwrap()
                .firing
        };

        assert!(!firing(&world), "an idle unit reads as not firing");

        // One combat tick fires (cooldown_ticks = 30 > flash window), lighting the flag.
        let mut events = Vec::new();
        run(&mut world, &terrain, &mut events);
        assert!(firing(&world), "the unit that just fired lights the firing flag");
        // The lone non-firing target never lights.
        let target_firing = Snapshot::capture(&world, &Territory::default(), &[], 0)
            .units
            .iter()
            .find(|u| u.entity_index == target.index)
            .unwrap()
            .firing;
        assert!(!target_firing, "a hold-fire unit never lights the flag");

        // Run upkeep-only ticks (cooldown decays 30→22, no re-fire) until past the window: clears.
        for _ in 0..MUZZLE_FLASH_TICKS {
            run(&mut world, &terrain, &mut events);
        }
        assert!(!firing(&world), "the flag clears once the cooldown decays past the flash window");
    }
}
