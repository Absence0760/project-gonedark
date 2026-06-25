//! Deterministic lockstep — the per-tick command exchange (Phase 3 workstream B, D27).
//!
//! Clients exchange **orders** ([`Command`]), not world state: because the
//! sim is deterministic, applying the identical per-tick command set on every peer keeps every
//! peer's world bit-identical (invariants #1, #7). This module owns the *protocol* — the
//! input-delay buffer, the per-tick command-set assembly, the gate/stall, and the wire codec —
//! and nothing else.
//!
//! **Sans-I/O.** [`Lockstep`] does **no networking**: it produces opaque byte frames to send
//! ([`Lockstep::drain_outbound`]) and consumes received ones ([`Lockstep::deliver`]); the host
//! (a `pal::Transport` impl in `pal-desktop`/`server`, D27) moves the bytes. So `core` never
//! names a socket — its dependency list stays empty (invariant #2) — and the whole protocol is
//! testable in-process against a simulated lossy channel, with no sockets (see the tests).
//!
//! **Input delay.** Input sampled while the loop is at tick `T` is stamped to execute at
//! `T + delay`, giving peers time to receive it before that tick runs. The command set executed
//! at tick `T` is the **merge of every peer's commands for `T`, concatenated in fixed peer
//! order** — which preserves the stable application order [`Sim::step`](crate::sim::Sim::step)
//! already relies on. An empty command set is sent explicitly, so quiet ticks don't stall the
//! gate; ticks `[0, delay)` are warmup (no input can exist yet) and execute empty.
//!
//! Determinism: the wire codec mirrors the little-endian, fixed-point (`Fixed::to_bits`)
//! discipline of [`checksum`](crate::checksum), so the bytes — and thus the reassembled command
//! set — are bit-identical on every arch. No floats, no hash iteration (slots/retained are
//! `BTreeMap`s, iterated in key order).

use std::collections::BTreeMap;

use crate::components::{BuildingKind, Faction, Order, Stance, UnitKind, Vec2};
use crate::ecs::Entity;
use crate::fixed::Fixed;
use crate::sim::Command;

/// Wire format version. Bumped on any codec change so a mismatched build is rejected, not
/// silently misparsed.
const WIRE_VERSION: u8 = 1;

/// A peer's index in the session, `0..peer_count`. Doubles as the fixed merge order.
pub type PeerId = u32;

/// Why a received frame could not be decoded. Decoding never panics — a malformed frame from the
/// wire is an error to handle, not a crash.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DecodeError {
    /// The buffer ended mid-field.
    UnexpectedEof,
    /// The frame's wire-format version byte did not match the expected version.
    BadVersion(u8),
    /// An enum tag byte did not match any known variant.
    BadTag(u8),
    /// A frame named a peer id outside the session.
    PeerOutOfRange(PeerId),
    /// The frame parsed but left unconsumed trailing bytes — a sign of codec/version skew
    /// between peers. Rejecting it makes that skew loud here instead of a silent later desync.
    TrailingBytes,
}

// ===========================================================================
// Byte writer / reader — little-endian, mirroring core::checksum's discipline.
// ===========================================================================

struct Writer {
    buf: Vec<u8>,
}

impl Writer {
    fn new() -> Self {
        Writer { buf: Vec::new() }
    }
    fn u8(&mut self, v: u8) {
        self.buf.push(v);
    }
    fn u32(&mut self, v: u32) {
        self.buf.extend_from_slice(&v.to_le_bytes());
    }
    fn u64(&mut self, v: u64) {
        self.buf.extend_from_slice(&v.to_le_bytes());
    }
    fn i32(&mut self, v: i32) {
        self.buf.extend_from_slice(&v.to_le_bytes());
    }
}

struct Reader<'a> {
    buf: &'a [u8],
    pos: usize,
}

