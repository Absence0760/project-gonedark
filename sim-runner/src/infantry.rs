//! The **infantry debug scenario** — the hitscan analogue of the tank `duel`: load a rifleman into
//! a tiny world and validate it *against everything* its combat model does.
//!
//! Two parts, both over the real `Sim::step` pipeline:
//!
//! 1. **The embodied scene** ([`simulate_scene`]) — seeds [`gonedark_core::scenario::seed_infantry`]
//!    (the SAME scene `app --scene infantry` renders), possesses the player rifleman, and fires `+X`
//!    on cadence at a row of HoldFire dummies each positioned to isolate one mechanic. It proves, in
//!    one run: **range** (the far dummy is unreachable until…), **crouch** (…the player crouches and
//!    the range bonus reaches it), the aim **cone** (the flank dummy, off-axis, is never hit),
//!    **cover** (the Light-cover dummy takes half damage), and **line of sight** (the walled dummy,
//!    behind Heavy cover, is never hit).
//! 2. **The auto-combat battery** ([`battery`]) — four focused fresh-sim checks the embodied scene
//!    can't show: **stance** (HoldFire holds, FireAtWill fires), **suppression** (a pinned unit can't
//!    fire), **retreat** (the health-threshold trigger installs FallBack), and the **reload** gate
//!    (an empty magazine dry-clicks; a reload refills it).
//!
//! Output mirrors `duel`/`--metrics`: the `<tick> <checksum>` stream of the embodied scene on
//! **stdout** (determinism-covered), the human-readable report on **stderr** (never touches stdout).

use gonedark_core::combat::SUPPRESSION_MAX;
use gonedark_core::components::{EntityKind, Faction, Order, Stance, UnitKind, Vec2};
use gonedark_core::ecs::Entity;
use gonedark_core::economy;
use gonedark_core::event::SimEvent;
use gonedark_core::fixed::Fixed;
use gonedark_core::scenario::{self, Infantry};
use gonedark_core::sim::{Command, Sim};

/// Tick the player is possessed on (an embodied `Fire` needs an Embodied input source).
const EMBODY_TICK: u64 = 1;
/// First shot; thereafter once per weapon cooldown.
const FIRST_FIRE_TICK: u64 = 2;

/// The produced Rifleman's cooldown, in ticks — the fire cadence (read from the real stat table so
/// the harness tracks any re-tune).
fn rifle_cooldown() -> u64 {
    economy::unit_stats(UnitKind::Rifleman).1.cooldown_ticks as u64
}

/// The player's aim each shot: straight `+X`, a unit `Fixed` vector (exact, no quantization needed).
fn plus_x() -> Vec2 {
    Vec2::new(Fixed::ONE, Fixed::ZERO)
}

fn alive(sim: &Sim, e: Entity) -> bool {
    sim.world.is_alive(e)
}

/// Current HP of `e` (`0` once dead/despawned).
fn hp(sim: &Sim, e: Entity) -> Fixed {
    if alive(sim, e) {
        sim.world.health[e.index as usize].cur
    } else {
        Fixed::ZERO
    }
}

/// `Fixed` → a short decimal for the report (host-side display only; never re-enters the sim).
fn show(f: Fixed) -> f64 {
    f.to_bits() as f64 / Fixed::SCALE as f64
}

/// Which seeded dummy is `e`, for the report + the per-mechanic assertions.
fn name_of(inf: &Infantry, e: Entity) -> Option<&'static str> {
    if e == inf.open {
        Some("open")
    } else if e == inf.cover {
        Some("cover")
    } else if e == inf.walled {
        Some("walled")
    } else if e == inf.far {
        Some("far")
    } else if e == inf.flank {
        Some("flank")
    } else {
        None
    }
}

