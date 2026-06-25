//! "Going dark" detection — can an enemy tell which unit you've possessed? (Q2 → D33).
//!
//! A **pure, checksum-excluded derivation**, on the same footing as [`fog`](crate::fog) and
//! [`alerts`](crate::alerts): it reads `&World`/`&Terrain`, **never** mutates sim state and is
//! **never folded into the per-tick checksum**, so computing it can never desync lockstep
//! (invariants #1/#7). The tell is a *presentation/intel* layer over the shared world, not sim
//! state.
//!
//! Three tunable modes ([`TellMode`], default `Subtle` per D33), so one build covers all of Q2 for
//! A/B playtesting:
//! - `Hidden` — no tell ever (pure inference). Returns nothing, recording nothing, so a PvE AI
//!   that consults this channel gains **zero** knowledge — making "no omniscient peek" (invariant
//!   #3) a structural property, not a discipline.
//! - `Subtle` — the embodied unit is revealed to an observer only when that observer has a living
//!   unit within `tell_range` **and** in line of sight; the tell then **lingers and ages** for
//!   `tell_linger_ticks` after sight is lost (a fading, last-known-position marker). The tell is
//!   *earned* by proximity + sightline and decays once lost — so a loss reads as "I stayed
//!   embodied too long, too close," never "the game robbed me" (invariant #6).
//! - `Marked` — a persistent marker on the embodied unit (the strongest tell).

use std::collections::BTreeMap;

use crate::components::{EntityKind, Faction, InputSource, Vec2};
use crate::ecs::{Entity, World};
use crate::fixed::Fixed;
use crate::terrain::Terrain;

/// Which tell an enemy gets about your embodied (possessed) unit.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum TellMode {
    /// No tell ever — pure inference (the enemy must *read* your play).
    Hidden,
    /// A soft, line-of-sight-gated tell that lingers and ages after sight is lost. The D33
    /// default — the soft-tell fork, shipped on to validate from play.
    #[default]
    Subtle,
    /// A persistent marker on the embodied unit, regardless of range or sight (the strongest tell).
    Marked,
}

/// Tuning for the gone-dark tell. Defaults to the D33 `Subtle` baseline — a starting point to tune
/// from play (like the D30 balance baseline), not a frozen lock.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct DetectionConfig {
    /// Which tell mode is active.
    pub tell_mode: TellMode,
    /// How close (world units) an observer's unit must be to acquire the `Subtle` tell.
    pub tell_range: Fixed,
    /// How many ticks a `Subtle` tell lingers (fading) after the observer loses sight.
    pub tell_linger_ticks: u32,
}

impl Default for DetectionConfig {
    fn default() -> Self {
        DetectionConfig {
            tell_mode: TellMode::Subtle,
            tell_range: Fixed::from_int(28),
            tell_linger_ticks: 90, // ~1500 ms at the 60 Hz tick
        }
    }
}

/// A revealed embodied enemy unit, from one observer faction's point of view.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Tell {
    /// The embodied (possessed) unit revealed.
    pub unit: Entity,
    /// Where to draw the tell — the live position while in sight, else the **last-seen** position
    /// (a lingering tell points at where the avatar was last seen, not where it secretly went).
    pub pos: Vec2,
    /// Ticks since the unit was last directly in sight. `0` == in sight now; `> 0` == fading
    /// (`Subtle` linger).
    pub age_ticks: u32,
}

/// Per-observer linger memory for `Subtle` tells. **Presentation state** — never sim state, never
/// folded into the checksum; each client holds its own for its own HUD. Keyed by the embodied
/// unit's `(index, generation)` so a reused slot with a new generation is a distinct unit.
#[derive(Clone, Default)]
pub struct DetectionMemory {
    seen: BTreeMap<(u32, u32), (u64, Vec2)>, // unit -> (last-seen tick, last-seen pos)
}

impl DetectionMemory {
    pub fn new() -> Self {
        Self::default()
    }
}

/// Hostile across distinct, non-`Neutral` factions — the same engagement rule combat uses
/// (invariant #3 keeps the tell literal: you are never "told" a friendly or neutral unit).
#[inline]
fn hostile(a: Faction, b: Faction) -> bool {
    a != b && a != Faction::Neutral && b != Faction::Neutral
}

