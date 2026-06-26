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
/// silently misparsed. 2 added the frame-kind tag (command vs. checksum report); 3 added the
/// `DelayChange` frame (the agreed RTT-adaptive input-delay change); 4 added the
/// `Command::Locomote` vocabulary (tag 11) — a build without it would only choke on `BadTag(11)`
/// mid-session, so the bump fails the skew loudly at the connection handshake instead; 5 added
/// the embodied `Command::Reload` (tag 12) + `Command::Crouch` (tag 13) vocabulary; 6 added the
/// embodied tank `Command::AimTurret` (tag 14) + `Command::DriveHull` (tag 15) vocabulary.
const WIRE_VERSION: u8 = 6;

/// Frame-kind tag, the byte after the version. Picks which payload follows so the codec can
/// carry command sets, checksum reports, and delay-change proposals over the one wire format.
/// Kept loud (a bad tag is a [`DecodeError::BadTag`]) so codec/version skew is rejected, never
/// silently misparsed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum FrameKind {
    /// A peer's per-tick command set (the original frame; layout unchanged after the tag).
    Command = 0,
    /// A peer's post-tick sim checksum report for cross-client agreement verification.
    Checksum = 1,
    /// An agreed input-delay change, to take effect at a shipped execution tick (B7).
    DelayChange = 2,
}

impl FrameKind {
    fn from_u8(v: u8) -> Result<Self, DecodeError> {
        match v {
            0 => Ok(FrameKind::Command),
            1 => Ok(FrameKind::Checksum),
            2 => Ok(FrameKind::DelayChange),
            t => Err(DecodeError::BadTag(t)),
        }
    }
}

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
        Command::Fire { entity, dir } => {
            w.u8(10);
            put_entity(w, entity);
            put_vec2(w, dir);
        }
        Command::Locomote { entity, dir } => {
            w.u8(11);
            put_entity(w, entity);
            put_vec2(w, dir);
        }
        Command::Reload { entity } => {
            w.u8(12);
            put_entity(w, entity);
        }
        Command::Crouch { entity, crouched } => {
            w.u8(13);
            put_entity(w, entity);
            w.u8(crouched as u8);
        }
        Command::AimTurret { entity, dir } => {
            w.u8(14);
            put_entity(w, entity);
            put_vec2(w, dir);
        }
        Command::DriveHull { entity, dir } => {
            w.u8(15);
            put_entity(w, entity);
            put_vec2(w, dir);
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
        10 => Command::Fire {
            entity: get_entity(r)?,
            dir: get_vec2(r)?,
        },
        11 => Command::Locomote {
            entity: get_entity(r)?,
            dir: get_vec2(r)?,
        },
        12 => Command::Reload {
            entity: get_entity(r)?,
        },
        13 => Command::Crouch {
            entity: get_entity(r)?,
            crouched: r.u8()? != 0,
        },
        14 => Command::AimTurret {
            entity: get_entity(r)?,
            dir: get_vec2(r)?,
        },
        15 => Command::DriveHull {
            entity: get_entity(r)?,
            dir: get_vec2(r)?,
        },
        t => return Err(DecodeError::BadTag(t)),
    })
}

/// A decoded wire frame: a command set for a tick, a peer's checksum report for a tick, or an
/// agreed delay-change proposal. The codec is a tagged union (version, kind, then the kind's
/// payload). All three carry `peer` then `tick` right after the kind tag, so the shared decode
/// reads those before dispatching — for [`DelayChange`](Frame::DelayChange) the `tick` slot *is*
/// the effective tick.
#[derive(Clone, Debug)]
enum Frame {
    /// `(peer, tick, commands)` — one peer's command set for one execution tick.
    Command(PeerId, u64, Vec<Command>),
    /// `(peer, tick, checksum)` — one peer's post-tick sim checksum for that tick. The checksum
    /// rides the wire as raw `u64` LE bytes (no float) — pure verification, no sim effect.
    Checksum(PeerId, u64, u64),
    /// `(proposer, effective_tick, seq, new_delay)` — a proposal to switch the session input
    /// delay to `new_delay`, taking effect at `effective_tick` (shipped as data so every peer
    /// applies the identical change at the identical tick). `seq` is the proposer-local proposal
    /// counter, used only to break ties between concurrent proposals deterministically. All
    /// integers — no float touches the wire (invariant #1).
    DelayChange(PeerId, u64, u64, u64),
}

/// Encode one peer's command set for one execution tick into a wire frame.
fn encode_frame(peer: PeerId, tick: u64, commands: &[Command]) -> Vec<u8> {
    let mut w = Writer::new();
    w.u8(WIRE_VERSION);
    w.u8(FrameKind::Command as u8);
    w.u32(peer);
    w.u64(tick);
    w.u32(u32::try_from(commands.len()).expect("a tick's command set fits in u32"));
    for c in commands {
        put_command(&mut w, c);
    }
    w.buf
}

/// Encode one peer's post-tick checksum report for `tick` into a wire frame. The checksum is
/// written as raw `u64` LE bytes — no float ever touches the wire (invariant #1).
fn encode_checksum_frame(peer: PeerId, tick: u64, checksum: u64) -> Vec<u8> {
    let mut w = Writer::new();
    w.u8(WIRE_VERSION);
    w.u8(FrameKind::Checksum as u8);
    w.u32(peer);
    w.u64(tick);
    w.u64(checksum);
    w.buf
}

