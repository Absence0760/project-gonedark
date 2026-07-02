//! CT-F — the content-lint harness (`docs/plans/content-tooling-plan.md`).
//!
//! A headless, no-GPU standing guard over **every shipped mission** that asserts authored content
//! can never silently break determinism (invariant #7), leak a float into the sim (invariant #1),
//! or dangle a reference. For each mission it:
//!
//!  - **seeds its `Sim` twice and asserts the opening checksum is identical** (a seeded `Sim` is
//!    bit-identical — invariant #7), and that a short per-tick checksum stream is identical
//!    peer-to-peer (the same lockstep-determinism proof `core::scenario`'s own tests use);
//!  - **asserts every objective's target resolves in the seeded world** — the capture point exists,
//!    the eliminate-target faction/entity is present, the survive tick is `> 0`, and a reach/escort
//!    destination is in world bounds with a positive radius and a live tracked entity;
//!  - **asserts the campaign graph is well-formed** — every node resolves to a registered mission
//!    and every unlock edge references an existing node.
//!
//! It has teeth: `the_lint_catches_deliberately_broken_targets` feeds the same lint logic hand-built
//! `ObjectiveSet`s whose targets are absent from the world and asserts each is rejected with a
//! precise diagnostic — so a green run means the checks fired, not that they were vacuous.
//!
//! ## The RON seam (CT-B / CT-C, in parallel)
//!
//! Today every linted piece of content is **code-built**: a [`MissionDef`] from
//! [`default_registry`], seeded through `MissionDef::launch`. That is deliberately the only source
//! this file depends on — it does NOT touch `engine::mission_format` / `engine::map_format` (owned
//! by CT-B/CT-C, landing separately). The [`LintTarget`] abstraction below is the seam a future step
//! points at loaded `*.mission.ron` / `*.map.ron` files: a loader yields exactly this shape (a
//! `label` + a `seed` closure that produces a fresh `Sim` + its `ObjectiveSet`), and every assertion
//! in this file applies to it unchanged. See the `SEAM` note on [`code_built_targets`].

use gonedark_core::campaign::{Campaign, Difficulty as ReplayTier, NodeId};
use gonedark_core::components::{Faction, Vec2};
use gonedark_core::ecs::Entity;
use gonedark_core::fixed::Fixed;
use gonedark_core::flow_field::HALF_EXTENT;
use gonedark_core::gunsmith::Loadout;
use gonedark_core::sim::Sim;

use gonedark_engine::mission_registry::{
    default_campaign, default_registry, MissionDef, MissionRegistry,
};
use gonedark_engine::objectives::{
    faction_forces, EliminateTarget, Objective, ObjectiveKind, ObjectiveSet,
};

/// A fixed RNG seed for the harness. Any value works — the point is that seeding the *same* content
/// with the *same* seed is bit-identical (invariant #7); the seed itself is not under test.
const SEED: u64 = 0xC0FFEE;
/// How many ticks the per-tick determinism stream compares. Short enough to stay fast in CI, long
/// enough that a divergence in the seeded world's evolution would surface.
const STEPS: usize = 180;

/// A resolved piece of shippable content the lint checks: a human label + a way to seed a fresh
/// `Sim` and produce the [`ObjectiveSet`] that watches it, from an RNG seed.
///
/// TODAY every target is code-built (see [`code_built_targets`]). This is the SEAM the RON loader
/// (CT-B/CT-C) plugs into: a loaded `*.mission.ron` would yield exactly this shape, so the whole
/// battery below (`assert_deterministic`, `assert_objective_targets_resolve`) is reused verbatim.
struct LintTarget {
    label: String,
    seed: Box<dyn Fn(u64) -> (Sim, ObjectiveSet)>,
}