/// The outcome of the embodied scene drive — the checksum stream, a narrated report, and the
/// per-mechanic flags the tests assert on.
#[derive(Default)]
struct SceneLog {
    checksums: Vec<u64>,
    report: Vec<(u64, String)>,
    /// Was the LoS-blocked dummy ever damaged? (Must stay false.)
    walled_hit: bool,
    /// Was the off-cone dummy ever damaged? (Must stay false.)
    flank_hit: bool,
    /// Was the far dummy hit while still standing? (Must stay false — it is out of base range.)
    far_hit_standing: bool,
    /// Was the far dummy hit after crouching? (Must become true — the crouch range bonus.)
    far_hit_crouched: bool,
    open_dead: bool,
    cover_dead: bool,
    far_dead: bool,
    /// Largest single hit landed on the open (full-damage) dummy.
    open_dmg_max: Fixed,
    /// Largest single hit landed on the Light-cover dummy (must be < `open_dmg_max`).
    cover_dmg_max: Fixed,
    crouch_tick: Option<u64>,
}

/// Drive the embodied infantry scene for `ticks` ticks. The player fires `+X` on cadence; once the
/// two standing targets (open, cover) are down it crouches to reach the far dummy. Pure (no I/O).
fn simulate_scene(ticks: u64) -> SceneLog {
    let mut sim = Sim::new(0x10FA);
    let inf = scenario::seed_infantry(&mut sim);
    let cd = rifle_cooldown();

    let mut log = SceneLog {
        checksums: vec![sim.checksum()],
        ..Default::default()
    };
    let mut crouched = false;

    for tick in 1..ticks {
        let mut cmds: Vec<Command> = Vec::new();
        if tick == EMBODY_TICK {
            cmds.push(Command::Embody { entity: inf.player });
        }
        // Crouch once both standing targets are down — the range bonus then reaches the far dummy.
        if !crouched && tick > EMBODY_TICK && !alive(&sim, inf.open) && !alive(&sim, inf.cover) {
            cmds.push(Command::Crouch {
                entity: inf.player,
                crouched: true,
            });
            crouched = true;
            log.crouch_tick = Some(tick);
            log.report
                .push((tick, "crouch — range ×5/4 now reaches the far dummy".to_string()));
        }
        // Fire on cadence while a reachable target lives (walled/flank are never reachable aiming +X).
        let reachable_alive =
            alive(&sim, inf.open) || alive(&sim, inf.cover) || alive(&sim, inf.far);
        if tick > EMBODY_TICK && (tick - FIRST_FIRE_TICK).is_multiple_of(cd) && reachable_alive {
            cmds.push(Command::Fire {
                entity: inf.player,
                dir: plus_x(),
            });
        }

        sim.step(&cmds);

        for ev in sim.events() {
            match ev {
                SimEvent::Damaged { entity, amount, .. } => {
                    if let Some(name) = name_of(&inf, *entity) {
                        let h = hp(&sim, *entity);
                        log.report.push((
                            tick,
                            format!(
                                "hit {name:<6} {:>4.0} dmg  (hp {:>3.0}){}",
                                show(*amount),
                                show(h),
                                if crouched { "  [crouched]" } else { "" },
                            ),
                        ));
                        match name {
                            "walled" => log.walled_hit = true,
                            "flank" => log.flank_hit = true,
                            "far" => {
                                if crouched {
                                    log.far_hit_crouched = true;
                                } else {
                                    log.far_hit_standing = true;
                                }
                            }
                            "open" => log.open_dmg_max = log.open_dmg_max.max(*amount),
                            "cover" => log.cover_dmg_max = log.cover_dmg_max.max(*amount),
                            _ => {}
                        }
                    }
                }
                SimEvent::Killed { entity, .. } => {
                    if let Some(name) = name_of(&inf, *entity) {
                        log.report.push((tick, format!("*** {name} down ***")));
                    }
                }
                _ => {}
            }
        }

        log.checksums.push(sim.checksum());
    }

    log.open_dead = !alive(&sim, inf.open);
    log.cover_dead = !alive(&sim, inf.cover);
    log.far_dead = !alive(&sim, inf.far);
    log
}

