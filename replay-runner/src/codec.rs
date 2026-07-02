//! The **replay wire codec** — a deterministic little-endian byte encoding for a
//! [`Replay`](crate::Replay)'s input log.
//!
//! This lives in the runner crate, not `core` (invariant #2: `core` stays serde-free and
//! dependency-free). It mirrors `core::lockstep`'s wire-codec discipline: fixed-point values go
//! out as their raw `i32` bits (`Fixed::to_bits`), handles as `index`+`generation`, and every
//! enum as a loud tag byte so a bad/skewed byte stream is an *error to handle*
//! ([`ReplayError`]), never a silent misparse that would fake a "successful" replay.
//!
//! The command tag numbers are deliberately the SAME as `core::lockstep`'s `put_command` /
//! `get_command` (0..=18) so the two encodings stay conceptually in lockstep; they are
//! independent code, but a reviewer can diff them tag-for-tag.

use gonedark_core::components::{
    Army, BuildingKind, Faction, Order, ShellKind, Stance, UnitKind, Vec2,
};
use gonedark_core::ecs::Entity;
use gonedark_core::fixed::Fixed;
use gonedark_core::sim::Command;

/// An error decoding a replay artifact — a corrupt or version-skewed stream is handled, not a
/// crash (mirroring `core::lockstep::DecodeError`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ReplayError {
    /// The leading magic bytes were not `GDRP`.
    BadMagic,
    /// The format version byte did not match [`FORMAT_VERSION`].
    BadVersion(u16),
    /// The stream ended mid-field.
    UnexpectedEof,
    /// A tag byte was outside the known range for `what` (e.g. an unknown command/order/enum tag).
    BadTag { what: &'static str, tag: u8 },
    /// The stream parsed but left trailing bytes — a sign of codec/version skew.
    TrailingBytes,
}

impl std::fmt::Display for ReplayError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ReplayError::BadMagic => write!(f, "not a Going Dark replay (bad magic)"),
            ReplayError::BadVersion(v) => {
                write!(f, "unsupported replay format version {v} (expected {FORMAT_VERSION})")
            }
            ReplayError::UnexpectedEof => write!(f, "replay stream ended mid-field"),
            ReplayError::BadTag { what, tag } => write!(f, "bad {what} tag {tag}"),
            ReplayError::TrailingBytes => write!(f, "trailing bytes after replay body"),
        }
    }
}

impl std::error::Error for ReplayError {}

/// Magic prefix: "Going Dark RePlay".
pub const MAGIC: [u8; 4] = *b"GDRP";
/// Replay format version. Bump on any codec change so a mismatched build is rejected, not
/// silently misparsed.
pub const FORMAT_VERSION: u16 = 1;

// ---- little-endian writer helpers (over a plain Vec<u8>; no dep) ----

pub(crate) fn put_u8(w: &mut Vec<u8>, v: u8) {
    w.push(v);
}
pub(crate) fn put_u16(w: &mut Vec<u8>, v: u16) {
    w.extend_from_slice(&v.to_le_bytes());
}
pub(crate) fn put_u32(w: &mut Vec<u8>, v: u32) {
    w.extend_from_slice(&v.to_le_bytes());
}
pub(crate) fn put_u64(w: &mut Vec<u8>, v: u64) {
    w.extend_from_slice(&v.to_le_bytes());
}
pub(crate) fn put_i32(w: &mut Vec<u8>, v: i32) {
    w.extend_from_slice(&v.to_le_bytes());
}

fn put_bool(w: &mut Vec<u8>, v: bool) {
    put_u8(w, v as u8);
}
fn put_fixed(w: &mut Vec<u8>, v: Fixed) {
    // Fixed-point goes out as its raw i32 bits — the same wire discipline core::lockstep uses,
    // so no float ever touches the encoding (invariant #1).
    put_i32(w, v.to_bits());
}
fn put_vec2(w: &mut Vec<u8>, v: Vec2) {
    put_fixed(w, v.x);
    put_fixed(w, v.y);
}
fn put_entity(w: &mut Vec<u8>, e: Entity) {
    put_u32(w, e.index);
    put_u32(w, e.generation);
}