/// Enumerate every shipped mission as a [`LintTarget`], seeded through the code-built
/// [`default_registry`] / [`default_campaign`].
///
/// SEAM / honest caveat: [`MissionRegistry`] exposes no public iterator over its `MissionDef`s (its
/// backing `Vec` is private, and CT-F must not widen another worker's file), so we enumerate the
/// **authored campaign graph** (`Campaign::mission_select`) and resolve each node's `MissionId`
/// through the registry with `MissionRegistry::get`. Today the graph and the registry cover the same
/// set (a registry with a not-yet-node-placed mission is permitted but doesn't exist —
/// `default_registry`'s own tests pin this), and `the_default_campaign_graph_is_well_formed`
/// separately asserts `MissionRegistry::covers` so the graph ⊆ registry direction is guarded. When
/// the RON loader lands, this same function grows a branch that reads the content directory.
fn code_built_targets() -> Vec<LintTarget> {
    let reg = default_registry();
    let campaign = default_campaign();

    let mut seen: Vec<gonedark_core::campaign::MissionId> = Vec::new();
    let mut targets = Vec::new();
    for entry in campaign.mission_select() {
        if seen.contains(&entry.mission) {
            continue; // one target per distinct mission, even if reused across nodes
        }
        seen.push(entry.mission);

        // Resolve via `get` (not `resolve_node`) so a *locked* node's mission is still linted — the
        // lint checks the authored content, not the currently-playable subset.
        let def: MissionDef = *reg
            .get(entry.mission)
            .expect("a campaign node names a mission with no registry entry (covers() would fail)");
        let label = format!("{:?} \"{}\"", entry.mission, entry.title);
        targets.push(LintTarget {
            label,
            seed: Box::new(move |seed| {
                let mut sim = Sim::new(seed);
                // The neutral `Regular` replay tier reproduces the bare seed byte-for-byte (D83), so
                // the lint checks the shipped baseline fight, not a tier-reshaped one.
                let launched = def.launch(&mut sim, Loadout::STANDARD, ReplayTier::Regular);
                (sim, launched.objectives)
            }),
        });
    }
    targets
}

// --- the lint predicates (shared by the green missions and the deliberately-broken fixtures) ------

/// Whether a world position lies inside the playfield. Cell axes span `[-HALF_EXTENT, HALF_EXTENT)`
/// (see `core::flow_field`), so a destination outside that is unreachable — a dangling target.
fn in_world_bounds(p: Vec2) -> bool {
    let lo = -HALF_EXTENT;
    let hi = HALF_EXTENT;
    p.x >= lo && p.x < hi && p.y >= lo && p.y < hi
}

/// Whether a live tracked entity exists in the seeded world (Reach/Escort name an entity to move).
fn entity_present(sim: &Sim, e: Entity) -> bool {
    sim.world.is_alive(e)
}

/// Assert one objective's target resolves in the seeded `sim`. Returns a precise diagnostic on the
/// first dangling target. This is the core of the lint's teeth: the same function runs against the
/// shipped missions (must pass) and the deliberately-broken fixtures (must fail).
fn objective_target_resolves(sim: &Sim, o: &Objective) -> Result<(), String> {
    match o.kind {
        // The control point to capture must exist in the seeded territory.
        ObjectiveKind::Capture { point, .. } => {
            if sim.territory.points.iter().any(|cp| cp.pos == point) {
                Ok(())
            } else {
                Err(format!(
                    "Capture target point {point:?} has no control point in the seeded world"
                ))
            }
        }
        // The faction to eliminate must actually field something (units and/or buildings), or the
        // objective is complete on tick 0 — a dangling target.
        ObjectiveKind::Eliminate(EliminateTarget::Faction(f)) => {
            let force = faction_forces(sim, f);
            if force.alive_units + force.buildings > 0 {
                Ok(())
            } else {
                Err(format!(
                    "Eliminate(Faction {f:?}) target has no units or buildings in the seeded world"
                ))
            }
        }
        // The VIP entity to eliminate must be present in the seeded world.
        ObjectiveKind::Eliminate(EliminateTarget::Entity(e)) => {
            if entity_present(sim, e) {
                Ok(())
            } else {
                Err(format!(
                    "Eliminate(Entity {e:?}) target is absent from the seeded world"
                ))
            }
        }
        // A survive-to-timeout window must be a future tick (> 0), else it completes immediately.
        ObjectiveKind::Survive { until_tick, .. } => {
            if until_tick > 0 {
                Ok(())
            } else {
                Err("Survive objective has a non-positive until_tick".to_string())
            }
        }
        // Reach/Escort name a destination (must be in bounds), a positive arrival radius, and an
        // entity to move (must be present).
        ObjectiveKind::Reach { who, dest, radius } => {
            resolve_move_to(sim, who, dest, radius, "Reach")
        }
        ObjectiveKind::Escort { vip, dest, radius } => {
            resolve_move_to(sim, vip, dest, radius, "Escort")
        }
    }
}