/// Which embodied enemy units are currently "told" to `observer`, under `config`, at tick `now`.
///
/// Pure: it reads the world and updates only the presentation-side `memory` (the `Subtle` linger),
/// never the sim. The returned tells are intel for `observer`'s HUD / a PvE AI — in `Hidden` it is
/// always empty, so consulting it conveys nothing.
pub fn detectable_embodiment(
    world: &World,
    terrain: &Terrain,
    config: &DetectionConfig,
    observer: Faction,
    now: u64,
    memory: &mut DetectionMemory,
) -> Vec<Tell> {
    let mut tells = Vec::new();
    if config.tell_mode == TellMode::Hidden {
        return tells; // no tell ever — and we record nothing, so it can never leak later
    }
    for i in 0..world.capacity() {
        if !world.is_index_alive(i)
            || world.kind[i] != EntityKind::Unit
            || world.input_source[i] != InputSource::Embodied
            || !hostile(observer, world.faction[i])
        {
            continue;
        }
        let unit = match world.entity(i) {
            Some(e) => e,
            None => continue,
        };
        let pos = world.pos[i];
        let key = (unit.index, unit.generation);
        match config.tell_mode {
            TellMode::Hidden => unreachable!("Hidden returns early above"),
            TellMode::Marked => {
                // Persistent marker — always revealed, no range/LoS gate.
                tells.push(Tell {
                    unit,
                    pos,
                    age_ticks: 0,
                });
            }
            TellMode::Subtle => {
                if observer_has_sight(world, terrain, observer, pos, config.tell_range) {
                    memory.seen.insert(key, (now, pos));
                    tells.push(Tell {
                        unit,
                        pos,
                        age_ticks: 0,
                    });
                } else if let Some(&(last_tick, last_pos)) = memory.seen.get(&key) {
                    let age = now.saturating_sub(last_tick);
                    if age <= config.tell_linger_ticks as u64 {
                        tells.push(Tell {
                            unit,
                            pos: last_pos,
                            age_ticks: age as u32,
                        });
                    } else {
                        memory.seen.remove(&key); // the linger expired — stop tracking it
                    }
                }
            }
        }
    }
    tells
}

