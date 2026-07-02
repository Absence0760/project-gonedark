//! **Replay record & playback — the determinism freebie** (roadmap PC-3).
//!
//! A match is a *seed* + an ordered *input log* (invariant #1): the sim is fixed-point and fully
//! deterministic, so the world is a pure function of (seed, per-tick commands). That makes replay
//! and spectating nearly free — you do not store world state, only the seed and the inputs, and
//! re-running them reconstructs the match bit-for-bit.
//!
//! This crate proves that property end-to-end, headless, exactly like `sim-runner` /
//! `net-sim-runner`:
//!
//! - **record** ([`record`]) — drive a bundled [`Scenario`] for N ticks, feeding the scripted
//!   command stream into the sim, and *capture* every non-empty tick's commands into a [`Replay`].
//!   Emit the per-tick checksum stream as it goes.
//! - **encode/decode** ([`Replay::encode`] / [`Replay::decode`]) — serialize the replay to a
//!   compact, deterministic byte artifact (the codec lives here, not in `core` — invariant #2).
//! - **playback** ([`playback`]) — decode the artifact, re-seed the *same* scenario, feed back the
//!   recorded commands tick-by-tick, and emit the checksum stream.
//! - **the proof** ([`round_trip_ok`], and the tests) — the playback checksum stream is asserted
//!   **bit-identical** to the record run. Same seed + same inputs ⇒ same world; if it ever
//!   diverges that is a real determinism bug, exactly what invariant #7's matrix guards.
//!
//! Two shapes ride this same seed+log foundation:
//!
//! - **single-peer** ([`Replay`]) — one ordered command stream per tick (a solo match or a
//!   spectator of one). This is the [D89] foundation.
//! - **multi-peer** ([`MultiReplay`]) — a lockstep PvP match, where each tick's inputs come from
//!   several peers. The record keeps every peer's set *separately* (`tick -> peer -> commands`)
//!   and merges them at playback in a **deterministic per-peer order** — ascending peer id,
//!   concatenated — which is byte-for-byte the rule [`core::lockstep`](gonedark_core::lockstep)
//!   applies in `try_advance` (fixed peer order = the stable application order `Sim::step`
//!   relies on). Because the merge is keyed by peer id, two peers' logs recorded in *different
//!   arrival orders* replay to the **identical** per-tick checksum stream — the load-bearing
//!   multi-peer property, proven in the tests.
//!
//! Still headless/CI-safe (checksum-proven, no GPU). A spectator *view* (rendering a playback)
//! is a render-side follow-up, out of scope here.

mod codec;
pub mod scenario;

use std::collections::BTreeMap;

use gonedark_core::lockstep::PeerId;
use gonedark_core::sim::{Command, Sim};

pub use codec::{ReplayError, FORMAT_VERSION, MAGIC, MAGIC_MULTI};
pub use scenario::Scenario;

/// A recorded match: which scenario, the seed, the tick count, and the captured per-tick command
/// log. This IS the replay artifact — encode it to bytes ([`Replay::encode`]) to persist it.
#[derive(Clone, Debug)]
pub struct Replay {
    pub scenario: Scenario,
    pub seed: u64,
    /// Total ticks the record run advanced (tick 0 is the seeded state; steps run for `1..ticks`).
    pub ticks: u64,
    /// The input log: ticks with at least one command, in tick order. Idle ticks are omitted (a
    /// replay of an idle sim is just the seed), so this is only as big as the actual inputs.
    log: BTreeMap<u64, Vec<Command>>,
}

impl Replay {
    /// The commands recorded for `tick` (empty for an idle tick).
    pub fn commands_for(&self, tick: u64) -> &[Command] {
        self.log.get(&tick).map(Vec::as_slice).unwrap_or(&[])
    }

    /// Total number of commands across the whole log (a quick "is this a meaningful replay?" size).
    pub fn command_count(&self) -> usize {
        self.log.values().map(Vec::len).sum()
    }

    /// Serialize to the deterministic byte artifact:
    /// `MAGIC | version:u16 | scenario_tag:u8 | seed:u64 | ticks:u64 | entries:u32`
    /// then, per non-empty tick: `tick:u64 | n_commands:u32 | commands…`.
    pub fn encode(&self) -> Vec<u8> {
        let mut w = Vec::new();
        w.extend_from_slice(&MAGIC);
        codec::put_u16(&mut w, FORMAT_VERSION);
        codec::put_u8(&mut w, self.scenario.tag());
        codec::put_u64(&mut w, self.seed);
        codec::put_u64(&mut w, self.ticks);
        codec::put_u32(&mut w, self.log.len() as u32);
        for (&tick, cmds) in &self.log {
            codec::put_u64(&mut w, tick);
            codec::put_u32(&mut w, cmds.len() as u32);
            for c in cmds {
                codec::put_command(&mut w, c);
            }
        }
        w
    }

