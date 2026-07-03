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
//! `core` owns ([`ScenarioModifiers::apply_to_sim`] — the reinforcement cadence), resolved from the
//! player's chosen **campaign replay tier** (D83, resolving Q21); at the neutral `Regular` tier that
//! is a no-op, so a `Regular` launch is **byte-identical** to the bare seed (asserted in the tests),
//! while the other tiers reshape the situation deliberately. The enemy commander difficulty it
//! reports back is a **host-side planning knob** ([`Game::set_commander_difficulty`]), never sim
//! state.

use gonedark_core::campaign::{
    Campaign, Conflict, ConflictId, Difficulty as ReplayTier, MissionId, NodeId, Operation,
    OperationId, OperationNode,
};
use gonedark_core::ecs::Entity;
use gonedark_core::gunsmith::Loadout;
use gonedark_core::mission_tuning::{
    Briefing, Difficulty, ScenarioModifiers, MISSION_ONE_BRIEFING, MISSION_THREE_BRIEFING,
    MISSION_TWO_BRIEFING,
};
use gonedark_core::sim::Sim;

use std::path::{Path, PathBuf};

use crate::map_format::MapSpec;
use crate::mission_format::{
    self, load_mission, parse_mission, LoadedMission, MissionLoadError, MissionSpec,
};
use crate::objectives::ObjectiveSet;

/// The shared identity of the WS-A *Seize* mission ("10 troops, take the base"). The single point
/// where the authored campaign node and its registry entry agree on a `MissionId` — change it in
/// one place and both move together. New missions get their own constant here as more
/// `core::scenario` seeders land.
pub const MISSION_SEIZE: MissionId = MissionId(1);

/// The shared identity of the WS-A *Hold the Line* mission (Mission 2, a Survive/defense archetype).
/// Wired to the engine's `seed_hold_mission_scene` seeder + the [`MISSION_TWO_BRIEFING`].
pub const MISSION_HOLD: MissionId = MissionId(2);

/// The shared identity of the *Break the Line* mission (Mission 3, the Push archetype — capture a
/// lane of guarded control points in order). Wired to the engine's `seed_push_mission_scene` seeder
/// + the [`MISSION_THREE_BRIEFING`].
pub const MISSION_PUSH: MissionId = MissionId(3);

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
    /// The enemy commander difficulty **band** the host applies via
    /// [`Game::set_commander_difficulty`](crate::Game::set_commander_difficulty) — the D83 4→3
    /// collapse of the player's chosen replay tier
    /// ([`commander_tier`](gonedark_core::campaign::Difficulty::commander_tier)), not the mission's
    /// authored tier. A host-side planning knob, not sim state.
    pub commander_difficulty: Difficulty,
}

impl MissionDef {
    /// Author a mission definition from its id, seeder, and briefing.
    pub const fn new(id: MissionId, seed: MissionSeedFn, briefing: Briefing) -> MissionDef {
        MissionDef { id, seed, briefing }
    }