impl<'a> Reader<'a> {
    fn new(buf: &'a [u8]) -> Self {
        Reader { buf, pos: 0 }
    }
    fn take(&mut self, n: usize) -> Result<&'a [u8], DecodeError> {
        let end = self.pos.checked_add(n).ok_or(DecodeError::UnexpectedEof)?;
        if end > self.buf.len() {
            return Err(DecodeError::UnexpectedEof);
        }
        let s = &self.buf[self.pos..end];
        self.pos = end;
        Ok(s)
    }
    fn u8(&mut self) -> Result<u8, DecodeError> {
        Ok(self.take(1)?[0])
    }
    fn u32(&mut self) -> Result<u32, DecodeError> {
        let b = self.take(4)?;
        Ok(u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
    }
    fn u64(&mut self) -> Result<u64, DecodeError> {
        let b = self.take(8)?;
        Ok(u64::from_le_bytes([
            b[0], b[1], b[2], b[3], b[4], b[5], b[6], b[7],
        ]))
    }
    fn i32(&mut self) -> Result<i32, DecodeError> {
        let b = self.take(4)?;
        Ok(i32::from_le_bytes([b[0], b[1], b[2], b[3]]))
    }
}

// ===========================================================================
// Command codec — tag byte + payload, mirroring the checksum's tag schemes.
// ===========================================================================

fn put_entity(w: &mut Writer, e: Entity) {
    w.u32(e.index);
    w.u32(e.generation);
}
fn get_entity(r: &mut Reader) -> Result<Entity, DecodeError> {
    Ok(Entity {
        index: r.u32()?,
        generation: r.u32()?,
    })
}

fn put_vec2(w: &mut Writer, v: Vec2) {
    w.i32(v.x.to_bits());
    w.i32(v.y.to_bits());
}
fn get_vec2(r: &mut Reader) -> Result<Vec2, DecodeError> {
    Ok(Vec2::new(
        Fixed::from_bits(r.i32()?),
        Fixed::from_bits(r.i32()?),
    ))
}

fn put_order(w: &mut Writer, o: Order) {
    match o {
        Order::Idle => w.u8(0),
        Order::MoveTo(t) => {
            w.u8(1);
            put_vec2(w, t);
        }
        Order::AttackMove(t) => {
            w.u8(2);
            put_vec2(w, t);
        }
        Order::Patrol { a, b, toward_b } => {
            w.u8(3);
            put_vec2(w, a);
            put_vec2(w, b);
            w.u8(toward_b as u8);
        }
        Order::HoldPosition => w.u8(4),
        Order::FallBack(t) => {
            w.u8(5);
            put_vec2(w, t);
        }
    }
}
fn get_order(r: &mut Reader) -> Result<Order, DecodeError> {
    Ok(match r.u8()? {
        0 => Order::Idle,
        1 => Order::MoveTo(get_vec2(r)?),
        2 => Order::AttackMove(get_vec2(r)?),
        3 => Order::Patrol {
            a: get_vec2(r)?,
            b: get_vec2(r)?,
            toward_b: r.u8()? != 0,
        },
        4 => Order::HoldPosition,
        5 => Order::FallBack(get_vec2(r)?),
        t => return Err(DecodeError::BadTag(t)),
    })
}

fn put_stance(w: &mut Writer, s: Stance) {
    w.u8(match s {
        Stance::HoldFire => 0,
        Stance::ReturnFire => 1,
        Stance::FireAtWill => 2,
    });
}
fn get_stance(r: &mut Reader) -> Result<Stance, DecodeError> {
    Ok(match r.u8()? {
        0 => Stance::HoldFire,
        1 => Stance::ReturnFire,
        2 => Stance::FireAtWill,
        t => return Err(DecodeError::BadTag(t)),
    })
}

fn put_faction(w: &mut Writer, f: Faction) {
    w.u8(match f {
        Faction::Player => 0,
        Faction::Enemy => 1,
        Faction::Neutral => 2,
    });
}
fn get_faction(r: &mut Reader) -> Result<Faction, DecodeError> {
    Ok(match r.u8()? {
        0 => Faction::Player,
        1 => Faction::Enemy,
        2 => Faction::Neutral,
        t => return Err(DecodeError::BadTag(t)),
    })
}

fn put_building_kind(w: &mut Writer, k: BuildingKind) {
    w.u8(match k {
        BuildingKind::Camp => 0,
    });
}
fn get_building_kind(r: &mut Reader) -> Result<BuildingKind, DecodeError> {
    Ok(match r.u8()? {
        0 => BuildingKind::Camp,
        t => return Err(DecodeError::BadTag(t)),
    })
}

