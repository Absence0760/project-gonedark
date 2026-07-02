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
//! Single-client scope: the log is one ordered command stream per tick (what a solo match or a
//! spectator of one already produces). Multi-peer replays would fold in `core::lockstep`'s
//! per-peer ordering; a spectator *view* (rendering a playback) is a render-side follow-up. Both
//! ride this same seed+log foundation.

mod codec;
pub mod scenario;

use std::collections::BTreeMap;

use gonedark_core::sim::{Command, Sim};

pub use codec::{ReplayError, FORMAT_VERSION, MAGIC};
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

#[cfg(test)]
mod tests {
    use super::*;

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
}