// --- The auto-combat battery --------------------------------------------------------------------

/// One battery result.
struct Check {
    name: &'static str,
    pass: bool,
    detail: String,
}

/// Spawn a full-stat produced Rifleman (range 14 / 6 dmg / 30-tick cd / 30-round mag) at `(x, y)`.
fn spawn_rifle(sim: &mut Sim, x: i32, y: i32, faction: Faction, stance: Stance) -> Entity {
    let (health, weapon) = economy::unit_stats(UnitKind::Rifleman);
    let e = sim.world.spawn();
    let i = e.index as usize;
    sim.world.kind[i] = EntityKind::Unit;
    sim.world.unit_kind[i] = UnitKind::Rifleman;
    sim.world.faction[i] = faction;
    sim.world.pos[i] = Vec2::new(Fixed::from_int(x), Fixed::from_int(y));
    sim.world.health[i] = health;
    sim.world.weapon[i] = weapon;
    sim.world.stance[i] = stance;
    e
}

/// **Stance:** a `FireAtWill` shooter engages a hostile in range; a `HoldFire` one never does
/// (invariant #3 — the literal executor only does what its stance says).
fn check_stance() -> Check {
    let mut sim = Sim::new(0x57A2CE);
    let shooter = spawn_rifle(&mut sim, 0, 0, Faction::Player, Stance::FireAtWill);
    let holder = spawn_rifle(&mut sim, 5, 0, Faction::Enemy, Stance::HoldFire);
    let shooter_hp0 = hp(&sim, shooter);
    let holder_hp0 = hp(&sim, holder);
    for _ in 0..40 {
        sim.step(&[]);
    }
    let dealt_to_holder = shooter_hp0_to(holder_hp0, hp(&sim, holder));
    let dealt_to_shooter = shooter_hp0_to(shooter_hp0, hp(&sim, shooter));
    Check {
        name: "stance",
        pass: dealt_to_holder > Fixed::ZERO && dealt_to_shooter == Fixed::ZERO,
        detail: format!(
            "FireAtWill dealt {:.0}; HoldFire dealt {:.0}",
            show(dealt_to_holder),
            show(dealt_to_shooter),
        ),
    }
}

/// HP delta `from - to` (damage taken), clamped at zero.
fn shooter_hp0_to(from: Fixed, to: Fixed) -> Fixed {
    (from - to).max(Fixed::ZERO)
}

/// **Suppression:** a unit pinned at/over `SUPPRESSION_PIN` may not fire; the same unit unpinned
/// does. Set the target's suppression to `SUPPRESSION_MAX` so it stays above the pin line for the
/// whole (short) window despite per-tick decay.
fn check_suppression() -> Check {
    // Unpinned: the shooter fires and damages its enemy.
    let mut a = Sim::new(0x5099);
    let s1 = spawn_rifle(&mut a, 0, 0, Faction::Enemy, Stance::FireAtWill);
    let t1 = spawn_rifle(&mut a, 5, 0, Faction::Player, Stance::HoldFire);
    let t1_hp0 = hp(&a, t1);
    let _ = s1;
    for _ in 0..20 {
        a.step(&[]);
    }
    let unpinned_dmg = shooter_hp0_to(t1_hp0, hp(&a, t1));

    // Pinned: the same shooter, suppressed to the max, never fires.
    let mut b = Sim::new(0x5099);
    let s2 = spawn_rifle(&mut b, 0, 0, Faction::Enemy, Stance::FireAtWill);
    let t2 = spawn_rifle(&mut b, 5, 0, Faction::Player, Stance::HoldFire);
    b.world.suppression[s2.index as usize] = SUPPRESSION_MAX;
    let t2_hp0 = hp(&b, t2);
    for _ in 0..20 {
        b.step(&[]);
    }
    let pinned_dmg = shooter_hp0_to(t2_hp0, hp(&b, t2));

    Check {
        name: "suppression",
        pass: unpinned_dmg > Fixed::ZERO && pinned_dmg == Fixed::ZERO,
        detail: format!(
            "unpinned dealt {:.0}; pinned dealt {:.0}",
            show(unpinned_dmg),
            show(pinned_dmg),
        ),
    }
}

