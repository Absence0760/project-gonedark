//! Read-only render snapshot (invariant #4). The renderer interpolates between two of
//! these and converts the Q16.16 positions to float at *its* boundary — it never calls
//! back into the sim to mutate state. Carrying raw `Fixed` keeps `core` float-free.
//!
//! Phase 2 widens the snapshot so the presentation layer can *show* the new systems:
//! each unit carries its faction, health fraction, and whether it is a building; and the
//! snapshot lists the territory control points. None of this is sim state — it is a copy
//! taken for rendering, so it is not checksummed (invariant #7 covers the world itself).

use crate::components::{
    Army, EntityKind, Faction, InputSource, UnitKind, Vec2, Weapon, FACTION_COUNT,
};
use crate::ecs::World;
use crate::fixed::Fixed;
use crate::projectile::Projectile;
use crate::territory::Territory;
use crate::trig::Angle;

/// One unit's renderable state at a tick.
#[derive(Clone, Debug)]
pub struct UnitSnapshot {
    /// The unit's world (ECS) index — the renderer matches command-layer selection against
    /// this to highlight selected units. Presentation only; not checksummed.
    pub entity_index: u32,
    pub pos: Vec2,
    pub vel: Vec2,
    pub embodied: bool,
    /// Which side it belongs to (drives the render color).
    pub faction: Faction,
    /// The [`Army`] identity `faction` currently fields (factions-plan WS-A/WS-C, D68), resolved at
    /// capture time via [`Sim::army_of`](crate::sim::Sim::army_of). Match-setup config, never folded
    /// into the per-tick checksum (invariant #7) — purely a presentation copy so the renderer can
    /// pick the faction-specific silhouette (`render::model_for_unit`) instead of the shared
    /// [`Army::Neutral`] greybox. A scene that never selects an army still reads every unit as
    /// `Army::Neutral`, so it stays byte-identical to before WS-C.
    pub army: Army,
    /// Health as a Fixed fraction in `[0, 1]` (the renderer draws a bar from this).
    pub health: Fixed,
    /// True for buildings (drawn larger / distinctly), false for units.
    pub building: bool,
    /// The producible archetype — renderer maps Heavy→tank, Rifleman→infantry; presentation only,
    /// not checksummed.
    pub unit_kind: UnitKind,
    /// Chassis facing (binary-radian [`Angle`]) — the direction the hull/tracks point. The sim slews
    /// it toward the unit's velocity (`heading_system`) or the embodied stick (`drive_hull`); the
    /// renderer orients the body mesh by it (tank embodiment P7, D55). Presentation copy — the real
    /// sim state is checksummed at its source, this snapshot is not (invariant #4/#7).
    pub hull_heading: Angle,
    /// Gun bearing (binary-radian [`Angle`]) — for a tank, the turret yaws independently of the hull
    /// (`turret_speed > 0`); for turret-less units it tracks the hull and is cosmetically irrelevant
    /// (no separate turret mesh). Same absolute frame as `hull_heading` (`+X = 0`, CCW). The renderer
    /// yaws the tank's turret mesh by it (P7). Presentation copy, not checksummed.
    pub turret_yaw: Angle,
    /// Did this unit fire within the last [`MUZZLE_FLASH_TICKS`] ticks? Derived purely from the
    /// (checksummed) weapon cooldown at capture (see `weapon_recently_fired`) — the debug overlay
    /// lights a muzzle flash on it so you can *see* a unit shooting from the command view, the
    /// AI-side analogue of the embodied viewmodel's `render::world::muzzle_flash_intensity`.
    /// Presentation only: adds no sim state and never enters the checksum fold (invariant #4/#7).
    pub firing: bool,
}

/// How many sim ticks a unit reads as "firing" after each shot — the muzzle-flash window the debug
/// overlay lights it for. Set to mirror the embodied viewmodel's flash length
/// (`render::world::MUZZLE_FLASH_TICKS`) so an AI unit's command-view flash and the player's
/// first-person flash last the same wall-clock time at the locked 60 Hz tick.
pub const MUZZLE_FLASH_TICKS: u16 = 8;

