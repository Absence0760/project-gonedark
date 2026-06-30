//! Host-side `MissionId → mission` registry (PvE WS-B) — the WS-A integration seam.
//!
//! [`gonedark_core::campaign`] models the Operations hub as an **opaque** node graph: each
//! [`OperationNode`] names a mission by [`MissionId`] only, never carrying its body (the scenario
//! seed, the `ObjectiveSet`, the tuning). That keeps the campaign model platform- and
//! GPU-free shared `core` data with **zero** mission machinery. This module is the other half it
//! documents: the **host-side registry** that plug-resolves a `MissionId` to a concrete, runnable
//! [`MissionDef`] — the scenario seed + the [`ObjectiveSet`](crate::objectives::ObjectiveSet) that
//! watches it + the WS-E [`Briefing`] (commander difficulty + scenario modifiers + narrative).
//!
//! ## Why it lives in `engine`, not `core`
//!
//! Resolving a mission needs the **host-side objective layer** ([`crate::objectives`]) and the
//! scene seeders — neither of which belongs in the deterministic `core` (the objective layer is
//! deliberately host-side so it adds no checksum surface; see [`crate::objectives`]). So the
//! registry sits exactly where `core::campaign` says it should: *outside* the campaign model, in
//! the host. The campaign graph names a mission; the registry knows how to run it; the two compose
//! without either reaching into the other.
//!
//! ## Zero checksum surface (invariants #1/#7)
//!
//! The registry only **selects** which already-deterministic [`gonedark_core::scenario`] seeder to
//! run and which [`ObjectiveSet`](crate::objectives::ObjectiveSet) (a host-side OBSERVE-only layer)
//! to attach. It folds nothing into the sim. [`MissionDef::launch`] seeds a `Sim` exactly as the
//! engine's existing scene path already does and then applies only the **one** scenario lever
//! `core` owns ([`ScenarioModifiers::apply_to_sim`] — the reinforcement cadence); with the
//! shipped mission's neutral modifiers that is a no-op, so a registry-launched mission is
//! **byte-identical** to the bare seed (asserted in the tests). The enemy commander difficulty it
//! reports back is a **host-side planning knob** ([`Game::set_commander_difficulty`]), never sim
//! state.

use gonedark_core::campaign::{Campaign, MissionId, NodeId, OperationNode};
use gonedark_core::ecs::Entity;
use gonedark_core::gunsmith::Loadout;
use gonedark_core::mission_tuning::{Briefing, Difficulty, ScenarioModifiers, MISSION_ONE_BRIEFING};
use gonedark_core::sim::Sim;

use crate::objectives::ObjectiveSet;

/// The shared identity of the WS-A *Seize* mission ("10 troops, take the base"). The single point
/// where the authored campaign node and its registry entry agree on a `MissionId` — change it in
/// one place and both move together. New missions get their own constant here as more
/// `core::scenario` seeders land.
pub const MISSION_SEIZE: MissionId = MissionId(1);

/// Seeds a `Sim` for a mission and hands back the runnable handles, matching the engine's existing
/// GPU-free scene seeders (e.g. the crate-private `seed_seize_mission_scene`): the embodiable/
/// selectable player entity, whether the scene boots embodied, and the host-side
/// [`ObjectiveSet`](crate::objectives::ObjectiveSet) that OBSERVES it. The player's pre-match
/// gunsmith [`Loadout`] is applied at match start (WS-C); `Loadout::STANDARD` is the no-op default.
pub type MissionSeedFn = fn(&mut Sim, Loadout) -> (Entity, bool, ObjectiveSet);

/// A concrete, runnable mission: the scenario seed (which `core::scenario` world to spawn), and the
/// WS-E [`Briefing`] (which carries the enemy-commander [`Difficulty`] tier, the
/// [`ScenarioModifiers`], and the narrative framing). The objective set is produced *by* the seed
/// (it is derived from the seeded world, e.g. the enemy's starting strength), so it is not a
/// separate field — [`launch`](MissionDef::launch) returns it.
///
/// `Copy` (the seed is a function pointer, the briefing is `Copy`), so a registry entry is cheap to
/// pass around.
#[derive(Clone, Copy)]
pub struct MissionDef {
    /// The opaque id a campaign node names this mission by (the WS-A seam).
    pub id: MissionId,
    /// The scenario seeder + objective-set builder.
    seed: MissionSeedFn,
    /// The WS-E tuning + narrative for this mission (commander difficulty + scenario modifiers +
    /// briefing copy).
    pub briefing: Briefing,
}

