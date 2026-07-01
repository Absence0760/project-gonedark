//! Embodied audio mix (invariant #6, game-design §6) — the PURE, platform-free layer that
//! turns the deterministic per-tick [`SimEvent`] stream into the positioned [`AudioCue`]s the
//! backend renders. While embodied, strategic sound bleeding into the FPS view is the *primary*
//! directional-awareness system: "alerts, not intel" by ear.
//!
//! This is a presentation derivation: it reads sim events + the listener pose and produces
//! cues; it MUST NOT mutate sim state (it never desyncs lockstep — invariant #1). Floats are
//! fine here (presentation, not the sim). It is a free function so it is unit-testable without
//! a GPU or an audio device.
//!
//! IMPLEMENTATION OWNER: worker 3 (embodied audio). KEEP the public signature intact.

use std::f32::consts::PI;

use gonedark_core::components::EntityKind;
use gonedark_core::ecs::World;
use gonedark_core::event::SimEvent;
use gonedark_pal::{AudioCue, SoundId};

/// The fire-and-forget `play_oneshot` id for the **host-clock weapon-fire** cue (WS-A, CP-2): a crack
/// emitted on the embodied trigger press, *decoupled* from the deterministic `Damaged`-event
/// `Gunfire` (which sounds only for connecting shots), so a **missed** shot still cracks in lockstep
/// with the muzzle flash. Presentation/host-side only (invariant #4/#6) — the host calls
/// `audio.play_oneshot(weapon_fire_cue())` once per [`Command::Fire`](gonedark_core::sim::Command) it
/// emits. The id round-trips through [`gonedark_pal::mix::oneshot_sound`] to [`SoundId::WeaponFire`].
#[inline]
pub fn weapon_fire_cue() -> u32 {
    SoundId::WeaponFire as u32
}

/// The fire-and-forget `play_oneshot` id for the **impact** cue (WS-A) — coupled to the impact VFX at
/// the point the avatar's own shot landed (feedback on the player's own action, invariant #6). The
/// host calls `audio.play_oneshot(impact_cue())` on the same frame it stamps the impact visual. The
/// id round-trips through [`gonedark_pal::mix::oneshot_sound`] to [`SoundId::Impact`].
#[inline]
pub fn impact_cue() -> u32 {
    SoundId::Impact as u32
}

/// Distance (world units) at which a cue's gain is halved. Chosen so the falloff
/// `1 / (1 + dist/FALLOFF)` reads as "audible across a camp, faint across the map": a source on
/// top of the listener is ~1.0, one `FALLOFF` away is 0.5, and far events tail toward 0 without
/// ever clipping to silence (a backend can still gate inaudibly-quiet cues if it wants).
const FALLOFF: f32 = 24.0;