    /// Seed `sim` with this mission's scenario, applying the player's pre-match gunsmith `loadout`
    /// (WS-C), and return the runnable [`LaunchedMission`] tuned to the player's chosen campaign
    /// `replay_tier` (D83, resolving Q21).
    ///
    /// The replay tier — not the mission's *authored* [`Briefing::difficulty`]/[`Briefing::modifiers`]
    /// — drives the fight: the tier's [`ScenarioModifiers`](gonedark_core::mission_tuning::ScenarioModifiers)
    /// (from [`Difficulty::scenario_modifiers`](gonedark_core::campaign::Difficulty::scenario_modifiers))
    /// are applied after seeding (the reinforcement cadence is the one lever `core` owns —
    /// [`ScenarioModifiers::apply_to_sim`]; force/time-limit/fog are host-owned and read off
    /// [`LaunchedMission`]), and the returned `commander_difficulty` is the tier's
    /// [`commander_tier`](gonedark_core::campaign::Difficulty::commander_tier) band. The authored
    /// briefing fields are preserved as the declared default/baseline for display (see
    /// [`Briefing`]); they no longer drive the launched fight.
    ///
    /// The `Regular` tier maps to the neutral baseline (no modifiers, Veteran commander band), so a
    /// `Regular` launch is **byte-identical** to the bare `core::scenario` seed (invariants #1/#7 —
    /// asserted in the tests); the other tiers deviate deliberately. The commander difficulty it
    /// reports back is a host-side planning knob ([`Game::set_commander_difficulty`]), never sim state.
    pub fn launch(&self, sim: &mut Sim, loadout: Loadout, replay_tier: ReplayTier) -> LaunchedMission {
        let (player, start_embodied, objectives) = (self.seed)(sim, loadout);
        let (commander_difficulty, modifiers) = replay_tier.combat_tuning();
        // The single scenario lever `core` owns (reinforcement cadence). Regular ⇒ neutral (`None`)
        // ⇒ no-op ⇒ byte-identical seed; the other tiers reshape the enemy economy's pace.
        modifiers.apply_to_sim(sim);
        LaunchedMission {
            mission: self.id,
            player,
            start_embodied,
            objectives,
            commander_difficulty,
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

/// The shipped host-side mission registry. It holds the three runnable campaign missions — the
/// *Seize* assault ("10 troops, take the base", [`MISSION_SEIZE`]), the *Hold the Line* defense
/// ([`MISSION_HOLD`]), and the *Break the Line* push ([`MISSION_PUSH`]) — each wired to its
/// GPU-free `core::scenario` seeder + WS-E briefing. New missions are added here as more seeders
/// ship.
///
/// Both missions are now **placed as nodes** in [`default_campaign`] (*Seize* → *Hold*), so the
/// registry and the campaign graph cover the same two missions. [`MissionRegistry::covers`] only
/// requires that every *campaign node* resolves (not the converse), so the registry can still carry
/// a mission that isn't yet node-placed — but today it doesn't: every registered mission is reachable
/// through the graph, and its Android `CampaignModel` mirror moved with it (`compose-shell-parity.md`).
pub fn default_registry() -> MissionRegistry {
    MissionRegistry::new(vec![
        MissionDef::new(MISSION_SEIZE, crate::seed_seize_mission_scene, MISSION_ONE_BRIEFING),
        MissionDef::new(MISSION_HOLD, crate::seed_hold_mission_scene, MISSION_TWO_BRIEFING),
        MissionDef::new(MISSION_PUSH, crate::seed_push_mission_scene, MISSION_THREE_BRIEFING),
    ])
}

/// The shipped Operations-hub campaign graph, wired to [`default_registry`]. **Four fictional
/// modern conflicts on the atlas** (D105), each a self-contained three-battle chain
/// Seize → Hold → Push over the three shipped seeders: *The Channel Crisis* (2027–2028, the
/// original D98 placeholder, framed from the `MISSION_*_BRIEFING` consts), then *The Meridian
/// Crisis* (2029–2030, Gulf of Guinea), *The Gotland Winter* (2031–2032, central Baltic), and
/// *The Santo Crisis* (2033–2034, Melanesia) — each with its own per-node titles/situations
/// authored inline. Every conflict's root is open from the start ("pick a war"); gating runs
/// only *within* a conflict, so clearing one war never unlocks another. Each node names its
/// mission by [`MissionId`] only — the three archetypes are deliberately **reused** across
/// conflicts (legal: nothing keys on node-mission uniqueness, and the content lint dedupes
/// targets by mission) — the registry resolves the body, and
/// [`Scene::for_mission`](crate::Scene::for_mission) maps that id to the launchable scene
/// (*Seize* → `Mission1`, *Hold* → `Mission2`, *Push* → `Mission3`). The hand-maintained
/// Android `CampaignModel` mirror moves in lock-step (`compose-shell-parity.md`). Every node's
/// [`MissionId`] resolves in [`default_registry`] ([`MissionRegistry::covers`] holds — a test
/// pins it), so launching any playable node always resolves to a runnable mission.
///
/// The conflicts stay inside the Q28 fork-1(c) lean: **fictional, modern, plausibly covered by
/// the shipped US/FR roster** — no historical conflict ships before the roster and
/// conflict-selection forks are decided. The atlas grouping remains pure metadata over the node
/// chains — unlock behavior and the persisted format are unchanged (a test pins the blob
/// byte-identical to the same nodes ungrouped) — but the node count is now 12: a pre-existing
/// 3-node progress blob no longer applies (`apply_progress` rejects the topology skew cleanly —
/// a fresh-progress reset, by design; see D105).
pub fn default_campaign() -> Campaign {
    Campaign::with_atlas(
        vec![
            Conflict {
                id: ConflictId(0),
                name: "The Channel Crisis".to_string(),
                start_year: 2027,
                end_year: 2028,
                summary: "A fictional modern flashpoint between US and French expeditionary forces \
                          on the Channel coast — the campaign's first (placeholder) conflict."
                    .to_string(),
                // Mid-Channel off the Cotentin coast (~50.0°N, 1.5°W) — the atlas pin (D103).
                lat_x10: 500,
                lon_x10: -15,
            },
            Conflict {
                id: ConflictId(1),
                name: "The Meridian Crisis".to_string(),
                start_year: 2029,
                end_year: 2030,
                summary: "A fictional near-future flashpoint on the Gulf of Guinea — US and French \
                          expeditionary task forces, deployed under rival stabilization mandates \
                          after a regional security pact collapses, come to blows over a deepwater \
                          port and the trans-Sahel supply corridor that runs north from it."
                    .to_string(),
                // Just inland of the Gulf of Guinea coast (~6.2°N, 0.6°E) — on the land mask.
                lat_x10: 62,
                lon_x10: 6,
            },
            Conflict {
                id: ConflictId(2),
                name: "The Gotland Winter".to_string(),
                start_year: 2031,
                end_year: 2032,
                summary: "A fictional near-future flashpoint in the central Baltic — US and French \
                          expeditionary forces contest Gotland through the winter of 2031, the war \
                          for the sea lanes decided on the frozen island that commands them."
                    .to_string(),
                // Visby, Gotland (~57.6°N, 18.3°E) — the island landmass reads on the mask.
                lat_x10: 576,
                lon_x10: 183,
            },
            Conflict {
                id: ConflictId(3),
                name: "The Santo Crisis".to_string(),
                start_year: 2033,
                end_year: 2034,
                summary: "A fictional near-future flashpoint in Melanesia — a state collapse leaves \
                          Espiritu Santo's deepwater port and airstrip ungoverned, and the US and \
                          French expeditionary task forces sent to secure them end up fighting each \
                          other for the island."
                    .to_string(),
                // Luganville, Espiritu Santo (~15.5°S, 167.2°E).
                lat_x10: -155,
                lon_x10: 1672,
            },
        ],
        vec![
            Operation {
                id: OperationId(0),
                conflict: ConflictId(0),
                name: "Operation First Light".to_string(),
            },
            Operation {
                id: OperationId(1),
                conflict: ConflictId(1),
                name: "Operation Dry Season".to_string(),
            },
            Operation {
                id: OperationId(2),
                conflict: ConflictId(2),
                name: "Operation Frostline".to_string(),
            },
            Operation {
                id: OperationId(3),
                conflict: ConflictId(3),
                name: "Operation Trade Wind".to_string(),
            },
        ],
        vec![
            // ---- The Channel Crisis — Operation First Light (the D98 originals) ----
            OperationNode::new(
                NodeId(0),
                MISSION_SEIZE,
                MISSION_ONE_BRIEFING.title,
                MISSION_ONE_BRIEFING.situation,
            )
            .in_operation(OperationId(0))
            // Quettehou, the heights behind Saint-Vaast-la-Hougue (~49.6°N, 1.3°W).
            .at(496, -13),
            OperationNode::new(
                NodeId(1),
                MISSION_HOLD,
                MISSION_TWO_BRIEFING.title,
                MISSION_TWO_BRIEFING.situation,
            )
            .requires([NodeId(0)])
            .in_operation(OperationId(0))
            // Sainte-Mere-Eglise, the inland N13 crossroads (~49.4°N, 1.3°W).
            .at(494, -13),
            OperationNode::new(
                NodeId(2),
                MISSION_PUSH,
                MISSION_THREE_BRIEFING.title,
                MISSION_THREE_BRIEFING.situation,
            )
            .requires([NodeId(1)])
            .in_operation(OperationId(0))
            // Valognes, up the N13 lane toward Cherbourg (~49.5°N, 1.5°W).
            .at(495, -15),
            // ---- The Meridian Crisis — Operation Dry Season ----
            OperationNode::new(
                NodeId(3),
                MISSION_SEIZE,
                "Take the Fuel Yard",
                "The port runs on one fuel yard, and their garrison is sitting on it. Ten of \
                 yours to take it. Command the assault from above — or go dark and breach the \
                 wire yourself. The dust hides them; it hides you too.",
            )
            .in_operation(OperationId(1))
            // Tank-farm flats behind the Port of Lome wharf (~6.2°N, 1.3°E).
            .at(62, 13),
            OperationNode::new(
                NodeId(4),
                MISSION_HOLD,
                "Hold the Causeway",
                "One causeway carries the only road into the port, and they want it back. Dig in \
                 and fight it from the map — or embody a rifle at the barricade. Go dark and the \
                 bank you can't see is the one they wade across.",
            )
            .requires([NodeId(3)])
            .in_operation(OperationId(1))
            // The Aneho lagoon crossing, the one road east (~6.3°N, 1.6°E).
            .at(63, 16),
            OperationNode::new(
                NodeId(5),
                MISSION_PUSH,
                "Open the Corridor",
                "Three checkpoints between the port and the highway north, every one of them \
                 held. Take them in order and hold what you take — or clear each gate yourself, \
                 rifle in hand. But the checkpoint you rush blind is the one that closes behind \
                 you.",
            )
            .requires([NodeId(4)])
            .in_operation(OperationId(1))
            // Tsevie, first checkpoint town on the N1 north (~6.4°N, 1.2°E).
            .at(64, 12),
            // ---- The Gotland Winter — Operation Frostline ----
            OperationNode::new(
                NodeId(6),
                MISSION_SEIZE,
                "Seize the Quay",
                "A garrison winters on the ice-bound harbor, and command wants it before the \
                 strait refreezes. Ten of yours, no more coming. Direct them from above — or \
                 take a rifle onto the ice yourself, and remember the quay you can't see is \
                 still shooting.",
            )
            .in_operation(OperationId(2))
            // Slite, Gotland's industrial deep harbor (~57.7°N, 18.8°E).
            .at(577, 188),
            OperationNode::new(
                NodeId(7),
                MISSION_HOLD,
                "Hold the Airfield",
                "The airstrip is yours; they want it back before first light. Fight the \
                 perimeter from the map, or embody one rifle in the snow and hold it by hand — \
                 but the treeline you go dark on is the one they come through.",
            )
            .requires([NodeId(6)])
            .in_operation(OperationId(2))
            // Visby Airport, interior Gotland (~57.7°N, 18.4°E).
            .at(577, 184),
            OperationNode::new(
                NodeId(8),
                MISSION_PUSH,
                "Break the Coast Road",
                "Three strongpoints up the coast road to Visby, every one dug into the drifts. \
                 Take them in order and keep them taken — or clear each one point-blank \
                 yourself. But the strongpoint you rush blind is the one retaken behind you.",
            )
            .requires([NodeId(7)])
            .in_operation(OperationId(2))
            // Klintehamn, southern end of the west-coast road (~57.4°N, 18.2°E).
            .at(574, 182),
            // ---- The Santo Crisis — Operation Trade Wind ----
            OperationNode::new(
                NodeId(9),
                MISSION_SEIZE,
                "Seize the Wharf",
                "Their task force beat you ashore and dug in on Santo's deepwater wharf. Ten of \
                 yours to take it before their heavy lift arrives. Command the assault from the \
                 ridge — or wade in yourself, and hope the wharf you can't see isn't \
                 reinforcing.",
            )
            .in_operation(OperationId(3))
            // Luganville Main Wharf on the Segond Channel (~15.5°S, 167.1°E).
            .at(-155, 1671),
            OperationNode::new(
                NodeId(10),
                MISSION_HOLD,
                "Hold the Airstrip",
                "The wharf bought you the airstrip; now they want it back before dawn. Hold the \
                 perimeter from above, or put yourself behind a rifle on the wire — but the \
                 stretch of runway you go dark on is the one they land on.",
            )
            .requires([NodeId(9)])
            .in_operation(OperationId(3))
            // The Pekoa/Turtle Bay airfield corridor (~15.4°S, 167.2°E).
            .at(-154, 1672),
            OperationNode::new(
                NodeId(11),
                MISSION_PUSH,
                "Break the Road to Luganville",
                "Three strongpoints down the coast road to Luganville, every one of them held. \
                 Take them in order and hold what you take — the town falls when the road does. \
                 But the post you clear blind is theirs again before you turn around.",
            )
            .requires([NodeId(10)])
            .in_operation(OperationId(3))
            // Hog Harbour on the East Coast Road (~15.1°S, 167.1°E).
            .at(-151, 1671),
        ],
    )
}

// ================= CT-D — data-backed registry + between-match content hot-reload ================
//
// Everything above builds the registry from **hardcoded Rust** (`default_registry`): a recompile per
// mission, and a Rust toolchain to author one. CT-D adds the payoff path — a registry built by
// **loading authored `*.mission.ron` / `*.map.ron` files** from a content directory, through the
// already-landed float-airlock loaders (`mission_format` / `map_format`). `default_registry` stays as
// the code-built fallback baseline and the CT-A oracle (it is NOT replaced): a data mission's opening
// checksum must match its code-built equivalent's, which is exactly what the mission_format Seize
// oracle already pins and the CT-D tests re-assert.
//
// **Zero new checksum surface (invariants #1/#7).** A `ContentMission` seeds a `Sim` by calling the
// SAME `mission_format::load_mission` path the byte-identical Seize oracle proves — the RON file
// never enters the checksum, only the seeded `Sim` does, on the exact footing of a hand-written
// seeder. The `id`, `map` reference, and briefing text are host-side wiring/presentation, never sim
// state.
//
// **Fail-soft hot-reload (the Rust weak-reload mitigation, D10).** [`ContentRegistry::load_dirs`]
// re-scans + re-validates the content dir and returns a [`ContentScan`]: the registry of every file
// that loaded cleanly, PLUS a list of per-file [`ContentError`]s for every file that did not. A
// malformed / dangling / duplicate file is rejected **loudly into `errors`** without taking the whole
// registry down — the good missions still load, so a designer's typo between matches costs one broken
// mission, not the campaign. [`ContentRegistry::reload`] is the return-to-title entry point: it
// re-scans the same dirs the registry was built from.

/// One mission built from an authored `*.mission.ron` file (CT-D) — the data-backed analogue of the
/// code-built [`MissionDef`]. Unlike `MissionDef` (a `fn` pointer + a `&'static` [`Briefing`], both
/// fixed at compile time), a `ContentMission` owns its parsed [`MissionSpec`] and its resolved
/// [`MapSpec`], so it can be re-read from disk between matches (hot-reload).
#[derive(Clone)]
pub struct ContentMission {
    /// The mission identity the campaign graph names it by (from [`MissionSpec::id`]).
    pub id: MissionId,
    /// The file this mission was loaded from — carried for diagnostics and hot-reload.
    pub source: PathBuf,
    /// The parsed, validated mission spec (drives the seed via [`load_mission`]).
    pub spec: MissionSpec,
    /// The battlefield this mission is fought on — resolved from [`MissionSpec::map`] against the
    /// content dir's `*.map.ron` files at load time (a dangling reference is rejected at load).
    pub map: MapSpec,
}

impl ContentMission {
    /// Seed `sim` with this mission by driving the CT-B float-airlock loader
    /// ([`mission_format::load_mission`]) — the SAME code path the mission_format *Seize* oracle
    /// proves **byte-identical** to `core::scenario::seed_seize_mission`. Returns the runnable
    /// [`LoadedMission`] (spawned entities in authored order + the host-side objective set + briefing).
    ///
    /// The spec was validated at load time, so this cannot fail on shipped content; it re-runs
    /// validation defensively (the loader does) and surfaces any error rather than seeding a
    /// half-built world.
    pub fn launch(&self, sim: &mut Sim) -> Result<LoadedMission, MissionLoadError> {
        load_mission(&self.spec, sim)
    }
}

/// Why one content file was rejected during a scan — the fail-loud-per-file diagnostic that keeps a
/// bad file out of the registry without downing the good ones. Carries the offending path plus a
/// human message (a parse/validation error, a dangling map reference, a duplicate id, or an I/O error).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContentError {
    /// The file (or directory, for a read error) the problem is in.
    pub path: PathBuf,
    /// A precise, human-readable diagnostic.
    pub message: String,
}

impl std::fmt::Display for ContentError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.path.display(), self.message)
    }
}