fn faction_tag(f: Faction) -> u8 {
    match f {
        Faction::Player => 0,
        Faction::Enemy => 1,
        Faction::Neutral => 2,
    }
}
fn building_tag(b: BuildingKind) -> u8 {
    match b {
        BuildingKind::Camp => 0,
        BuildingKind::Barracks => 1,
    }
}
fn unit_tag(u: UnitKind) -> u8 {
    match u {
        UnitKind::Rifleman => 0,
        UnitKind::Heavy => 1,
        UnitKind::Tank => 2,
        UnitKind::Medic => 3,
        UnitKind::AntiTank => 4,
    }
}
fn stance_tag(s: Stance) -> u8 {
    match s {
        Stance::HoldFire => 0,
        Stance::ReturnFire => 1,
        Stance::FireAtWill => 2,
    }
}
fn army_tag(a: Army) -> u8 {
    match a {
        Army::Neutral => 0,
        Army::Us => 1,
        Army::Fr => 2,
    }
}
fn shell_tag(s: ShellKind) -> u8 {
    match s {
        ShellKind::Ap => 0,
        ShellKind::Aphe => 1,
        ShellKind::He => 2,
    }
}

fn put_order(w: &mut Vec<u8>, o: Order) {
    match o {
        Order::Idle => put_u8(w, 0),
        Order::MoveTo(p) => {
            put_u8(w, 1);
            put_vec2(w, p);
        }
        Order::AttackMove(p) => {
            put_u8(w, 2);
            put_vec2(w, p);
        }
        Order::Patrol { a, b, toward_b } => {
            put_u8(w, 3);
            put_vec2(w, a);
            put_vec2(w, b);
            put_bool(w, toward_b);
        }
        Order::HoldPosition => put_u8(w, 4),
        Order::FallBack(p) => {
            put_u8(w, 5);
            put_vec2(w, p);
        }
    }
}

/// Append the byte encoding of one [`Command`] to `w`. Exhaustive over the enum: adding a new
/// `Command` variant is a compile error here until the codec learns it (no silent drop).
pub(crate) fn put_command(w: &mut Vec<u8>, c: &Command) {
    match *c {
        Command::Move { entity, target } => {
            put_u8(w, 0);
            put_entity(w, entity);
            put_vec2(w, target);
        }
        Command::AttackMove { entity, target } => {
            put_u8(w, 1);
            put_entity(w, entity);
            put_vec2(w, target);
        }
        Command::SetOrder { entity, order } => {
            put_u8(w, 2);
            put_entity(w, entity);
            put_order(w, order);
        }
        Command::SetStance { entity, stance } => {
            put_u8(w, 3);
            put_entity(w, entity);
            put_u8(w, stance_tag(stance));
        }
        Command::SetRetreatThreshold { entity, fraction } => {
            put_u8(w, 4);
            put_entity(w, entity);
            put_fixed(w, fraction);
        }
        Command::Embody { entity } => {
            put_u8(w, 5);
            put_entity(w, entity);
        }
        Command::Surface { entity } => {
            put_u8(w, 6);
            put_entity(w, entity);
        }
        Command::Build { faction, kind, pos } => {
            put_u8(w, 7);
            put_u8(w, faction_tag(faction));
            put_u8(w, building_tag(kind));
            put_vec2(w, pos);
        }
        Command::Upgrade { camp } => {
            put_u8(w, 8);
            put_entity(w, camp);
        }
        Command::QueueProduction { camp, unit } => {
            put_u8(w, 9);
            put_entity(w, camp);
            put_u8(w, unit_tag(unit));
        }
        Command::Fire { entity, dir } => {
            put_u8(w, 10);
            put_entity(w, entity);
            put_vec2(w, dir);
        }
        Command::Locomote { entity, dir } => {
            put_u8(w, 11);
            put_entity(w, entity);
            put_vec2(w, dir);
        }
        Command::Reload { entity } => {
            put_u8(w, 12);
            put_entity(w, entity);
        }
        Command::Crouch { entity, crouched } => {
            put_u8(w, 13);
            put_entity(w, entity);
            put_bool(w, crouched);
        }
        Command::AimTurret { entity, dir } => {
            put_u8(w, 14);
            put_entity(w, entity);
            put_vec2(w, dir);
        }
        Command::DriveHull { entity, dir } => {
            put_u8(w, 15);
            put_entity(w, entity);
            put_vec2(w, dir);
        }
        Command::SelectArmy { faction, army } => {
            put_u8(w, 16);
            put_u8(w, faction_tag(faction));
            put_u8(w, army_tag(army));
        }
        Command::SelectShell { entity, shell } => {
            put_u8(w, 17);
            put_entity(w, entity);
            put_u8(w, shell_tag(shell));
        }
        Command::SetCampRally { camp, rally } => {
            put_u8(w, 18);
            put_entity(w, camp);
            put_vec2(w, rally);
        }
    }
}