/// The result of launching a [`MissionDef`] onto a fresh `Sim`: the seeded handles plus the
/// host-side tuning the host applies. Carries no `Sim` and no sim-state field — it is pure
/// presentation/session data, so it can never perturb the checksum (invariants #1/#7).
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct LaunchedMission {
    /// Which mission this is (the resolved [`MissionId`]).
    pub mission: MissionId,
    /// The embodiable/selectable player entity the host follows.
    pub player: Entity,
    /// Whether the scene boots already embodied (the campaign missions boot in the command view —
    /// `false` — unlike the debug sandboxes).
    pub start_embodied: bool,
    /// The host-side objective set that watches this match (OBSERVE-only; never folded).
    pub objectives: ObjectiveSet,
    /// The enemy commander difficulty tier the host applies via
    /// [`Game::set_commander_difficulty`](crate::Game::set_commander_difficulty). A host-side
    /// planning knob, not sim state.
    pub commander_difficulty: Difficulty,
}

impl MissionDef {
    /// Author a mission definition from its id, seeder, and briefing.
    pub const fn new(id: MissionId, seed: MissionSeedFn, briefing: Briefing) -> MissionDef {
        MissionDef { id, seed, briefing }
    }

    /// Seed `sim` with this mission's scenario, applying the player's pre-match gunsmith `loadout`
    /// (WS-C), and return the runnable [`LaunchedMission`]. After seeding, applies the one scenario
    /// lever `core` owns — the reinforcement cadence ([`ScenarioModifiers::apply_to_sim`]); with
    /// neutral modifiers this is a no-op, so the seeded world is byte-identical to the bare seed
    /// (invariants #1/#7). The commander difficulty + the scenario force/time-limit modifiers are
    /// returned/exposed for the host to apply (they are host-side, never sim state).
    pub fn launch(&self, sim: &mut Sim, loadout: Loadout) -> LaunchedMission {
        let (player, start_embodied, objectives) = (self.seed)(sim, loadout);
        // The single scenario lever `core` owns. Neutral (`None`) ⇒ no-op ⇒ byte-identical seed.
        self.briefing.modifiers.apply_to_sim(sim);
        LaunchedMission {
            mission: self.id,
            player,
            start_embodied,
            objectives,
            commander_difficulty: self.briefing.difficulty,
        }
    }

    /// The scenario modifiers this mission runs at (force size / reinforcement cadence / fog regime
    /// / time limit) — the host reads the force/time-limit/fog ones it owns. A convenience read.
    pub fn modifiers(&self) -> ScenarioModifiers {
        self.briefing.modifiers
    }
}

/// The host-side `MissionId → mission` registry. A dense, deterministic list (no `HashMap`, so no
/// process-randomised iteration — the same determinism discipline `core::campaign` keeps for host
/// data). Build it with [`default_registry`] or [`MissionRegistry::new`]; consult it when a
/// campaign node is launched ([`resolve_node`](MissionRegistry::resolve_node)).
#[derive(Clone)]
pub struct MissionRegistry {
    missions: Vec<MissionDef>,
}

impl MissionRegistry {
    /// Build a registry from its mission definitions.
    ///
    /// Panics if two definitions share a [`MissionId`] — that is an authoring bug (an ambiguous
    /// resolution), caught loudly by the content's own tests, exactly like
    /// [`Campaign::new`](gonedark_core::campaign::Campaign::new)'s topology assertions.
    pub fn new(missions: Vec<MissionDef>) -> MissionRegistry {
        for (i, m) in missions.iter().enumerate() {
            assert!(
                !missions[..i].iter().any(|other| other.id == m.id),
                "duplicate MissionId {:?} in the registry",
                m.id
            );
        }
        MissionRegistry { missions }
    }

    /// Number of registered missions.
    pub fn len(&self) -> usize {
        self.missions.len()
    }

    /// Whether the registry is empty.
    pub fn is_empty(&self) -> bool {
        self.missions.is_empty()
    }

    /// The mission definition for a [`MissionId`], or `None` if no mission is registered under it
    /// (an unregistered id — a content gap, never guessed).
    pub fn get(&self, id: MissionId) -> Option<&MissionDef> {
        self.missions.iter().find(|m| m.id == id)
    }

    /// Resolve a campaign **node** to its runnable mission, honouring the unlock gate: the single
    /// "the shell launches this node" wiring. Returns `None` when the node id is out of range, when
    /// the node is still [`Locked`](gonedark_core::campaign::NodeProgress::Locked) (you cannot
    /// launch what you cannot play), or when the node's [`MissionId`] has no registered definition.
    /// A cleared node is replayable, so it resolves like an available one.
    pub fn resolve_node<'a>(&'a self, campaign: &Campaign, node: NodeId) -> Option<&'a MissionDef> {
        let n = campaign.node(node)?;
        if !campaign.progress(node).is_playable() {
            return None;
        }
        self.get(n.mission)
    }

    /// Whether **every** node in `campaign` resolves to a registered mission — the authoring
    /// consistency guarantee. A node naming a `MissionId` with no definition is a content bug; a
    /// shipped campaign + registry pair must satisfy this (asserted in the tests). Ignores the
    /// unlock gate — it checks the whole authored graph, not just the currently-playable nodes.
    pub fn covers(&self, campaign: &Campaign) -> bool {
        campaign
            .mission_select()
            .iter()
            .all(|entry| self.get(entry.mission).is_some())
    }
}