/// **Retreat:** a unit whose health drops below its `retreat_below` threshold has its order replaced
/// with `FallBack` (D23) and falls back toward its rally (origin, with no friendly building).
fn check_retreat() -> Check {
    let mut sim = Sim::new(0x4E72EA7);
    let u = spawn_rifle(&mut sim, 10, 0, Faction::Enemy, Stance::HoldFire);
    let i = u.index as usize;
    // Program "fall back below 50% HP" and send it advancing +X (away from its origin rally).
    sim.step(&[
        Command::SetRetreatThreshold {
            entity: u,
            fraction: Fixed::from_ratio(1, 2),
        },
        Command::AttackMove {
            entity: u,
            target: Vec2::new(Fixed::from_int(20), Fixed::ZERO),
        },
    ]);
    let advancing = matches!(sim.world.order[i], Order::AttackMove(_));
    // Wound it below half → the trigger installs FallBack on the next order tick.
    sim.world.health[i].cur = Fixed::from_int(40); // < 50 of 100
    let x_before = sim.world.pos[i].x;
    sim.step(&[]);
    let fell_back = matches!(sim.world.order[i], Order::FallBack(_));
    for _ in 0..40 {
        sim.step(&[]);
    }
    let x_after = sim.world.pos[i].x;
    let retreated = x_after < x_before; // toward the origin rally, away from the (20,0) advance

    Check {
        name: "retreat",
        pass: advancing && fell_back && retreated,
        detail: format!(
            "order AttackMove→FallBack={fell_back}, x {:.0}→{:.0} (toward rally)",
            show(x_before),
            show(x_after),
        ),
    }
}

/// **Reload:** an embodied magazine weapon dry-clicks on an empty mag (no damage, no spend) and
/// fires again only after a `Reload` refills it.
fn check_reload() -> Check {
    let mut sim = Sim::new(0x4E10AD);
    let p = spawn_rifle(&mut sim, 0, 0, Faction::Player, Stance::FireAtWill);
    let e = spawn_rifle(&mut sim, 5, 0, Faction::Enemy, Stance::HoldFire);
    sim.step(&[Command::Embody { entity: p }]);
    sim.world.weapon[p.index as usize].ammo = 1; // one round chambered
    let e_hp0 = hp(&sim, e);

    // Shot 1: fires, hits, empties the mag.
    sim.step(&[Command::Fire {
        entity: p,
        dir: plus_x(),
    }]);
    let after_shot1 = hp(&sim, e);

    // Dry clicks while empty: cooldown clears but the empty-mag gate blocks every shot — no damage.
    for _ in 0..40 {
        sim.step(&[Command::Fire {
            entity: p,
            dir: plus_x(),
        }]);
    }
    let after_empty = hp(&sim, e);

    // Reload, let it complete, then a shot lands again.
    sim.step(&[Command::Reload { entity: p }]);
    for _ in 0..100 {
        sim.step(&[]);
    }
    sim.step(&[Command::Fire {
        entity: p,
        dir: plus_x(),
    }]);
    let after_reload = hp(&sim, e);

    let shot1_hit = after_shot1 < e_hp0;
    let empty_blocked = after_empty == after_shot1;
    let reload_restored = after_reload < after_empty;
    Check {
        name: "reload",
        pass: shot1_hit && empty_blocked && reload_restored,
        detail: format!(
            "hp {:.0}→{:.0} (shot) →{:.0} (empty, held) →{:.0} (reloaded)",
            show(e_hp0),
            show(after_shot1),
            show(after_empty),
            show(after_reload),
        ),
    }
}

/// Run the four auto-combat checks.
fn battery() -> Vec<Check> {
    vec![
        check_stance(),
        check_suppression(),
        check_retreat(),
        check_reload(),
    ]
}