/// The result of scanning a content directory: the [`ContentRegistry`] of everything that loaded
/// cleanly, plus every per-file [`ContentError`]. A caller shows the errors (a designer's between-match
/// feedback) and still ships the good missions — the fail-soft contract.
pub struct ContentScan {
    /// The registry of missions/maps that loaded and validated cleanly.
    pub registry: ContentRegistry,
    /// Every file that was rejected, with why. Empty on a clean content dir (the CT-F lint pins this).
    pub errors: Vec<ContentError>,
}

/// A `MissionId → ContentMission` registry built from authored `*.mission.ron` / `*.map.ron` files
/// (CT-D). Same query surface as the code-built [`MissionRegistry`] ([`get`](Self::get),
/// [`resolve_node`](Self::resolve_node), [`covers`](Self::covers)), so the campaign graph resolves a
/// data mission exactly as it resolves a code one. Built by [`load_dirs`](Self::load_dirs) /
/// [`load_dir`](Self::load_dir); re-scanned between matches by [`reload`](Self::reload).
#[derive(Clone, Default)]
pub struct ContentRegistry {
    /// The directories this registry was scanned from — re-scanned on [`reload`](Self::reload).
    dirs: Vec<PathBuf>,
    /// The loaded missions, in deterministic (path-sorted) order.
    missions: Vec<ContentMission>,
    /// Every loaded map, keyed by filename stem (`crossroads.map.ron` → `"crossroads"`) — the id a
    /// mission's `map` field references. Kept so the CT-F lint can check standalone maps too.
    maps: Vec<(String, MapSpec)>,
}

/// The `"<name>"` a map file is referenced by: its filename with the `.map.ron` suffix stripped.
fn map_id_of(path: &Path) -> String {
    let name = path.file_name().and_then(|s| s.to_str()).unwrap_or("");
    name.strip_suffix(".map.ron").unwrap_or(name).to_string()
}

impl ContentRegistry {
    /// Scan a single content directory for `*.mission.ron` + `*.map.ron`. Convenience over
    /// [`load_dirs`](Self::load_dirs).
    pub fn load_dir(dir: impl AsRef<Path>) -> ContentScan {
        Self::load_dirs([dir.as_ref().to_path_buf()])
    }