/// Encode an agreed delay-change proposal. `effective_tick` rides the shared `tick` slot; `seq`
/// and `new_delay` follow. All integers — no float on the wire (invariant #1).
fn encode_delay_change(proposer: PeerId, effective_tick: u64, seq: u64, new_delay: u64) -> Vec<u8> {
    let mut w = Writer::new();
    w.u8(WIRE_VERSION);
    w.u8(FrameKind::DelayChange as u8);
    w.u32(proposer);
    w.u64(effective_tick);
    w.u64(seq);
    w.u64(new_delay);
    w.buf
}

/// Decode a wire frame back into a [`Frame`]. Never panics on malformed input — a bad version,
/// kind tag, command tag, short buffer, or trailing byte is a [`DecodeError`], not a crash.
fn decode_frame(bytes: &[u8]) -> Result<Frame, DecodeError> {
    let mut r = Reader::new(bytes);
    let ver = r.u8()?;
    if ver != WIRE_VERSION {
        return Err(DecodeError::BadVersion(ver));
    }
    let kind = FrameKind::from_u8(r.u8()?)?;
    let peer = r.u32()?;
    let tick = r.u64()?;
    let frame = match kind {
        FrameKind::Command => {
            let n = r.u32()? as usize;
            // Cap the pre-allocation so a garbage length can't request a huge Vec; the loop
            // still reads exactly `n` and fails with UnexpectedEof if the bytes run short.
            let mut commands = Vec::with_capacity(n.min(256));
            for _ in 0..n {
                commands.push(get_command(&mut r)?);
            }
            Frame::Command(peer, tick, commands)
        }
        FrameKind::Checksum => Frame::Checksum(peer, tick, r.u64()?),
        // `peer` = proposer, `tick` = effective_tick; then seq, new_delay.
        FrameKind::DelayChange => Frame::DelayChange(peer, tick, r.u64()?, r.u64()?),
    };
    if r.pos != r.buf.len() {
        return Err(DecodeError::TrailingBytes);
    }
    Ok(frame)
}

// ===========================================================================
// The lockstep state machine.
// ===========================================================================

/// A detected cross-client checksum disagreement: a peer reported a different post-tick
/// checksum than ours for the same `tick`. This is **detection only** — surfacing it never
/// mutates sim/lockstep stepping (that policy decision belongs to the host, D27).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Desync {
    /// The execution tick whose checksums disagreed.
    pub tick: u64,
    /// The remote peer whose report disagreed with ours.
    pub peer: PeerId,
    /// Our own recorded post-tick checksum for `tick`.
    pub local: u64,
    /// The checksum the remote peer reported for `tick`.
    pub remote: u64,
}

/// Why a proposed delay change could not be scheduled.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ProposeError {
    /// A delay change is already pending; the host must wait for it to commit before proposing
    /// another (keeps at most one change in flight, so the apply tick is unambiguous).
    AlreadyPending,
}

