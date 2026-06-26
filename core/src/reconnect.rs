//! Reconnect policy (Phase 3 workstream C): resume a peer from an authoritative snapshot plus a
//! replay of the lockstep-buffered merged command stream.
//!
//! A returning peer is rebuilt by `deserialize`ing an authoritative snapshot taken at tick `T0`
//! ([`Sim::serialize`](crate::sim::Sim::serialize), D28) and then replaying the merged command
//! sets `[T0, now)` the live session buffered ([`Lockstep::replay_range`](crate::lockstep::Lockstep::replay_range))
//! through a plain [`Sim::step`](crate::sim::Sim::step) loop. This is **correct by construction**
//! once D28's round-trip-replay invariant holds: it is the same `serialize@T0 → deserialize →
//! step(cmds[T0..now])` walk, driven from the live buffer — so the resumed peer's checksum stream
//! is bit-identical to a peer that never disconnected.
//!
//! Transport-free and serde-free — pure `core` (invariants #1, #2). The Wi-Fi↔cellular **handoff**
//! half of workstream C (surviving a network switch without a full reconnect) needs QUIC
//! connection migration and is **deferred until a QUIC transport exists** (only a UDP transport
//! has landed; `docs/plans/phase-3-plan.md` §B/§C, D28).

use crate::lockstep::Lockstep;
use crate::persist::DeserializeError;
use crate::sim::Sim;

/// Why a reconnect could not be served from a snapshot + the live command buffer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReconnectError {
    /// The snapshot bytes were malformed or incompatible (forwarded from
    /// [`Sim::deserialize`](crate::sim::Sim::deserialize)).
    Snapshot(DeserializeError),
    /// The snapshot's embedded tick did not match the requested `from` tick — a caller bookkeeping
    /// bug (the snapshot was taken at a different tick than claimed). Rejected rather than replayed
    /// from the wrong base, which would resume to a divergent state.
    SnapshotTickMismatch { snapshot_tick: u64, expected: u64 },
    /// A command set in `[from, to)` had been pruned out of the live retain window. The host must
    /// snapshot more often (advance [`Lockstep::retain_from`](crate::lockstep::Lockstep::retain_from))
    /// — surfaced loudly rather than served as a silent short replay that would desync.
    CommandsPruned { from: u64, to: u64 },
    /// The requested replay range was invalid: `to < from` (which would silently return an
    /// under-stepped sim), or `to` is beyond the live session's executed frontier (`next_tick`,
    /// reported as `live_tick`). Rejected at the boundary rather than resumed to the wrong tick.
    InvalidRange { from: u64, to: u64, live_tick: u64 },
}