    /// Scan one or more content directories (the repo keeps `missions/` and `maps/` separate), parse
    /// + validate every `*.mission.ron` and `*.map.ron`, resolve each mission's `map` reference, and
    /// build a registry — **fail-soft per file**: any file that fails to read, parse, validate,
    /// resolve, or that duplicates an id is dropped into [`ContentScan::errors`] with a precise
    /// message, and the rest still load. This is also the hot-reload entry point (call it again on
    /// return-to-title, or via [`reload`](Self::reload)).
    ///
    /// Files are processed in path-sorted order so the registry (and thus any downstream seed order)
    /// is deterministic regardless of the OS directory-iteration order.
    pub fn load_dirs<I, P>(dirs: I) -> ContentScan
    where
        I: IntoIterator<Item = P>,
        P: Into<PathBuf>,
    {
        let dirs: Vec<PathBuf> = dirs.into_iter().map(Into::into).collect();
        let mut errors = Vec::new();

        // 1. Gather every content file, deterministically ordered.
        let mut mission_files: Vec<PathBuf> = Vec::new();
        let mut map_files: Vec<PathBuf> = Vec::new();
        for dir in &dirs {
            match std::fs::read_dir(dir) {
                Ok(rd) => {
                    for entry in rd.flatten() {
                        let p = entry.path();
                        let Some(name) = p.file_name().and_then(|s| s.to_str()) else {
                            continue;
                        };
                        if name.ends_with(".mission.ron") {
                            mission_files.push(p);
                        } else if name.ends_with(".map.ron") {
                            map_files.push(p);
                        }
                    }
                }
                Err(e) => errors.push(ContentError {
                    path: dir.clone(),
                    message: format!("cannot read content directory: {e}"),
                }),
            }
        }
        mission_files.sort();
        map_files.sort();

        // 2. Load the maps first — missions cross-reference them by filename-stem id.
        let mut maps: Vec<(String, MapSpec)> = Vec::new();
        for path in &map_files {
            let id = map_id_of(path);
            match std::fs::read_to_string(path).map_err(|e| format!("cannot read file: {e}")) {
                Ok(text) => match MapSpec::load(&text) {
                    Ok(spec) => {
                        if maps.iter().any(|(k, _)| *k == id) {
                            errors.push(ContentError {
                                path: path.clone(),
                                message: format!("duplicate map id {id:?} (already loaded)"),
                            });
                        } else {
                            maps.push((id, spec));
                        }
                    }
                    Err(e) => errors.push(ContentError {
                        path: path.clone(),
                        message: e.to_string(),
                    }),
                },
                Err(msg) => errors.push(ContentError {
                    path: path.clone(),
                    message: msg,
                }),
            }
        }

        // 3. Load the missions, resolving each `map` reference against the loaded maps.
        let mut missions: Vec<ContentMission> = Vec::new();
        for path in &mission_files {
            let built = Self::build_mission(path, &maps);
            match built {
                Ok(m) => {
                    if let Some(dup) = missions.iter().find(|other| other.id == m.id) {
                        errors.push(ContentError {
                            path: path.clone(),
                            message: format!(
                                "duplicate MissionId {:?} (already loaded from {})",
                                m.id,
                                dup.source.display()
                            ),
                        });
                    } else {
                        missions.push(m);
                    }
                }
                Err(msg) => errors.push(ContentError {
                    path: path.clone(),
                    message: msg,
                }),
            }
        }

        ContentScan {
            registry: ContentRegistry {
                dirs,
                missions,
                maps,
            },
            errors,
        }
    }

    /// Parse + validate one `*.mission.ron` and resolve its map reference, or return a precise
    /// rejection message. Never touches a `Sim` — pure load-time logic (the repo's testable-seam
    /// convention).
    fn build_mission(path: &Path, maps: &[(String, MapSpec)]) -> Result<ContentMission, String> {
        let text = std::fs::read_to_string(path).map_err(|e| format!("cannot read file: {e}"))?;
        let spec: MissionSpec = parse_mission(&text).map_err(|e| e.to_string())?;
        mission_format::validate(&spec).map_err(|e| e.to_string())?;
        if spec.id == 0 {
            return Err(
                "mission has no `id` (or id 0, the unassigned sentinel); give it a stable, non-zero \
                 `id:` so a campaign node can name it"
                    .to_string(),
            );
        }
        let map = maps
            .iter()
            .find(|(k, _)| *k == spec.map)
            .map(|(_, m)| m.clone())
            .ok_or_else(|| {
                format!(
                    "map reference {:?} resolves to no loaded *.map.ron in the content dir",
                    spec.map
                )
            })?;
        Ok(ContentMission {
            id: MissionId(spec.id),
            source: path.to_path_buf(),
            spec,
            map,
        })
    }

    /// Re-scan + re-validate the directories this registry was built from — the **between-match
    /// hot-reload** (call on return-to-title). Returns a fresh [`ContentScan`] so the caller can swap
    /// in the new registry and surface any newly-introduced errors, all without a recompile.
    pub fn reload(&self) -> ContentScan {
        Self::load_dirs(self.dirs.clone())
    }

    /// Number of loaded missions.
    pub fn len(&self) -> usize {
        self.missions.len()
    }

    /// Whether no mission loaded.
    pub fn is_empty(&self) -> bool {
        self.missions.is_empty()
    }

    /// The content mission registered under a [`MissionId`], or `None` (a content gap — never guessed).
    pub fn get(&self, id: MissionId) -> Option<&ContentMission> {
        self.missions.iter().find(|m| m.id == id)
    }

    /// All loaded missions, in deterministic order.
    pub fn missions(&self) -> &[ContentMission] {
        &self.missions
    }

    /// All loaded maps (id + spec), for standalone-map linting.
    pub fn maps(&self) -> &[(String, MapSpec)] {
        &self.maps
    }

    /// A loaded map by its id (filename stem).
    pub fn map(&self, id: &str) -> Option<&MapSpec> {
        self.maps.iter().find(|(k, _)| k == id).map(|(_, m)| m)
    }

    /// Resolve a campaign **node** to its content mission, honouring the unlock gate — mirrors
    /// [`MissionRegistry::resolve_node`] so a data-backed registry drops into the same shell wiring.
    pub fn resolve_node(&self, campaign: &Campaign, node: NodeId) -> Option<&ContentMission> {
        let n = campaign.node(node)?;
        if !campaign.progress(node).is_playable() {
            return None;
        }
        self.get(n.mission)
    }

    /// Whether every node in `campaign` resolves to a loaded mission (the authoring-consistency
    /// guarantee) — mirrors [`MissionRegistry::covers`].
    pub fn covers(&self, campaign: &Campaign) -> bool {
        campaign
            .mission_select()
            .iter()
            .all(|entry| self.get(entry.mission).is_some())
    }
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
    fn default_registry_holds_the_shipped_missions() {
        let reg = default_registry();
        assert_eq!(reg.len(), 3, "the Seize assault + the Hold defense + the Push lane");

        let seize = reg.get(MISSION_SEIZE).expect("the Seize mission is registered");
        assert_eq!(seize.id, MISSION_SEIZE);
        // Seize is briefed at the Recruit tier with neutral modifiers.
        assert_eq!(seize.briefing.difficulty, CommanderDifficulty::Recruit);
        assert_eq!(seize.modifiers(), ScenarioModifiers::default());

        let hold = reg.get(MISSION_HOLD).expect("the Hold mission is registered");
        assert_eq!(hold.id, MISSION_HOLD);
        // Hold is briefed a step up (Veteran) with neutral modifiers.
        assert_eq!(hold.briefing.difficulty, CommanderDifficulty::Veteran);
        assert_eq!(hold.modifiers(), ScenarioModifiers::default());
        let push = reg.get(MISSION_PUSH).expect("the Push mission is registered");
        assert_eq!(push.id, MISSION_PUSH);
        // Push tops the opening ramp (Elite) with neutral modifiers.
        assert_eq!(push.briefing.difficulty, CommanderDifficulty::Elite);
        assert_eq!(push.modifiers(), ScenarioModifiers::default());
        assert_ne!(seize.briefing.title, hold.briefing.title, "distinct missions");
        assert_ne!(hold.briefing.title, push.briefing.title, "distinct missions");

        // An unregistered id resolves to nothing (a content gap, never guessed).
        assert!(reg.get(MissionId(999)).is_none());
    }

