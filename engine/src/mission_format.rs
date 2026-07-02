//! CT-B — the RON **mission format** + its host-side **float-airlock** loader ([D76]).
//!
//! This module is the data layer the content-tooling plan calls for: a designer authors a
//! `*.mission.ron` file, and this loader turns it into a seeded [`Sim`] by driving the serde-free
//! CT-A [`ScenarioBuilder`](gonedark_core::scenario::ScenarioBuilder). It is the **only** place a
//! text number becomes a sim number, and it is the load-bearing boundary that keeps two invariants
//! intact:
//!
//! - **The float airlock (invariant #1).** Every numeric field in the schema is an **integer** —
//!   cells (whole world units), whole degrees, whole seconds, fixed-point **milli-units** for
//!   distances. There is **no `f32`/`f64` anywhere in the type graph from file to sim**: the loader
//!   converts integer → [`Fixed`] via [`Fixed::from_int`] / [`Fixed::from_ratio`], and a whole
//!   turn maps to [`ANGLE_FULL`] with exact integer arithmetic. A float literal in the RON cannot
//!   even *deserialize* into an integer field — it fails loudly at parse.
//! - **`core` stays serde-free (invariant #2).** serde and RON live in `engine`; the spec types
//!   here MIRROR the `core` vocabulary (`Faction`/`Army`/`UnitKind`/`Stance`/`Difficulty`) as local
//!   `Deserialize` enums that map onto the real `core` types, so `core` gains no dependency.
//!
//! The loader **range-validates and fails LOUD** ([`MissionLoadError`]) — it never silently clamps
//! a bad coordinate or dangling reference into a playable-but-wrong mission. Because it drives the
//! CT-A builder, the seeded `Sim` rides the *exact* same per-tick checksum footing as a
//! hand-written seeder (invariant #7); the RON file itself never enters the checksum.
//!
//! ## The proof obligation
//!
//! [`missions/seize.mission.ron`] is a faithful re-expression of
//! [`seed_seize_mission`](gonedark_core::scenario::seed_seize_mission): loaded, its opening
//! checksum is **byte-identical** to the code-built *Seize* (pinned in the tests against the CT-A
//! golden `0x474cdbf2ad913ecb`). The format is a second *spelling* of a mission, not a second code
//! path.
//!
//! [D76]: ../../../docs/decisions.md
//! [`missions/seize.mission.ron`]: ../../../missions/seize.mission.ron

use std::fmt;

use serde::Deserialize;

use gonedark_core::components::{Army, Faction, Stance, UnitKind, Vec2};
use gonedark_core::ecs::Entity;
use gonedark_core::fixed::Fixed;
use gonedark_core::flow_field::HALF_EXTENT;
use gonedark_core::mission_tuning::Difficulty;
use gonedark_core::scenario::ScenarioBuilder;
use gonedark_core::sim::{Sim, TICK_HZ};
use gonedark_core::trig::{Angle, ANGLE_FULL};

use crate::objectives::{Objective, ObjectiveSet};

// ---- the schema (all integer, all serde-mirrors of the `core` vocabulary) ----------------------

/// A cell / world coordinate pair — whole world units (`CELL_SIZE == 1`), never a float. RON writes
/// it as a tuple `(x, y)`.
type Cell = (i32, i32);