/// Did `w` fire within the last [`MUZZLE_FLASH_TICKS`] ticks? A shot resets `cooldown_left` to
/// `cooldown_ticks` (both `combat::combat_system` and `combat::resolve_fire`), which then counts
/// down one per tick — so a freshly-fired weapon sits near the top of its cooldown and a never-fired
/// one rests at zero. An unarmed weapon (`cooldown_ticks == 0`, never settable to a non-zero
/// `cooldown_left`) reads as not firing. A weapon whose whole cooldown is shorter than the window
/// stays lit for every on-cooldown tick (a continuously-firing unit glows). Pure + float-free
/// (invariant #1) — the testable seam for the `firing` snapshot flag.
fn weapon_recently_fired(w: &Weapon) -> bool {
    w.cooldown_ticks > 0 && w.cooldown_left > w.cooldown_ticks.saturating_sub(MUZZLE_FLASH_TICKS)
}

/// One in-flight shell's renderable state at a tick (tank embodiment P7, D55). A presentation copy
/// of a [`Projectile`] — enough to draw a tracer and extrapolate it smoothly between ticks. The
/// real projectile pool is the checksummed sim state (`Sim::fold`); this copy is not checksummed
/// (invariant #4/#7). All-`Fixed`, so it stays float-free in `core` (the renderer converts at its
/// boundary).
#[derive(Clone, Debug)]
pub struct ProjectileSnapshot {
    /// Ground-plane position (world units) this tick.
    pub pos: Vec2,
    /// Ground-plane velocity (world units/tick) — the renderer extrapolates `pos + vel·alpha` for a
    /// smooth tracer between sim ticks, and reads the travel direction (its yaw) from it.
    pub vel: Vec2,
    /// Height above the ground plane (world units) — the shell's vertical position (its arc).
    pub height: Fixed,
    /// Vertical velocity (world units/tick) — the renderer extrapolates `height + vz·alpha`.
    pub vz: Fixed,
    /// The firing side (drives the tracer tint).
    pub faction: Faction,
}

/// One control point's renderable state at a tick.
#[derive(Clone, Debug)]
pub struct ControlPointSnapshot {
    pub pos: Vec2,
    pub owner: Faction,
    /// Capture progress toward the current contester, Fixed in `[0, 1]`.
    pub progress: Fixed,
}

/// An immutable copy of the renderable world at one sim tick.
#[derive(Clone, Debug, Default)]
pub struct Snapshot {
    pub tick: u64,
    pub units: Vec<UnitSnapshot>,
    pub control_points: Vec<ControlPointSnapshot>,
    /// In-flight shells to draw as tracers (tank embodiment P7). Embodied-only by construction
    /// (invariant #3 — only an embodied unit's `Fire` launches a ballistic shell), so every entry is
    /// a physical, transient object, not strategic map intel.
    pub projectiles: Vec<ProjectileSnapshot>,
}