    /// Parse a byte artifact produced by [`Replay::encode`]. Errors (never panics) on a corrupt or
    /// version-skewed stream — a truncated/garbage replay is data to reject, not a crash.
    pub fn decode(bytes: &[u8]) -> Result<Replay, ReplayError> {
        let mut r = codec::Reader::new(bytes);
        if r.magic()? != MAGIC {
            return Err(ReplayError::BadMagic);
        }
        let version = r.u16()?;
        if version != FORMAT_VERSION {
            return Err(ReplayError::BadVersion(version));
        }
        let scenario = Scenario::from_tag(r.u8()?)
            .ok_or(ReplayError::BadTag { what: "scenario", tag: 0xFF })?;
        let seed = r.u64()?;
        let ticks = r.u64()?;
        let entries = r.u32()?;
        let mut log: BTreeMap<u64, Vec<Command>> = BTreeMap::new();
        for _ in 0..entries {
            let tick = r.u64()?;
            let n = r.u32()?;
            let mut cmds = Vec::with_capacity(n as usize);
            for _ in 0..n {
                cmds.push(r.command()?);
            }
            log.insert(tick, cmds);
        }
        if !r.at_end() {
            return Err(ReplayError::TrailingBytes);
        }
        Ok(Replay {
            scenario,
            seed,
            ticks,
            log,
        })
    }
}

/// Fold the whole per-tick checksum stream of a run into one summary hash, so two runs can be
/// compared (and printed) as a single value as well as tick-by-tick. Pure host-side helper — it
/// mixes the *sim's own* `checksum()` outputs (invariant #7's entry point), never a parallel one.
pub fn stream_digest(checksums: &[u64]) -> u64 {
    // A simple FNV-1a-style fold over the checksum stream. Host-side only; not part of the sim.
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for &c in checksums {
        h ^= c;
        h = h.wrapping_mul(0x0000_0100_0000_01B3);
    }
    h
}

/// Drive `scenario` (seeded from `seed`) for `ticks` ticks, capturing the command log into a
/// [`Replay`] and returning `(per_tick_checksums, replay)`.
///
/// The checksum vector has one entry per emitted tick: index 0 is the seeded state (tick 0), then
/// one after each of the `1..ticks` steps — the same `<tick> <checksum>` stream sim-runner prints.
pub fn record(scenario: Scenario, seed: u64, ticks: u64) -> (Vec<u64>, Replay) {
    let scenario::Built {
        mut sim,
        scripted,
    } = scenario::build(scenario, seed);
    let mut log: BTreeMap<u64, Vec<Command>> = BTreeMap::new();
    let mut checksums = Vec::with_capacity(ticks as usize);

    let empty: Vec<Command> = Vec::new();
    checksums.push(sim.checksum()); // tick 0: seeded state
    for t in 1..ticks {
        let cmds: &[Command] = scripted.get(&t).unwrap_or(&empty);
        // Capture the exact command set fed on this tick — this IS the recording step. In a live
        // match these would arrive from input; here they come from the script, but the record path
        // is identical: whatever we step with, we log.
        if !cmds.is_empty() {
            log.insert(t, cmds.to_vec());
        }
        sim.step(cmds);
        checksums.push(sim.checksum());
    }

    let replay = Replay {
        scenario,
        seed,
        ticks,
        log,
    };
    (checksums, replay)
}

/// Re-run a [`Replay`]: re-seed its scenario and feed back the recorded commands tick-by-tick,
/// returning the per-tick checksum stream. Note it feeds ONLY the recorded log — never the
/// scenario's build-time script — so a match equal to [`record`]'s stream proves the input log
/// alone reproduces the world.
pub fn playback(replay: &Replay) -> Vec<u64> {
    // Re-seed the identical world from (scenario, seed). We deliberately discard the builder's own
    // scripted commands and drive purely from the recorded log.
    let built = scenario::build(replay.scenario, replay.seed);
    let mut sim: Sim = built.sim;
    let mut checksums = Vec::with_capacity(replay.ticks as usize);

    checksums.push(sim.checksum()); // tick 0
    for t in 1..replay.ticks {
        sim.step(replay.commands_for(t));
        checksums.push(sim.checksum());
    }
    checksums
}