/// A whole authored mission. `#[serde(deny_unknown_fields)]` so a typo'd or stray key fails loudly
/// at parse rather than being silently ignored. Every numeric field is an integer — the float
/// airlock is the *type graph itself* (invariant #1).
#[derive(Deserialize, Debug, Clone, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct MissionSpec {
    /// The mission's stable identity — the `u32` a campaign node names it by, mapped to a
    /// [`MissionId`](gonedark_core::campaign::MissionId) by the data-backed registry (CT-D). It is
    /// **not** a sim number and never reaches the checksum; it only wires the authored campaign graph
    /// to its content file. `#[serde(default)]` so a file that omits it still parses (as id `0`, the
    /// unassigned sentinel) — a designer opts in by giving the mission a real, non-zero id.
    #[serde(default)]
    pub id: u32,
    /// The battlefield this mission is fought on — a string id resolved by the map format (CT-C).
    /// CT-B only **carries** the reference; a blank id is rejected here, but full cross-file
    /// resolution against a map registry is CT-C/CT-D/CT-F's job.
    pub map: String,
    /// Income accrual period in ticks (the scenario-local economy *pace* lever). Must be `>= 1`.
    pub income_period: u32,
    /// Every faction's uniform starting purse. Must be `>= 0`.
    pub starting_purse: i64,
    /// Which real-army roster each side fields.
    pub armies: ArmiesSpec,
    /// Neutral control points to fight over (empty for a fixed-force assault like *Seize*).
    #[serde(default)]
    pub control_points: Vec<Cell>,
    /// The opening force, in **spawn order** — the loader spawns them in list order, so the ECS
    /// slot layout (and thus the checksum stream) is exactly the authored order.
    pub forces: Vec<ForceSpec>,
    /// The host-side objective set that watches the match (never folded into the sim).
    pub objectives: Vec<ObjectiveSpec>,
    /// The commander difficulty tier this mission is briefed at.
    pub difficulty: DifficultySpec,
    /// Light narrative framing ([Q16] keeps depth deferred).
    pub briefing: BriefingSpec,
}

/// Which [`Army`] each side fields. Neutral is left at the `Sim` default (unset).
#[derive(Deserialize, Debug, Clone, Copy, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ArmiesSpec {
    pub player: ArmySpec,
    pub enemy: ArmySpec,
}

/// Light per-node briefing text (owned `String`s — the host renders them; unlike `core`'s
/// `&'static str` [`Briefing`](gonedark_core::mission_tuning::Briefing), which is compile-time).
#[derive(Deserialize, Debug, Clone, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct BriefingSpec {
    pub title: String,
    pub situation: String,
    pub objective_line: String,
}

/// One opening-force entry. A `Unit` draws its HP/weapon from the faction's per-[`Army`] roster
/// (exactly like the hand seeders), so no combat number is authorable here — invariant #1 keeps the
/// D30 balance out of content. A `Camp` is built operational through the canonical build path.
#[derive(Deserialize, Debug, Clone, PartialEq, Eq)]
pub enum ForceSpec {
    /// A unit at `cell`, facing `facing_deg` whole degrees (`0` = +X / east, `180` = −X / west).
    Unit {
        kind: UnitKindSpec,
        faction: FactionSpec,
        cell: Cell,
        stance: StanceSpec,
        facing_deg: i32,
    },
    /// An operational base camp at `cell` for `faction`.
    Camp { faction: FactionSpec, cell: Cell },
}

impl ForceSpec {
    fn faction(&self) -> FactionSpec {
        match self {
            ForceSpec::Unit { faction, .. } | ForceSpec::Camp { faction, .. } => *faction,
        }
    }

    fn cell(&self) -> Cell {
        match self {
            ForceSpec::Unit { cell, .. } | ForceSpec::Camp { cell, .. } => *cell,
        }
    }
}

/// The authorable objective vocabulary — a serde mirror of [`ObjectiveKind`](crate::objectives::ObjectiveKind).
/// Entity-targeting variants reference a force by its **index in `forces`** (a stable, authorable
/// handle); a dangling index fails at load.
#[derive(Deserialize, Debug, Clone, PartialEq, Eq)]
pub enum ObjectiveSpec {
    /// Flip the control point at `point` to `who`. `point` must be a declared `control_point`.
    Capture {
        owner: FactionSpec,
        who: FactionSpec,
        point: Cell,
        label: String,
    },
    /// Wipe out the `target` faction (its goal — destroyable strength — is computed from `forces`).
    EliminateFaction {
        owner: FactionSpec,
        target: FactionSpec,
        label: String,
    },
    /// Kill a single VIP force (by its index in `forces`).
    EliminateEntity {
        owner: FactionSpec,
        target_force: usize,
        label: String,
    },
    /// Keep `who` alive for `seconds` whole seconds (converted to ticks at the locked 60 Hz).
    Survive {
        who: FactionSpec,
        seconds: u64,
        label: String,
    },
    /// Move force `who` within `radius_mu` **milli-units** of `dest`.
    Reach {
        owner: FactionSpec,
        who: usize,
        dest: Cell,
        radius_mu: i32,
        label: String,
    },
    /// Escort force `vip` (alive) within `radius_mu` **milli-units** of `dest`.
    Escort {
        owner: FactionSpec,
        vip: usize,
        dest: Cell,
        radius_mu: i32,
        label: String,
    },
}

