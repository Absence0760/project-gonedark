//! Render-side death linger (CP-3 / WS-B follow-up). Drives [`crate::anim::AnimClip::Death`] to the
//! screen even though `core::snapshot::Snapshot::capture` drops a dead unit from the render
//! snapshot the moment the sim despawns it (see the `anim` module doc). That drop timing is real
//! sim/checksum surface (invariant #1/#4/#7) and is explicitly OUT OF SCOPE here — this module
//! never reads/writes `core` state and never changes when a unit disappears from the snapshot. It
//! only *remembers*, in the render view, a unit that WAS present-and-alive in the previous
//! snapshot and is now gone from the current one, and keeps emitting a frozen [`UnitInstance`] for
//! it — playing the Death clip — for a short fade window before evicting it. Pure bookkeeping: no
//! GPU, no `core` type mutated, no checksum surface.
//!
//! ## Fairness (invariant #6)
//!
//! A lingering instance is appended to the SAME draw list [`crate::fog::visible_instances`] filters
//! every other instance from, so it is gated by the identical visibility mask as a living unit — it
//! can never outlive what the viewer would have been able to see. It carries no
//! [`crate::FLAG_EMBODIED`]/[`crate::FLAG_RING`] always-keep flag, so it gets no special fog
//! exemption either.
//!
//! ## Embodiment (invariant #5)
//!
//! The possessed avatar's own death is never lingered ([`DeathLinger::update`] skips any vanished
//! unit that was `embodied` in the previous snapshot) — ejecting to command is the engine's job, and
//! drawing a frozen "body" for the very entity the player was just controlling reads too close to a
//! respawn/character system this game deliberately does not have. Every other unit (AI-controlled,
//! ally or enemy) lingers exactly the same way whether or not the local player happens to be
//! embodied at the time.
//!
//! ## Entity-index reuse
//!
//! `entity_index` is a recycled ECS slot, not a stable identity. A unit is only ever a linger
//! candidate if its index is ABSENT from `curr` — if a freshly-spawned unit already reuses the same
//! index this tick, the index is present-and-alive in `curr`, so it is never added as a linger (and
//! any stale linger entry for that index is evicted immediately, before the new unit's own instance
//! is drawn by [`crate::interpolate_instances`]).

use crate::anim::AnimClip;
use crate::theme::Palette;
use crate::{
    faction_color_in, faction_shape, fixed_to_f32, interp_angle, model_for_unit, UnitInstance,
    BUILDING_HALF, NO_HEALTH_BAR, NO_TOKEN_ICON, UNIT_HALF,
};
use gonedark_core::snapshot::Snapshot;

/// Ticks (at the locked 60 Hz sim tick) for the topple/sink to reach its fully-fallen pose. Purely
/// presentation tuning: [`crate::anim::anim_pose`]'s `death_progress` clamps at `1.0`, so a linger
/// held past this point simply stays fully collapsed until [`DEATH_LINGER_TICKS`] evicts it.
const DEATH_ANIM_TICKS: f32 = 30.0;

/// Total ticks a vanished unit's frozen instance is kept on screen before eviction — long enough to
/// read the topple AND rest fully fallen for a beat, short enough that corpses don't pile up.
const DEATH_LINGER_TICKS: f32 = 90.0;

/// One vanished unit's frozen presentation pose, captured at the moment it was last seen
/// alive (the last tick it appeared in a snapshot). Everything here is copied out of that
/// snapshot's `UnitSnapshot`/interpolation math — never re-derived from a live sim read.
#[derive(Clone, Copy, Debug, PartialEq)]
struct LingerEntry {
    entity_index: u32,
    x: f32,
    y: f32,
    half_extent: f32,
    color: [f32; 3],
    shape: u32,
    model: u32,
    hull_yaw: f32,
    turret_yaw: f32,
    kind: u32,
    /// The sim tick (as captured on `curr` the first frame the unit was found missing) the fade
    /// clock starts counting from.
    start_tick: u64,
}

/// Whether `entity_index` is alive in `curr` this frame — the single "present in curr" check that
/// both gates new linger candidates and evicts a stale entry on index reuse.
fn alive_in_curr(curr: &Snapshot, entity_index: u32) -> bool {
    curr.units.iter().any(|u| u.entity_index == entity_index)
}

/// The render-side death-linger buffer (CP-3 follow-up). Owned by [`crate::Renderer`] across frames
/// (it needs frame-to-frame memory to notice a unit vanish, exactly like the prev/curr snapshot pair
/// it reads), fed once per [`crate::Renderer::prepare`] call via [`DeathLinger::update`].
#[derive(Clone, Debug, Default)]
pub struct DeathLinger {
    entries: Vec<LingerEntry>,
}

impl DeathLinger {
    pub fn new() -> Self {
        Self::default()
    }