/// Run the infantry scenario and print: the embodied scene's `<tick> <checksum>` stream on
/// **stdout**, the narrated report + battery on **stderr**.
pub fn run(ticks: u64) {
    let log = simulate_scene(ticks);

    for (tick, sum) in log.checksums.iter().enumerate() {
        println!("{tick} {sum:016x}");
    }

    eprintln!("== infantry sandbox ==  (player rifleman vs HoldFire dummies; produced rifle stats)");
    for (tick, line) in &log.report {
        eprintln!("  t{tick:>3}  {line}");
    }
    eprintln!("embodied verdicts:");
    eprintln!(
        "  cover   : Light cover halved the hit ({:.0} vs open {:.0})  [{}]",
        show(log.cover_dmg_max),
        show(log.open_dmg_max),
        pass_str(log.cover_dmg_max < log.open_dmg_max && log.open_dmg_max > Fixed::ZERO),
    );
    eprintln!(
        "  LoS     : the walled dummy was never hit  [{}]",
        pass_str(!log.walled_hit && log.cover_dead),
    );
    eprintln!(
        "  cone    : the off-axis flank dummy was never hit  [{}]",
        pass_str(!log.flank_hit),
    );
    eprintln!(
        "  range   : far dummy unreachable standing, hit only after crouch  [{}]",
        pass_str(!log.far_hit_standing && log.far_hit_crouched),
    );

    eprintln!("auto-combat battery:");
    let mut all_pass = true;
    for c in battery() {
        all_pass &= c.pass;
        eprintln!("  {:<12}: {}  [{}]", c.name, c.detail, pass_str(c.pass));
    }
    eprintln!(
        "result: {}",
        if all_pass && log.far_dead && !log.walled_hit && !log.flank_hit {
            "all infantry mechanics behaved as designed"
        } else {
            "SOME CHECK FAILED — see [FAIL] above"
        }
    );
}

fn pass_str(b: bool) -> &'static str {
    if b {
        "PASS"
    } else {
        "FAIL"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The embodied scene proves range / crouch / cone / cover / LoS end-to-end through real ticks.
    #[test]
    fn embodied_scene_validates_every_mechanic() {
        let log = simulate_scene(300);
        // The reachable dummies (open, cover, far) all die; the unreachable ones never take a hit.
        assert!(
            log.open_dead && log.cover_dead && log.far_dead,
            "the reachable dummies are eliminated",
        );
        assert!(!log.walled_hit, "LoS: the Heavy wall blocks the walled dummy");
        assert!(!log.flank_hit, "cone: the off-axis flank dummy is never hit aiming +X");
        // range + crouch: far is out of base range until the player crouches.
        assert!(!log.far_hit_standing, "far is beyond base range while standing");
        assert!(log.far_hit_crouched, "the crouch range bonus reaches far");
        assert!(log.crouch_tick.is_some(), "the player crouched");
        // cover: a Light-cover hit is strictly less than a full open-ground hit.
        assert!(log.open_dmg_max > Fixed::ZERO);
        assert!(
            log.cover_dmg_max < log.open_dmg_max,
            "Light cover halves the incoming hit ({} vs {})",
            show(log.cover_dmg_max),
            show(log.open_dmg_max),
        );
    }

    /// The headless scene is deterministic (invariant #1) and pins a golden final checksum.
    #[test]
    fn scene_is_deterministic() {
        assert_eq!(simulate_scene(300).checksums, simulate_scene(300).checksums);
        assert_eq!(
            simulate_scene(300).checksums.last().copied(),
            // D66: rifleman damage ×5 (6→30) moved the embodied-scene golden.
            Some(0xcead_40f4_566a_ab82),
        );
    }

    /// Every auto-combat battery check passes (stance / suppression / retreat / reload).
    #[test]
    fn battery_all_pass() {
        for c in battery() {
            assert!(c.pass, "battery check `{}` failed: {}", c.name, c.detail);
        }
    }
}