/// Build this frame's positioned audio mix from `events` (this tick's deterministic stream),
/// the `embodied` flag, the `listener` world position (the avatar, when embodied), the listener
/// `yaw` (radians, presentation-only), and a read-only `world` for any classification.
///
/// One cue per event, in stream order (deterministic — events arrive already ordered):
/// - sound: `Killed`→`UnitDown`, `Captured`→`Capture`, `UnitProduced`→`ProductionReady`,
///   `Damaged`→`BaseHit` if the target is a `Building` else `Gunfire`.
/// - `azimuth`: bearing from `listener` to the event, relative to `yaw`, normalized to
///   `(-PI, PI]` (0 = dead ahead, positive = right).
/// - `gain`: distance attenuation in `[0, 1]` (near→~1, far→~0).
/// - `muffled`: true only when `embodied` AND the sound is *strategic* (`Capture`, `BaseHit`,
///   `ProductionReady`) — the off-map bleed. Local combat (`Gunfire`, `UnitDown`) is never
///   muffled; nothing is muffled while commanding (`embodied == false`).
///
/// Read-only over `world`; never mutates sim state.
pub fn mix_cues(
    events: &[SimEvent],
    embodied: bool,
    listener: (f32, f32),
    yaw: f32,
    world: &World,
) -> Vec<AudioCue> {
    let mut cues = Vec::with_capacity(events.len());
    for event in events {
        // The event's world position (every variant carries one) and its sound class.
        let (pos, sound) = match *event {
            SimEvent::Damaged { entity, pos, .. } => {
                // The target may have been despawned the same tick — guard the index, exactly
                // as alerts.rs does. Out-of-range → treat as a unit (Gunfire), never panic.
                let idx = entity.index as usize;
                let is_building = idx < world.capacity() && world.kind[idx] == EntityKind::Building;
                let sound = if is_building {
                    SoundId::BaseHit
                } else {
                    SoundId::Gunfire
                };
                (pos, sound)
            }
            SimEvent::Killed { pos, .. } => (pos, SoundId::UnitDown),
            SimEvent::Captured { pos, .. } => (pos, SoundId::Capture),
            SimEvent::UnitProduced { pos, .. } => (pos, SoundId::ProductionReady),
            // A committed trigger pull. The *player's own* gun crack is played host-side off the
            // `avatar_fired` seam (a single, non-positional cue — it's the weapon in your hands), so
            // the positional mix skips `Fired` here to avoid a doubled report. `resolve_fire` — hence
            // `Fired` — is embodied-only, so this is never an AI unit's shot.
            SimEvent::Fired { .. } => continue,
        };

        // Q16.16 → f32 hop happens HERE, via the one sanctioned converter (never in core).
        let ex = gonedark_render::fixed_to_f32(pos.x);
        let ey = gonedark_render::fixed_to_f32(pos.y);
        let dx = ex - listener.0;
        let dy = ey - listener.1;

        // World bearing to the source, then rotate into the listener's frame and normalize to
        // (-PI, PI]. 0 = dead ahead along `yaw`, positive = to the right. The engine frame is
        // right-handed with forward `(cos yaw, sin yaw)` and up `+Z`, so the player's right is
        // the CLOCKWISE side — `yaw - world_bearing` (matching the HUD's bearing in `render::hud`).
        let world_bearing = dy.atan2(dx);
        let azimuth = normalize_angle(yaw - world_bearing);

        // Distance attenuation: 1 / (1 + dist/FALLOFF) ∈ (0, 1]. Zero distance → 1.0.
        let dist = dx.hypot(dy);
        let gain = 1.0 / (1.0 + dist / FALLOFF);

        // Strategic sound bleeding into the embodied mix is the off-map "muffled" bleed.
        let strategic = matches!(
            sound,
            SoundId::Capture | SoundId::BaseHit | SoundId::ProductionReady
        );
        let muffled = embodied && strategic;

        cues.push(AudioCue {
            sound,
            azimuth,
            gain,
            muffled,
        });
    }
    cues
}

/// Wrap an angle (radians) into `(-PI, PI]`. Keeps relative bearings comparable regardless of
/// how far `yaw` has wound past a full turn.
#[inline]
fn normalize_angle(mut a: f32) -> f32 {
    while a > PI {
        a -= 2.0 * PI;
    }
    while a <= -PI {
        a += 2.0 * PI;
    }
    a
}

#[cfg(test)]
mod tests {
    use super::*;
    use gonedark_core::components::{EntityKind, Faction, Vec2};
    use gonedark_core::ecs::{Entity, World};
    use gonedark_core::event::SimEvent;
    use gonedark_core::fixed::Fixed;

    const EPS: f32 = 1e-4;

    fn pos(x: i32, y: i32) -> Vec2 {
        Vec2::new(Fixed::from_int(x), Fixed::from_int(y))
    }

    /// A world with one unit (index 0) and one building (index 1) — the alerts.rs pattern.
    fn world_unit_and_building() -> (World, Entity, Entity) {
        let mut w = World::new();
        let unit = w.spawn();
        let bldg = w.spawn();
        w.kind[bldg.index as usize] = EntityKind::Building;
        (w, unit, bldg)
    }