    /// How many units are currently lingering (test/debug convenience).
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Advance the linger set for one `prepare()` call and return the Death-clip instances to draw
    /// this frame. `prev`/`curr` are the exact snapshot pair [`crate::interpolate_instances`] reads;
    /// `tick_f` is `curr.tick as f32 + alpha` — the same presentation clock
    /// [`crate::anim::unit_phase`] walks, so the topple advances smoothly across sub-tick render
    /// frames and repeated calls within one sim tick never double-insert (eviction/insertion are
    /// both keyed off the definitive "present in curr" check, not off how many times this has been
    /// called this tick).
    ///
    /// Order matters: eviction runs BEFORE discovery, so an index that both lost its old occupant
    /// and gained a new one in the same tick is resolved once — `alive_in_curr` is true for it, so
    /// the stale entry is dropped and no new entry is created (the reused index draws only the new
    /// unit's own live instance, never a phantom corpse).
    ///
    /// Relies on the caller always handing in the *immediately preceding* tick's snapshot as
    /// `prev` (exactly how `engine`'s `self.prev = self.curr.clone()` step keeps them) — a vanished
    /// unit then appears in exactly one `prev`-but-not-`curr` pair, ever, so discovery only fires
    /// once per death and never re-adds an entry this fn already evicted for staying too long.
    pub fn update(
        &mut self,
        prev: &Snapshot,
        curr: &Snapshot,
        tick_f: f32,
        palette: &Palette,
    ) -> Vec<UnitInstance> {
        // 1. Evict: the index reappeared alive in curr (reuse), or the fade window elapsed.
        self.entries.retain(|e| {
            !alive_in_curr(curr, e.entity_index)
                && tick_f - e.start_tick as f32 <= DEATH_LINGER_TICKS
        });

        // 2. Discover newly-vanished units: alive in prev, absent from curr, not the embodied
        // avatar (invariant #5), and not already tracked (a prior frame within this same tick
        // already found it).
        for u in &prev.units {
            if u.embodied {
                continue;
            }
            if alive_in_curr(curr, u.entity_index) {
                continue;
            }
            if self
                .entries
                .iter()
                .any(|e| e.entity_index == u.entity_index)
            {
                continue;
            }
            let color = faction_color_in(u.faction, palette);
            self.entries.push(LingerEntry {
                entity_index: u.entity_index,
                x: fixed_to_f32(u.pos.x),
                y: fixed_to_f32(u.pos.y),
                half_extent: if u.building { BUILDING_HALF } else { UNIT_HALF },
                color,
                shape: faction_shape(u.faction),
                model: model_for_unit(u.army, u.building, u.unit_kind) as u32,
                // Frozen facing: reuse the tested shortest-arc interpolator with prev==curr==the
                // last-known heading (delta is exactly zero), rather than hand-rolling a second
                // Angle→radians conversion.
                hull_yaw: interp_angle(u.hull_heading, u.hull_heading, 0.0),
                turret_yaw: interp_angle(u.turret_yaw, u.turret_yaw, 0.0),
                kind: if u.building {
                    NO_TOKEN_ICON
                } else {
                    u.unit_kind as u32
                },
                start_tick: curr.tick,
            });
        }

        // 3. Emit a frozen Death-clip instance per lingering entry, phase advancing off the shared
        // presentation clock.
        self.entries
            .iter()
            .map(|e| {
                let phase = (tick_f - e.start_tick as f32) / DEATH_ANIM_TICKS;
                UnitInstance {
                    x: e.x,
                    y: e.y,
                    half_extent: e.half_extent,
                    r: e.color[0],
                    g: e.color[1],
                    b: e.color[2],
                    health: NO_HEALTH_BAR,
                    flags: 0,
                    shape: e.shape,
                    model: e.model,
                    hull_yaw: e.hull_yaw,
                    turret_yaw: e.turret_yaw,
                    kind: e.kind,
                    anim_clip: AnimClip::Death.as_u32(),
                    anim_phase: phase,
                }
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::theme::Palette;
    use gonedark_core::components::{Army, Faction, UnitKind, Vec2};
    use gonedark_core::fixed::Fixed;
    use gonedark_core::snapshot::UnitSnapshot;
    use gonedark_core::trig::Angle;

    fn unit(entity_index: u32, embodied: bool) -> UnitSnapshot {
        UnitSnapshot {
            entity_index,
            pos: Vec2::new(Fixed::from_int(3), Fixed::from_int(4)),
            vel: Vec2::ZERO,
            embodied,
            faction: Faction::Enemy,
            army: Army::Neutral,
            health: Fixed::ZERO,
            building: false,
            unit_kind: UnitKind::Rifleman,
            hull_heading: Angle(0),
            turret_yaw: Angle(0),
            firing: false,
        }
    }

    fn snapshot(tick: u64, units: Vec<UnitSnapshot>) -> Snapshot {
        Snapshot {
            tick,
            units,
            control_points: Vec::new(),
            projectiles: Vec::new(),
        }
    }

    #[test]
    fn empty_when_nothing_died() {
        let prev = snapshot(0, vec![unit(0, false)]);
        let curr = snapshot(1, vec![unit(0, false)]); // still alive in curr
        let mut linger = DeathLinger::new();
        let out = linger.update(&prev, &curr, 1.0, &Palette::DEFAULT);
        assert!(out.is_empty());
        assert!(linger.is_empty());
    }

    #[test]
    fn vanished_unit_produces_a_death_clip_linger_instance() {
        let prev = snapshot(0, vec![unit(5, false)]);
        let curr = snapshot(1, vec![]); // unit 5 is gone
        let mut linger = DeathLinger::new();
        let out = linger.update(&prev, &curr, 1.0, &Palette::DEFAULT);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].anim_clip, AnimClip::Death.as_u32());
        assert_eq!(linger.len(), 1);
        // Position is the frozen last-known pose.
        assert!((out[0].x - 3.0).abs() < 1e-4);
        assert!((out[0].y - 4.0).abs() < 1e-4);
    }

    #[test]
    fn linger_advances_phase_and_is_evicted_after_the_window() {
        // Tick 1: unit 5 just vanished (in prev at tick 0, gone from curr at tick 1).
        let prev0 = snapshot(0, vec![unit(5, false)]);
        let curr0 = snapshot(1, vec![]);
        let mut linger = DeathLinger::new();

        let early = linger.update(&prev0, &curr0, 1.0, &Palette::DEFAULT);
        let early_phase = early[0].anim_phase;

        // Later real ticks: mirroring actual per-tick calls, unit 5 has been gone from every
        // snapshot since it died, so `prev`/`curr` no longer mention it at all — only the clock
        // (`tick_f`) advances. The phase should have advanced.
        let prev_mid = snapshot(10, vec![]);
        let curr_mid = snapshot(11, vec![]);
        let mid = linger.update(&prev_mid, &curr_mid, 20.0, &Palette::DEFAULT);
        assert_eq!(mid.len(), 1, "still lingering mid-window");
        assert!(
            mid[0].anim_phase > early_phase,
            "death phase advances with the clock"
        );

        // Past the total window (measured from the tick it was first found missing), the entry is
        // evicted.
        let prev_late = snapshot(90, vec![]);
        let curr_late = snapshot(91, vec![]);
        let late = linger.update(
            &prev_late,
            &curr_late,
            1.0 + DEATH_LINGER_TICKS + 1.0,
            &Palette::DEFAULT,
        );
        assert!(late.is_empty(), "evicted once the fade window elapses");
        assert!(linger.is_empty());
    }

    #[test]
    fn unit_still_present_in_curr_produces_no_linger() {
        let prev = snapshot(0, vec![unit(7, false)]);
        let curr = snapshot(1, vec![unit(7, false)]); // never vanished
        let mut linger = DeathLinger::new();
        let out = linger.update(&prev, &curr, 1.0, &Palette::DEFAULT);
        assert!(out.is_empty());
        assert!(linger.is_empty());
    }

    #[test]
    fn index_reuse_is_not_treated_as_still_lingering() {
        // Tick 0: unit 9 (enemy) is alive.
        let prev0 = snapshot(0, vec![unit(9, false)]);
        // Tick 1: unit 9 died (absent) — this frame should linger it.
        let curr1 = snapshot(1, vec![]);
        let mut linger = DeathLinger::new();
        let out1 = linger.update(&prev0, &curr1, 1.0, &Palette::DEFAULT);
        assert_eq!(out1.len(), 1);
        assert_eq!(linger.len(), 1);

        // Tick 2: a freshly-spawned unit reuses index 9 and is alive again. It must NOT read as a
        // still-lingering corpse — the linger entry for index 9 is evicted, and no new one is added
        // (the reused index is present-in-curr, never a linger candidate).
        let prev1 = curr1.clone();
        let curr2 = snapshot(2, vec![unit(9, false)]);
        let out2 = linger.update(&prev1, &curr2, 2.0, &Palette::DEFAULT);
        assert!(
            out2.is_empty(),
            "reused index draws only via interpolate_instances, never a linger"
        );
        assert!(linger.is_empty());
    }

    #[test]
    fn embodied_avatar_death_is_never_lingered() {
        // Invariant #5: the possessed avatar's death ejects to command; no lingering "body".
        let prev = snapshot(0, vec![unit(3, true)]); // embodied
        let curr = snapshot(1, vec![]); // avatar died
        let mut linger = DeathLinger::new();
        let out = linger.update(&prev, &curr, 1.0, &Palette::DEFAULT);
        assert!(out.is_empty());
        assert!(linger.is_empty());
    }
}