/// The load-bearing property: record a run, round-trip it through the byte artifact, play it back,
/// and confirm the playback checksum stream is **bit-identical** to the record run. Returns the
/// (record_stream, playback_stream, replay) so callers can print the proof.
pub fn round_trip(
    scenario: Scenario,
    seed: u64,
    ticks: u64,
) -> Result<(Vec<u64>, Vec<u64>, Replay), ReplayError> {
    let (record_stream, replay) = record(scenario, seed, ticks);
    let bytes = replay.encode();
    let decoded = Replay::decode(&bytes)?;
    let playback_stream = playback(&decoded);
    Ok((record_stream, playback_stream, decoded))
}

/// Convenience boolean: does the recorded run replay bit-identically (through the artifact)?
pub fn round_trip_ok(scenario: Scenario, seed: u64, ticks: u64) -> bool {
    match round_trip(scenario, seed, ticks) {
        Ok((rec, play, _)) => rec == play,
        Err(_) => false,
    }
}

// ===========================================================================
// Multi-peer replay — the lockstep PvP form.
// ===========================================================================

/// A recorded **multi-peer** match: which scenario, the seed, the tick count, the session peer
/// count, and the per-tick, **per-peer** command log.
///
/// The log is `tick -> (peer -> that peer's commands for the tick)`. Both maps are `BTreeMap`s, so
/// the executed set for a tick — [`merged_for`](Self::merged_for) — is every peer's commands
/// concatenated in **ascending peer order**, deterministically, *regardless of the order the
/// per-peer sets were recorded in*. That is exactly the merge rule
/// [`Lockstep::try_advance`](gonedark_core::lockstep::Lockstep) uses (Vec index = fixed peer order
/// = the stable application order `Sim::step` relies on), so a recorded PvP match replays to the
/// same world the live session produced.
#[derive(Clone, Debug)]
pub struct MultiReplay {
    pub scenario: Scenario,
    pub seed: u64,
    /// Total ticks the record run advanced (tick 0 is the seeded state; steps run for `1..ticks`).
    pub ticks: u64,
    /// The number of peers in the recorded session. A peer id `>= peer_count` in a decoded
    /// artifact is rejected ([`ReplayError::PeerOutOfRange`]).
    peer_count: u32,
    /// The per-tick, per-peer input log. A tick with no commands from any peer is omitted, and a
    /// peer contributing nothing to a stored tick simply has no entry (its merged contribution is
    /// empty either way).
    log: BTreeMap<u64, BTreeMap<PeerId, Vec<Command>>>,
}

impl MultiReplay {
    /// The number of peers in the recorded session.
    pub fn peer_count(&self) -> u32 {
        self.peer_count
    }

    /// The commands recorded for `(tick, peer)` (empty if that peer was idle on that tick).
    pub fn commands_for_peer(&self, tick: u64, peer: PeerId) -> &[Command] {
        self.log
            .get(&tick)
            .and_then(|per_peer| per_peer.get(&peer))
            .map(Vec::as_slice)
            .unwrap_or(&[])
    }

    /// The **merged, deterministically-ordered** command set executed on `tick`: every peer's
    /// commands concatenated in ascending peer order. This is the multi-peer ordering rule — and
    /// it is byte-for-byte what `core::lockstep` produces for the same per-peer sets. Because it
    /// iterates a `BTreeMap` by key, the result is independent of the order the peers' sets were
    /// recorded/inserted in.
    pub fn merged_for(&self, tick: u64) -> Vec<Command> {
        let mut merged = Vec::new();
        if let Some(per_peer) = self.log.get(&tick) {
            for cmds in per_peer.values() {
                merged.extend_from_slice(cmds);
            }
        }
        merged
    }

    /// Total number of commands across every tick and peer (a quick "is this meaningful?" size).
    pub fn command_count(&self) -> usize {
        self.log
            .values()
            .flat_map(BTreeMap::values)
            .map(Vec::len)
            .sum()
    }