/// serde mirror of [`Faction`]. Local so `core` stays serde-free (invariant #2).
#[derive(Deserialize, Debug, Clone, Copy, PartialEq, Eq)]
pub enum FactionSpec {
    Player,
    Enemy,
    Neutral,
}

impl FactionSpec {
    fn to_core(self) -> Faction {
        match self {
            FactionSpec::Player => Faction::Player,
            FactionSpec::Enemy => Faction::Enemy,
            FactionSpec::Neutral => Faction::Neutral,
        }
    }
}

/// serde mirror of [`Army`].
#[derive(Deserialize, Debug, Clone, Copy, PartialEq, Eq)]
pub enum ArmySpec {
    Neutral,
    Us,
    Fr,
}

impl ArmySpec {
    fn to_core(self) -> Army {
        match self {
            ArmySpec::Neutral => Army::Neutral,
            ArmySpec::Us => Army::Us,
            ArmySpec::Fr => Army::Fr,
        }
    }
}

/// serde mirror of [`UnitKind`].
#[derive(Deserialize, Debug, Clone, Copy, PartialEq, Eq)]
pub enum UnitKindSpec {
    Rifleman,
    Heavy,
    Tank,
    Medic,
    AntiTank,
}

impl UnitKindSpec {
    fn to_core(self) -> UnitKind {
        match self {
            UnitKindSpec::Rifleman => UnitKind::Rifleman,
            UnitKindSpec::Heavy => UnitKind::Heavy,
            UnitKindSpec::Tank => UnitKind::Tank,
            UnitKindSpec::Medic => UnitKind::Medic,
            UnitKindSpec::AntiTank => UnitKind::AntiTank,
        }
    }
}

/// serde mirror of [`Stance`].
#[derive(Deserialize, Debug, Clone, Copy, PartialEq, Eq)]
pub enum StanceSpec {
    HoldFire,
    ReturnFire,
    FireAtWill,
}

impl StanceSpec {
    fn to_core(self) -> Stance {
        match self {
            StanceSpec::HoldFire => Stance::HoldFire,
            StanceSpec::ReturnFire => Stance::ReturnFire,
            StanceSpec::FireAtWill => Stance::FireAtWill,
        }
    }
}

/// serde mirror of [`Difficulty`].
#[derive(Deserialize, Debug, Clone, Copy, PartialEq, Eq)]
pub enum DifficultySpec {
    Recruit,
    Veteran,
    Elite,
}

impl DifficultySpec {
    fn to_core(self) -> Difficulty {
        match self {
            DifficultySpec::Recruit => Difficulty::Recruit,
            DifficultySpec::Veteran => Difficulty::Veteran,
            DifficultySpec::Elite => Difficulty::Elite,
        }
    }
}

// ---- the loud, typed failure ------------------------------------------------------------------

/// Why a mission failed to load. There is no silent-clamp path: a bad file becomes one of these,
/// host-side, before any `Sim` is seeded — never a quietly-wrong mission (the fail-loud discipline).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MissionLoadError {
    /// The RON text did not parse into the schema — a syntax error, a **float literal in an integer
    /// field** (the airlock's first line of defence), an unknown field (`deny_unknown_fields`), or a
    /// bad enum variant. Carries the deserializer's own message.
    Parse(String),
    /// The spec parsed but is semantically invalid — an out-of-range value or a dangling reference.
    Validation(String),
}

impl fmt::Display for MissionLoadError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            MissionLoadError::Parse(m) => write!(f, "mission parse error: {m}"),
            MissionLoadError::Validation(m) => write!(f, "mission validation error: {m}"),
        }
    }
}