    /// The Hold mission launches into a runnable defense with a live Survive objective and, at the
    /// neutral `Regular` tier, seeds a `Sim` **byte-identical** to the bare `core::scenario` seed — the
    /// same zero-checksum-surface guarantee the Seize launch has (invariants #1/#7).
    #[test]
    fn hold_launch_at_regular_is_byte_identical_to_the_bare_seed() {
        use gonedark_core::scenario::seed_hold_mission;
        let reg = default_registry();
        let def = reg.get(MISSION_HOLD).unwrap();

        let mut launched_sim = Sim::new(0xD00D);
        let launched = def.launch(&mut launched_sim, Loadout::STANDARD, CampaignDifficulty::Regular);

        let mut bare_sim = Sim::new(0xD00D);
        seed_hold_mission(&mut bare_sim);
        assert_eq!(
            launched_sim.checksum(),
            bare_sim.checksum(),
            "a Regular-tier Hold launch adds no checksum surface over the bare seed",
        );

        assert!(!launched.objectives.is_empty(), "the Hold mission has a live objective set");
        assert!(!launched.start_embodied, "a campaign mission boots in the command view");
        assert_eq!(launched.mission, MISSION_HOLD);
        assert_eq!(
            launched_sim.world.faction[launched.player.index as usize],
            Faction::Player,
            "the followed player is a defender",
        );
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

    /// The shipped campaign is a **three-node chain**: Seize (root) → Hold → Push, each gated on
    /// the one before. A locked node never launches; a cleared node stays replayable throughout.
    #[test]
    fn default_campaign_is_seize_then_hold_then_push() {
        let reg = default_registry();
        let mut campaign = default_campaign();
        assert_eq!(campaign.mission_select().len(), 12, "four conflicts x three battles (D105)");

        // Node 0 is the root Seize; node 1 is the gated Hold, framed from MISSION_TWO_BRIEFING.
        assert_eq!(campaign.node(NodeId(0)).unwrap().mission, MISSION_SEIZE);
        let hold_node = campaign.node(NodeId(1)).expect("the second node is placed");
        assert_eq!(hold_node.mission, MISSION_HOLD);
        assert_eq!(hold_node.title, MISSION_TWO_BRIEFING.title);
        assert_eq!(hold_node.prerequisites, vec![NodeId(0)], "Hold is gated behind Seize");
        let push_node = campaign.node(NodeId(2)).expect("the third node is placed");
        assert_eq!(push_node.mission, MISSION_PUSH);
        assert_eq!(push_node.title, MISSION_THREE_BRIEFING.title);
        assert_eq!(push_node.prerequisites, vec![NodeId(1)], "Push is gated behind Hold");

        // Locked until the one before clears: neither gated node launches at the start.
        assert_eq!(campaign.progress(NodeId(0)), NodeProgress::Available);
        assert_eq!(campaign.progress(NodeId(1)), NodeProgress::Locked);
        assert_eq!(campaign.progress(NodeId(2)), NodeProgress::Locked);
        assert!(reg.resolve_node(&campaign, NodeId(1)).is_none(), "Hold is locked at the start");
        assert!(reg.resolve_node(&campaign, NodeId(2)).is_none(), "Push is locked at the start");

        // Clear Seize → Hold unlocks and resolves to MISSION_HOLD; Push stays locked (its gate is
        // Hold, not Seize); Seize stays replayable.
        campaign.clear(NodeId(0), CampaignDifficulty::Recruit).unwrap();
        assert_eq!(campaign.progress(NodeId(1)), NodeProgress::Available);
        assert_eq!(campaign.progress(NodeId(2)), NodeProgress::Locked, "Push waits on Hold");
        assert_eq!(reg.resolve_node(&campaign, NodeId(1)).map(|m| m.id), Some(MISSION_HOLD));
        assert_eq!(
            reg.resolve_node(&campaign, NodeId(0)).map(|m| m.id),
            Some(MISSION_SEIZE),
            "a cleared node stays replayable",
        );

        // Clear Hold → Push finally unlocks and resolves; the cleared nodes stay replayable.
        campaign.clear(NodeId(1), CampaignDifficulty::Veteran).unwrap();
        assert!(matches!(campaign.progress(NodeId(1)), NodeProgress::Cleared { .. }));
        assert_eq!(campaign.progress(NodeId(2)), NodeProgress::Available);
        assert_eq!(reg.resolve_node(&campaign, NodeId(1)).map(|m| m.id), Some(MISSION_HOLD));
        assert_eq!(reg.resolve_node(&campaign, NodeId(2)).map(|m| m.id), Some(MISSION_PUSH));
    }

    /// The shipped graph carries the Q28 conflict-atlas grouping, grown to the D105 four-conflict
    /// atlas: every conflict holds exactly one operation holding its three battles, all conflicts
    /// are fictional-modern (the Q28 fork-1(c) lean), and the grouping is pure metadata (the
    /// progress blob is byte-identical to the same node set built with no atlas).
    #[test]
    fn default_campaign_is_grouped_under_the_conflict_atlas() {
        let mut campaign = default_campaign();

        // Four conflicts, four operations, linked one-to-one, all modern fiction (Q28 fork 1(c):
        // wars the shipped US/FR roster plausibly covers — no historical conflict before the
        // roster/selection forks are decided).
        assert_eq!(campaign.conflicts().len(), 4);
        assert_eq!(campaign.operations().len(), 4);
        assert_eq!(campaign.conflict(ConflictId(0)).unwrap().name, "The Channel Crisis");
        assert_eq!(campaign.conflict(ConflictId(1)).unwrap().name, "The Meridian Crisis");
        assert_eq!(campaign.conflict(ConflictId(2)).unwrap().name, "The Gotland Winter");
        assert_eq!(campaign.conflict(ConflictId(3)).unwrap().name, "The Santo Crisis");
        for i in 0..4u16 {
            let conflict = campaign.conflict(ConflictId(i)).unwrap();
            assert!(conflict.start_year >= 2020, "every shipped conflict is modern (Q28 lean)");
            let op = campaign.operation(OperationId(i)).unwrap();
            assert_eq!(op.conflict, ConflictId(i), "operations link to their conflicts in order");
            assert_eq!(campaign.operations_in(ConflictId(i)), vec![OperationId(i)]);
            assert_eq!(campaign.nodes_in(OperationId(i)).len(), 3, "three battles per war");
        }
        // The eras are staggered so the atlas year scrubber has spread (2027..=2034, no gaps).
        let spans: Vec<(i16, i16)> = campaign
            .conflicts()
            .iter()
            .map(|c| (c.start_year, c.end_year))
            .collect();
        assert_eq!(spans, vec![(2027, 2028), (2029, 2030), (2031, 2032), (2033, 2034)]);

        // The Channel chain still sits in its operation; the rollup tracks it end-to-end — and
        // completing one war completes only that war.
        assert_eq!(campaign.nodes_in(OperationId(0)), vec![NodeId(0), NodeId(1), NodeId(2)]);
        let fresh = campaign.operation_progress(OperationId(0));
        assert_eq!((fresh.cleared, fresh.total, fresh.playable), (0, 3, true));
        campaign.clear(NodeId(0), CampaignDifficulty::Recruit).unwrap();
        campaign.clear(NodeId(1), CampaignDifficulty::Recruit).unwrap();
        campaign.clear(NodeId(2), CampaignDifficulty::Recruit).unwrap();
        assert!(campaign.operation_progress(OperationId(0)).is_complete());
        assert!(campaign.conflict_progress(ConflictId(0)).is_complete());
        assert!(!campaign.conflict_progress(ConflictId(1)).is_complete());

        // Pure metadata: the progress blob matches the same node set built with no atlas — the
        // grouping itself cannot change the persisted format. (Same-node-set only: the D105
        // node-count growth DOES retire pre-D105 3-node blobs — apply_progress rejects the
        // topology skew cleanly, a deliberate fresh-progress reset.)
        let grouped = default_campaign();
        let ungrouped = Campaign::new(
            (0..grouped.len())
                .map(|i| {
                    let n = grouped.node(NodeId(i as u32)).unwrap().clone();
                    OperationNode::new(n.id, n.mission, n.title, n.briefing)
                        .requires(n.prerequisites)
                })
                .collect(),
        );
        assert_eq!(default_campaign().serialize_progress(), ungrouped.serialize_progress());
    }

    /// Every conflict on the D105 atlas is a self-contained Seize → Hold → Push chain: the root is
    /// open from the start ("pick a war"), gating runs only within the conflict, and clearing one
    /// war unlocks nothing in another.
    #[test]
    fn every_conflict_is_a_self_contained_chain() {
        let reg = default_registry();
        let mut campaign = default_campaign();

        let expected = [MISSION_SEIZE, MISSION_HOLD, MISSION_PUSH];
        for i in 0..4u16 {
            let nodes = campaign.nodes_in(OperationId(i));
            assert_eq!(nodes.len(), 3);
            for (j, (node, mission)) in nodes.iter().zip(expected).enumerate() {
                let n = campaign.node(*node).unwrap();
                assert_eq!(n.mission, mission, "each war runs Seize -> Hold -> Push");
                let want: Vec<NodeId> = if j == 0 { vec![] } else { vec![nodes[j - 1]] };
                assert_eq!(n.prerequisites, want, "gating chains within the war only");
                assert!(reg.get(n.mission).is_some(), "every node resolves to a seeder");
            }
            // Every war's root is open before anything anywhere has been cleared.
            assert_eq!(campaign.progress(nodes[0]), NodeProgress::Available);
        }

        // Clearing all of one war leaves every other war's gated nodes locked.
        for node in campaign.nodes_in(OperationId(1)) {
            campaign.clear(node, CampaignDifficulty::Recruit).unwrap();
        }
        assert!(campaign.conflict_progress(ConflictId(1)).is_complete());
        for i in [0u16, 2, 3] {
            let nodes = campaign.nodes_in(OperationId(i));
            assert_eq!(campaign.progress(nodes[1]), NodeProgress::Locked);
            assert_eq!(campaign.progress(nodes[2]), NodeProgress::Locked);
        }
    }

    /// Every shipped battle is anchored on the atlas (D106): the anchor sits near its own war's
    /// pin (the battlefield overview zooms to a region, not a hemisphere) and no two battles in
    /// one war share ground (their pins must separate when zoomed in). Range validity is already
    /// `Campaign::with_atlas`'s panic; this pins the *authoring* quality on top.
    #[test]
    fn every_shipped_battle_is_anchored_near_its_war() {
        let campaign = default_campaign();
        for i in 0..4u16 {
            let conflict = campaign.conflict(ConflictId(i)).unwrap();
            let nodes = campaign.nodes_in(OperationId(i));
            let anchors: Vec<(i16, i16)> = nodes
                .iter()
                .map(|&n| {
                    campaign
                        .node(n)
                        .unwrap()
                        .anchor
                        .unwrap_or_else(|| panic!("shipped node {n:?} has no battlefield anchor"))
                })
                .collect();
            for (j, &(lat, lon)) in anchors.iter().enumerate() {
                // Within ~1.5 degrees of the war's pin (tenths-of-a-degree units).
                assert!(
                    (lat - conflict.lat_x10).abs() <= 15 && (lon - conflict.lon_x10).abs() <= 15,
                    "battle {j} of {} strays from its war's pin: ({lat}, {lon}) vs ({}, {})",
                    conflict.name,
                    conflict.lat_x10,
                    conflict.lon_x10,
                );
                for &other in &anchors[..j] {
                    assert_ne!((lat, lon), other, "two battles of {} share ground", conflict.name);
                }
            }
        }
    }

    /// The shipped graph round-trips through the host progress blob (a cleared Seize survives
    /// serialize→apply onto a freshly-built campaign, so Hold stays unlocked across a restart).
    #[test]
    fn default_campaign_progress_round_trips() {
        let mut campaign = default_campaign();
        campaign.clear(NodeId(0), CampaignDifficulty::Veteran).unwrap();
        let blob = campaign.serialize_progress();

        let mut restored = default_campaign();
        restored.apply_progress(&blob).expect("a same-topology blob applies cleanly");
        assert!(matches!(restored.progress(NodeId(0)), NodeProgress::Cleared { .. }));
        assert_eq!(restored.progress(NodeId(1)), NodeProgress::Available, "Hold stays unlocked");
        assert_eq!(
            default_registry().resolve_node(&restored, NodeId(1)).map(|m| m.id),
            Some(MISSION_HOLD),
        );
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
    fn launch_at_regular_is_byte_identical_to_the_bare_seed() {
        let reg = default_registry();
        let def = reg.get(MISSION_SEIZE).unwrap();

        // The neutral `Regular` replay tier reproduces the baseline (D83): no modifiers, Veteran
        // commander band — so the seeded world matches the bare `core::scenario` seed byte-for-byte.
        let mut launched_sim = Sim::new(0xA11CE);
        let launched = def.launch(&mut launched_sim, Loadout::STANDARD, CampaignDifficulty::Regular);

        let mut bare_sim = Sim::new(0xA11CE);
        seed_seize_mission(&mut bare_sim);

        assert_eq!(
            launched_sim.checksum(),
            bare_sim.checksum(),
            "a Regular-tier registry launch adds no checksum surface over the bare seed",
        );

        // The launch hands back a runnable mission: a live objective set, the command-view boot, the
        // commander band the host applies (Regular → Veteran, the baseline), and a real player entity.
        assert!(!launched.objectives.is_empty(), "the mission has a live objective set");
        assert!(!launched.start_embodied, "a campaign mission boots in the command view");
        assert_eq!(launched.commander_difficulty, CommanderDifficulty::Veteran);
        assert_eq!(launched.mission, MISSION_SEIZE);
        assert_eq!(
            launched_sim.world.faction[launched.player.index as usize],
            Faction::Player,
        );
    }

    /// D83: the player's chosen replay tier — not the mission's authored briefing — drives the
    /// launched fight, on both axes. Each tier applies its own commander band and its own scenario
    /// modifiers (here read back through the sim's income period, the cadence lever `core` owns).
    #[test]
    fn launch_applies_the_replay_tier_not_the_authored_briefing() {
        use gonedark_core::mission_tuning::Difficulty as Cmd;
        let reg = default_registry();
        let def = reg.get(MISSION_SEIZE).unwrap();

        // The authored briefing is preserved (declared default/baseline) and is NOT what the launch
        // applies once a replay tier is chosen.
        assert_eq!(def.briefing.difficulty, Cmd::Recruit, "authored tier preserved for display");

        // Each replay tier → its D83 commander band + its scenario cadence on the sim.
        let cases = [
            (CampaignDifficulty::Recruit, Cmd::Recruit, Some(900u32)),
            (CampaignDifficulty::Regular, Cmd::Veteran, None),
            (CampaignDifficulty::Veteran, Cmd::Veteran, Some(360)),
            (CampaignDifficulty::Elite, Cmd::Elite, Some(240)),
        ];
        // The seize baseline income period (what `None` leaves untouched) — asserted so the "Regular
        // keeps the baseline" claim is concrete.
        let baseline_period = {
            let mut s = Sim::new(1);
            seed_seize_mission(&mut s);
            s.income_period()
        };
        for (tier, band, period_override) in cases {
            let mut sim = Sim::new(1);
            let launched = def.launch(&mut sim, Loadout::STANDARD, tier);
            assert_eq!(launched.commander_difficulty, band, "commander band for {tier:?}");
            let expected = period_override.unwrap_or(baseline_period);
            assert_eq!(sim.income_period(), expected, "reinforcement cadence for {tier:?}");
        }
    }

    /// D83 peer-parity + divergence (mirrors the `scenario.rs` checksum pattern, GPU-free): two peers
    /// at the **same** replay tier stay bit-identical every tick, and **different** tiers diverge —
    /// the tier's cadence lever reaches the checksummed sim (via the enemy purse it accrues). `Regular`
    /// reproduces the bare-seed baseline evolution exactly.
    #[test]
    fn replay_tiers_diverge_and_same_tier_stays_bit_identical() {
        let reg = default_registry();
        let def = reg.get(MISSION_SEIZE).unwrap();

        // Launch at `tier`, drive `TICKS`, and collect the per-tick checksum stream. The budget is
        // long enough that every tier's cadence has accrued a distinct number of times (600/900 only
        // separate from 240/360 well past the accrual boundary — see the module tests).
        const TICKS: usize = 1800;
        let stream = |tier: CampaignDifficulty| -> Vec<u64> {
            let mut sim = Sim::new(0xA11CE);
            def.launch(&mut sim, Loadout::STANDARD, tier);
            let mut cs = Vec::with_capacity(TICKS);
            for _ in 0..TICKS {
                sim.step(&[]);
                cs.push(sim.checksum());
            }
            cs
        };

        // Two peers at the same tier: bit-identical every tick (the lockstep invariant, #7).
        assert_eq!(
            stream(CampaignDifficulty::Veteran),
            stream(CampaignDifficulty::Veteran),
            "same replay tier is bit-identical tick-for-tick across peers",
        );

        // Regular reproduces the neutral bare-seed evolution exactly (byte-identical baseline fight).
        let baseline = {
            let mut sim = Sim::new(0xA11CE);
            seed_seize_mission(&mut sim);
            let mut cs = Vec::with_capacity(TICKS);
            for _ in 0..TICKS {
                sim.step(&[]);
                cs.push(sim.checksum());
            }
            cs
        };
        assert_eq!(stream(CampaignDifficulty::Regular), baseline, "Regular == neutral baseline fight");

        // Different tiers diverge by the final tick (the cadence lever bit the checksummed sim).
        let last = |tier| *stream(tier).last().unwrap();
        let recruit = last(CampaignDifficulty::Recruit);
        let regular = last(CampaignDifficulty::Regular);
        let veteran = last(CampaignDifficulty::Veteran);
        let elite = last(CampaignDifficulty::Elite);
        assert_ne!(recruit, regular, "Recruit (slower drip) diverges from Regular");
        assert_ne!(veteran, regular, "Veteran (faster drip) diverges from Regular");
        assert_ne!(elite, regular, "Elite (fastest drip) diverges from Regular");
        assert_ne!(veteran, elite, "Veteran and Elite field distinct cadences");
        assert_ne!(recruit, veteran, "the easiest and a harder tier diverge");
    }

    /// Resolving a node and launching it composes end-to-end: node → MissionId → MissionDef →
    /// seeded, runnable mission. The whole point of WS-B's registry.
    #[test]
    fn resolve_then_launch_composes_end_to_end() {
        let reg = default_registry();
        let campaign = default_campaign();
        let def = reg.resolve_node(&campaign, NodeId(0)).expect("root resolves");

        let mut sim = Sim::new(0xC0FFEE);
        let launched = def.launch(&mut sim, Loadout::STANDARD, CampaignDifficulty::Regular);
        assert_eq!(launched.mission, MISSION_SEIZE);
        // Determinism: the same node launched onto the same seed at the same tier twice is
        // bit-identical.
        let mut sim2 = Sim::new(0xC0FFEE);
        let _ = def.launch(&mut sim2, Loadout::STANDARD, CampaignDifficulty::Regular);
        assert_eq!(sim.checksum(), sim2.checksum());
    }
}

// ================= CT-D — data-backed registry + hot-reload tests ================================

#[cfg(test)]
mod content_tests {
    use super::*;
    use gonedark_core::scenario::seed_seize_mission;
    use std::sync::atomic::{AtomicU64, Ordering};

    /// The shipped *Seize* mission + its battlefield, compiled into the test binary so a fresh temp
    /// content dir can be materialised with zero dependency on the repo working directory.
    const SEIZE_MISSION: &str = include_str!("../../missions/seize.mission.ron");
    const SEIZE_MAP: &str = include_str!("../../maps/seize_outpost.map.ron");

    /// The CT-A golden opening checksum for *Seize* — the byte-identical oracle a data-loaded mission
    /// must reproduce (also pinned in `core::scenario` and `mission_format`).
    const SEIZE_OPENING_GOLDEN: u64 = 0x474c_dbf2_ad91_3ecb;

    /// The seed the CT-A golden was captured under.
    fn golden_seed() -> Sim {
        Sim::new(0xD0E1)
    }

    // ---- a tiny dependency-free temp-dir helper (no tempfile crate in the tree) ------------------

    /// A unique scratch directory, recursively removed on drop. Unique per (pid, process-atomic
    /// counter, wall-clock nanos) so parallel test threads never collide.
    struct TempContent(PathBuf);

    impl TempContent {
        fn new(tag: &str) -> TempContent {
            static N: AtomicU64 = AtomicU64::new(0);
            let n = N.fetch_add(1, Ordering::Relaxed);
            let nanos = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos();
            let dir = std::env::temp_dir().join(format!(
                "gonedark-ctd-{tag}-{}-{n}-{nanos}",
                std::process::id()
            ));
            std::fs::create_dir_all(&dir).expect("create temp content dir");
            TempContent(dir)
        }

        fn write(&self, name: &str, contents: &str) -> PathBuf {
            let p = self.0.join(name);
            std::fs::write(&p, contents).expect("write content file");
            p
        }

        fn remove(&self, name: &str) {
            let _ = std::fs::remove_file(self.0.join(name));
        }

        fn path(&self) -> &Path {
            &self.0
        }
    }

    impl Drop for TempContent {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    /// A second, minimal-but-valid mission (id 2) on its own tiny map — used to prove reload picks up
    /// an *added* file. Deliberately NOT a re-expression of a shipped mission: it exercises a
    /// different force/objective shape so the format carries it independently.
    const ARENA_MAP: &str = r#"MapSpec(terrain: 0, control_points: [CellRef(x: 64, y: 64)])"#;
    fn arena_mission(title: &str) -> String {
        format!(
            r#"MissionSpec(
    id: 2,
    map: "arena",
    income_period: 300,
    starting_purse: 0,
    armies: (player: Us, enemy: Fr),
    control_points: [],
    forces: [
        Unit(kind: Rifleman, faction: Player, cell: (-5, 0), stance: FireAtWill, facing_deg: 0),
        Camp(faction: Enemy, cell: (5, 0)),
    ],
    objectives: [
        EliminateFaction(owner: Player, target: Enemy, label: "Clear the arena"),
    ],
    difficulty: Veteran,
    briefing: (title: "{title}", situation: "S", objective_line: "O"),
)"#
        )
    }

    // ---- CT-D test 1: a data registry resolves the same node byte-identically to default_registry --

    /// The load-bearing CT-D proof: a registry built from a content DIR resolves the *Seize* node to a
    /// mission whose seeded `Sim` is **byte-identical** to (a) the code-built `default_registry`'s
    /// Seize launch and (b) the bare `seed_seize_mission`, and matches the CT-A golden. The data path
    /// is a faithful re-expression, not a second code path — it adds no checksum surface (#1/#7).
    #[test]
    fn data_registry_resolves_the_seize_node_byte_identically_to_default_registry() {
        let dir = TempContent::new("seize");
        dir.write("seize.mission.ron", SEIZE_MISSION);
        dir.write("seize_outpost.map.ron", SEIZE_MAP);

        let scan = ContentRegistry::load_dir(dir.path());
        assert!(scan.errors.is_empty(), "shipped content must load clean: {:?}", scan.errors);
        let reg = scan.registry;
        assert_eq!(reg.len(), 1, "one authored mission (Seize)");

        // Resolves under MISSION_SEIZE — the same id the code-built default_registry uses.
        let data = reg.get(MISSION_SEIZE).expect("Seize resolves in the data registry");
        assert_eq!(data.id, MISSION_SEIZE);

        // (a) data-loaded Seize == bare seed == the CT-A golden.
        let mut data_sim = golden_seed();
        let loaded = data.launch(&mut data_sim).expect("data Seize seeds");
        assert_eq!(loaded.forces.len(), 15, "ten troops + camp + four garrison");
        let mut bare = golden_seed();
        seed_seize_mission(&mut bare);
        assert_eq!(data_sim.checksum(), bare.checksum(), "data Seize == bare seed");
        assert_eq!(data_sim.checksum(), SEIZE_OPENING_GOLDEN, "data Seize == CT-A golden");

        // (b) data-loaded Seize == code-built default_registry Seize launch (Regular = neutral).
        let mut code_sim = golden_seed();
        default_registry()
            .get(MISSION_SEIZE)
            .unwrap()
            .launch(&mut code_sim, Loadout::STANDARD, ReplayTier::Regular);
        assert_eq!(
            data_sim.checksum(),
            code_sim.checksum(),
            "the data registry resolves the Seize node to the same Sim as default_registry",
        );
    }

    // ---- CT-D test 2: reload picks up an added AND an edited file --------------------------------

    #[test]
    fn reload_picks_up_an_added_and_an_edited_file() {
        let dir = TempContent::new("reload");
        dir.write("seize.mission.ron", SEIZE_MISSION);
        dir.write("seize_outpost.map.ron", SEIZE_MAP);

        // Initial scan: just Seize.
        let reg = {
            let scan = ContentRegistry::load_dir(dir.path());
            assert!(scan.errors.is_empty(), "{:?}", scan.errors);
            scan.registry
        };
        assert_eq!(reg.len(), 1);
        assert!(reg.get(MissionId(2)).is_none(), "arena not authored yet");

        // ADD a second mission (id 2) + its map, then hot-reload the SAME dirs via `reload`.
        dir.write("arena.map.ron", ARENA_MAP);
        dir.write("arena.mission.ron", &arena_mission("Arena"));
        let scan = reg.reload();
        assert!(scan.errors.is_empty(), "the added files must load clean: {:?}", scan.errors);
        let reg = scan.registry;
        assert_eq!(reg.len(), 2, "reload picked up the added mission");
        let arena = reg.get(MissionId(2)).expect("the added mission resolves after reload");
        assert_eq!(arena.spec.briefing.title, "Arena");
        // Seize is still present and still byte-identical after the reload.
        let mut s = golden_seed();
        reg.get(MISSION_SEIZE).unwrap().launch(&mut s).unwrap();
        assert_eq!(s.checksum(), SEIZE_OPENING_GOLDEN);

        // EDIT the added mission (change the briefing title) and reload again — the change is picked up.
        dir.write("arena.mission.ron", &arena_mission("Arena Reforged"));
        let scan = reg.reload();
        assert!(scan.errors.is_empty(), "{:?}", scan.errors);
        let arena = scan.registry.get(MissionId(2)).expect("still present after edit");
        assert_eq!(
            arena.spec.briefing.title, "Arena Reforged",
            "reload reflected the edited briefing",
        );
    }

    // ---- CT-D test 3: a malformed file is rejected WITHOUT downing the registry ------------------

    #[test]
    fn a_malformed_file_is_rejected_without_downing_the_registry() {
        let dir = TempContent::new("malformed");
        // A good, complete mission + its map.
        dir.write("seize.mission.ron", SEIZE_MISSION);
        dir.write("seize_outpost.map.ron", SEIZE_MAP);
        // A malformed mission: an unknown field (`deny_unknown_fields`) — should NOT crash the scan.
        dir.write(
            "broken.mission.ron",
            &SEIZE_MISSION.replace("id: 1,", "id: 7,\n    bogus_field: 3,"),
        );

        let scan = ContentRegistry::load_dir(dir.path());

        // The good mission still loaded — the registry did NOT go down.
        assert_eq!(scan.registry.len(), 1, "the good Seize mission survives a malformed sibling");
        assert!(scan.registry.get(MISSION_SEIZE).is_some());
        assert!(
            scan.registry.get(MissionId(7)).is_none(),
            "the malformed mission never entered the registry",
        );

        // The malformed file is reported with a precise, path-scoped diagnostic.
        assert_eq!(scan.errors.len(), 1, "exactly the one broken file is reported");
        let e = &scan.errors[0];
        assert!(e.path.ends_with("broken.mission.ron"), "names the offending file: {}", e.path.display());
        assert!(
            e.message.contains("bogus_field") || e.message.to_lowercase().contains("unknown"),
            "diagnostic names the parse failure: {}",
            e.message,
        );
    }

    // ---- CT-D test 4: a dangling map reference is rejected (fail-loud cross-reference) -----------

    #[test]
    fn a_dangling_map_reference_is_rejected_but_good_missions_load() {
        let dir = TempContent::new("dangling");
        dir.write("seize.mission.ron", SEIZE_MISSION);
        dir.write("seize_outpost.map.ron", SEIZE_MAP);
        // A mission whose map names a battlefield that is not present in the dir.
        dir.write("orphan.mission.ron", &arena_mission("Orphan").replace(r#"map: "arena""#, r#"map: "nonexistent""#));

        let scan = ContentRegistry::load_dir(dir.path());
        assert_eq!(scan.registry.len(), 1, "Seize still loads; the orphan is rejected");
        assert_eq!(scan.errors.len(), 1);
        assert!(
            scan.errors[0].message.contains("nonexistent")
                && scan.errors[0].message.contains("no loaded"),
            "diagnostic names the dangling map reference: {}",
            scan.errors[0].message,
        );
    }

    // ---- CT-D test 5: a duplicate MissionId is rejected without a panic --------------------------

    #[test]
    fn a_duplicate_mission_id_is_rejected_without_panicking() {
        let dir = TempContent::new("dup");
        dir.write("seize.mission.ron", SEIZE_MISSION);
        dir.write("seize_outpost.map.ron", SEIZE_MAP);
        // A second file that also claims id 1 (against its own map).
        dir.write("clash.map.ron", ARENA_MAP);
        dir.write(
            "clash.mission.ron",
            &arena_mission("Clash").replace("id: 2,", "id: 1,").replace(r#"map: "arena""#, r#"map: "clash""#),
        );

        // Must not panic (unlike MissionRegistry::new, which panics on a dup — a data scan is fail-soft).
        let scan = ContentRegistry::load_dir(dir.path());
        assert_eq!(scan.registry.len(), 1, "exactly one mission keeps id 1");
        assert_eq!(scan.errors.len(), 1, "the clashing file is reported, not fatal");
        assert!(
            scan.errors[0].message.contains("duplicate MissionId"),
            "diagnostic names the id clash: {}",
            scan.errors[0].message,
        );
    }

    // ---- CT-D test 6: an unreadable content dir is a soft error, not a crash ---------------------

    #[test]
    fn a_missing_content_dir_is_a_soft_error_not_a_crash() {
        let missing = std::env::temp_dir().join(format!("gonedark-ctd-missing-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&missing);
        let scan = ContentRegistry::load_dir(&missing);
        assert!(scan.registry.is_empty(), "no missions from a missing dir");
        assert_eq!(scan.errors.len(), 1, "the unreadable dir is reported");
        assert!(scan.errors[0].message.contains("cannot read content directory"));
    }

    // ---- CT-D test 7: reload re-scans the SAME dirs the registry was built from ------------------

    #[test]
    fn reload_rescans_the_original_dirs() {
        let dir = TempContent::new("rescan");
        dir.write("seize.mission.ron", SEIZE_MISSION);
        dir.write("seize_outpost.map.ron", SEIZE_MAP);
        let reg = ContentRegistry::load_dir(dir.path()).registry;
        assert_eq!(reg.len(), 1);

        // Delete the mission file, reload → the registry re-scans the same dir and now finds nothing.
        dir.remove("seize.mission.ron");
        let scan = reg.reload();
        assert!(scan.registry.is_empty(), "reload reflects a removed file");
        // The dangling map alone is not an error (an unused map is fine).
        assert!(scan.errors.is_empty(), "an unreferenced map is not an error: {:?}", scan.errors);
    }
}