impl Snapshot {
    pub fn capture(
        world: &World,
        territory: &Territory,
        projectiles: &[Projectile],
        tick: u64,
        armies: &[Army; FACTION_COUNT],
    ) -> Self {
        let mut units = Vec::new();
        for i in 0..world.capacity() {
            if !world.is_index_alive(i) {
                continue;
            }
            let faction = world.faction[i];
            units.push(UnitSnapshot {
                entity_index: i as u32,
                pos: world.pos[i],
                vel: world.vel[i],
                embodied: world.input_source[i] == InputSource::Embodied,
                faction,
                army: armies[faction.index()],
                health: world.health[i].fraction(),
                building: world.kind[i] == EntityKind::Building,
                unit_kind: world.unit_kind[i],
                hull_heading: world.hull_heading[i],
                turret_yaw: world.turret_yaw[i],
                firing: weapon_recently_fired(&world.weapon[i]),
            });
        }
        let control_points = territory
            .points
            .iter()
            .map(|p| ControlPointSnapshot {
                pos: p.pos,
                owner: p.owner,
                progress: p.progress,
            })
            .collect();
        let projectiles = projectiles
            .iter()
            .map(|p| ProjectileSnapshot {
                pos: p.pos2d,
                vel: p.vel2d,
                height: p.height,
                vz: p.vz,
                faction: p.faction,
            })
            .collect();
        Snapshot {
            tick,
            units,
            control_points,
            projectiles,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A weapon with a given cooldown profile; other fields default (float-free — invariant #1).
    fn gun(cooldown_ticks: u16, cooldown_left: u16) -> Weapon {
        Weapon {
            range: Fixed::from_int(10),
            damage: Fixed::ONE,
            cooldown_ticks,
            cooldown_left,
            ..Weapon::default()
        }
    }

    #[test]
    fn just_fired_reads_as_firing() {
        assert!(weapon_recently_fired(&gun(30, 30)));
    }

    #[test]
    fn firing_window_closes_after_muzzle_flash_ticks() {
        let cd = 30;
        assert!(weapon_recently_fired(&gun(cd, cd - MUZZLE_FLASH_TICKS + 1)));
        assert!(!weapon_recently_fired(&gun(cd, cd - MUZZLE_FLASH_TICKS)));
    }

    #[test]
    fn never_fired_or_unarmed_is_not_firing() {
        assert!(!weapon_recently_fired(&gun(30, 0)), "ready, never fired");
        assert!(
            !weapon_recently_fired(&gun(0, 0)),
            "unarmed: no cooldown to flash from"
        );
    }

    #[test]
    fn fast_weapon_stays_lit_through_its_whole_cooldown() {
        // A cooldown shorter than the flash window: lit for every on-cooldown tick, dark only when ready.
        assert!(weapon_recently_fired(&gun(3, 3)));
        assert!(weapon_recently_fired(&gun(3, 1)));
        assert!(!weapon_recently_fired(&gun(3, 0)));
    }

    // ---- WS-C last mile: per-unit `army` resolved into the snapshot at capture time -------------

    use crate::ecs::World;
    use crate::territory::Territory;

    /// Spawn a bare unit of `faction` and return its world index.
    fn spawn_unit(world: &mut World, faction: Faction) -> usize {
        let e = world.spawn();
        let i = e.index as usize;
        world.faction[i] = faction;
        i
    }

    /// [`Snapshot::capture`] resolves each unit's `army` field from the caller-supplied
    /// `armies: &[Army; FACTION_COUNT]` (the sim's `sim.army_of` table), indexed by the unit's own
    /// `faction` — never a fixed/global army. A Player-faction unit reads whatever army Player picked;
    /// an Enemy-faction unit reads Enemy's pick independently, even in the same capture.
    #[test]
    fn capture_resolves_each_units_army_from_its_own_faction() {
        let mut world = World::new();
        spawn_unit(&mut world, Faction::Player);
        spawn_unit(&mut world, Faction::Enemy);
        spawn_unit(&mut world, Faction::Neutral);

        let mut armies = [Army::Neutral; FACTION_COUNT];
        armies[Faction::Player.index()] = Army::Us;
        armies[Faction::Enemy.index()] = Army::Fr;
        // Faction::Neutral is left at the default Army::Neutral.

        let snap = Snapshot::capture(&world, &Territory::default(), &[], 0, &armies);
        let army_of_faction = |f: Faction| {
            snap.units
                .iter()
                .find(|u| u.faction == f)
                .expect("faction present")
                .army
        };
        assert_eq!(army_of_faction(Faction::Player), Army::Us);
        assert_eq!(army_of_faction(Faction::Enemy), Army::Fr);
        assert_eq!(army_of_faction(Faction::Neutral), Army::Neutral);
    }

    /// A scene that never selects an army (every side `Army::Neutral`, the default `Sim::new`
    /// config) captures every unit's `army` as `Army::Neutral` — the pre-WS-C behaviour the renderer
    /// keeps drawing byte-identically (the render-side test `interpolate_sets_token_model_from_...`
    /// covers the mesh-selection half of this).
    #[test]
    fn capture_defaults_every_unit_to_neutral_army_when_no_army_selected() {
        let mut world = World::new();
        spawn_unit(&mut world, Faction::Player);
        spawn_unit(&mut world, Faction::Enemy);

        let armies = [Army::Neutral; FACTION_COUNT];
        let snap = Snapshot::capture(&world, &Territory::default(), &[], 0, &armies);
        assert!(snap.units.iter().all(|u| u.army == Army::Neutral));
    }
}