impl std::error::Error for MissionLoadError {}

// ---- the loaded result ------------------------------------------------------------------------

/// The runnable result of loading a mission onto a `Sim`: the spawned entities (in authored spawn
/// order, so `forces[i]` is the entity for spec `forces[i]`), the host-side objective set, and the
/// carried difficulty + briefing. Carries no `Sim` — pure session/presentation data (invariant #7).
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct LoadedMission {
    /// The spawned entities, index-aligned with [`MissionSpec::forces`].
    pub forces: Vec<Entity>,
    /// The host-side objective set watching this match.
    pub objectives: ObjectiveSet,
    /// The commander difficulty tier the mission is briefed at (a host-side planning knob).
    pub difficulty: Difficulty,
    /// The briefing text (owned, host-rendered).
    pub briefing: BriefingSpec,
}

// ---- parse + validate + build -----------------------------------------------------------------

/// Parse RON text into a [`MissionSpec`]. This is the airlock's first gate: because every numeric
/// field is an integer type, a float literal (`600.5`) or an unknown/typo'd field cannot
/// deserialize and fails here with the deserializer's message — no float can reach the sim.
pub fn parse_mission(src: &str) -> Result<MissionSpec, MissionLoadError> {
    ron::from_str::<MissionSpec>(src).map_err(|e| MissionLoadError::Parse(e.to_string()))
}

/// The largest in-bounds world coordinate (exclusive): the playfield spans
/// `[-HALF_EXTENT, HALF_EXTENT)` world units ([`HALF_EXTENT`]). A coordinate at or past this would
/// be *silently clamped* into the grid by `terrain`, so the loader rejects it instead (fail-loud).
fn world_half_extent() -> i32 {
    HALF_EXTENT.to_int()
}

/// Whole degrees → a fixed-point [`Angle`] in binary radians, exact integer arithmetic (invariant
/// #1). `0° → Angle(0)` (+X), `180° → Angle(ANGLE_FULL/2)` (−X). No float, no transcendental.
fn angle_from_degrees(deg: i32) -> Angle {
    // deg ∈ [0, 360) validated by the caller; i64 intermediate so the multiply cannot overflow.
    let units = (deg as i64 * ANGLE_FULL as i64) / 360;
    Angle(units as i32)
}

/// A cell coordinate → a world [`Vec2`] (whole world units; `CELL_SIZE == 1`), via
/// [`Fixed::from_int`] only — the integer→`Fixed` airlock (invariant #1).
fn cell_to_world(cell: Cell) -> Vec2 {
    Vec2::new(Fixed::from_int(cell.0), Fixed::from_int(cell.1))
}