/// Does any living `observer`-faction unit sit within `range` of `target` AND hold line of sight?
fn observer_has_sight(
    world: &World,
    terrain: &Terrain,
    observer: Faction,
    target: Vec2,
    range: Fixed,
) -> bool {
    let range_sq = range * range;
    for i in 0..world.capacity() {
        if !world.is_index_alive(i)
            || world.kind[i] != EntityKind::Unit
            || world.faction[i] != observer
        {
            continue;
        }
        let from = world.pos[i];
        if (target - from).len_sq() > range_sq {
            continue;
        }
        if terrain.line_of_sight(from, target) {
            return true;
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::components::EntityKind;
    use crate::terrain::Cover;

    fn fx(n: i32) -> Fixed {
        Fixed::from_int(n)
    }

    /// Spawn a unit at `(x, y)` with a faction and input source; returns its entity.
    fn spawn(world: &mut World, x: i32, y: i32, faction: Faction, src: InputSource) -> Entity {
        let e = world.spawn();
        let i = e.index as usize;
        world.pos[i] = Vec2::new(fx(x), fx(y));
        world.faction[i] = faction;
        world.kind[i] = EntityKind::Unit;
        world.input_source[i] = src;
        e
    }

    fn cfg(mode: TellMode) -> DetectionConfig {
        DetectionConfig {
            tell_mode: mode,
            ..DetectionConfig::default()
        }
    }

    #[test]
    fn default_config_is_subtle() {
        assert_eq!(DetectionConfig::default().tell_mode, TellMode::Subtle);
    }

    #[test]
    fn hidden_reveals_nothing_even_in_plain_sight() {
        // An embodied enemy unit right next to an observer with clear LoS — Hidden tells nothing.
        let mut world = World::new();
        let terrain = Terrain::open();
        spawn(&mut world, 0, 0, Faction::Player, InputSource::Orders); // observer's own unit
        spawn(&mut world, 2, 0, Faction::Enemy, InputSource::Embodied); // the hero, in plain sight
        let mut mem = DetectionMemory::new();
        let tells = detectable_embodiment(&world, &terrain, &cfg(TellMode::Hidden), Faction::Player, 0, &mut mem);
        assert!(tells.is_empty(), "Hidden must reveal nothing");
    }

    #[test]
    fn subtle_reveals_when_observer_in_range_and_los() {
        let mut world = World::new();
        let terrain = Terrain::open();
        spawn(&mut world, 0, 0, Faction::Player, InputSource::Orders);
        let hero = spawn(&mut world, 5, 0, Faction::Enemy, InputSource::Embodied);
        let mut mem = DetectionMemory::new();
        let tells = detectable_embodiment(&world, &terrain, &cfg(TellMode::Subtle), Faction::Player, 0, &mut mem);
        assert_eq!(tells.len(), 1);
        assert_eq!(tells[0].unit, hero);
        assert_eq!(tells[0].age_ticks, 0, "in sight now");
        assert_eq!(tells[0].pos, Vec2::new(fx(5), fx(0)));
    }

    #[test]
    fn subtle_no_tell_when_out_of_range() {
        let mut world = World::new();
        let terrain = Terrain::open();
        spawn(&mut world, 0, 0, Faction::Player, InputSource::Orders);
        // Far beyond the default tell_range (28).
        spawn(&mut world, 60, 0, Faction::Enemy, InputSource::Embodied);
        let mut mem = DetectionMemory::new();
        let tells = detectable_embodiment(&world, &terrain, &cfg(TellMode::Subtle), Faction::Player, 0, &mut mem);
        assert!(tells.is_empty(), "out of range, never seen → no tell");
    }

    #[test]
    fn subtle_blocked_by_terrain_line_of_sight() {
        let mut world = World::new();
        // Heavy cover (a wall) strictly between the observer (cell 64) and hero (cell 74): cell 69.
        let mut terrain = Terrain::open();
        terrain.set_cover(69, 64, Cover::Heavy);
        spawn(&mut world, 0, 0, Faction::Player, InputSource::Orders);
        spawn(&mut world, 10, 0, Faction::Enemy, InputSource::Embodied);
        // Sanity: the wall really blocks the sightline.
        assert!(!terrain.line_of_sight(Vec2::new(fx(0), fx(0)), Vec2::new(fx(10), fx(0))));
        let mut mem = DetectionMemory::new();
        let tells = detectable_embodiment(&world, &terrain, &cfg(TellMode::Subtle), Faction::Player, 0, &mut mem);
        assert!(tells.is_empty(), "in range but no LoS → no tell");
    }

    #[test]
    fn subtle_tell_lingers_then_ages_then_expires() {
        let mut world = World::new();
        let terrain = Terrain::open();
        let observer = spawn(&mut world, 0, 0, Faction::Player, InputSource::Orders);
        let hero_i = spawn(&mut world, 5, 0, Faction::Enemy, InputSource::Embodied).index as usize;
        let mut mem = DetectionMemory::new();
        let config = DetectionConfig {
            tell_mode: TellMode::Subtle,
            tell_range: fx(28),
            tell_linger_ticks: 10,
        };
        // Tick 0: in sight → age 0, last-seen pos recorded.
        let t = detectable_embodiment(&world, &terrain, &config, Faction::Player, 0, &mut mem);
        assert_eq!(t[0].age_ticks, 0);
        // Move the observer far away so sight is lost; move the hero too (the tell must point at the
        // LAST-SEEN pos, not the hero's new secret position).
        world.pos[observer.index as usize] = Vec2::new(fx(90), fx(90));
        world.pos[hero_i] = Vec2::new(fx(7), fx(2));
        // Tick 6: lingering, aged 6, at the last-seen (5,0) — not the new (7,2).
        let t = detectable_embodiment(&world, &terrain, &config, Faction::Player, 6, &mut mem);
        assert_eq!(t.len(), 1);
        assert_eq!(t[0].age_ticks, 6);
        assert_eq!(t[0].pos, Vec2::new(fx(5), fx(0)), "linger marks where it was last seen");
        // Tick 11: past tell_linger_ticks (10) → expired, no tell.
        let t = detectable_embodiment(&world, &terrain, &config, Faction::Player, 11, &mut mem);
        assert!(t.is_empty(), "linger expired");
    }

    #[test]
    fn marked_reveals_always_regardless_of_range_and_los() {
        let mut world = World::new();
        let mut terrain = Terrain::open();
        terrain.set_cover(69, 64, Cover::Heavy); // wall between, would block Subtle
        spawn(&mut world, 0, 0, Faction::Player, InputSource::Orders);
        // Far AND blocked — Marked still reveals it.
        let hero = spawn(&mut world, 60, 0, Faction::Enemy, InputSource::Embodied);
        let mut mem = DetectionMemory::new();
        let tells = detectable_embodiment(&world, &terrain, &cfg(TellMode::Marked), Faction::Player, 0, &mut mem);
        assert_eq!(tells.len(), 1);
        assert_eq!(tells[0].unit, hero);
        assert_eq!(tells[0].age_ticks, 0);
    }

    #[test]
    fn computing_detection_is_checksum_neutral() {
        // The load-bearing fairness/determinism guard (D33): computing the tell every tick must
        // leave the sim checksum stream byte-identical — it is a read-only derivation that can
        // never feed back into the sim (invariants #1/#7, #6 fairness, #3 no omniscient peek).
        use crate::sim::Sim;
        let mut with = Sim::new(0xD37EC7);
        let mut without = Sim::new(0xD37EC7);
        // Embody a unit in BOTH sims identically (so the only difference is the detection call).
        for sim in [&mut with, &mut without] {
            let e = sim.world.spawn();
            let i = e.index as usize;
            sim.world.kind[i] = EntityKind::Unit;
            sim.world.faction[i] = Faction::Enemy;
            sim.world.pos[i] = Vec2::new(fx(3), fx(0));
            sim.world.input_source[i] = InputSource::Embodied;
            // An observer unit too, so Subtle actually produces a tell to compute.
            let o = sim.world.spawn();
            let oi = o.index as usize;
            sim.world.kind[oi] = EntityKind::Unit;
            sim.world.faction[oi] = Faction::Player;
            sim.world.pos[oi] = Vec2::new(fx(1), fx(0));
        }
        let config = DetectionConfig::default();
        let mut mem = DetectionMemory::new();
        for t in 0..30u64 {
            with.step(&[]);
            without.step(&[]);
            // Compute the tell on `with` every tick (and assert it actually fires, so the test
            // would catch a regression that made detection a silent no-op).
            let tells = detectable_embodiment(
                &with.world,
                &with.terrain,
                &config,
                Faction::Player,
                t,
                &mut mem,
            );
            assert!(!tells.is_empty(), "observer should see the embodied enemy");
            assert_eq!(
                with.checksum(),
                without.checksum(),
                "computing detection must not change the sim checksum at tick {t}"
            );
        }
    }

    #[test]
    fn only_hostile_embodied_units_are_told() {
        let mut world = World::new();
        let terrain = Terrain::open();
        spawn(&mut world, 0, 0, Faction::Player, InputSource::Orders); // observer
        spawn(&mut world, 2, 0, Faction::Player, InputSource::Embodied); // a FRIENDLY embodied unit
        spawn(&mut world, 3, 0, Faction::Enemy, InputSource::Orders); // an enemy, NOT embodied
        spawn(&mut world, 4, 0, Faction::Neutral, InputSource::Embodied); // neutral embodied
        let mut mem = DetectionMemory::new();
        // Marked is the most permissive mode, so if anything wrongly leaks it shows here.
        let tells = detectable_embodiment(&world, &terrain, &cfg(TellMode::Marked), Faction::Player, 0, &mut mem);
        assert!(
            tells.is_empty(),
            "only hostile, embodied units are told — not friendly, not non-embodied, not neutral"
        );
    }
}