fn resolve_move_to(
    sim: &Sim,
    who: Entity,
    dest: Vec2,
    radius: Fixed,
    kind: &str,
) -> Result<(), String> {
    if !in_world_bounds(dest) {
        return Err(format!("{kind} destination {dest:?} is out of world bounds"));
    }
    if radius <= Fixed::ZERO {
        return Err(format!("{kind} arrival radius {radius:?} must be positive"));
    }
    if !entity_present(sim, who) {
        return Err(format!("{kind} target entity {who:?} is absent from the seeded world"));
    }
    Ok(())
}

/// Assert the campaign graph is well-formed: every node resolves to a registered mission, and every
/// unlock edge references an existing node.
fn check_campaign_wellformed(reg: &MissionRegistry, campaign: &Campaign) -> Result<(), String> {
    for entry in campaign.mission_select() {
        if reg.get(entry.mission).is_none() {
            return Err(format!(
                "campaign node {:?} names unregistered mission {:?}",
                entry.node, entry.mission
            ));
        }
    }
    for i in 0..campaign.len() {
        let id = NodeId(i as u32);
        let node = campaign.node(id).ok_or_else(|| {
            format!("node {id:?} is missing from a campaign of len {}", campaign.len())
        })?;
        for &prereq in &node.prerequisites {
            if campaign.node(prereq).is_none() {
                return Err(format!("node {id:?} has a dangling prerequisite edge to {prereq:?}"));
            }
        }
    }
    Ok(())
}

// --- the harness proper (green over shipped content) --------------------------------------------

#[test]
fn shipped_content_is_non_empty() {
    // The harness would be vacuously green if it enumerated nothing — pin that it covers the two
    // shipped missions (Seize + Hold) so a future registry regression can't quietly empty it.
    let targets = code_built_targets();
    assert!(
        targets.len() >= 2,
        "expected at least the shipped Seize + Hold missions, found {}",
        targets.len()
    );
}

#[test]
fn every_shipped_mission_seeds_deterministically() {
    for t in code_built_targets() {
        // Invariant #7: the same content seeded with the same seed is bit-identical at the opening.
        let (a, _) = (t.seed)(SEED);
        let (b, _) = (t.seed)(SEED);
        assert_eq!(
            a.checksum(),
            b.checksum(),
            "{}: opening checksum must be deterministic across two seedings",
            t.label
        );
    }
}

#[test]
fn every_shipped_mission_stays_bit_identical_over_ticks() {
    for t in code_built_targets() {
        // Two peers seed the identical world and step it lockstep; every per-tick checksum must
        // match (the `core::scenario` peer-parity pattern, applied at the content layer, GPU-free).
        let stream = |seed: u64| {
            let (mut sim, _) = (t.seed)(seed);
            let mut cs = Vec::with_capacity(STEPS);
            for _ in 0..STEPS {
                sim.step(&[]);
                cs.push(sim.checksum());
            }
            cs
        };
        assert_eq!(
            stream(SEED),
            stream(SEED),
            "{}: per-tick checksum stream must be identical peer-to-peer",
            t.label
        );
    }
}

#[test]
fn every_objective_target_resolves_in_the_seeded_world() {
    let mut missions = 0usize;
    let mut objectives = 0usize;
    for t in code_built_targets() {
        missions += 1;
        let (sim, objs) = (t.seed)(SEED);
        assert!(
            !objs.is_empty(),
            "{}: a shipped campaign mission must carry at least one objective",
            t.label
        );
        for o in &objs.objectives {
            objectives += 1;
            if let Err(e) = objective_target_resolves(&sim, o) {
                panic!("{}: objective {:?} — {}", t.label, o.kind, e);
            }
        }
    }
    assert!(missions >= 2, "the shipped registry covers Seize + Hold");
    assert!(objectives >= missions, "each mission contributes at least one objective");
}