/// Validate a [`MissionSpec`] — range-check every value and resolve every reference — WITHOUT
/// touching a `Sim`. Pure logic (the repo's "extract a testable seam" convention), so the rejection
/// battery can exercise it directly. Returns the first problem found, loudly.
pub fn validate(spec: &MissionSpec) -> Result<(), MissionLoadError> {
    let err = |m: String| Err(MissionLoadError::Validation(m));

    if spec.map.trim().is_empty() {
        return err("`map` reference is empty (a mission must name its battlefield)".into());
    }
    if spec.income_period < 1 {
        return err("`income_period` must be >= 1 (0 would stall income accrual)".into());
    }
    if spec.starting_purse < 0 {
        return err(format!("`starting_purse` must be >= 0 (got {})", spec.starting_purse));
    }

    let half = world_half_extent();
    let in_bounds = |c: Cell| -half <= c.0 && c.0 < half && -half <= c.1 && c.1 < half;
    let bounds_msg = |what: &str, c: Cell| {
        format!(
            "{what} cell {c:?} is out of bounds (each axis must be in [{}, {}))",
            -half, half
        )
    };

    // Control points must be in-bounds.
    for &cp in &spec.control_points {
        if !in_bounds(cp) {
            return err(bounds_msg("control_point", cp));
        }
    }

    // Every force: in-bounds cell, and (for a Unit) a legal facing.
    for (i, f) in spec.forces.iter().enumerate() {
        if !in_bounds(f.cell()) {
            return err(bounds_msg(&format!("forces[{i}]"), f.cell()));
        }
        if let ForceSpec::Unit { facing_deg, .. } = f {
            if !(0..360).contains(facing_deg) {
                return err(format!(
                    "forces[{i}] facing_deg {facing_deg} out of range (whole degrees in [0, 360))"
                ));
            }
        }
    }

    // Objectives: resolve every reference (dangling force index / control point / empty target).
    let force_count = spec.forces.len();
    let force_ref = |what: &str, idx: usize| -> Result<(), MissionLoadError> {
        if idx >= force_count {
            return Err(MissionLoadError::Validation(format!(
                "{what} references force index {idx}, but only {force_count} forces are declared"
            )));
        }
        Ok(())
    };
    for (i, o) in spec.objectives.iter().enumerate() {
        match o {
            ObjectiveSpec::Capture { point, .. } => {
                if !in_bounds(*point) {
                    return err(bounds_msg(&format!("objectives[{i}] Capture point"), *point));
                }
                if !spec.control_points.contains(point) {
                    return err(format!(
                        "objectives[{i}] Capture targets {point:?}, which is not a declared control_point"
                    ));
                }
            }
            ObjectiveSpec::EliminateFaction { target, .. } => {
                let target = *target;
                if !spec.forces.iter().any(|f| f.faction() == target) {
                    return err(format!(
                        "objectives[{i}] EliminateFaction targets {target:?}, which fields no forces"
                    ));
                }
            }
            ObjectiveSpec::EliminateEntity { target_force, .. } => {
                force_ref(&format!("objectives[{i}] EliminateEntity"), *target_force)?;
            }
            ObjectiveSpec::Survive { .. } => {}
            ObjectiveSpec::Reach { who, dest, radius_mu, .. } => {
                force_ref(&format!("objectives[{i}] Reach"), *who)?;
                if !in_bounds(*dest) {
                    return err(bounds_msg(&format!("objectives[{i}] Reach dest"), *dest));
                }
                if *radius_mu < 0 {
                    return err(format!("objectives[{i}] Reach radius_mu must be >= 0"));
                }
            }
            ObjectiveSpec::Escort { vip, dest, radius_mu, .. } => {
                force_ref(&format!("objectives[{i}] Escort"), *vip)?;
                if !in_bounds(*dest) {
                    return err(bounds_msg(&format!("objectives[{i}] Escort dest"), *dest));
                }
                if *radius_mu < 0 {
                    return err(format!("objectives[{i}] Escort radius_mu must be >= 0"));
                }
            }
        }
    }

    Ok(())
}

/// Load a validated [`MissionSpec`] onto `sim`, driving the CT-A [`ScenarioBuilder`]. The seeding
/// order mirrors the hand seeders exactly: set the pace + armies, spawn the forces in authored
/// order, then set the uniform purse — so the seeded `Sim` is byte-identical to the equivalent
/// hand-written mission (the *Seize* oracle proves it). Returns the runnable [`LoadedMission`], or a
/// [`MissionLoadError`] if the spec is invalid (nothing is spawned on an error path *before*
/// validation; validation runs first).
pub fn load_mission(spec: &MissionSpec, sim: &mut Sim) -> Result<LoadedMission, MissionLoadError> {
    validate(spec)?;

    let mut b = ScenarioBuilder::new(sim);
    // Economy pace + army identity BEFORE any spawn, so each unit's per-army roster read
    // (`unit_stats_for`) sees the right army — the hand-seeder ordering.
    b.set_income(spec.income_period);
    b.set_army(Faction::Player, spec.armies.player.to_core());
    b.set_army(Faction::Enemy, spec.armies.enemy.to_core());

    // Neutral control points to fight over.
    for &cp in &spec.control_points {
        b.control_point(cell_to_world(cp));
    }

    // The opening force, in authored spawn order → stable ECS slot layout (invariant #1/#7).
    let mut forces = Vec::with_capacity(spec.forces.len());
    for f in &spec.forces {
        let e = match f {
            ForceSpec::Unit {
                kind,
                faction,
                cell,
                stance,
                facing_deg,
            } => b.spawn(
                kind.to_core(),
                cell_to_world(*cell),
                faction.to_core(),
                stance.to_core(),
                angle_from_degrees(*facing_deg),
            ),
            ForceSpec::Camp { faction, cell } => {
                b.build_camp(cell_to_world(*cell), faction.to_core())
            }
        };
        forces.push(e);
    }

    // The uniform scenario purse, applied last (it overwrites every faction, wiping any temporary
    // funding `build_camp` left behind — the hand-seeder temp-purse dance). Spawns never touch
    // resources, so applying it here is byte-identical to the interleaved hand-seeder ordering.
    b.set_purse(spec.starting_purse);

    let objectives = build_objectives(spec, &forces);

    Ok(LoadedMission {
        forces,
        objectives,
        difficulty: spec.difficulty.to_core(),
        briefing: spec.briefing.clone(),
    })
}

