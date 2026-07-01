//! The **tank-duel debug scenario** — the headless half of the "debug versions" the player asked
//! for: load two tanks into a tiny world, fire the gun, and *validate that the hitboxes work*.
//!
//! It seeds [`gonedark_core::scenario::seed_duel`] (the SAME scene the desktop `app`/`viz-runner`
//! render, so this report and a screenshot describe one world), embodies the player tank, and
//! drives its ballistic main gun on cadence — exercising the armour-facet hitbox model
//! **end-to-end through the real shell pipeline** (`fire_ballistic` → `projectile_system` →
//! `apply_impact`), not by calling the resolver directly. Two phases prove the lesson the whole
//! D55 model exists to teach — *angle the hull, flank to kill*:
//!
//! - **Phase A** — the enemy faces the player head-on. Every `+X` shell strikes the **Front**
//!   facet, whose armour overmatches the gun's penetration → a clean **bounce** (0 damage). The
//!   enemy is untouched no matter how many rounds land.
//! - **Phase B** — the enemy is turned to expose its flank. The *identical* `+X` shell now strikes
//!   the **Side** facet, which the gun pens → full damage, and two hits kill it.
//!
//! Output, mirroring `--time`/`--metrics`: the `<tick> <checksum>` stream goes to **stdout**
//! (so the duel is determinism-covered exactly like `phase2`/`stress`), and the human-readable
//! per-event report goes to **stderr** — it never touches stdout, so it cannot affect determinism.

use gonedark_core::combat::{shot_facet, Facet};
use gonedark_core::components::Vec2;
use gonedark_core::ecs::Entity;
use gonedark_core::event::SimEvent;
use gonedark_core::fixed::Fixed;
use gonedark_core::scenario::{self, DUEL_TANK_HP};
use gonedark_core::sim::{Command, Sim};
use gonedark_core::trig::{Angle, ANGLE_FULL};

/// Tick the player is possessed on (a ballistic `Fire` only launches a shell from an *embodied*
/// shooter — `sim::Command::Fire`'s P3 dispatch).
const EMBODY_TICK: u64 = 1;
/// First shot; thereafter the gun fires every time it comes off cooldown (see [`fire_tick`]).
const FIRST_FIRE_TICK: u64 = 2;
/// Tick the enemy is rotated to expose its flank — the Phase A → Phase B boundary. Late enough
/// that every Phase A shell has already crossed the gap and bounced.
const FLANK_TICK: u64 = 90;

/// Does the gun fire this tick? It fires on [`FIRST_FIRE_TICK`] and then once per weapon cooldown,
/// so each shot lands with the gun ready (no wasted dry clicks in the report).
fn fire_tick(tick: u64) -> bool {
    tick >= FIRST_FIRE_TICK
        && (tick - FIRST_FIRE_TICK).is_multiple_of(scenario::DUEL_GUN_COOLDOWN as u64)
}

/// The player's aim each shot: straight `+X`, into the enemy. A unit `Fixed` vector, quantized
/// already (it's exact), so it crosses into the sim like any embodied `Fire` (invariant #1).
fn aim_plus_x() -> Vec2 {
    Vec2::new(Fixed::ONE, Fixed::ZERO)
}

/// One thing worth narrating in the report.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Note {
    /// The player pulled the trigger (a shell was launched).
    Fired,
    /// The enemy was turned to expose its flank (Phase A → B).
    Flanked,
    /// A shell struck the enemy: which facet, how much damage got through, the enemy's HP after.
    Impact {
        facet: Facet,
        amount: Fixed,
        enemy_hp: Fixed,
    },
    /// The enemy died.
    EnemyKilled,
}

/// The full outcome of a duel run — the checksum stream (for the determinism assertion) plus the
/// narrated notes and the two load-bearing measurements the harness asserts on.
struct DuelLog {
    checksums: Vec<u64>,
    notes: Vec<(u64, Note)>,
    /// Enemy HP measured at [`FLANK_TICK`], after every Phase A (front) shell has resolved — the
    /// proof that head-on fire does nothing.
    enemy_hp_before_flank: Fixed,
    /// The tick the enemy died on, if it did. Phase B (flank) fire should make this `Some`.
    enemy_killed_tick: Option<u64>,
}

/// Is `e` still a live entity in `sim`?
fn alive(sim: &Sim, e: Entity) -> bool {
    sim.world.is_alive(e)
}