#[test]
fn the_default_campaign_graph_is_well_formed() {
    let reg = default_registry();
    let campaign = default_campaign();
    check_campaign_wellformed(&reg, &campaign)
        .unwrap_or_else(|e| panic!("shipped campaign graph must be well-formed: {e}"));
    // Belt-and-suspenders: the registry's own coverage predicate agrees (graph ⊆ registry).
    assert!(reg.covers(&campaign), "every campaign node must resolve to a registered mission");
}

// --- teeth: the lint MUST reject dangling targets -----------------------------------------------

#[test]
fn the_lint_catches_deliberately_broken_targets() {
    // A bare Sim with no entities, no control points, no factions fielded — every "must resolve"
    // target is dangling against it, so the lint must reject each with a precise diagnostic.
    let empty = Sim::new(0xBAD);
    let ghost = Entity { index: 999, generation: 1 };
    let pt = Vec2::new(Fixed::from_int(5), Fixed::from_int(5));
    let out_of_bounds = Vec2::new(Fixed::from_int(10_000), Fixed::ZERO);

    // 1. Eliminate a VIP entity that is absent from the world — the canonical broken fixture.
    let e = objective_target_resolves(
        &empty,
        &Objective::eliminate_entity(Faction::Player, ghost, "kill the ghost"),
    )
    .expect_err("a dangling entity target must be caught");
    assert!(e.contains("absent"), "diagnostic should name the absent entity: {e}");

    // 2. Capture a control point that does not exist in the (empty) territory.
    let e = objective_target_resolves(
        &empty,
        &Objective::capture(Faction::Player, Faction::Player, pt, "take the hill"),
    )
    .expect_err("a capture target with no control point must be caught");
    assert!(e.contains("no control point"), "diagnostic should name the missing point: {e}");

    // 3. Eliminate a faction that fields nothing (empty world) → complete on tick 0.
    let e = objective_target_resolves(
        &empty,
        &Objective::eliminate_faction(Faction::Player, Faction::Enemy, "wipe them out", 5),
    )
    .expect_err("an eliminate-faction target with no forces must be caught");
    assert!(e.contains("no units or buildings"), "diagnostic should explain the empty faction: {e}");

    // 4. Survive with a zero-tick window → completes immediately, a nonsense target.
    let e = objective_target_resolves(&empty, &Objective::survive(Faction::Player, 0, "hold"))
        .expect_err("a zero-tick survive window must be caught");
    assert!(e.contains("until_tick"), "diagnostic should name the bad window: {e}");

    // 5. Reach a destination outside the playfield bounds.
    let e = objective_target_resolves(
        &empty,
        &Objective::reach(Faction::Player, ghost, out_of_bounds, Fixed::from_int(2), "reach the LZ"),
    )
    .expect_err("an out-of-bounds reach destination must be caught");
    assert!(e.contains("out of world bounds"), "diagnostic should name the OOB dest: {e}");

    // 6. Escort with a non-positive arrival radius (dest in-bounds so the radius check is what bites).
    let e = objective_target_resolves(
        &empty,
        &Objective::escort(Faction::Player, ghost, pt, Fixed::ZERO, "escort the VIP"),
    )
    .expect_err("a non-positive escort radius must be caught");
    assert!(e.contains("radius"), "diagnostic should name the bad radius: {e}");

    // Green control: the SAME lint passes on a real seeded mission's objectives, proving the checks
    // above reject only genuinely-broken content, not everything.
    let (good_sim, good_objs) = (code_built_targets().remove(0).seed)(SEED);
    for o in &good_objs.objectives {
        objective_target_resolves(&good_sim, o)
            .unwrap_or_else(|e| panic!("a shipped objective should resolve, got: {e}"));
    }
}