/// Build the host-side [`ObjectiveSet`] from the spec + the spawned entity handles. `forces` is
/// assumed index-valid (guaranteed by [`validate`], which the only caller runs first).
fn build_objectives(spec: &MissionSpec, forces: &[Entity]) -> ObjectiveSet {
    let mut out = Vec::with_capacity(spec.objectives.len());
    for o in &spec.objectives {
        let obj = match o {
            ObjectiveSpec::Capture { owner, who, point, label } => Objective::capture(
                owner.to_core(),
                who.to_core(),
                cell_to_world(*point),
                label.clone(),
            ),
            ObjectiveSpec::EliminateFaction { owner, target, label } => {
                // Goal = the target faction's destroyable strength (units + camps), the HUD bar.
                let goal = spec
                    .forces
                    .iter()
                    .filter(|f| f.faction() == *target)
                    .count() as u32;
                Objective::eliminate_faction(owner.to_core(), target.to_core(), label.clone(), goal)
            }
            ObjectiveSpec::EliminateEntity { owner, target_force, label } => {
                Objective::eliminate_entity(owner.to_core(), forces[*target_force], label.clone())
            }
            ObjectiveSpec::Survive { who, seconds, label } => Objective::survive(
                who.to_core(),
                seconds * TICK_HZ as u64,
                label.clone(),
            ),
            ObjectiveSpec::Reach { owner, who, dest, radius_mu, label } => Objective::reach(
                owner.to_core(),
                forces[*who],
                cell_to_world(*dest),
                Fixed::from_ratio(*radius_mu, 1000),
                label.clone(),
            ),
            ObjectiveSpec::Escort { owner, vip, dest, radius_mu, label } => Objective::escort(
                owner.to_core(),
                forces[*vip],
                cell_to_world(*dest),
                Fixed::from_ratio(*radius_mu, 1000),
                label.clone(),
            ),
        };
        out.push(obj);
    }
    ObjectiveSet::new(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use gonedark_core::scenario::seed_seize_mission;

    /// The shipped *Seize* mission file, loaded and compiled into the test binary so the oracle runs
    /// with zero filesystem dependency (and CI needs no working directory).
    const SEIZE_RON: &str = include_str!("../../missions/seize.mission.ron");

    /// The CT-A golden opening checksum for *Seize* (`core::scenario` tests pin the same value). The
    /// data-loaded mission must reproduce it byte-for-byte.
    const SEIZE_OPENING_GOLDEN: u64 = 0x474c_dbf2_ad91_3ecb;

    /// The seed the CT-A golden was captured under (`core::scenario::tests::fresh`).
    fn fresh() -> Sim {
        Sim::new(0xD0E1)
    }

    // ---- schema round-trip --------------------------------------------------------------------

    #[test]
    fn seize_ron_parses_into_the_schema() {
        let spec = parse_mission(SEIZE_RON).expect("the shipped Seize file parses");
        assert_eq!(spec.map, "seize_outpost");
        assert_eq!(spec.income_period, 600);
        assert_eq!(spec.starting_purse, 0);
        assert_eq!(spec.armies, ArmiesSpec { player: ArmySpec::Us, enemy: ArmySpec::Fr });
        // 10 player troops + 1 enemy camp + 4 garrison = 15 forces, in spawn order.
        assert_eq!(spec.forces.len(), 15);
        assert_eq!(spec.difficulty, DifficultySpec::Recruit);
        assert!(validate(&spec).is_ok(), "the shipped file is semantically valid");
    }

    // ---- THE byte-identical Seize oracle ------------------------------------------------------

    /// The load-bearing CT-B proof: the *Seize* mission LOADED FROM RON seeds a `Sim` whose opening
    /// checksum is **byte-identical** to the code-built `seed_seize_mission` — AND matches the CT-A
    /// golden. The RON file is a faithful re-expression, not a second code path. A drift in the
    /// loader (spawn order, the integer→`Fixed` airlock, the temp-purse dance) would break this.
    #[test]
    fn seize_loaded_from_ron_is_byte_identical_to_the_code_built_mission() {
        // Code-built oracle.
        let mut code = fresh();
        seed_seize_mission(&mut code);
        let code_cs = code.checksum();

        // Data-built, same seed.
        let mut data = fresh();
        let spec = parse_mission(SEIZE_RON).expect("parse");
        let loaded = load_mission(&spec, &mut data).expect("load");
        let data_cs = data.checksum();

        assert_eq!(
            data_cs, code_cs,
            "data-loaded Seize must be byte-identical to the code-built mission"
        );
        assert_eq!(
            data_cs, SEIZE_OPENING_GOLDEN,
            "…and must match the CT-A golden 0x474cdbf2ad913ecb"
        );

        // The loaded mission is runnable: 15 entities in spawn order + one required objective.
        assert_eq!(loaded.forces.len(), 15);
        assert_eq!(loaded.objectives.objectives.len(), 1);
        assert_eq!(loaded.difficulty, Difficulty::Recruit);
        // Enemy destroyable strength (4 garrison + 1 camp) is the objective goal.
        assert_eq!(loaded.objectives.objectives[0].progress.goal, 5);
    }

    /// The airlock is degree-exact where it must be: the two facings *Seize* uses map to the exact
    /// `Angle` values the hand seeder writes (`Angle(0)` and `Angle(ANGLE_FULL/2)`), with no float.
    #[test]
    fn degrees_map_to_exact_binary_radian_angles() {
        assert_eq!(angle_from_degrees(0), Angle(0));
        assert_eq!(angle_from_degrees(180), Angle(ANGLE_FULL / 2));
        assert_eq!(angle_from_degrees(90), Angle(ANGLE_FULL / 4));
        assert_eq!(angle_from_degrees(270), Angle(ANGLE_FULL * 3 / 4));
    }

    // ---- the rejection battery (fail LOUD, never silently clamp) ------------------------------

    /// Helper: parse-then-load, returning whatever error fires first.
    fn try_load(src: &str) -> Result<LoadedMission, MissionLoadError> {
        let spec = parse_mission(src)?;
        let mut sim = fresh();
        load_mission(&spec, &mut sim)
    }

    /// A minimal-but-valid mission template the rejection tests mutate one field at a time.
    fn valid_ron() -> String {
        r#"MissionSpec(
    map: "test",
    income_period: 600,
    starting_purse: 0,
    armies: (player: Us, enemy: Fr),
    control_points: [],
    forces: [
        Unit(kind: Rifleman, faction: Player, cell: (-10, 0), stance: FireAtWill, facing_deg: 0),
        Camp(faction: Enemy, cell: (10, 0)),
    ],
    objectives: [
        EliminateFaction(owner: Player, target: Enemy, label: "Take it"),
    ],
    difficulty: Recruit,
    briefing: (title: "T", situation: "S", objective_line: "O"),
)"#
        .to_string()
    }

    #[test]
    fn the_template_itself_loads() {
        assert!(try_load(&valid_ron()).is_ok(), "the rejection-test template must be valid");
    }

    #[test]
    fn rejects_a_float_literal_in_a_numeric_field() {
        // The airlock's core promise: a float where an integer is expected cannot deserialize.
        let src = valid_ron().replace("income_period: 600,", "income_period: 600.5,");
        let e = try_load(&src).expect_err("a float literal must be rejected");
        assert!(matches!(e, MissionLoadError::Parse(_)), "float → a parse-time airlock failure");
    }

    #[test]
    fn rejects_a_float_literal_in_a_cell_coordinate() {
        // Even nested inside a tuple, a float cannot deserialize into an i32 cell axis.
        let src = valid_ron().replace("cell: (-10, 0)", "cell: (-10.0, 0)");
        let e = try_load(&src).expect_err("a float cell axis must be rejected");
        assert!(matches!(e, MissionLoadError::Parse(_)));
    }

    #[test]
    fn rejects_an_unknown_field() {
        let src = valid_ron().replace("map: \"test\",", "map: \"test\",\n    sneaky: 1,");
        let e = try_load(&src).expect_err("deny_unknown_fields must reject a stray key");
        assert!(matches!(e, MissionLoadError::Parse(_)));
    }

    #[test]
    fn rejects_an_out_of_range_cell() {
        // 200 is well past HALF_EXTENT (64) — silently clamping it would place the unit wrong, so
        // the loader fails loudly instead.
        let src = valid_ron().replace("cell: (10, 0)", "cell: (200, 0)");
        let e = try_load(&src).expect_err("an out-of-bounds cell must be rejected");
        match e {
            MissionLoadError::Validation(m) => assert!(m.contains("out of bounds"), "clear msg: {m}"),
            other => panic!("expected a validation error, got {other:?}"),
        }
    }

    #[test]
    fn rejects_a_dangling_eliminate_entity_force_ref() {
        let src = valid_ron().replace(
            "EliminateFaction(owner: Player, target: Enemy, label: \"Take it\"),",
            "EliminateEntity(owner: Player, target_force: 99, label: \"Kill VIP\"),",
        );
        let e = try_load(&src).expect_err("a dangling force index must be rejected");
        match e {
            MissionLoadError::Validation(m) => assert!(m.contains("index 99"), "clear msg: {m}"),
            other => panic!("expected a validation error, got {other:?}"),
        }
    }

    #[test]
    fn rejects_a_dangling_capture_point() {
        // A Capture objective whose point is not among the declared control_points.
        let src = valid_ron().replace(
            "EliminateFaction(owner: Player, target: Enemy, label: \"Take it\"),",
            "Capture(owner: Player, who: Player, point: (5, 5), label: \"Cap\"),",
        );
        let e = try_load(&src).expect_err("a Capture with no matching control_point must be rejected");
        match e {
            MissionLoadError::Validation(m) => {
                assert!(m.contains("not a declared control_point"), "clear msg: {m}")
            }
            other => panic!("expected a validation error, got {other:?}"),
        }
    }

    #[test]
    fn rejects_a_blank_map_reference() {
        let src = valid_ron().replace("map: \"test\",", "map: \"\",");
        let e = try_load(&src).expect_err("a blank map ref must be rejected");
        assert!(matches!(e, MissionLoadError::Validation(m) if m.contains("map")));
    }

    #[test]
    fn rejects_an_out_of_range_facing() {
        let src = valid_ron().replace("facing_deg: 0)", "facing_deg: 360)");
        let e = try_load(&src).expect_err("facing must be in [0, 360)");
        assert!(matches!(e, MissionLoadError::Validation(m) if m.contains("facing_deg")));
    }

    #[test]
    fn rejects_eliminate_faction_with_no_forces() {
        // Target a faction (Neutral) that fields no forces — a dangling objective target.
        let src = valid_ron().replace("target: Enemy,", "target: Neutral,");
        let e = try_load(&src).expect_err("an EliminateFaction with no target forces must be rejected");
        assert!(matches!(e, MissionLoadError::Validation(m) if m.contains("fields no forces")));
    }
}