    fn damaged(entity: Entity, p: Vec2) -> SimEvent {
        SimEvent::Damaged {
            entity,
            faction: Faction::Player,
            source: entity,
            amount: Fixed::from_int(5),
            pos: p,
        }
    }

    // --- sound mapping ----------------------------------------------------------------------

    #[test]
    fn damage_on_building_is_basehit_on_unit_is_gunfire() {
        let (w, unit, bldg) = world_unit_and_building();
        let cues = mix_cues(
            &[damaged(bldg, pos(0, 0)), damaged(unit, pos(0, 0))],
            false,
            (0.0, 0.0),
            0.0,
            &w,
        );
        assert_eq!(cues.len(), 2);
        assert_eq!(cues[0].sound, SoundId::BaseHit);
        assert_eq!(cues[1].sound, SoundId::Gunfire);
    }

    #[test]
    fn killed_captured_produced_map_to_their_sounds() {
        let (w, unit, _) = world_unit_and_building();
        let events = [
            SimEvent::Killed {
                entity: unit,
                faction: Faction::Player,
                source: unit,
                pos: pos(0, 0),
            },
            SimEvent::Captured {
                pos: pos(0, 0),
                from: Faction::Player,
                to: Faction::Enemy,
            },
            SimEvent::UnitProduced {
                faction: Faction::Player,
                pos: pos(0, 0),
            },
        ];
        let cues = mix_cues(&events, false, (0.0, 0.0), 0.0, &w);
        assert_eq!(cues.len(), 3);
        assert_eq!(cues[0].sound, SoundId::UnitDown);
        assert_eq!(cues[1].sound, SoundId::Capture);
        assert_eq!(cues[2].sound, SoundId::ProductionReady);
    }

    #[test]
    fn damage_to_despawned_index_does_not_panic_and_is_gunfire() {
        let (w, _, _) = world_unit_and_building();
        let phantom = Entity {
            index: 999,
            generation: 0,
        };
        let cues = mix_cues(&[damaged(phantom, pos(0, 0))], false, (0.0, 0.0), 0.0, &w);
        assert_eq!(cues.len(), 1);
        assert_eq!(cues[0].sound, SoundId::Gunfire);
    }

    // --- azimuth ----------------------------------------------------------------------------

    #[test]
    fn event_straight_ahead_is_azimuth_zero() {
        let (w, unit, _) = world_unit_and_building();
        // Listener at origin facing +x (yaw 0); event at (+x, 0) is dead ahead.
        let cues = mix_cues(&[damaged(unit, pos(10, 0))], false, (0.0, 0.0), 0.0, &w);
        assert!(cues[0].azimuth.abs() < EPS, "azimuth {}", cues[0].azimuth);
    }

    #[test]
    fn event_to_one_side_has_consistent_signs() {
        let (w, unit, _) = world_unit_and_building();
        // Facing +x with up +Z (right-handed), the player's right is -y. So a source at (0,+y)
        // is to the LEFT (azimuth -PI/2) and one at (0,-y) is to the RIGHT (azimuth +PI/2).
        let left = mix_cues(&[damaged(unit, pos(0, 10))], false, (0.0, 0.0), 0.0, &w);
        assert!(left[0].azimuth < 0.0, "azimuth {}", left[0].azimuth);
        assert!((left[0].azimuth + PI / 2.0).abs() < EPS);

        let right = mix_cues(&[damaged(unit, pos(0, -10))], false, (0.0, 0.0), 0.0, &w);
        assert!(right[0].azimuth > 0.0, "azimuth {}", right[0].azimuth);
        assert!((right[0].azimuth - PI / 2.0).abs() < EPS);
    }