/// Rebuild a peer's [`Sim`] from an authoritative `snapshot` taken at tick `from` plus a replay of
/// the merged command sets `[from, to)` held by `live`. The result is bit-identical to a peer that
/// ran uninterrupted to `to`: it computes the same checksum stream from `from` onward.
///
/// `to` is normally `live.next_tick()`. `live` is read-only here (the live session is untouched).
pub fn resume_from_snapshot(
    snapshot: &[u8],
    from: u64,
    to: u64,
    live: &Lockstep,
) -> Result<Sim, ReconnectError> {
    let live_tick = live.next_tick();
    if to < from || to > live_tick {
        return Err(ReconnectError::InvalidRange {
            from,
            to,
            live_tick,
        });
    }
    let mut sim = Sim::deserialize(snapshot).map_err(ReconnectError::Snapshot)?;
    if sim.tick_count() != from {
        return Err(ReconnectError::SnapshotTickMismatch {
            snapshot_tick: sim.tick_count(),
            expected: from,
        });
    }
    let cmds = live
        .replay_range(from, to)
        .ok_or(ReconnectError::CommandsPruned { from, to })?;
    for set in cmds {
        sim.step(set);
    }
    Ok(sim)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::components::{EntityKind, Faction, Stance, UnitKind, Vec2};
    use crate::economy::{self, Resources};
    use crate::fixed::Fixed;
    use crate::sim::Command;
    use crate::territory::ControlPoint;

    const SCENE_SEED: u64 = 0x9E3779B97F4A7C15;

    fn v(x: i32, y: i32) -> Vec2 {
        Vec2::new(Fixed::from_int(x), Fixed::from_int(y))
    }

    fn spawn_rifleman(sim: &mut Sim, x: i32, y: i32, faction: Faction) {
        let (health, weapon) = economy::unit_stats(UnitKind::Rifleman);
        let ent = sim.world.spawn();
        let i = ent.index as usize;
        sim.world.kind[i] = EntityKind::Unit;
        sim.world.faction[i] = faction;
        sim.world.pos[i] = v(x, y);
        sim.world.health[i] = health;
        sim.world.weapon[i] = weapon;
        sim.world.stance[i] = Stance::FireAtWill;
    }

    /// An identical deterministic scene + a player camp (so production spawns can exercise the
    /// free list across a resume).
    fn scene(sim: &mut Sim) -> crate::ecs::Entity {
        sim.resources = Resources::new(100_000);
        sim.territory.points.push(ControlPoint::neutral(Vec2::ZERO));
        spawn_rifleman(sim, -5, 0, Faction::Player);
        spawn_rifleman(sim, 5, 0, Faction::Enemy);
        spawn_rifleman(sim, 5, 2, Faction::Enemy);
        economy::build(
            &mut sim.world,
            &mut sim.resources,
            Faction::Player,
            crate::components::BuildingKind::Camp,
            v(-20, 20),
        )
        .expect("camp affordable")
    }

    /// Per-tick scripted commands (keyed by execution tick), shared by the live and reference runs.
    fn script(camp: crate::ecs::Entity, t: u64) -> Vec<Command> {
        match t {
            5 => vec![Command::QueueProduction {
                camp,
                unit: UnitKind::Rifleman,
            }],
            12 => vec![Command::Build {
                faction: Faction::Player,
                kind: crate::components::BuildingKind::Camp,
                pos: v(-22, 18),
            }],
            _ => Vec::new(),
        }
    }

    /// Drive a single-peer lockstep session (delay 0, so every submitted tick executes) to `len`,
    /// taking a snapshot at `snap_at`, and return: the control checksum stream, the snapshot bytes,
    /// the live `Lockstep` (with `retain_from(snap_at)` installed), and the camp handle.
    fn drive(
        len: u64,
        snap_at: u64,
    ) -> (Vec<u64>, Vec<u8>, Lockstep, crate::ecs::Entity) {
        let mut sim = Sim::new(SCENE_SEED);
        let camp = scene(&mut sim);
        let mut ls = Lockstep::new(1, 0, 0);
        let mut control = Vec::with_capacity(len as usize);
        let mut snapshot = Vec::new();
        for t in 0..len {
            ls.submit(script(camp, t));
            let cmds = ls.try_advance().expect("single-peer session always advances");
            sim.step(&cmds);
            control.push(sim.checksum());
            if t + 1 == snap_at {
                // Snapshot the post-step state at tick `snap_at` (sim.tick_count() == snap_at now).
                snapshot = sim.serialize();
                ls.retain_from(snap_at);
            }
        }
        (control, snapshot, ls, camp)
    }

    #[test]
    fn reconnect_resumes_bit_identically() {
        let len = 40;
        let snap_at = 15;
        let (control, snapshot, ls, _camp) = drive(len, snap_at);

        let mut resumed = resume_from_snapshot(&snapshot, snap_at, len, &ls).expect("resume");
        assert_eq!(
            resumed.checksum(),
            control[(len - 1) as usize],
            "resumed state must match the never-interrupted peer at the resume-to tick"
        );
        // And it keeps stepping in lockstep: replay a few more scripted ticks and compare to a
        // fresh control run carried past `len`.
        let mut control_sim = Sim::new(SCENE_SEED);
        let camp = scene(&mut control_sim);
        for t in 0..len {
            control_sim.step(&script(camp, t));
        }
        for t in len..len + 5 {
            control_sim.step(&script(camp, t));
            resumed.step(&script(camp, t));
            assert_eq!(
                resumed.checksum(),
                control_sim.checksum(),
                "post-resume stream diverged at tick {t}"
            );
        }
    }

    #[test]
    fn reconnect_at_genesis_replays_whole_script() {
        let len = 30;
        // Snapshot at tick 0 (genesis): the whole script must replay.
        let mut sim = Sim::new(SCENE_SEED);
        let camp = scene(&mut sim);
        let snapshot = sim.serialize(); // tick 0, before any step
        let mut ls = Lockstep::new(1, 0, 0);
        ls.retain_from(0);
        let mut control = Vec::new();
        for t in 0..len {
            ls.submit(script(camp, t));
            let cmds = ls.try_advance().expect("advances");
            sim.step(&cmds);
            control.push(sim.checksum());
        }
        let resumed = resume_from_snapshot(&snapshot, 0, len, &ls).expect("resume from genesis");
        assert_eq!(resumed.checksum(), control[(len - 1) as usize]);
    }

    #[test]
    fn reconnect_with_zero_replay_is_the_snapshot_itself() {
        let len = 25;
        let snap_at = 20;
        let (control, snapshot, ls, _camp) = drive(len, snap_at);
        // Resume to exactly the snapshot tick: an empty replay range, so deserialize alone must
        // reproduce the state.
        let resumed = resume_from_snapshot(&snapshot, snap_at, snap_at, &ls).expect("resume");
        assert_eq!(resumed.tick_count(), snap_at);
        assert_eq!(resumed.checksum(), control[(snap_at - 1) as usize]);
    }

    #[test]
    fn reconnect_rejects_pruned_command_range() {
        // A genuine early snapshot whose replay commands were later pruned when the retention
        // floor advanced → a loud CommandsPruned, never a silent short replay that would desync.
        let mut sim = Sim::new(SCENE_SEED);
        let camp = scene(&mut sim);
        let mut ls = Lockstep::new(1, 0, 0);
        let mut snapshot = Vec::new();
        for t in 0..40 {
            ls.submit(script(camp, t));
            let cmds = ls.try_advance().expect("advances");
            sim.step(&cmds);
            if t + 1 == 5 {
                snapshot = sim.serialize(); // a real snapshot at tick 5
                ls.retain_from(5);
            }
        }
        // A later snapshot advances the floor, pruning the early commands [5, 35).
        ls.retain_from(35);
        // The snapshot is genuinely at tick 5 (tick check passes), but its replay range was pruned.
        let err = resume_from_snapshot(&snapshot, 5, 40, &ls)
            .err()
            .expect("expected an error");
        assert_eq!(err, ReconnectError::CommandsPruned { from: 5, to: 40 });
    }

    #[test]
    fn reconnect_rejects_invalid_range() {
        let len = 20;
        let snap_at = 10;
        let (_control, snapshot, ls, _camp) = drive(len, snap_at);
        let live_tick = ls.next_tick();
        // `to` < `from`: would otherwise silently return an under-stepped sim.
        let err = resume_from_snapshot(&snapshot, snap_at, snap_at - 1, &ls)
            .err()
            .expect("expected an error");
        assert_eq!(
            err,
            ReconnectError::InvalidRange {
                from: snap_at,
                to: snap_at - 1,
                live_tick,
            }
        );
        // `to` beyond the live executed frontier: unsatisfiable, rejected precisely.
        let err = resume_from_snapshot(&snapshot, snap_at, len + 100, &ls)
            .err()
            .expect("expected an error");
        assert_eq!(
            err,
            ReconnectError::InvalidRange {
                from: snap_at,
                to: len + 100,
                live_tick,
            }
        );
    }

    #[test]
    fn reconnect_rejects_snapshot_tick_mismatch() {
        let len = 30;
        let snap_at = 15;
        let (_control, snapshot, ls, _camp) = drive(len, snap_at);
        // Claim the snapshot is from a different tick than it actually is.
        let err = resume_from_snapshot(&snapshot, snap_at + 1, len, &ls).err().expect("expected an error");
        assert_eq!(
            err,
            ReconnectError::SnapshotTickMismatch {
                snapshot_tick: snap_at,
                expected: snap_at + 1,
            }
        );
    }

    #[test]
    fn reconnect_rejects_malformed_snapshot() {
        let len = 20;
        let snap_at = 10;
        let (_control, _snapshot, ls, _camp) = drive(len, snap_at);
        let err = resume_from_snapshot(&[0xFF, 0x00, 0x01], snap_at, len, &ls).err().expect("expected an error");
        assert!(matches!(err, ReconnectError::Snapshot(_)), "got {err:?}");
    }
}