fn put_unit_kind(w: &mut Writer, k: UnitKind) {
    w.u8(match k {
        UnitKind::Rifleman => 0,
        UnitKind::Heavy => 1,
    });
}
fn get_unit_kind(r: &mut Reader) -> Result<UnitKind, DecodeError> {
    Ok(match r.u8()? {
        0 => UnitKind::Rifleman,
        1 => UnitKind::Heavy,
        t => return Err(DecodeError::BadTag(t)),
    })
}

fn put_command(w: &mut Writer, c: &Command) {
    match *c {
        Command::Move { entity, target } => {
            w.u8(0);
            put_entity(w, entity);
            put_vec2(w, target);
        }
        Command::AttackMove { entity, target } => {
            w.u8(1);
            put_entity(w, entity);
            put_vec2(w, target);
        }
        Command::SetOrder { entity, order } => {
            w.u8(2);
            put_entity(w, entity);
            put_order(w, order);
        }
        Command::SetStance { entity, stance } => {
            w.u8(3);
            put_entity(w, entity);
            put_stance(w, stance);
        }
        Command::SetRetreatThreshold { entity, fraction } => {
            w.u8(4);
            put_entity(w, entity);
            w.i32(fraction.to_bits());
        }
        Command::Embody { entity } => {
            w.u8(5);
            put_entity(w, entity);
        }
        Command::Surface { entity } => {
            w.u8(6);
            put_entity(w, entity);
        }
        Command::Build { faction, kind, pos } => {
            w.u8(7);
            put_faction(w, faction);
            put_building_kind(w, kind);
            put_vec2(w, pos);
        }
        Command::Upgrade { camp } => {
            w.u8(8);
            put_entity(w, camp);
        }
        Command::QueueProduction { camp, unit } => {
            w.u8(9);
            put_entity(w, camp);
            put_unit_kind(w, unit);
        }
    }
}

fn get_command(r: &mut Reader) -> Result<Command, DecodeError> {
    Ok(match r.u8()? {
        0 => Command::Move {
            entity: get_entity(r)?,
            target: get_vec2(r)?,
        },
        1 => Command::AttackMove {
            entity: get_entity(r)?,
            target: get_vec2(r)?,
        },
        2 => Command::SetOrder {
            entity: get_entity(r)?,
            order: get_order(r)?,
        },
        3 => Command::SetStance {
            entity: get_entity(r)?,
            stance: get_stance(r)?,
        },
        4 => Command::SetRetreatThreshold {
            entity: get_entity(r)?,
            fraction: Fixed::from_bits(r.i32()?),
        },
        5 => Command::Embody {
            entity: get_entity(r)?,
        },
        6 => Command::Surface {
            entity: get_entity(r)?,
        },
        7 => Command::Build {
            faction: get_faction(r)?,
            kind: get_building_kind(r)?,
            pos: get_vec2(r)?,
        },
        8 => Command::Upgrade {
            camp: get_entity(r)?,
        },
        9 => Command::QueueProduction {
            camp: get_entity(r)?,
            unit: get_unit_kind(r)?,
        },
        t => return Err(DecodeError::BadTag(t)),
    })
}

/// Encode one peer's command set for one execution tick into a wire frame.
fn encode_frame(peer: PeerId, tick: u64, commands: &[Command]) -> Vec<u8> {
    let mut w = Writer::new();
    w.u8(WIRE_VERSION);
    w.u32(peer);
    w.u64(tick);
    w.u32(u32::try_from(commands.len()).expect("a tick's command set fits in u32"));
    for c in commands {
        put_command(&mut w, c);
    }
    w.buf
}

/// Decode a wire frame back into `(peer, tick, commands)`. Never panics on malformed input.
fn decode_frame(bytes: &[u8]) -> Result<(PeerId, u64, Vec<Command>), DecodeError> {
    let mut r = Reader::new(bytes);
    let ver = r.u8()?;
    if ver != WIRE_VERSION {
        return Err(DecodeError::BadVersion(ver));
    }
    let peer = r.u32()?;
    let tick = r.u64()?;
    let n = r.u32()? as usize;
    // Cap the pre-allocation so a garbage length can't request a huge Vec; the loop still reads
    // exactly `n` and fails with UnexpectedEof if the bytes run short.
    let mut commands = Vec::with_capacity(n.min(256));
    for _ in 0..n {
        commands.push(get_command(&mut r)?);
    }
    if r.pos != r.buf.len() {
        return Err(DecodeError::TrailingBytes);
    }
    Ok((peer, tick, commands))
}