    /// Flatten the log into the per-`(tick, peer)` command sets, in `(tick, peer)` ascending order.
    /// This is the "peer command feed" — the arrivals a live session would have exchanged. Feeding
    /// these back through [`from_arrivals`](Self::from_arrivals) in *any* order reconstructs the
    /// identical replay (the arrival-order-independence property).
    pub fn arrivals(&self) -> Vec<(u64, PeerId, Vec<Command>)> {
        let mut out = Vec::with_capacity(self.log.values().map(BTreeMap::len).sum());
        for (&tick, per_peer) in &self.log {
            for (&peer, cmds) in per_peer {
                out.push((tick, peer, cmds.clone()));
            }
        }
        out
    }

    /// Assemble a [`MultiReplay`] from per-`(tick, peer)` command sets delivered in **any order**
    /// (as they would arrive from the wire). The first non-empty set seen for a given `(tick,
    /// peer)` wins — mirroring `core::lockstep`'s "first set seen for a `(tick, peer)` wins"
    /// (re-sends are identical), so a duplicated arrival is harmless. Empty sets are dropped. The
    /// resulting log — and thus every merged tick and the encoded bytes — is identical no matter
    /// what order the arrivals came in, because it is keyed by peer id, not insertion order.
    pub fn from_arrivals<I>(
        scenario: Scenario,
        seed: u64,
        ticks: u64,
        peer_count: u32,
        arrivals: I,
    ) -> MultiReplay
    where
        I: IntoIterator<Item = (u64, PeerId, Vec<Command>)>,
    {
        let mut log: BTreeMap<u64, BTreeMap<PeerId, Vec<Command>>> = BTreeMap::new();
        for (tick, peer, cmds) in arrivals {
            if cmds.is_empty() {
                continue;
            }
            log.entry(tick).or_default().entry(peer).or_insert(cmds);
        }
        MultiReplay {
            scenario,
            seed,
            ticks,
            peer_count,
            log,
        }
    }

    /// Serialize to the deterministic byte artifact:
    /// `MAGIC_MULTI | version:u16 | scenario_tag:u8 | peer_count:u32 | seed:u64 | ticks:u64 |
    /// entries:u32` then, per non-empty tick: `tick:u64 | n_peers:u32` then, per peer (ascending):
    /// `peer:u32 | n_commands:u32 | commands…`.
    pub fn encode(&self) -> Vec<u8> {
        let mut w = Vec::new();
        w.extend_from_slice(&MAGIC_MULTI);
        codec::put_u16(&mut w, FORMAT_VERSION);
        codec::put_u8(&mut w, self.scenario.tag());
        codec::put_u32(&mut w, self.peer_count);
        codec::put_u64(&mut w, self.seed);
        codec::put_u64(&mut w, self.ticks);
        codec::put_u32(&mut w, self.log.len() as u32);
        for (&tick, per_peer) in &self.log {
            codec::put_u64(&mut w, tick);
            codec::put_u32(&mut w, per_peer.len() as u32);
            for (&peer, cmds) in per_peer {
                codec::put_u32(&mut w, peer);
                codec::put_u32(&mut w, cmds.len() as u32);
                for c in cmds {
                    codec::put_command(&mut w, c);
                }
            }
        }
        w
    }

    /// Parse a byte artifact produced by [`MultiReplay::encode`]. Errors (never panics) on a
    /// corrupt/version-skewed stream, a wrong magic (e.g. a single-peer artifact), or a peer id
    /// outside the recorded session.
    pub fn decode(bytes: &[u8]) -> Result<MultiReplay, ReplayError> {
        let mut r = codec::Reader::new(bytes);
        if r.magic()? != MAGIC_MULTI {
            return Err(ReplayError::BadMagic);
        }
        let version = r.u16()?;
        if version != FORMAT_VERSION {
            return Err(ReplayError::BadVersion(version));
        }
        let tag = r.u8()?;
        let scenario =
            Scenario::from_tag(tag).ok_or(ReplayError::BadTag { what: "scenario", tag })?;
        let peer_count = r.u32()?;
        let seed = r.u64()?;
        let ticks = r.u64()?;
        let entries = r.u32()?;
        let mut log: BTreeMap<u64, BTreeMap<PeerId, Vec<Command>>> = BTreeMap::new();
        for _ in 0..entries {
            let tick = r.u64()?;
            let n_peers = r.u32()?;
            let mut per_peer: BTreeMap<PeerId, Vec<Command>> = BTreeMap::new();
            for _ in 0..n_peers {
                let peer = r.u32()?;
                if peer >= peer_count {
                    return Err(ReplayError::PeerOutOfRange(peer));
                }
                let n = r.u32()?;
                let mut cmds = Vec::with_capacity(n as usize);
                for _ in 0..n {
                    cmds.push(r.command()?);
                }
                per_peer.insert(peer, cmds);
            }
            log.insert(tick, per_peer);
        }
        if !r.at_end() {
            return Err(ReplayError::TrailingBytes);
        }
        Ok(MultiReplay {
            scenario,
            seed,
            ticks,
            peer_count,
            log,
        })
    }
}