/// The shipped host-side mission registry. Today it holds the one runnable campaign mission — the
/// WS-A *Seize* mission ("10 troops, take the base") — wired to the engine's existing GPU-free
/// `seed_seize_mission_scene` seeder and the WS-E [`MISSION_ONE_BRIEFING`]. New missions are added
/// here as more `core::scenario` seeders ship; [`default_campaign`] stays in lock-step with it.
pub fn default_registry() -> MissionRegistry {
    MissionRegistry::new(vec![MissionDef::new(
        MISSION_SEIZE,
        crate::seed_seize_mission_scene,
        MISSION_ONE_BRIEFING,
    )])
}

/// The shipped Operations-hub campaign graph, wired to [`default_registry`]. Today it is the single
/// root node — the *Seize* mission — because that is the one runnable scene WS-A has shipped; more
/// nodes (and their unlock edges) land here as more missions are added to the registry. Every
/// node's [`MissionId`] resolves in [`default_registry`] ([`MissionRegistry::covers`] holds — a
/// test pins it), so launching any node always resolves to a runnable mission.
pub fn default_campaign() -> Campaign {
    Campaign::new(vec![OperationNode::new(
        NodeId(0),
        MISSION_SEIZE,
        MISSION_ONE_BRIEFING.title,
        MISSION_ONE_BRIEFING.situation,
    )])
}

#[cfg(test)]
mod tests {
    use super::*;
    use gonedark_core::campaign::{Difficulty as CampaignDifficulty, NodeProgress};
    use gonedark_core::components::Faction;
    use gonedark_core::mission_tuning::Difficulty as CommanderDifficulty;
    use gonedark_core::scenario::seed_seize_mission;

    // A second, synthetic mission def for multi-node resolution tests: it reuses the Seize seeder
    // (the only one that ships) under a different id, so the registry has two distinct entries to
    // resolve without inventing a new scene.
    const MISSION_ALT: MissionId = MissionId(2);

    fn alt_def() -> MissionDef {
        MissionDef::new(MISSION_ALT, crate::seed_seize_mission_scene, MISSION_ONE_BRIEFING)
    }

    // ---- the shipped registry + campaign ------------------------------------------------------

    #[test]
    fn default_registry_holds_the_seize_mission() {
        let reg = default_registry();
        assert_eq!(reg.len(), 1);
        let m = reg.get(MISSION_SEIZE).expect("the Seize mission is registered");
        assert_eq!(m.id, MISSION_SEIZE);
        // The shipped mission runs at the briefing's Recruit tier with neutral modifiers.
        assert_eq!(m.briefing.difficulty, CommanderDifficulty::Recruit);
        assert_eq!(m.modifiers(), ScenarioModifiers::default());
        // An unregistered id resolves to nothing (a content gap, never guessed).
        assert!(reg.get(MissionId(999)).is_none());
    }

    /// The wiring guarantee: every node in the shipped campaign resolves to a registered mission.
    #[test]
    fn default_registry_covers_the_default_campaign() {
        let reg = default_registry();
        let campaign = default_campaign();
        assert!(reg.covers(&campaign), "every campaign node must resolve to a mission");
        // And the root node resolves to the Seize mission specifically.
        let def = reg
            .resolve_node(&campaign, NodeId(0))
            .expect("the root node is available and registered");
        assert_eq!(def.id, MISSION_SEIZE);
    }

    /// `covers` catches the authoring bug it exists for: a node naming an unregistered mission.
    #[test]
    fn covers_detects_an_unregistered_node_mission() {
        let reg = default_registry();
        // A campaign whose node names a MissionId with no definition.
        let orphan = Campaign::new(vec![OperationNode::new(NodeId(0), MissionId(42), "Orphan", "")]);
        assert!(!reg.covers(&orphan), "an unregistered MissionId must fail coverage");
        assert!(reg.resolve_node(&orphan, NodeId(0)).is_none());
    }

    // ---- node resolution honours the unlock gate ----------------------------------------------