// ===========================================================================
// The lockstep state machine.
// ===========================================================================

/// A single peer's view of a deterministic lockstep session: it buffers per-tick command sets
/// (its own, stamped at `submit_tick = delay + submits`, and peers' as they arrive), gates the
/// sim on having every peer's set for the next tick, and hands the host the merged set to apply.
///
/// Sans-I/O: feed it bytes with [`deliver`](Self::deliver), take bytes to send with
/// [`drain_outbound`](Self::drain_outbound); it never touches a socket.
pub struct Lockstep {
    peer_count: u32,
    local: PeerId,
    delay: u64,
    /// The next tick to execute (the gate target).
    next_tick: u64,
    /// How many local sets have been submitted; the next stamps at `delay + submitted`.
    submitted: u64,
    /// `tick -> [per-peer command set]`; a `None` slot is a peer we are still waiting on.
    slots: BTreeMap<u64, Vec<Option<Vec<Command>>>>,
    /// Our own encoded frames, kept for (re)transmission until they fall out of the active
    /// window. Re-sending every drain makes delivery loss-tolerant without ACKs — a deliberate
    /// first-slice simplification; a real ACK/retransmit + flow-control layer is a later slice
    /// (`docs/phase-3-plan.md` §"Workstream B").
    retained: BTreeMap<u64, Vec<u8>>,
}

impl Lockstep {
    /// A fresh session for `peer_count` peers, this client being `local`, with `delay` ticks of
    /// input delay.
    pub fn new(peer_count: u32, local: PeerId, delay: u64) -> Self {
        assert!(peer_count >= 1, "a session needs at least one peer");
        assert!(
            local < peer_count,
            "local peer id {local} >= peer_count {peer_count}"
        );
        Lockstep {
            peer_count,
            local,
            delay,
            next_tick: 0,
            submitted: 0,
            slots: BTreeMap::new(),
            retained: BTreeMap::new(),
        }
    }

    /// The next tick the sim will execute (None of it has run yet at `next_tick`).
    pub fn next_tick(&self) -> u64 {
        self.next_tick
    }

    /// The execution tick the next [`submit`](Self::submit) will stamp its input onto.
    pub fn submit_tick(&self) -> u64 {
        self.delay + self.submitted
    }

    /// Stamp this peer's local input for the next submit tick, record it locally, and retain it
    /// for sending. Call once per tick you intend to advance.
    pub fn submit(&mut self, commands: Vec<Command>) {
        let tick = self.submit_tick();
        self.submitted += 1;
        self.retained
            .insert(tick, encode_frame(self.local, tick, &commands));
        let slot = self
            .slots
            .entry(tick)
            .or_insert_with(|| vec![None; self.peer_count as usize]);
        slot[self.local as usize] = Some(commands);
    }

    /// Encoded frames to hand to the transport this pump. Re-sends every retained (not-yet-pruned)
    /// frame; the receiver ignores stale/duplicate ticks, so this is loss-tolerant without ACKs.
    pub fn drain_outbound(&mut self) -> Vec<Vec<u8>> {
        self.retained.values().cloned().collect()
    }

    /// Ingest a received frame. Ignores our own echo and frames for already-executed ticks; the
    /// first set seen for a `(tick, peer)` wins (re-sends are identical). Returns `Err` only if
    /// the bytes are malformed.
    pub fn deliver(&mut self, bytes: &[u8]) -> Result<(), DecodeError> {
        let (peer, tick, commands) = decode_frame(bytes)?;
        if peer >= self.peer_count {
            return Err(DecodeError::PeerOutOfRange(peer));
        }
        if peer == self.local || tick < self.next_tick {
            return Ok(()); // our own echo, or a tick we have already executed
        }
        let slot = self
            .slots
            .entry(tick)
            .or_insert_with(|| vec![None; self.peer_count as usize]);
        if slot[peer as usize].is_none() {
            slot[peer as usize] = Some(commands);
        }
        Ok(())
    }