/// Drive the multi-peer form of `scenario` (seeded from `seed`) for `ticks` ticks, capturing the
/// per-peer command log into a [`MultiReplay`] and returning `(per_tick_checksums, replay)`.
///
/// The sim is stepped with the **merged** set for each tick ([`MultiReplay::merged_for`]) — the
/// exact same merge path playback uses — so the record and playback streams are identical by
/// construction, and both match what a live lockstep session stepping the same per-peer sets
/// would produce.
pub fn record_multi(scenario: Scenario, seed: u64, ticks: u64) -> (Vec<u64>, MultiReplay) {
    let scenario::BuiltMulti {
        mut sim,
        peer_count,
        scripted,
    } = scenario::build_multi(scenario, seed);

    // Capture the per-peer log first (skipping empty per-peer sets), then step from it — so record
    // drives the sim through the identical `merged_for` path playback will.
    let mut log: BTreeMap<u64, BTreeMap<PeerId, Vec<Command>>> = BTreeMap::new();
    for (&tick, per_peer) in &scripted {
        for (&peer, cmds) in per_peer {
            if !cmds.is_empty() {
                log.entry(tick).or_default().insert(peer, cmds.clone());
            }
        }
    }
    let replay = MultiReplay {
        scenario,
        seed,
        ticks,
        peer_count,
        log,
    };

    let mut checksums = Vec::with_capacity(ticks as usize);
    checksums.push(sim.checksum()); // tick 0: seeded state
    for t in 1..ticks {
        sim.step(&replay.merged_for(t));
        checksums.push(sim.checksum());
    }
    (checksums, replay)
}

/// Re-run a [`MultiReplay`]: re-seed its scenario and feed back the **merged, peer-ordered**
/// command set tick-by-tick, returning the per-tick checksum stream. Drives ONLY the recorded log
/// (never the builder's own script), so a match equal to [`record_multi`]'s stream proves the
/// per-peer log alone reproduces the world.
pub fn playback_multi(replay: &MultiReplay) -> Vec<u64> {
    let built = scenario::build_multi(replay.scenario, replay.seed);
    let mut sim: Sim = built.sim;
    let mut checksums = Vec::with_capacity(replay.ticks as usize);
    checksums.push(sim.checksum()); // tick 0
    for t in 1..replay.ticks {
        sim.step(&replay.merged_for(t));
        checksums.push(sim.checksum());
    }
    checksums
}

/// The multi-peer load-bearing property: record a run, round-trip it through the byte artifact,
/// play it back, and confirm the playback checksum stream is **bit-identical** to the record run.
/// Returns `(record_stream, playback_stream, replay)` so callers can print the proof.
pub fn round_trip_multi(
    scenario: Scenario,
    seed: u64,
    ticks: u64,
) -> Result<(Vec<u64>, Vec<u64>, MultiReplay), ReplayError> {
    let (record_stream, replay) = record_multi(scenario, seed, ticks);
    let bytes = replay.encode();
    let decoded = MultiReplay::decode(&bytes)?;
    let playback_stream = playback_multi(&decoded);
    Ok((record_stream, playback_stream, decoded))
}