/// A single peer's view of a deterministic lockstep session: it buffers per-tick command sets
/// (its own, stamped at a monotonic `submit_tick`, and peers' as they arrive), gates the sim on
/// having every peer's set for the next tick, and hands the host the merged set to apply.
///
/// Sans-I/O: feed it bytes with [`deliver`](Self::deliver), take bytes to send with
/// [`drain_outbound`](Self::drain_outbound); it never touches a socket.
pub struct Lockstep {
    peer_count: u32,
    local: PeerId,
    /// The active input delay. **Mutable** via the agreed [`propose_delay`](Self::propose_delay)
    /// protocol (B7); after the `submit_tick`/`warmup_until` refactor below it influences ONLY
    /// the prune/retention window, so changing it mid-session (identically on every peer, at a
    /// shipped effective tick) never perturbs which command executes at which tick.
    delay: u64,
    /// The next tick to execute (the gate target).
    next_tick: u64,
    /// The next tick a local [`submit`](Self::submit) will stamp. Starts at the initial delay and
    /// increments by one per submit — a monotonic cursor **independent of `delay`**, so an
    /// adaptive delay change can never make it jump backward (re-stamp) or forward (open a stall
    /// gap). This decoupling is what makes the delay change safe (B7).
    next_submit_tick: u64,
    /// The warmup boundary: ticks `[0, warmup_until)` execute empty (no input can exist that
    /// early). Fixed at the *initial* delay and **never** changed — a later adaptive delay change
    /// must not retroactively turn an executing tick back into warmup.
    warmup_until: u64,
    /// A pending agreed delay change `(effective_tick, new_delay, proposer, seq)`, applied in
    /// [`try_advance`](Self::try_advance) exactly when `next_tick == effective_tick`. `None` when
    /// no change is in flight. At most one at a time ([`ProposeError::AlreadyPending`]).
    pending_delay: Option<(u64, u64, PeerId, u64)>,
    /// Our own encoded `DelayChange` frame while a change we proposed is pending, re-sent every
    /// pump (loss-tolerant, like command/checksum frames) until it commits. `None` unless we are
    /// the proposer of the in-flight change.
    pending_frame: Option<Vec<u8>>,
    /// Monotonic proposal counter, stamped into our `DelayChange` frames so concurrent proposals
    /// break ties deterministically on `(effective_tick, proposer, seq)`.
    delay_seq: u64,
    /// `tick -> [per-peer command set]`; a `None` slot is a peer we are still waiting on.
    slots: BTreeMap<u64, Vec<Option<Vec<Command>>>>,
    /// Our own encoded frames, kept for (re)transmission until they fall out of the active
    /// window. Re-sending every drain makes delivery loss-tolerant without ACKs — a deliberate
    /// first-slice simplification; a real ACK/retransmit + flow-control layer is a later slice
    /// (`docs/plans/phase-3-plan.md` §"Workstream B").
    retained: BTreeMap<u64, Vec<u8>>,
    /// Our own post-tick checksum per recently-executed tick, recorded by the host via
    /// [`record_checksum`](Self::record_checksum). Used to (a) emit our checksum reports in
    /// [`drain_outbound`](Self::drain_outbound) and (b) compare against incoming reports in
    /// [`deliver`](Self::deliver). Pruned on the same window as `retained`/`slots`.
    checksums: BTreeMap<u64, u64>,
    /// Detected cross-client checksum disagreements, queued for the host to drain via
    /// [`take_desyncs`](Self::take_desyncs). Detection only — never alters stepping.
    desyncs: Vec<Desync>,
    /// Executed merged per-tick command sets, retained so a reconnecting peer can be replayed from
    /// an authoritative snapshot (`core::reconnect`, workstream C). Keyed by execution tick.
    /// Pruned on the normal active window unless a snapshot-retention floor (`retain_floor`) holds
    /// older ticks. Pure side state: capturing it never changes which commands execute or any
    /// checksum.
    executed: BTreeMap<u64, Vec<Command>>,
    /// The oldest tick the reconnect-replay buffer must keep (the last authoritative snapshot
    /// tick). `None` ⇒ no snapshot retention configured, so `executed` prunes on the normal
    /// window. Set/advanced via [`retain_from`](Self::retain_from); monotonic.
    retain_floor: Option<u64>,
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
            next_submit_tick: delay,
            warmup_until: delay,
            pending_delay: None,
            pending_frame: None,
            delay_seq: 0,
            slots: BTreeMap::new(),
            retained: BTreeMap::new(),
            checksums: BTreeMap::new(),
            desyncs: Vec::new(),
            executed: BTreeMap::new(),
            retain_floor: None,
        }
    }

    /// Install/advance the snapshot-retention floor: the reconnect-replay buffer keeps every
    /// executed merged command set at or after `tick`, so a peer can later be resumed from an
    /// authoritative snapshot taken at `tick` and replayed forward (`core::reconnect`, workstream
    /// C). The host calls this when it captures a new snapshot. Monotonic — a lower tick than the
    /// current floor is ignored (snapshots only move forward) — and re-prunes to free anything now
    /// older than the floor.
    pub fn retain_from(&mut self, tick: u64) {
        self.retain_floor = Some(match self.retain_floor {
            Some(f) => f.max(tick),
            None => tick,
        });
        self.prune();
    }

    /// The executed merged command sets for `[from, to)`, in tick order, for replaying a peer from
    /// a snapshot@`from` up to the live tick `to` (normally [`next_tick`](Self::next_tick)).
    /// Returns `None` if any tick in the range has been pruned out of the buffer — a loud miss the
    /// caller must handle (snapshot more often / widen the floor), never a silent short replay
    /// that would resume to a divergent state.
    pub fn replay_range(&self, from: u64, to: u64) -> Option<Vec<&[Command]>> {
        let mut out = Vec::with_capacity(to.saturating_sub(from) as usize);
        for t in from..to {
            out.push(self.executed.get(&t)?.as_slice());
        }
        Some(out)
    }

    /// The session's current input delay (in ticks). Adaptive via the agreed
    /// [`propose_delay`](Self::propose_delay) protocol (B7).
    pub fn delay(&self) -> u64 {
        self.delay
    }

    /// The pending agreed delay change as `(effective_tick, new_delay)`, or `None` if no change
    /// is scheduled. Lets the host/tests observe a change between proposal and commit.
    pub fn pending_delay(&self) -> Option<(u64, u64)> {
        self.pending_delay.map(|(eff, nd, _, _)| (eff, nd))
    }

    /// Propose switching the session input delay to `new_delay`, taking effect `guard` ticks
    /// (clamped to at least `current_delay + 1`) beyond the current frontier. Returns the agreed
    /// `effective_tick` and queues a `DelayChange` frame for broadcast; it does **not** change
    /// `delay` now — the switch happens in [`try_advance`](Self::try_advance) at `effective_tick`,
    /// identically on every peer that receives the frame.
    ///
    /// RTT lives entirely host-side: `core` reads no clock and sees only the integer `new_delay`
    /// and `guard` the host chose (invariants #1/#2). The host should pick `guard` larger than the
    /// worst-case one-way latency in ticks so every peer receives the frame before `effective_tick`.
    pub fn propose_delay(&mut self, new_delay: u64, guard: u64) -> Result<u64, ProposeError> {
        if self.pending_delay.is_some() {
            return Err(ProposeError::AlreadyPending);
        }
        let frontier = self.submit_tick().max(self.next_tick);
        let lead = guard.max(self.delay + 1);
        let effective_tick = frontier + lead;
        self.delay_seq += 1;
        let seq = self.delay_seq;
        self.pending_delay = Some((effective_tick, new_delay, self.local, seq));
        self.pending_frame = Some(encode_delay_change(self.local, effective_tick, seq, new_delay));
        Ok(effective_tick)
    }

    /// The next tick the sim will execute (None of it has run yet at `next_tick`).
    pub fn next_tick(&self) -> u64 {
        self.next_tick
    }

    /// The execution tick the next [`submit`](Self::submit) will stamp its input onto. A monotonic
    /// cursor independent of `delay`, so an adaptive delay change never perturbs it.
    pub fn submit_tick(&self) -> u64 {
        self.next_submit_tick
    }

    /// Stamp this peer's local input for the next submit tick, record it locally, and retain it
    /// for sending. Call once per tick you intend to advance.
    pub fn submit(&mut self, commands: Vec<Command>) {
        let tick = self.submit_tick();
        self.next_submit_tick += 1;
        self.retained
            .insert(tick, encode_frame(self.local, tick, &commands));
        let slot = self
            .slots
            .entry(tick)
            .or_insert_with(|| vec![None; self.peer_count as usize]);
        slot[self.local as usize] = Some(commands);
    }

    /// Record our own post-tick sim checksum for `tick`. The host calls this after each
    /// [`Sim::step`](crate::sim::Sim::step) (the tick just advanced by [`try_advance`]). The
    /// value is kept in a bounded recent window (pruned like `retained`/`slots`) so we can both
    /// broadcast it and compare incoming peer reports against it. Pure verification: recording a
    /// checksum never changes which commands execute or the checksums themselves.
    ///
    /// [`try_advance`]: Self::try_advance
    pub fn record_checksum(&mut self, tick: u64, checksum: u64) {
        self.checksums.insert(tick, checksum);
    }

    /// Encoded frames to hand to the transport this pump. Re-sends every retained command frame
    /// **and** a checksum report for every recently-recorded tick; the receiver ignores
    /// stale/duplicate ticks, so this is loss-tolerant without ACKs (same posture as command
    /// frames). Checksum reports are pure verification — they never change stepping.
    pub fn drain_outbound(&mut self) -> Vec<Vec<u8>> {
        let mut frames: Vec<Vec<u8>> = self.retained.values().cloned().collect();
        for (&tick, &sum) in &self.checksums {
            frames.push(encode_checksum_frame(self.local, tick, sum));
        }
        // Re-send our in-flight delay-change proposal until it commits (loss-tolerant resend,
        // same posture as command/checksum frames).
        if let Some(frame) = &self.pending_frame {
            frames.push(frame.clone());
        }
        frames
    }

    /// Drain the cross-client checksum disagreements detected since the last call. Each is a
    /// live desync (a peer's post-tick checksum differed from ours for the same tick). Detection
    /// only — the host decides what to do (halt, snapshot, etc.); lockstep stepping is untouched.
    pub fn take_desyncs(&mut self) -> Vec<Desync> {
        std::mem::take(&mut self.desyncs)
    }

    /// Ingest a received frame. Ignores our own echo and command frames for already-executed
    /// ticks; the first set seen for a `(tick, peer)` wins (re-sends are identical). A checksum
    /// report is compared against our own recorded checksum for that tick: a match is a no-op, a
    /// mismatch queues a [`Desync`] (drainable via [`take_desyncs`](Self::take_desyncs)), and a
    /// report for a tick whose checksum we have not recorded (not yet executed, or already pruned
    /// out of our window) is **ignored** — we only verify what we can directly compare, so a late
    /// or far-ahead report never produces a false desync. Returns `Err` only if the bytes are
    /// malformed.
    pub fn deliver(&mut self, bytes: &[u8]) -> Result<(), DecodeError> {
        match decode_frame(bytes)? {
            Frame::Command(peer, tick, commands) => {
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
            }
            Frame::Checksum(peer, tick, remote) => {
                if peer >= self.peer_count {
                    return Err(DecodeError::PeerOutOfRange(peer));
                }
                if peer == self.local {
                    return Ok(()); // our own echo
                }
                // Only compare ticks we have a recorded checksum for. Unknown ticks (not yet
                // executed locally, or pruned out of the window) are ignored — see the doc above.
                if let Some(&local) = self.checksums.get(&tick) {
                    // Report each genuine `(tick, peer)` divergence ONCE. Checksum frames are
                    // re-sent every pump (loss-tolerant resend), so without this guard a single
                    // desync would push one entry per retransmit (up to a window's worth),
                    // flooding `take_desyncs` callers. Detection is per-divergence, not per-frame.
                    let already = self
                        .desyncs
                        .iter()
                        .any(|d| d.tick == tick && d.peer == peer);
                    if local != remote && !already {
                        self.desyncs.push(Desync {
                            tick,
                            peer,
                            local,
                            remote,
                        });
                    }
                }
            }
            Frame::DelayChange(proposer, effective_tick, seq, new_delay) => {
                if proposer >= self.peer_count {
                    return Err(DecodeError::PeerOutOfRange(proposer));
                }
                if proposer == self.local {
                    return Ok(()); // our own echo; we set the pending change in propose_delay
                }
                if effective_tick <= self.next_tick {
                    return Ok(()); // already at/past the switch point — too late to apply safely
                }
                // Deterministic tiebreak: a strictly-"earlier" proposal (lower
                // (effective_tick, proposer, seq)) replaces a pending one; an equal key is a
                // no-op resend. Both peers see both broadcasts and converge on the same winner.
                let incoming = (effective_tick, proposer, seq);
                let replace = match self.pending_delay {
                    None => true,
                    Some((e, _, p, s)) => incoming < (e, p, s),
                };
                if replace {
                    self.pending_delay = Some((effective_tick, new_delay, proposer, seq));
                }
            }
        }
        Ok(())
    }

    /// If every peer's command set for the next tick is present, merge them in fixed peer order,
    /// advance, and return the set for the host to apply to its `Sim`. Returns `None` if still
    /// waiting on a peer (stall). Warmup ticks `[0, delay)` advance immediately with an empty set.
    pub fn try_advance(&mut self) -> Option<Vec<Command>> {
        let t = self.next_tick;
        // Apply an agreed delay change exactly at its effective tick — identically on every peer,
        // since `effective_tick`/`new_delay` are shipped as data, never recomputed locally. This
        // touches only `delay` (and thus the prune window), never the monotonic submit cursor or
        // the fixed warmup boundary, so no command is re-stamped or dropped (B7).
        if let Some((eff, new_delay, _, _)) = self.pending_delay {
            if t >= eff {
                // Apply at the effective tick — or, if a post-stall catch-up loop galloped past
                // it in one burst, immediately after. `delay`'s only live effect is the prune
                // window size, so a late apply is idempotent and still converges every peer to the
                // identical value; it can never re-stamp/drop a command or desync the stream.
                self.delay = new_delay;
                self.pending_delay = None;
                self.pending_frame = None;
            }
        }
        if t < self.warmup_until {
            self.executed.insert(t, Vec::new());
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
        // Retain the executed merged set for reconnect replay (workstream C). Capturing it cannot
        // change stepping or the checksum — it is read only by `replay_range`.
        self.executed.insert(t, merged.clone());
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
        let stale: Vec<u64> = self.checksums.range(..lo).map(|(k, _)| *k).collect();
        for k in stale {
            self.checksums.remove(&k);
        }
        // The reconnect-replay buffer keeps the normal window OR back to the snapshot floor,
        // whichever is older — so a peer can be replayed from the last snapshot to now.
        let lo_exec = match self.retain_floor {
            Some(f) => lo.min(f),
            None => lo,
        };
        let stale: Vec<u64> = self.executed.range(..lo_exec).map(|(k, _)| *k).collect();
        for k in stale {
            self.executed.remove(&k);
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
            Command::Fire {
                entity: ent(9, 1),
                dir: v(1, 0),
            },
            Command::Locomote {
                entity: ent(9, 1),
                dir: v(0, 1),
            },
            Command::Reload { entity: ent(9, 1) },
            Command::Crouch {
                entity: ent(9, 1),
                crouched: true,
            },
            Command::Crouch {
                entity: ent(9, 1),
                crouched: false,
            },
            Command::AimTurret {
                entity: ent(10, 0),
                dir: v(1, 0),
            },
            Command::DriveHull {
                entity: ent(11, 1),
                dir: v(0, 1),
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
        let (peer, tick, decoded) = match decode_frame(&bytes).expect("decode") {
            Frame::Command(p, t, c) => (p, t, c),
            other => panic!("expected a command frame, got {other:?}"),
        };
        assert_eq!(peer, 0);
        assert_eq!(tick, 42);
        assert_eq!(decoded.len(), cmds.len());
        // Command has no PartialEq; re-encoding the decoded set and comparing bytes is a stronger
        // codec check (it would catch a field silently dropped/reordered).
        assert_eq!(bytes, encode_frame(peer, tick, &decoded));
    }

    #[test]
    fn checksum_frame_roundtrips() {
        let bytes = encode_checksum_frame(1, 77, 0xDEAD_BEEF_F00D_CAFE);
        match decode_frame(&bytes).expect("decode") {
            Frame::Checksum(peer, tick, sum) => {
                assert_eq!(peer, 1);
                assert_eq!(tick, 77);
                assert_eq!(sum, 0xDEAD_BEEF_F00D_CAFE);
            }
            other => panic!("expected a checksum frame, got {other:?}"),
        }
        // Re-encoding the decoded report reproduces the exact bytes (catches a dropped field).
        assert_eq!(bytes, encode_checksum_frame(1, 77, 0xDEAD_BEEF_F00D_CAFE));
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
        w.u8(200); // a frame-kind tag that does not exist
        assert_eq!(decode_frame(&w.buf).unwrap_err(), DecodeError::BadTag(200));

        let mut w = Writer::new();
        w.u8(WIRE_VERSION);
        w.u8(FrameKind::Command as u8);
        w.u32(0);
        w.u64(0);
        w.u32(1);
        w.u8(250); // a command tag that does not exist
        assert_eq!(decode_frame(&w.buf).unwrap_err(), DecodeError::BadTag(250));

        let mut w = Writer::new();
        w.u8(WIRE_VERSION);
        w.u8(FrameKind::Command as u8);
        w.u32(0);
        w.u64(0);
        w.u32(5); // claims 5 commands, provides none
        assert_eq!(
            decode_frame(&w.buf).unwrap_err(),
            DecodeError::UnexpectedEof
        );

        let mut w = Writer::new();
        w.u8(WIRE_VERSION);
        w.u8(FrameKind::Command as u8);
        w.u32(0);
        w.u64(0);
        w.u32(0); // a valid empty command frame …
        w.u8(0xFF); // … then an unexpected trailing byte (codec/version skew)
        assert_eq!(
            decode_frame(&w.buf).unwrap_err(),
            DecodeError::TrailingBytes
        );

        // A checksum frame that ends mid-checksum (only 4 of the 8 LE bytes present).
        let mut w = Writer::new();
        w.u8(WIRE_VERSION);
        w.u8(FrameKind::Checksum as u8);
        w.u32(0);
        w.u64(0);
        w.u32(0); // 4 bytes where the 8-byte checksum should be
        assert_eq!(
            decode_frame(&w.buf).unwrap_err(),
            DecodeError::UnexpectedEof
        );

        // A checksum frame with a trailing byte past the checksum is rejected (codec skew).
        let mut w = Writer::new();
        w.u8(WIRE_VERSION);
        w.u8(FrameKind::Checksum as u8);
        w.u32(0);
        w.u64(0);
        w.u64(0);
        w.u8(0xAB);
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

    // ----- checksum agreement / desync detection -----

    #[test]
    fn checksum_report_agreement_is_a_no_op() {
        let mut ls = Lockstep::new(2, 0, 0);
        ls.record_checksum(7, 0xABCD);
        // A matching report from peer 1 for the same tick: no desync.
        ls.deliver(&encode_checksum_frame(1, 7, 0xABCD)).unwrap();
        assert!(
            ls.take_desyncs().is_empty(),
            "matching checksum is agreement"
        );
    }

    #[test]
    fn checksum_report_mismatch_is_detected_and_surfaced() {
        let mut ls = Lockstep::new(2, 0, 0);
        ls.record_checksum(7, 0xABCD);
        ls.deliver(&encode_checksum_frame(1, 7, 0x9999)).unwrap();
        let d = ls.take_desyncs();
        assert_eq!(
            d,
            vec![Desync {
                tick: 7,
                peer: 1,
                local: 0xABCD,
                remote: 0x9999,
            }]
        );
        // Draining clears the queue.
        assert!(ls.take_desyncs().is_empty(), "desyncs drained once");
    }

    #[test]
    fn repeated_mismatch_reports_yield_one_desync() {
        // Checksum frames are re-sent every pump (loss-tolerant resend), so a single genuine
        // divergence arrives many times. It must be reported ONCE per `(tick, peer)`, not flood
        // `take_desyncs` with a window's worth of duplicates.
        let mut ls = Lockstep::new(2, 0, 0);
        ls.record_checksum(7, 0xABCD);
        for _ in 0..5 {
            ls.deliver(&encode_checksum_frame(1, 7, 0x9999)).unwrap();
        }
        assert_eq!(
            ls.take_desyncs().len(),
            1,
            "a re-sent divergence is one desync, not one per frame"
        );
    }

    #[test]
    fn checksum_report_for_unknown_tick_is_ignored() {
        let mut ls = Lockstep::new(2, 0, 0);
        // No recorded checksum for tick 9 yet → cannot compare → ignored, no false desync.
        ls.deliver(&encode_checksum_frame(1, 9, 0x1234)).unwrap();
        assert!(ls.take_desyncs().is_empty(), "unknown tick must not desync");
    }

    #[test]
    fn checksum_report_ignores_our_own_echo() {
        let mut ls = Lockstep::new(2, 0, 0);
        ls.record_checksum(3, 0xAAAA);
        // Our own report echoed back, even with a (impossible) different value, is ignored.
        ls.deliver(&encode_checksum_frame(0, 3, 0xBBBB)).unwrap();
        assert!(ls.take_desyncs().is_empty(), "our own echo never desyncs");
    }

    #[test]
    fn checksum_report_rejects_out_of_range_peer() {
        let mut ls = Lockstep::new(2, 0, 0);
        ls.record_checksum(1, 0xABCD);
        let bad = encode_checksum_frame(5, 1, 0xABCD);
        assert_eq!(ls.deliver(&bad), Err(DecodeError::PeerOutOfRange(5)));
    }

    #[test]
    fn drain_outbound_emits_checksum_reports() {
        let mut ls = Lockstep::new(2, 0, 1);
        ls.record_checksum(0, 0x1111);
        ls.record_checksum(1, 0x2222);
        let frames = ls.drain_outbound();
        // Decode every frame; collect the checksum reports we emitted.
        let mut reports: Vec<(u64, u64)> = frames
            .iter()
            .filter_map(|f| match decode_frame(f).unwrap() {
                Frame::Checksum(peer, tick, sum) => {
                    assert_eq!(peer, 0, "we emit reports under our own peer id");
                    Some((tick, sum))
                }
                Frame::Command(..) | Frame::DelayChange(..) => None,
            })
            .collect();
        reports.sort_unstable();
        assert_eq!(reports, vec![(0, 0x1111), (1, 0x2222)]);
    }

    #[test]
    fn checksum_window_prunes_with_the_active_window() {
        // The checksum window prunes on the same `2*delay+1` tail as slots/retained. With
        // delay 0 the tail is 1 tick: after advancing well past a recorded tick, that tick's
        // checksum is dropped, so a *mismatching* report for it from a real peer is ignored
        // (nothing to compare against) — no false desync from a long-gone tick.
        let mut ls = Lockstep::new(2, 0, 0);
        ls.record_checksum(0, 0xDEAD);
        // Drive the 2-peer session past tick 0: submit locally and feed peer 1's empty sets.
        for t in 0..5 {
            ls.submit(Vec::new());
            ls.deliver(&encode_frame(1, t, &[])).unwrap();
            ls.try_advance().expect("both peers present → advances");
        }
        assert!(ls.next_tick() >= 5);
        // Tick 0's checksum is pruned; peer 1 reporting a different value for it is a no-op.
        ls.deliver(&encode_checksum_frame(1, 0, 0xBEEF)).unwrap();
        assert!(
            ls.take_desyncs().is_empty(),
            "a pruned tick must not produce a desync"
        );
        // Sanity: an *in-window* mismatch is still caught (proves it's prune, not a dead path).
        let cur = ls.next_tick();
        ls.record_checksum(cur, 0xC0FFEE);
        ls.deliver(&encode_checksum_frame(1, cur, 0xBAD)).unwrap();
        assert_eq!(
            ls.take_desyncs().len(),
            1,
            "in-window mismatch still caught"
        );
    }

    #[test]
    fn delay_accessor_returns_configured_delay() {
        assert_eq!(Lockstep::new(2, 0, 0).delay(), 0);
        assert_eq!(Lockstep::new(2, 1, 7).delay(), 7);
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
        } else if t == delay + 10 {
            // Mid-embodiment (p[0] is embodied from `delay` to `delay + 25`): drive the avatar with
            // a live Locomote so the full gate → merge → step → `step_along` → pos/vel → checksum
            // path runs across the two-peer channel, not just the codec round-trip. dir is a Fixed
            // unit vector (what the engine seam will quantize to); both peers see the identical
            // command, so the checksum must still agree (invariant #7).
            match peer {
                0 => vec![Command::Locomote {
                    entity: h.p[0],
                    dir: v(1, 0),
                }],
                _ => Vec::new(),
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

    // ----- B7: agreed RTT-adaptive delay change -----

    #[test]
    fn delay_change_frame_roundtrips() {
        let bytes = encode_delay_change(1, 200, 7, 5);
        match decode_frame(&bytes).expect("decode") {
            Frame::DelayChange(proposer, eff, seq, nd) => {
                assert_eq!((proposer, eff, seq, nd), (1, 200, 7, 5));
            }
            other => panic!("expected a DelayChange frame, got {other:?}"),
        }
        // Re-encoding the decoded proposal reproduces the bytes (catches a dropped/reordered field).
        assert_eq!(bytes, encode_delay_change(1, 200, 7, 5));
    }

    #[test]
    fn delay_change_decode_rejects_malformed() {
        // Ends mid `new_delay` (4 of the 8 LE bytes present).
        let mut w = Writer::new();
        w.u8(WIRE_VERSION);
        w.u8(FrameKind::DelayChange as u8);
        w.u32(0);
        w.u64(10);
        w.u64(1);
        w.u32(0);
        assert_eq!(
            decode_frame(&w.buf).unwrap_err(),
            DecodeError::UnexpectedEof
        );
        // A valid proposal then an unexpected trailing byte (codec/version skew).
        let mut w = Writer::new();
        w.u8(WIRE_VERSION);
        w.u8(FrameKind::DelayChange as u8);
        w.u32(0);
        w.u64(10);
        w.u64(1);
        w.u64(5);
        w.u8(0xAB);
        assert_eq!(
            decode_frame(&w.buf).unwrap_err(),
            DecodeError::TrailingBytes
        );
    }

    #[test]
    fn older_wire_version_frames_now_rejected() {
        // Every codec bump must be enforced: a frame written under any older WIRE_VERSION is
        // rejected loudly, never silently misparsed against the new layout. Cover the original
        // frame-kind bump (2), the pre-`Locomote` version (3), and the immediately-previous
        // version (5, pre-`AimTurret`/`DriveHull`) — the P2 tank vocabulary (tags 14/15) is the
        // reason WIRE_VERSION is now 6, so a pre-P2 peer's frame must be rejected at the handshake.
        for old in [2u8, 3, 5] {
            let mut w = Writer::new();
            w.u8(old);
            w.u8(FrameKind::Command as u8);
            w.u32(0);
            w.u64(0);
            w.u32(0);
            assert_eq!(decode_frame(&w.buf).unwrap_err(), DecodeError::BadVersion(old));
        }
    }

    #[test]
    fn propose_delay_rejects_a_second_pending_change() {
        let mut ls = Lockstep::new(2, 0, 2);
        ls.propose_delay(5, 3).unwrap();
        assert_eq!(
            ls.propose_delay(7, 3),
            Err(ProposeError::AlreadyPending),
            "only one delay change may be in flight"
        );
    }

    #[test]
    fn delay_change_applies_exactly_at_effective_tick() {
        // Single-peer session: the gate never stalls, so we can watch `delay()` cross `eff`.
        let mut ls = Lockstep::new(1, 0, 2);
        for _ in 0..10 {
            ls.submit(Vec::new());
        }
        let eff = ls.propose_delay(5, 3).unwrap();
        assert_eq!(ls.delay(), 2, "delay must not change at proposal time");
        assert_eq!(ls.pending_delay(), Some((eff, 5)));
        // Cover the slots through `eff` so the gate can reach it.
        while ls.submit_tick() <= eff + 2 {
            ls.submit(Vec::new());
        }
        while ls.next_tick() < eff {
            assert_eq!(ls.delay(), 2, "delay must hold until the effective tick {eff}");
            ls.try_advance().expect("single-peer session always advances");
        }
        assert_eq!(ls.next_tick(), eff);
        ls.try_advance().expect("advance across the effective tick");
        assert_eq!(ls.delay(), 5, "delay switches exactly at the effective tick");
        assert_eq!(ls.pending_delay(), None, "the pending change is cleared once applied");
    }

    /// Like [`run_two_client`] but optionally proposes an agreed delay change mid-run, submitting
    /// per-pump (so `submit_tick` stays a small lead ahead of execution and the effective tick is
    /// actually reached). The script stays keyed to the *initial* delay, so a correct delay change
    /// — which only resizes the prune window, never re-stamps a command — leaves the agreed stream
    /// bit-identical to the no-change reference. Returns each peer's checksum stream, the
    /// reference, and each peer's final delay.
    #[allow(clippy::too_many_arguments)]
    fn run_two_client_adaptive(
        initial_delay: u64,
        base_delay: u64,
        jitter: u32,
        loss_num: u32,
        loss_den: u32,
        net_seed: u64,
        target: u64,
        change: Option<(u64, PeerId, u64, u64)>, // (at_tick, proposer, new_delay, guard)
    ) -> ([Vec<u64>; 2], Vec<u64>, [u64; 2]) {
        let mut sims = [Sim::new(SCENE_SEED), Sim::new(SCENE_SEED)];
        let h = scene(&mut sims[0]);
        let _ = scene(&mut sims[1]);

        let mut refsim = Sim::new(SCENE_SEED);
        let _ = scene(&mut refsim);
        let mut refsums = Vec::with_capacity(target as usize);
        for t in 0..target {
            let mut merged = Vec::new();
            if t >= initial_delay {
                merged.extend(script(&h, 0, t, initial_delay));
                merged.extend(script(&h, 1, t, initial_delay));
            }
            refsim.step(&merged);
            refsums.push(refsim.checksum());
        }

        let mut sessions = [
            Lockstep::new(2, 0, initial_delay),
            Lockstep::new(2, 1, initial_delay),
        ];
        let mut net = Net::new(net_seed, base_delay, jitter, loss_num, loss_den);
        let mut sums = [
            Vec::with_capacity(target as usize),
            Vec::with_capacity(target as usize),
        ];
        let lead_bound = initial_delay + 2;
        let mut proposed = false;
        let mut it = 0u64;
        loop {
            for i in 0..2 {
                while sessions[i].submit_tick() < target
                    && sessions[i].submit_tick() <= sessions[i].next_tick() + lead_bound
                {
                    let t = sessions[i].submit_tick();
                    let cmds = script(&h, i as PeerId, t, initial_delay);
                    sessions[i].submit(cmds);
                }
            }
            if let Some((at, proposer, nd, guard)) = change {
                if !proposed && sessions[proposer as usize].next_tick() >= at {
                    sessions[proposer as usize]
                        .propose_delay(nd, guard)
                        .expect("first proposal");
                    proposed = true;
                }
            }
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
                "adaptive lockstep failed to converge (change={change:?})"
            );
        }
        let finals = [sessions[0].delay(), sessions[1].delay()];
        (sums, refsums, finals)
    }

    #[test]
    fn delay_change_clean_channel_applies_and_stays_in_sync() {
        let (sums, refsums, finals) =
            run_two_client_adaptive(3, 0, 0, 0, 1, 0xC0FFEE, 120, Some((40, 0, 6, 4)));
        assert_eq!(sums[0].len(), 120);
        assert_eq!(sums[0], sums[1], "peers diverged across a delay change");
        assert_eq!(
            sums[0], refsums,
            "a delay change must not alter which command executes at which tick"
        );
        assert_eq!(finals, [6, 6], "both peers applied the agreed new delay");
    }

    #[test]
    fn delay_change_increase_under_loss_stays_in_sync() {
        // Peer 1 proposes raising the delay 3 → 7 mid-run, over a lossy/jittery/reordering channel.
        let (sums, refsums, finals) =
            run_two_client_adaptive(3, 1, 2, 1, 4, 0x1234567, 140, Some((45, 1, 7, 5)));
        assert_eq!(sums[0], sums[1], "peers diverged on a delay increase under loss");
        assert_eq!(sums[0], refsums, "reference mismatch on a delay increase");
        assert_eq!(finals, [7, 7], "both peers applied the increased delay");
    }

    #[test]
    fn delay_change_decrease_stays_in_sync() {
        // Peer 0 proposes lowering the delay 6 → 2 mid-run, over a lossy channel.
        let (sums, refsums, finals) =
            run_two_client_adaptive(6, 1, 2, 1, 5, 0x9999, 140, Some((50, 0, 2, 4)));
        assert_eq!(sums[0], sums[1], "peers diverged on a delay decrease");
        assert_eq!(sums[0], refsums, "reference mismatch on a delay decrease");
        assert_eq!(finals, [2, 2], "both peers applied the decreased delay");
    }
}