    /// If every peer's command set for the next tick is present, merge them in fixed peer order,
    /// advance, and return the set for the host to apply to its `Sim`. Returns `None` if still
    /// waiting on a peer (stall). Warmup ticks `[0, delay)` advance immediately with an empty set.
    pub fn try_advance(&mut self) -> Option<Vec<Command>> {
        let t = self.next_tick;
        if t < self.delay {
            self.next_tick += 1;
            self.prune();
            return Some(Vec::new());
        }
        let ready = matches!(self.slots.get(&t), Some(slot) if slot.iter().all(Option::is_some));
        if !ready {
            return None;
        }
        let slot = self.slots.remove(&t).expect("ready implies present");
        let mut merged = Vec::new();
        for peer in slot {
            // Vec index order == fixed peer order == the stable application order.
            merged.extend(peer.expect("ready implies every slot Some"));
        }
        self.next_tick += 1;
        self.prune();
        Some(merged)
    }

    /// Drop buffered state below the active window. The window keeps a `2*delay+1` tail below
    /// `next_tick` so a peer lagging within the lockstep bound can still be (re)served.
    fn prune(&mut self) {
        let lo = self
            .next_tick
            .saturating_sub(self.delay.saturating_mul(2).saturating_add(1));
        let stale: Vec<u64> = self.retained.range(..lo).map(|(k, _)| *k).collect();
        for k in stale {
            self.retained.remove(&k);
        }
        let stale: Vec<u64> = self.slots.range(..lo).map(|(k, _)| *k).collect();
        for k in stale {
            self.slots.remove(&k);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::components::EntityKind;
    use crate::economy::{self, Resources};
    use crate::rng::Rng;
    use crate::sim::Sim;
    use crate::territory::ControlPoint;

    fn v(x: i32, y: i32) -> Vec2 {
        Vec2::new(Fixed::from_int(x), Fixed::from_int(y))
    }
    fn ent(index: u32, generation: u32) -> Entity {
        Entity { index, generation }
    }

    // ----- wire codec -----

    #[test]
    fn frame_codec_roundtrips_all_variants() {
        let cmds = vec![
            Command::Move {
                entity: ent(1, 0),
                target: v(3, -4),
            },
            Command::AttackMove {
                entity: ent(2, 1),
                target: v(-7, 8),
            },
            Command::SetOrder {
                entity: ent(3, 2),
                order: Order::Patrol {
                    a: v(1, 2),
                    b: v(3, 4),
                    toward_b: false,
                },
            },
            Command::SetStance {
                entity: ent(4, 0),
                stance: Stance::HoldFire,
            },
            Command::SetRetreatThreshold {
                entity: ent(5, 9),
                fraction: Fixed::from_ratio(2, 5),
            },
            Command::Embody { entity: ent(6, 0) },
            Command::Surface { entity: ent(6, 0) },
            Command::Build {
                faction: Faction::Enemy,
                kind: BuildingKind::Camp,
                pos: v(-30, 30),
            },
            Command::Upgrade { camp: ent(7, 3) },
            Command::QueueProduction {
                camp: ent(7, 3),
                unit: UnitKind::Heavy,
            },
            // Cover the remaining Order variants too.
            Command::SetOrder {
                entity: ent(8, 0),
                order: Order::Idle,
            },
            Command::SetOrder {
                entity: ent(8, 0),
                order: Order::MoveTo(v(9, 9)),
            },
            Command::SetOrder {
                entity: ent(8, 0),
                order: Order::AttackMove(v(-9, -9)),
            },
            Command::SetOrder {
                entity: ent(8, 0),
                order: Order::HoldPosition,
            },
            Command::SetOrder {
                entity: ent(8, 0),
                order: Order::FallBack(v(5, 5)),
            },
        ];
        let bytes = encode_frame(0, 42, &cmds);
        let (peer, tick, decoded) = decode_frame(&bytes).expect("decode");
        assert_eq!(peer, 0);
        assert_eq!(tick, 42);
        assert_eq!(decoded.len(), cmds.len());
        // Command has no PartialEq; re-encoding the decoded set and comparing bytes is a stronger
        // codec check (it would catch a field silently dropped/reordered).
        assert_eq!(bytes, encode_frame(peer, tick, &decoded));
    }

    #[test]
    fn decode_rejects_malformed_frames() {
        assert_eq!(decode_frame(&[]).unwrap_err(), DecodeError::UnexpectedEof);

        let mut w = Writer::new();
        w.u8(99); // bad version
        assert_eq!(
            decode_frame(&w.buf).unwrap_err(),
            DecodeError::BadVersion(99)
        );

        let mut w = Writer::new();
        w.u8(WIRE_VERSION);
        w.u32(0);
        w.u64(0);
        w.u32(1);
        w.u8(250); // a command tag that does not exist
        assert_eq!(decode_frame(&w.buf).unwrap_err(), DecodeError::BadTag(250));

        let mut w = Writer::new();
        w.u8(WIRE_VERSION);
        w.u32(0);
        w.u64(0);
        w.u32(5); // claims 5 commands, provides none
        assert_eq!(
            decode_frame(&w.buf).unwrap_err(),
            DecodeError::UnexpectedEof
        );

        let mut w = Writer::new();
        w.u8(WIRE_VERSION);
        w.u32(0);
        w.u64(0);
        w.u32(0); // a valid empty frame …
        w.u8(0xFF); // … then an unexpected trailing byte (codec/version skew)
        assert_eq!(
            decode_frame(&w.buf).unwrap_err(),
            DecodeError::TrailingBytes
        );
    }

    // ----- gate / stall / merge -----

    #[test]
    fn warmup_ticks_advance_empty_without_frames() {
        let mut ls = Lockstep::new(2, 0, 3);
        for t in 0..3 {
            assert_eq!(
                ls.try_advance().as_deref().map(<[_]>::len),
                Some(0),
                "warmup tick {t} should advance with an empty set"
            );
        }
        // The first post-warmup tick needs frames it does not have → stall.
        assert!(ls.try_advance().is_none());
        assert_eq!(ls.next_tick(), 3);
    }

    #[test]
    fn gate_stalls_until_all_peers_present_then_merges_in_peer_order() {
        let mut ls = Lockstep::new(2, 0, 0); // no warmup: tick 0 needs frames
        ls.submit(vec![Command::Embody { entity: ent(1, 0) }]); // peer 0, tick 0
        assert!(ls.try_advance().is_none(), "peer 1 still missing");

        let peer1 = encode_frame(1, 0, &[Command::Surface { entity: ent(2, 0) }]);
        ls.deliver(&peer1).unwrap();

        let merged = ls.try_advance().expect("ready once both present");
        assert_eq!(merged.len(), 2);
        // Fixed peer order: peer 0's command first, then peer 1's.
        assert!(matches!(merged[0], Command::Embody { .. }), "peer 0 first");
        assert!(
            matches!(merged[1], Command::Surface { .. }),
            "peer 1 second"
        );
    }

    #[test]
    fn empty_set_from_every_peer_still_clears_the_gate() {
        let mut ls = Lockstep::new(2, 0, 0);
        ls.submit(Vec::new()); // peer 0 has nothing this tick
        assert!(ls.try_advance().is_none(), "peer 1 not heard from yet");
        ls.deliver(&encode_frame(1, 0, &[])).unwrap(); // peer 1: explicit empty
        assert_eq!(
            ls.try_advance().map(|m| m.len()),
            Some(0),
            "all-empty must still advance"
        );
    }

    #[test]
    fn deliver_rejects_out_of_range_peer() {
        let mut ls = Lockstep::new(2, 0, 0);
        let bad = encode_frame(5, 0, &[]); // peer 5 not in a 2-peer session
        assert_eq!(ls.deliver(&bad), Err(DecodeError::PeerOutOfRange(5)));
    }

    // ----- the headline test: two clients agree over a nasty channel -----

    const SCENE_SEED: u64 = 0x9E3779B97F4A7C15;

    struct Handles {
        p: [Entity; 3],
        e: [Entity; 3],
        camp: Entity,
    }

    fn spawn_rifleman(sim: &mut Sim, x: i32, y: i32, faction: Faction) -> Entity {
        let (health, weapon) = economy::unit_stats(UnitKind::Rifleman);
        let ent = sim.world.spawn();
        let i = ent.index as usize;
        sim.world.kind[i] = EntityKind::Unit;
        sim.world.faction[i] = faction;
        sim.world.pos[i] = v(x, y);
        sim.world.health[i] = health;
        sim.world.weapon[i] = weapon;
        sim.world.stance[i] = Stance::FireAtWill;
        ent
    }

    /// Spawn an identical deterministic scene and return its handles. Spawn order is fixed, so
    /// the handles are bit-identical across every sim built this way.
    fn scene(sim: &mut Sim) -> Handles {
        sim.resources = Resources::new(100_000);
        sim.territory.points.push(ControlPoint::neutral(Vec2::ZERO));
        let p = [
            spawn_rifleman(sim, -5, 0, Faction::Player),
            spawn_rifleman(sim, -5, 3, Faction::Player),
            spawn_rifleman(sim, -6, 1, Faction::Player),
        ];
        let e = [
            spawn_rifleman(sim, 5, 0, Faction::Enemy),
            spawn_rifleman(sim, 5, 3, Faction::Enemy),
            spawn_rifleman(sim, 6, 1, Faction::Enemy),
        ];
        let camp = economy::build(
            &mut sim.world,
            &mut sim.resources,
            Faction::Player,
            BuildingKind::Camp,
            v(-20, 20),
        )
        .expect("camp affordable");
        Handles { p, e, camp }
    }

    /// Peer 0 drives the player units; peer 1 the enemy. Exercises every Command variant across a
    /// couple of ticks; quiet otherwise.
    fn script(h: &Handles, peer: PeerId, t: u64, delay: u64) -> Vec<Command> {
        if t == delay {
            match peer {
                0 => vec![
                    Command::AttackMove {
                        entity: h.p[0],
                        target: v(5, 0),
                    },
                    Command::SetOrder {
                        entity: h.p[1],
                        order: Order::Patrol {
                            a: v(-5, 3),
                            b: v(-5, -8),
                            toward_b: true,
                        },
                    },
                    Command::SetStance {
                        entity: h.p[2],
                        stance: Stance::FireAtWill,
                    },
                    Command::SetRetreatThreshold {
                        entity: h.p[1],
                        fraction: Fixed::from_ratio(1, 3),
                    },
                    Command::Embody { entity: h.p[0] },
                    Command::Build {
                        faction: Faction::Player,
                        kind: BuildingKind::Camp,
                        pos: v(-22, 18),
                    },
                    Command::QueueProduction {
                        camp: h.camp,
                        unit: UnitKind::Rifleman,
                    },
                ],
                _ => vec![
                    Command::AttackMove {
                        entity: h.e[0],
                        target: v(-5, 0),
                    },
                    Command::SetStance {
                        entity: h.e[1],
                        stance: Stance::HoldFire,
                    },
                    Command::Move {
                        entity: h.e[2],
                        target: v(0, 5),
                    },
                ],
            }
        } else if t == delay + 25 {
            match peer {
                0 => vec![
                    Command::Surface { entity: h.p[0] },
                    Command::Upgrade { camp: h.camp },
                ],
                _ => vec![Command::SetOrder {
                    entity: h.e[0],
                    order: Order::FallBack(v(8, 8)),
                }],
            }
        } else {
            Vec::new()
        }
    }

    /// A deterministic in-process channel: a single seeded RNG drives per-frame loss and jitter,
    /// and the variable per-frame delay produces reordering. No sockets.
    struct Net {
        rng: Rng,
        base_delay: u64,
        jitter: u32,
        loss_num: u32,
        loss_den: u32,
        inflight: Vec<(u64, PeerId, Vec<u8>)>, // (deliver-at iteration, recipient, bytes)
    }

    impl Net {
        fn new(seed: u64, base_delay: u64, jitter: u32, loss_num: u32, loss_den: u32) -> Self {
            Net {
                rng: Rng::new(seed),
                base_delay,
                jitter,
                loss_num,
                loss_den,
                inflight: Vec::new(),
            }
        }
        fn send(&mut self, now: u64, to: PeerId, frames: Vec<Vec<u8>>) {
            for bytes in frames {
                if self.loss_den > 0 && self.rng.below(self.loss_den) < self.loss_num {
                    continue; // dropped this round (a later resend will carry it)
                }
                let jit = if self.jitter > 0 {
                    self.rng.below(self.jitter + 1) as u64
                } else {
                    0
                };
                self.inflight.push((now + self.base_delay + jit, to, bytes));
            }
        }
        fn deliver_due(&mut self, now: u64, sessions: &mut [Lockstep]) {
            let drained = std::mem::take(&mut self.inflight);
            for (due, to, bytes) in drained {
                if due <= now {
                    sessions[to as usize]
                        .deliver(&bytes)
                        .expect("well-formed frame");
                } else {
                    self.inflight.push((due, to, bytes));
                }
            }
        }
    }

    /// Run a 2-client lockstep session over the given channel and return each peer's per-tick
    /// checksum stream plus a no-network single-run reference stream.
    fn run_two_client(
        delay: u64,
        base_delay: u64,
        jitter: u32,
        loss_num: u32,
        loss_den: u32,
        net_seed: u64,
        target: u64,
    ) -> ([Vec<u64>; 2], Vec<u64>) {
        debug_assert!(target >= delay, "target must exceed the input delay");
        let mut sims = [Sim::new(SCENE_SEED), Sim::new(SCENE_SEED)];
        let h = scene(&mut sims[0]);
        let _ = scene(&mut sims[1]); // identical handles by determinism

        // Reference: one sim fed the merged command set every tick, no network.
        let mut refsim = Sim::new(SCENE_SEED);
        let _ = scene(&mut refsim);
        let mut refsums = Vec::with_capacity(target as usize);
        for t in 0..target {
            let mut merged = Vec::new();
            if t >= delay {
                merged.extend(script(&h, 0, t, delay));
                merged.extend(script(&h, 1, t, delay));
            }
            refsim.step(&merged);
            refsums.push(refsim.checksum());
        }

        let mut sessions = [Lockstep::new(2, 0, delay), Lockstep::new(2, 1, delay)];
        for k in 0..(target - delay) {
            let t = delay + k;
            sessions[0].submit(script(&h, 0, t, delay));
            sessions[1].submit(script(&h, 1, t, delay));
        }

        let mut net = Net::new(net_seed, base_delay, jitter, loss_num, loss_den);
        let mut sums = [
            Vec::with_capacity(target as usize),
            Vec::with_capacity(target as usize),
        ];
        let mut it = 0u64;
        loop {
            let f0 = sessions[0].drain_outbound();
            net.send(it, 1, f0);
            let f1 = sessions[1].drain_outbound();
            net.send(it, 0, f1);
            net.deliver_due(it, &mut sessions);

            for i in 0..2 {
                while let Some(cmds) = sessions[i].try_advance() {
                    sims[i].step(&cmds);
                    sums[i].push(sims[i].checksum());
                }
            }
            if sessions[0].next_tick() >= target && sessions[1].next_tick() >= target {
                break;
            }
            it += 1;
            assert!(
                it < 1_000_000,
                "lockstep failed to converge (delay={delay}, loss={loss_num}/{loss_den})"
            );
        }
        (sums, refsums)
    }

    #[test]
    fn lockstep_two_clients_agree_clean_channel() {
        let (sums, refsums) = run_two_client(3, 0, 0, 0, 1, 0xABCDEF, 120);
        assert_eq!(sums[0].len(), 120);
        assert_eq!(sums[0], sums[1], "the two peers must agree every tick");
        assert_eq!(
            sums[0], refsums,
            "lockstep must match the single-run reference"
        );
    }

    #[test]
    fn lockstep_two_clients_agree_under_jitter_reorder_loss() {
        // base delay 1, jitter 0..3 (→ reordering), 25% packet loss.
        let (sums, refsums) = run_two_client(4, 1, 3, 1, 4, 0x13579B, 120);
        assert_eq!(sums[0], sums[1], "peers diverged under a lossy channel");
        assert_eq!(
            sums[0], refsums,
            "lossy lockstep must still match the reference"
        );
    }

    #[test]
    fn lockstep_matches_reference_across_delays() {
        for delay in [1u64, 2, 5, 8] {
            let (sums, refsums) = run_two_client(delay, 0, 2, 1, 6, 0x55 + delay, 100);
            assert_eq!(sums[0], sums[1], "peers diverged at delay {delay}");
            assert_eq!(sums[0], refsums, "reference mismatch at delay {delay}");
        }
    }
}