/// Convenience boolean: does the recorded multi-peer run replay bit-identically (through bytes)?
pub fn round_trip_multi_ok(scenario: Scenario, seed: u64, ticks: u64) -> bool {
    match round_trip_multi(scenario, seed, ticks) {
        Ok((rec, play, _)) => rec == play,
        Err(_) => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gonedark_core::ecs::Entity;

    const SEED: u64 = 0x60ED_DA47; // "goned-da47" — the replay-runner's canonical seed.

    #[test]
    fn scenario_tag_and_token_roundtrip() {
        for s in [Scenario::SkirmishScript] {
            assert_eq!(Scenario::from_tag(s.tag()), Some(s));
            assert_eq!(Scenario::parse(s.token()), Some(s));
        }
        assert_eq!(Scenario::from_tag(0xFF), None);
        assert_eq!(Scenario::parse("nope"), None);
    }

    #[test]
    fn record_captures_a_meaningful_log() {
        // The scripted skirmish must produce a non-trivial input log — not an idle sim — so the
        // replay actually exercises Move/production/Fire/embodiment commands.
        let (_stream, replay) = record(Scenario::SkirmishScript, SEED, 300);
        assert!(
            replay.command_count() >= 20,
            "expected a rich command log, got {}",
            replay.command_count()
        );
        assert_eq!(replay.seed, SEED);
        assert_eq!(replay.ticks, 300);
    }

    #[test]
    fn record_run_advances_the_sim() {
        // Sanity: the checksum evolves as the sim does work (not frozen), and there's one checksum
        // per emitted tick.
        let (stream, _replay) = record(Scenario::SkirmishScript, SEED, 300);
        assert_eq!(stream.len(), 300);
        assert_ne!(
            stream.first(),
            stream.last(),
            "checksum should change as the sim advances"
        );
    }

    #[test]
    fn artifact_roundtrips_through_bytes() {
        // encode → decode is lossless: the decoded replay produces the identical playback stream,
        // and re-encoding yields identical bytes.
        let (_rec, replay) = record(Scenario::SkirmishScript, SEED, 300);
        let bytes = replay.encode();
        let decoded = Replay::decode(&bytes).expect("decode");
        assert_eq!(decoded.scenario, replay.scenario);
        assert_eq!(decoded.seed, replay.seed);
        assert_eq!(decoded.ticks, replay.ticks);
        assert_eq!(decoded.command_count(), replay.command_count());
        assert_eq!(decoded.encode(), bytes, "re-encode must be byte-stable");
    }

    #[test]
    fn playback_matches_record_bit_identical() {
        // THE PROOF (invariant #1 / determinism freebie): same seed + same inputs ⇒ same world,
        // tick-for-tick, through the serialized artifact.
        let (rec, play, _replay) =
            round_trip(Scenario::SkirmishScript, SEED, 300).expect("round-trip");
        assert_eq!(rec.len(), play.len());
        assert_eq!(rec, play, "playback checksum stream diverged from record");
        assert_eq!(stream_digest(&rec), stream_digest(&play));
    }

    #[test]
    fn round_trip_ok_is_true_across_tick_counts() {
        for ticks in [1u64, 45, 60, 150, 300] {
            assert!(
                round_trip_ok(Scenario::SkirmishScript, SEED, ticks),
                "round-trip failed at {ticks} ticks"
            );
        }
    }

    #[test]
    fn record_is_reproducible() {
        // Two independent record runs of the same (scenario, seed) yield the identical stream and
        // the identical artifact bytes — the deterministic property the whole thing rests on.
        let (s1, r1) = record(Scenario::SkirmishScript, SEED, 300);
        let (s2, r2) = record(Scenario::SkirmishScript, SEED, 300);
        assert_eq!(s1, s2);
        assert_eq!(r1.encode(), r2.encode());
    }

    #[test]
    fn different_seed_is_a_different_replay() {
        // Guards against the seed being ignored: a different seed must (very likely) diverge.
        let (a, _) = record(Scenario::SkirmishScript, SEED, 200);
        let (b, _) = record(Scenario::SkirmishScript, SEED ^ 0x1234_5678, 200);
        assert_ne!(a, b, "distinct seeds should produce distinct streams");
    }

    #[test]
    fn decode_rejects_garbage() {
        assert_eq!(Replay::decode(b"not a replay").unwrap_err(), ReplayError::BadMagic);
        // Right magic, wrong version.
        let mut bad = MAGIC.to_vec();
        bad.extend_from_slice(&999u16.to_le_bytes());
        assert_eq!(Replay::decode(&bad).unwrap_err(), ReplayError::BadVersion(999));
        // Truncated body (valid header start, then EOF).
        let (_s, replay) = record(Scenario::SkirmishScript, SEED, 50);
        let bytes = replay.encode();
        assert_eq!(
            Replay::decode(&bytes[..bytes.len() - 1]).unwrap_err(),
            ReplayError::UnexpectedEof
        );
        // Trailing junk.
        let mut extra = bytes.clone();
        extra.push(0xAB);
        assert_eq!(Replay::decode(&extra).unwrap_err(), ReplayError::TrailingBytes);
    }

    // ===================================================================
    // Multi-peer replay ordering.
    // ===================================================================

    fn ent(index: u32, generation: u32) -> Entity {
        Entity { index, generation }
    }

    #[test]
    fn multi_record_captures_a_meaningful_two_peer_log() {
        // The scripted multi-peer skirmish must produce a non-trivial log from BOTH commanders —
        // so the replay actually exercises the cross-peer ordering, not a one-sided stream.
        let (_stream, replay) = record_multi(Scenario::SkirmishScript, SEED, 300);
        assert_eq!(replay.peer_count(), scenario::SKIRMISH_PEER_COUNT);
        assert!(
            replay.command_count() >= 20,
            "expected a rich per-peer log, got {}",
            replay.command_count()
        );
        // Both peers must actually contribute (peer 1 issues the enemy patrol at tick 1).
        assert!(
            !replay.commands_for_peer(1, 0).is_empty(),
            "peer 0 (Player) must issue commands at tick 1"
        );
        assert!(
            !replay.commands_for_peer(1, 1).is_empty(),
            "peer 1 (Enemy) must issue commands at tick 1"
        );
        assert_eq!(replay.seed, SEED);
        assert_eq!(replay.ticks, 300);
    }

    #[test]
    fn multi_playback_matches_record_bit_identical() {
        // THE MULTI-PEER PROOF: same seed + same per-peer log ⇒ same world, tick-for-tick, through
        // the serialized artifact (invariants #1/#7).
        let (rec, play, _replay) =
            round_trip_multi(Scenario::SkirmishScript, SEED, 300).expect("round-trip");
        assert_eq!(rec.len(), play.len());
        assert_eq!(rec, play, "multi-peer playback stream diverged from record");
        assert_eq!(stream_digest(&rec), stream_digest(&play));
    }

    #[test]
    fn multi_arrival_order_does_not_change_the_replay() {
        // THE HEADLINE PROPERTY: two peers' command logs, recorded/delivered in DIFFERENT arrival
        // orders, must replay to the IDENTICAL per-tick checksum stream. We take the canonical
        // record's per-(tick,peer) arrivals, rebuild the replay from them in forward order and in
        // fully-reversed order, and assert everything downstream is byte-for-byte identical.
        let (rec_stream, replay) = record_multi(Scenario::SkirmishScript, SEED, 300);
        let peers = replay.peer_count();

        let forward = replay.arrivals();
        let mut backward = forward.clone();
        backward.reverse();
        // Sanity: the two arrival orders really are different (peer 1's tick-1 set now precedes
        // peer 0's), so the test isn't vacuous.
        assert_ne!(
            forward.iter().map(|(t, p, _)| (*t, *p)).collect::<Vec<_>>(),
            backward.iter().map(|(t, p, _)| (*t, *p)).collect::<Vec<_>>(),
            "the reversed arrivals must differ in order"
        );

        let fa = MultiReplay::from_arrivals(Scenario::SkirmishScript, SEED, 300, peers, forward);
        let ba = MultiReplay::from_arrivals(Scenario::SkirmishScript, SEED, 300, peers, backward);

        // The reconstructed artifact is byte-identical regardless of arrival order — the log is
        // keyed by peer id, not insertion order.
        assert_eq!(
            fa.encode(),
            ba.encode(),
            "arrival order must not change the encoded replay"
        );
        // ...and it matches the original record's own artifact.
        assert_eq!(fa.encode(), replay.encode());

        // The load-bearing bit: both replay to the identical checksum stream, equal to the record.
        let pf = playback_multi(&fa);
        let pb = playback_multi(&ba);
        assert_eq!(pf, pb, "differently-ordered peer logs must replay identically");
        assert_eq!(
            pf, rec_stream,
            "arrival-order-independent playback must reproduce the record run bit-identically"
        );
    }

    #[test]
    fn multi_merge_is_ascending_peer_order_like_lockstep() {
        // Focused proof of the ordering RULE: even when peer 1's set arrives before peer 0's, the
        // merged tick must be peer-0's commands THEN peer-1's — exactly `core::lockstep`'s fixed
        // peer order. Command has no PartialEq, so we compare the encoded command bytes.
        let a = Command::Embody { entity: ent(1, 0) }; // peer 0
        let b = Command::Surface { entity: ent(2, 0) }; // peer 1

        // Deliver peer 1 FIRST, then peer 0 — the "wrong" arrival order.
        let replay = MultiReplay::from_arrivals(
            Scenario::SkirmishScript,
            SEED,
            5,
            2,
            [(1u64, 1 as PeerId, vec![b.clone()]), (1u64, 0 as PeerId, vec![a.clone()])],
        );
        let merged = replay.merged_for(1);
        let mut got = Vec::new();
        for c in &merged {
            codec::put_command(&mut got, c);
        }
        // The rule says peer 0 (Embody) first, then peer 1 (Surface).
        let mut want = Vec::new();
        codec::put_command(&mut want, &a);
        codec::put_command(&mut want, &b);
        assert_eq!(got, want, "merge must be ascending peer order (lockstep's rule)");
    }

    #[test]
    fn multi_artifact_roundtrips_through_bytes() {
        let (_rec, replay) = record_multi(Scenario::SkirmishScript, SEED, 300);
        let bytes = replay.encode();
        let decoded = MultiReplay::decode(&bytes).expect("decode");
        assert_eq!(decoded.scenario, replay.scenario);
        assert_eq!(decoded.seed, replay.seed);
        assert_eq!(decoded.ticks, replay.ticks);
        assert_eq!(decoded.peer_count(), replay.peer_count());
        assert_eq!(decoded.command_count(), replay.command_count());
        assert_eq!(decoded.encode(), bytes, "re-encode must be byte-stable");
    }

    #[test]
    fn multi_round_trip_ok_across_tick_counts() {
        for ticks in [1u64, 45, 60, 150, 300] {
            assert!(
                round_trip_multi_ok(Scenario::SkirmishScript, SEED, ticks),
                "multi-peer round-trip failed at {ticks} ticks"
            );
        }
    }

    #[test]
    fn multi_record_is_reproducible() {
        let (s1, r1) = record_multi(Scenario::SkirmishScript, SEED, 300);
        let (s2, r2) = record_multi(Scenario::SkirmishScript, SEED, 300);
        assert_eq!(s1, s2);
        assert_eq!(r1.encode(), r2.encode());
    }

    #[test]
    fn multi_decode_rejects_garbage() {
        // Wrong magic.
        assert_eq!(
            MultiReplay::decode(b"not a replay").unwrap_err(),
            ReplayError::BadMagic
        );
        // A single-peer artifact must NOT decode as multi-peer (distinct magic), and vice-versa.
        let (_s, single) = record(Scenario::SkirmishScript, SEED, 50);
        assert_eq!(
            MultiReplay::decode(&single.encode()).unwrap_err(),
            ReplayError::BadMagic,
            "a single-peer artifact must not misparse as multi-peer"
        );
        let (_m, multi) = record_multi(Scenario::SkirmishScript, SEED, 50);
        assert_eq!(
            Replay::decode(&multi.encode()).unwrap_err(),
            ReplayError::BadMagic,
            "a multi-peer artifact must not misparse as single-peer"
        );

        // Right magic, wrong version.
        let mut bad = MAGIC_MULTI.to_vec();
        bad.extend_from_slice(&999u16.to_le_bytes());
        assert_eq!(
            MultiReplay::decode(&bad).unwrap_err(),
            ReplayError::BadVersion(999)
        );

        // A peer id outside the recorded session is rejected loudly (never silently mis-merged).
        let oob = MultiReplay::from_arrivals(
            Scenario::SkirmishScript,
            SEED,
            10,
            1, // one-peer session …
            [(1u64, 5 as PeerId, vec![Command::Reload { entity: ent(1, 0) }])], // … but peer 5 appears
        );
        assert_eq!(
            MultiReplay::decode(&oob.encode()).unwrap_err(),
            ReplayError::PeerOutOfRange(5)
        );

        // Truncated body.
        let bytes = multi.encode();
        assert_eq!(
            MultiReplay::decode(&bytes[..bytes.len() - 1]).unwrap_err(),
            ReplayError::UnexpectedEof
        );
        // Trailing junk.
        let mut extra = bytes.clone();
        extra.push(0xAB);
        assert_eq!(
            MultiReplay::decode(&extra).unwrap_err(),
            ReplayError::TrailingBytes
        );
    }

    #[test]
    fn multi_different_seed_is_a_different_replay() {
        let (a, _) = record_multi(Scenario::SkirmishScript, SEED, 200);
        let (b, _) = record_multi(Scenario::SkirmishScript, SEED ^ 0x1234_5678, 200);
        assert_ne!(a, b, "distinct seeds should produce distinct multi-peer streams");
    }
}