/// Run the duel for `ticks` ticks and collect the outcome. Pure (no I/O) so the tests can assert
/// on it directly — the printed [`run`] is a thin wrapper over this.
fn simulate(ticks: u64) -> DuelLog {
    let mut sim = Sim::new(0xD0E1);
    let duel = scenario::seed_duel(&mut sim);

    let mut log = DuelLog {
        checksums: vec![sim.checksum()],
        notes: Vec::new(),
        enemy_hp_before_flank: DUEL_TANK_HP,
        enemy_killed_tick: None,
    };

    for tick in 1..ticks {
        // Build this tick's command list. Possess the player, fire on cadence; the enemy holds
        // fire (HoldFire, literal executor — invariant #3) so the only shells are the player's.
        let mut cmds: Vec<Command> = Vec::new();
        if tick == EMBODY_TICK {
            cmds.push(Command::Embody { entity: duel.player });
        }
        if tick > EMBODY_TICK && fire_tick(tick) && alive(&sim, duel.enemy) {
            cmds.push(Command::Fire {
                entity: duel.player,
                dir: aim_plus_x(),
            });
            log.notes.push((tick, Note::Fired));
        }

        // Host-side debug action at the phase boundary: turn the enemy to expose its flank to the
        // unchanged `+X` shots. A debug rig directly reorients the target — the player achieves the
        // same flank by *driving* in the playable sandbox; the shell/impact/facet path under test is
        // identical either way.
        if tick == FLANK_TICK && alive(&sim, duel.enemy) {
            sim.world.hull_heading[duel.enemy.index as usize] = Angle(ANGLE_FULL / 4); // face +Y
            log.notes.push((tick, Note::Flanked));
        }

        sim.step(&cmds);

        // Record impacts on the enemy this tick, tagging each with the facet the shell struck
        // (recomputed from the `+X` shot direction against the enemy's hull heading at impact). The
        // SoA slot stays readable by index even on the kill tick, so no liveness guard is needed.
        let enemy_hull = sim.world.hull_heading[duel.enemy.index as usize];
        for ev in sim.events() {
            match ev {
                SimEvent::Damaged { entity, amount, .. } if *entity == duel.enemy => {
                    let enemy_hp = if alive(&sim, duel.enemy) {
                        sim.world.health[duel.enemy.index as usize].cur
                    } else {
                        Fixed::ZERO
                    };
                    log.notes.push((
                        tick,
                        Note::Impact {
                            facet: shot_facet(aim_plus_x(), enemy_hull),
                            amount: *amount,
                            enemy_hp,
                        },
                    ));
                }
                SimEvent::Killed { entity, .. } if *entity == duel.enemy => {
                    log.notes.push((tick, Note::EnemyKilled));
                    if log.enemy_killed_tick.is_none() {
                        log.enemy_killed_tick = Some(tick);
                    }
                }
                _ => {}
            }
        }

        // Snapshot the enemy's HP the instant before the flank turn — the Phase A verdict.
        if tick == FLANK_TICK - 1 && alive(&sim, duel.enemy) {
            log.enemy_hp_before_flank = sim.world.health[duel.enemy.index as usize].cur;
        }

        log.checksums.push(sim.checksum());
    }

    log
}

/// `Fixed` → a short decimal for the report (host-side display only — never re-enters the sim, so
/// the float is harmless, exactly like `--time`'s millis).
fn show(f: Fixed) -> f64 {
    f.to_bits() as f64 / Fixed::SCALE as f64
}

fn facet_name(f: Facet) -> &'static str {
    match f {
        Facet::Front => "FRONT",
        Facet::Side => "SIDE",
        Facet::Rear => "REAR",
    }
}