    #[test]
    fn azimuth_is_relative_to_yaw() {
        let (w, unit, _) = world_unit_and_building();
        // Event at (+x, 0); listener faces +y (yaw PI/2, i.e. "north"). Facing north, east (+x)
        // is to the player's right → azimuth +PI/2 (positive = right, per the cue contract).
        let cues = mix_cues(
            &[damaged(unit, pos(10, 0))],
            false,
            (0.0, 0.0),
            PI / 2.0,
            &w,
        );
        assert!(
            (cues[0].azimuth - PI / 2.0).abs() < EPS,
            "azimuth {}",
            cues[0].azimuth
        );
    }

    // --- gain -------------------------------------------------------------------------------

    #[test]
    fn gain_falls_off_with_distance_and_stays_in_range() {
        let (w, unit, _) = world_unit_and_building();
        let near = mix_cues(&[damaged(unit, pos(1, 0))], false, (0.0, 0.0), 0.0, &w);
        let far = mix_cues(&[damaged(unit, pos(500, 0))], false, (0.0, 0.0), 0.0, &w);
        assert!(near[0].gain > far[0].gain);
        for g in [near[0].gain, far[0].gain] {
            assert!((0.0..=1.0).contains(&g), "gain {} out of range", g);
        }
        // Zero distance → essentially full gain.
        let here = mix_cues(&[damaged(unit, pos(0, 0))], false, (0.0, 0.0), 0.0, &w);
        assert!((here[0].gain - 1.0).abs() < EPS);
    }

    // --- muffled (the off-map strategic bleed) ----------------------------------------------

    #[test]
    fn strategic_sound_is_muffled_only_while_embodied() {
        let (w, _, _) = world_unit_and_building();
        let captured = SimEvent::Captured {
            pos: pos(5, 5),
            from: Faction::Player,
            to: Faction::Enemy,
        };
        let embodied = mix_cues(&[captured], true, (0.0, 0.0), 0.0, &w);
        assert!(
            embodied[0].muffled,
            "strategic cue should bleed in muffled while embodied"
        );

        let commanding = mix_cues(&[captured], false, (0.0, 0.0), 0.0, &w);
        assert!(!commanding[0].muffled, "no bleed concept while commanding");
    }

    #[test]
    fn local_combat_is_never_muffled_even_while_embodied() {
        let (w, unit, _) = world_unit_and_building();
        let gunfire = mix_cues(&[damaged(unit, pos(2, 0))], true, (0.0, 0.0), 0.0, &w);
        assert!(!gunfire[0].muffled);

        let killed = mix_cues(
            &[SimEvent::Killed {
                entity: unit,
                faction: Faction::Player,
                source: unit,
                pos: pos(2, 0),
            }],
            true,
            (0.0, 0.0),
            0.0,
            &w,
        );
        assert!(!killed[0].muffled);
    }

    // --- ordering / empty -------------------------------------------------------------------

    #[test]
    fn empty_events_produce_empty_mix() {
        let (w, _, _) = world_unit_and_building();
        let cues = mix_cues(&[], true, (1.0, 2.0), 0.5, &w);
        assert!(cues.is_empty());
    }

    // --- host-clock fire/impact cue ids (WS-A) -----------------------------------------------

    #[test]
    fn host_clock_cue_ids_round_trip_to_their_sounds() {
        // The host-clock fire/impact `play_oneshot` ids must decode to the matching SoundId via the
        // shared backend table — so a press cracks WeaponFire and a landed shot thuds Impact, both
        // decoupled from the `Damaged`-event Gunfire above.
        assert_eq!(
            gonedark_pal::mix::oneshot_sound(weapon_fire_cue()),
            SoundId::WeaponFire
        );
        assert_eq!(
            gonedark_pal::mix::oneshot_sound(impact_cue()),
            SoundId::Impact
        );
        // And they are distinct from the connecting-shot Gunfire / the UI HitConfirm tick.
        assert_ne!(weapon_fire_cue(), SoundId::Gunfire as u32);
        assert_ne!(impact_cue(), SoundId::HitConfirm as u32);
    }
}