// ---- little-endian reader ----

pub(crate) struct Reader<'a> {
    buf: &'a [u8],
    pos: usize,
}

impl<'a> Reader<'a> {
    pub(crate) fn new(buf: &'a [u8]) -> Self {
        Reader { buf, pos: 0 }
    }

    pub(crate) fn at_end(&self) -> bool {
        self.pos >= self.buf.len()
    }

    fn take(&mut self, n: usize) -> Result<&'a [u8], ReplayError> {
        let end = self.pos.checked_add(n).ok_or(ReplayError::UnexpectedEof)?;
        let slice = self.buf.get(self.pos..end).ok_or(ReplayError::UnexpectedEof)?;
        self.pos = end;
        Ok(slice)
    }

    pub(crate) fn u8(&mut self) -> Result<u8, ReplayError> {
        Ok(self.take(1)?[0])
    }
    pub(crate) fn u16(&mut self) -> Result<u16, ReplayError> {
        Ok(u16::from_le_bytes(self.take(2)?.try_into().unwrap()))
    }
    pub(crate) fn u32(&mut self) -> Result<u32, ReplayError> {
        Ok(u32::from_le_bytes(self.take(4)?.try_into().unwrap()))
    }
    pub(crate) fn u64(&mut self) -> Result<u64, ReplayError> {
        Ok(u64::from_le_bytes(self.take(8)?.try_into().unwrap()))
    }
    fn i32(&mut self) -> Result<i32, ReplayError> {
        Ok(i32::from_le_bytes(self.take(4)?.try_into().unwrap()))
    }
    fn bool(&mut self) -> Result<bool, ReplayError> {
        Ok(self.u8()? != 0)
    }
    pub(crate) fn magic(&mut self) -> Result<[u8; 4], ReplayError> {
        Ok(self.take(4)?.try_into().unwrap())
    }
    fn fixed(&mut self) -> Result<Fixed, ReplayError> {
        Ok(Fixed::from_bits(self.i32()?))
    }
    fn vec2(&mut self) -> Result<Vec2, ReplayError> {
        Ok(Vec2::new(self.fixed()?, self.fixed()?))
    }
    fn entity(&mut self) -> Result<Entity, ReplayError> {
        Ok(Entity {
            index: self.u32()?,
            generation: self.u32()?,
        })
    }
    fn faction(&mut self) -> Result<Faction, ReplayError> {
        match self.u8()? {
            0 => Ok(Faction::Player),
            1 => Ok(Faction::Enemy),
            2 => Ok(Faction::Neutral),
            tag => Err(ReplayError::BadTag { what: "faction", tag }),
        }
    }
    fn building(&mut self) -> Result<BuildingKind, ReplayError> {
        match self.u8()? {
            0 => Ok(BuildingKind::Camp),
            1 => Ok(BuildingKind::Barracks),
            tag => Err(ReplayError::BadTag { what: "building", tag }),
        }
    }
    fn unit(&mut self) -> Result<UnitKind, ReplayError> {
        match self.u8()? {
            0 => Ok(UnitKind::Rifleman),
            1 => Ok(UnitKind::Heavy),
            2 => Ok(UnitKind::Tank),
            3 => Ok(UnitKind::Medic),
            4 => Ok(UnitKind::AntiTank),
            tag => Err(ReplayError::BadTag { what: "unit", tag }),
        }
    }
    fn stance(&mut self) -> Result<Stance, ReplayError> {
        match self.u8()? {
            0 => Ok(Stance::HoldFire),
            1 => Ok(Stance::ReturnFire),
            2 => Ok(Stance::FireAtWill),
            tag => Err(ReplayError::BadTag { what: "stance", tag }),
        }
    }
    fn army(&mut self) -> Result<Army, ReplayError> {
        match self.u8()? {
            0 => Ok(Army::Neutral),
            1 => Ok(Army::Us),
            2 => Ok(Army::Fr),
            tag => Err(ReplayError::BadTag { what: "army", tag }),
        }
    }
    fn shell(&mut self) -> Result<ShellKind, ReplayError> {
        match self.u8()? {
            0 => Ok(ShellKind::Ap),
            1 => Ok(ShellKind::Aphe),
            2 => Ok(ShellKind::He),
            tag => Err(ReplayError::BadTag { what: "shell", tag }),
        }
    }
    fn order(&mut self) -> Result<Order, ReplayError> {
        match self.u8()? {
            0 => Ok(Order::Idle),
            1 => Ok(Order::MoveTo(self.vec2()?)),
            2 => Ok(Order::AttackMove(self.vec2()?)),
            3 => Ok(Order::Patrol {
                a: self.vec2()?,
                b: self.vec2()?,
                toward_b: self.bool()?,
            }),
            4 => Ok(Order::HoldPosition),
            5 => Ok(Order::FallBack(self.vec2()?)),
            tag => Err(ReplayError::BadTag { what: "order", tag }),
        }
    }

    /// Decode one command. Errors (never panics) on an unknown tag or a short stream.
    pub(crate) fn command(&mut self) -> Result<Command, ReplayError> {
        Ok(match self.u8()? {
            0 => Command::Move {
                entity: self.entity()?,
                target: self.vec2()?,
            },
            1 => Command::AttackMove {
                entity: self.entity()?,
                target: self.vec2()?,
            },
            2 => Command::SetOrder {
                entity: self.entity()?,
                order: self.order()?,
            },
            3 => Command::SetStance {
                entity: self.entity()?,
                stance: self.stance()?,
            },
            4 => Command::SetRetreatThreshold {
                entity: self.entity()?,
                fraction: self.fixed()?,
            },
            5 => Command::Embody {
                entity: self.entity()?,
            },
            6 => Command::Surface {
                entity: self.entity()?,
            },
            7 => Command::Build {
                faction: self.faction()?,
                kind: self.building()?,
                pos: self.vec2()?,
            },
            8 => Command::Upgrade {
                camp: self.entity()?,
            },
            9 => Command::QueueProduction {
                camp: self.entity()?,
                unit: self.unit()?,
            },
            10 => Command::Fire {
                entity: self.entity()?,
                dir: self.vec2()?,
            },
            11 => Command::Locomote {
                entity: self.entity()?,
                dir: self.vec2()?,
            },
            12 => Command::Reload {
                entity: self.entity()?,
            },
            13 => Command::Crouch {
                entity: self.entity()?,
                crouched: self.bool()?,
            },
            14 => Command::AimTurret {
                entity: self.entity()?,
                dir: self.vec2()?,
            },
            15 => Command::DriveHull {
                entity: self.entity()?,
                dir: self.vec2()?,
            },
            16 => Command::SelectArmy {
                faction: self.faction()?,
                army: self.army()?,
            },
            17 => Command::SelectShell {
                entity: self.entity()?,
                shell: self.shell()?,
            },
            18 => Command::SetCampRally {
                camp: self.entity()?,
                rally: self.vec2()?,
            },
            tag => return Err(ReplayError::BadTag { what: "command", tag }),
        })
    }
}