/// Run the duel and print: the `<tick> <checksum>` stream on **stdout**, the narrated report on
/// **stderr**. The default `ticks` (200) is enough to cover both phases and the kill.
pub fn run(ticks: u64) {
    let log = simulate(ticks);

    // The determinism-covered stream (stdout), identical shape to the other scenarios.
    for (tick, sum) in log.checksums.iter().enumerate() {
        println!("{tick} {sum:016x}");
    }

    // The watchable report (stderr).
    eprintln!("== tank duel ==  (two {}-HP tanks; gun pen {} vs front {} / side {} / rear {})",
        show(DUEL_TANK_HP),
        show(scenario::DUEL_GUN_PENETRATION),
        show(scenario::DUEL_ARMOR_FRONT),
        show(scenario::DUEL_ARMOR_SIDE),
        show(scenario::DUEL_ARMOR_REAR),
    );
    for (tick, note) in &log.notes {
        match note {
            Note::Fired => eprintln!("  t{tick:>3}  player fires +X"),
            Note::Flanked => eprintln!("  t{tick:>3}  >>> enemy turned — flank now exposed <<<"),
            Note::Impact {
                facet,
                amount,
                enemy_hp,
            } => eprintln!(
                "  t{tick:>3}  shell hits {} facet  → {:>5.0} dmg   enemy HP {:>5.0}",
                facet_name(*facet),
                show(*amount),
                show(*enemy_hp),
            ),
            Note::EnemyKilled => eprintln!("  t{tick:>3}  *** enemy destroyed ***"),
        }
    }
    eprintln!(
        "phase A verdict: enemy HP before flank = {:.0} / {:.0}  ({})",
        show(log.enemy_hp_before_flank),
        show(DUEL_TANK_HP),
        if log.enemy_hp_before_flank == DUEL_TANK_HP {
            "head-on fire bounced — UNHARMED, as designed"
        } else {
            "WARNING: head-on fire dealt damage (front facet should bounce)"
        },
    );
    match log.enemy_killed_tick {
        Some(t) => eprintln!("phase B verdict: enemy destroyed on t{t} via flank pens"),
        None => eprintln!("phase B verdict: enemy SURVIVED (flank shots failed to pen — unexpected)"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The end-to-end hitbox property, asserted through the real shell pipeline: head-on shells
    /// bounce (enemy full HP through all of Phase A), flank shells pen and kill (Phase B).
    #[test]
    fn front_fire_bounces_then_flank_fire_kills() {
        let log = simulate(200);
        // Phase A: every front shell bounced — the enemy is untouched at the flank boundary.
        assert_eq!(
            log.enemy_hp_before_flank, DUEL_TANK_HP,
            "head-on fire must bounce off the frontal facet (0 damage)",
        );
        // Phase B: the flank shots penetrated and destroyed it.
        let killed = log
            .enemy_killed_tick
            .expect("flank fire should kill the enemy");
        assert!(killed > FLANK_TICK, "the kill must follow the flank turn");

        // Every Phase A impact is a FRONT-facet hit for exactly zero damage; the SIDE hits only
        // appear after the flank turn and carry real damage.
        for (tick, note) in &log.notes {
            if let Note::Impact { facet, amount, .. } = note {
                if *tick < FLANK_TICK {
                    assert_eq!(*facet, Facet::Front, "pre-flank hits strike the front");
                    assert_eq!(*amount, Fixed::ZERO, "front hits bounce for 0 damage");
                } else {
                    assert_eq!(*facet, Facet::Side, "post-flank hits strike the exposed side");
                    assert!(*amount > Fixed::ZERO, "side hits penetrate for real damage");
                }
            }
        }
    }

    /// Determinism (invariant #1): two duel runs produce a bit-identical checksum stream — the
    /// property the cross-arch CI matrix diffs. The final checksum is also pinned, so a regression
    /// that drifts to a wrong-but-stable stream is caught (defence-in-depth alongside the core
    /// `ballistic_pipeline_is_deterministic` golden). Re-pin only on an intended scene change.
    #[test]
    fn duel_is_deterministic() {
        assert_eq!(simulate(200).checksums, simulate(200).checksums);
        assert_eq!(
            simulate(200).checksums.last().copied(),
            // D67: re-pinned after the Weapon fold grew reserve + reserve_max (two more u32/slot).
            // D55 P5+P6: re-pinned after the fold grew a per-slot `dispersion` word + a loaded-shell
            // tag and the projectile fold grew a shell tag + splash pair. The duel tank fires from a
            // standstill (dispersion stays 0, AP default → identical shells); only the raw stream
            // value shifted by the appended fields, by design.
            // D85 (gunsmith breadth): re-pinned after the Weapon fold grew four Stock/Muzzle delta
            // words per slot (all zero here — the duel gun carries no loadout), so the fight is
            // byte-identical and only the raw stream value shifted, by design.
            Some(0x4209_3fde_b61c_0eb2),
        );
    }

    /// The schedule actually fires more than once and on the cooldown grid (guards the cadence
    /// helper against an off-by-one that would fire once and stop).
    #[test]
    fn fires_on_the_cooldown_cadence() {
        assert!(fire_tick(FIRST_FIRE_TICK));
        assert!(!fire_tick(FIRST_FIRE_TICK + 1));
        assert!(fire_tick(FIRST_FIRE_TICK + scenario::DUEL_GUN_COOLDOWN as u64));
        // At least one pre-flank and one post-flank shot exist, or the scene proves nothing.
        let log = simulate(200);
        let shots: Vec<u64> = log
            .notes
            .iter()
            .filter(|(_, n)| *n == Note::Fired)
            .map(|(t, _)| *t)
            .collect();
        assert!(shots.iter().any(|&t| t < FLANK_TICK), "a Phase A shot");
        assert!(shots.iter().any(|&t| t > FLANK_TICK), "a Phase B shot");
    }
}