    #[test]
    fn resolve_node_honours_locked_available_and_cleared() {
        // A two-node chain A -> B, both wired to registered missions.
        let reg = MissionRegistry::new(vec![
            MissionDef::new(MISSION_SEIZE, crate::seed_seize_mission_scene, MISSION_ONE_BRIEFING),
            alt_def(),
        ]);
        let mut campaign = Campaign::new(vec![
            OperationNode::new(NodeId(0), MISSION_SEIZE, "A", ""),
            OperationNode::new(NodeId(1), MISSION_ALT, "B", "").requires([NodeId(0)]),
        ]);
        assert!(reg.covers(&campaign));

        // A is Available → resolves; B is Locked → does NOT resolve (cannot launch what you can't
        // play), even though its mission IS registered.
        assert_eq!(campaign.progress(NodeId(0)), NodeProgress::Available);
        assert_eq!(campaign.progress(NodeId(1)), NodeProgress::Locked);
        assert_eq!(reg.resolve_node(&campaign, NodeId(0)).map(|m| m.id), Some(MISSION_SEIZE));
        assert!(reg.resolve_node(&campaign, NodeId(1)).is_none(), "a locked node won't launch");

        // Clear A → its successor B unlocks and now resolves; A stays replayable and still resolves.
        campaign.clear(NodeId(0), CampaignDifficulty::Recruit).unwrap();
        assert!(matches!(campaign.progress(NodeId(0)), NodeProgress::Cleared { .. }));
        assert_eq!(campaign.progress(NodeId(1)), NodeProgress::Available);
        assert_eq!(reg.resolve_node(&campaign, NodeId(1)).map(|m| m.id), Some(MISSION_ALT));
        assert_eq!(
            reg.resolve_node(&campaign, NodeId(0)).map(|m| m.id),
            Some(MISSION_SEIZE),
            "a cleared node is replayable, so it still resolves",
        );

        // An out-of-range node resolves to nothing.
        assert!(reg.resolve_node(&campaign, NodeId(99)).is_none());
    }

    #[test]
    #[should_panic(expected = "duplicate MissionId")]
    fn duplicate_mission_ids_are_rejected() {
        // Two defs under the same id is an ambiguous resolution — rejected loudly at construction.
        MissionRegistry::new(vec![
            MissionDef::new(MISSION_SEIZE, crate::seed_seize_mission_scene, MISSION_ONE_BRIEFING),
            MissionDef::new(MISSION_SEIZE, crate::seed_seize_mission_scene, MISSION_ONE_BRIEFING),
        ]);
    }

    // ---- launch produces a runnable mission with ZERO checksum surface ------------------------

    /// Launching the shipped mission seeds a `Sim` **byte-identical** to the bare `core::scenario`
    /// seed: the registry only selects the seeder + attaches a host-side OBSERVE-only objective set
    /// and applies the neutral modifiers (a no-op), so it adds no checksum surface (invariants
    /// #1/#7). This is the structural proof that the registry "confirms the sim it observes is
    /// unchanged".
    #[test]
    fn launch_is_byte_identical_to_the_bare_seed() {
        let reg = default_registry();
        let def = reg.get(MISSION_SEIZE).unwrap();

        let mut launched_sim = Sim::new(0xA11CE);
        let launched = def.launch(&mut launched_sim, Loadout::STANDARD);

        let mut bare_sim = Sim::new(0xA11CE);
        seed_seize_mission(&mut bare_sim);

        assert_eq!(
            launched_sim.checksum(),
            bare_sim.checksum(),
            "a registry launch adds no checksum surface over the bare seed",
        );

        // The launch hands back a runnable mission: a live objective set, the command-view boot, the
        // commander tier the host applies, and a real player entity in the Player faction.
        assert!(!launched.objectives.is_empty(), "the mission has a live objective set");
        assert!(!launched.start_embodied, "a campaign mission boots in the command view");
        assert_eq!(launched.commander_difficulty, CommanderDifficulty::Recruit);
        assert_eq!(launched.mission, MISSION_SEIZE);
        assert_eq!(
            launched_sim.world.faction[launched.player.index as usize],
            Faction::Player,
        );
    }

    /// Resolving a node and launching it composes end-to-end: node → MissionId → MissionDef →
    /// seeded, runnable mission. The whole point of WS-B's registry.
    #[test]
    fn resolve_then_launch_composes_end_to_end() {
        let reg = default_registry();
        let campaign = default_campaign();
        let def = reg.resolve_node(&campaign, NodeId(0)).expect("root resolves");

        let mut sim = Sim::new(0xC0FFEE);
        let launched = def.launch(&mut sim, Loadout::STANDARD);
        assert_eq!(launched.mission, MISSION_SEIZE);
        // Determinism: the same node launched onto the same seed twice is bit-identical.
        let mut sim2 = Sim::new(0xC0FFEE);
        let _ = def.launch(&mut sim2, Loadout::STANDARD);
        assert_eq!(sim.checksum(), sim2.checksum());
    }
}
