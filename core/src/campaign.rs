//! The Operations-hub campaign — host-side meta-progression (PvE plan WS-B).
//!
//! This is the node-graph campaign model the native out-of-match shell ([D32]) reaches through
//! the [`core::shell`](crate::shell) seam ([D34]): a graph of [`OperationNode`]s, each pointing at
//! a mission, with **unlock state** (clearing a node opens its successors) and
//! **replay-at-higher-difficulty** support. It is the meta-progression analogue of a CoH /
//! Delta-Force *Operations* hub ([`docs/pve-campaign.md`], D58).
//!
//! ## It is HOST-SIDE — never in the sim, never in the checksum (invariants #1/#7)
//!
//! Campaign progress is the same footing as everything else on the [`shell`](crate::shell) read
//! side: a **derived/owned host state**, not sim state. A tick never reads or mutates it; it is
//! **never folded into [`Sim::fold`](crate::sim::Sim::fold)** and so can never perturb the per-tick
//! checksum or desync lockstep. The campaign module deliberately does **not** import
//! [`Sim`](crate::sim::Sim) at all — that absence is the structural guarantee. Progress is
//! persisted to its **own host blob** ([`Campaign::serialize_progress`]), *separate* from the
//! authoritative [`Sim::serialize`](crate::sim::Sim::serialize) snapshot, precisely so meta-state
//! can never leak into the checksum fold. (The plan offered "alongside `Sim::serialize` or a
//! separate host file"; a separate blob is the lower-risk choice and keeps the sim codec
//! untouched.)
//!
//! It still lives in `core` (not a platform crate) because it is shared, GPU-free, platform-free
//! data (invariant #2) and reuses the [`persist`](crate::persist) byte codec — so it is float-free
//! (invariant #1) like the rest of `core`, even though it is not sim state.
//!
//! ## WS-A INTEGRATION SEAM
//!
//! WS-A (the mission/objective core) owns the real mission format (scenario params, the
//! `ObjectiveSet`, force composition). Until it lands, a node references a mission only by an
//! **opaque** [`MissionId`]. When WS-A exists, the resolution `MissionId -> real mission descriptor`
//! plugs in **outside** this module (a host-side registry the shell consults when it launches a
//! node) — this model never needs the mission *body*, only its identity, so the two compose
//! without either side reaching into the other. The single point to revisit is documented on
//! [`MissionId`].

use crate::persist::{DeserializeError, Reader, StateSink, Writer};

// ===========================================================================
// WS-A SEAM — opaque mission reference
// ===========================================================================

/// An **opaque** handle to a mission. This is the WS-A integration seam: WS-A owns the real
/// mission descriptor (scenario seed/params, the `ObjectiveSet`, force composition); the campaign
/// graph only needs to *name* a mission, never to read its body. A host-side registry maps a
/// `MissionId` to the launchable mission when a node is started — that mapping lives outside this
/// module, so WS-A can land without touching the hub model.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug, Default)]
pub struct MissionId(pub u32);

// ===========================================================================
// Difficulty tiers (integer-ordered — invariant #1, no floats)
// ===========================================================================

/// A campaign difficulty tier, for replay-at-higher-difficulty. Declared in **ascending** order
/// so the derived [`Ord`] ranks `Recruit < Regular < Veteran < Elite` — the ordering
/// [`Campaign::clear`] uses to keep only the *best* clear. Each tier is an integer rank (a
/// [`Difficulty::tier`] `u8`); there is no float anywhere (invariant #1).
///
/// What a tier *means* mechanically (commander reserve/cadence/aggression) is WS-E's job, threaded
/// into the seeded planner — never an omniscient cheat (invariant #6). Here a tier is only the
/// progression coordinate the hub records and replays against.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug, Default)]
pub enum Difficulty {
    #[default]
    Recruit,
    Regular,
    Veteran,
    Elite,
}

impl Difficulty {
    /// Every tier, lowest-to-highest — for a shell to list the replay options.
    pub const ALL: [Difficulty; 4] = [
        Difficulty::Recruit,
        Difficulty::Regular,
        Difficulty::Veteran,
        Difficulty::Elite,
    ];

    /// The integer rank of this tier (`0..=3`). Stable wire value for persistence and the basis of
    /// the [`Ord`] comparison.
    pub fn tier(self) -> u8 {
        match self {
            Difficulty::Recruit => 0,
            Difficulty::Regular => 1,
            Difficulty::Veteran => 2,
            Difficulty::Elite => 3,
        }
    }

    /// Inverse of [`tier`](Difficulty::tier): the tier for a rank, or `None` for an out-of-range
    /// value (a corrupt/foreign persistence byte — rejected, never guessed).
    pub fn from_tier(tier: u8) -> Option<Difficulty> {
        match tier {
            0 => Some(Difficulty::Recruit),
            1 => Some(Difficulty::Regular),
            2 => Some(Difficulty::Veteran),
            3 => Some(Difficulty::Elite),
            _ => None,
        }
    }

    /// A stable, human-readable id a native shell keys a localized label off (never the label
    /// itself — localization is the shell's job, above the seam; mirrors `OrderKind::id`).
    pub fn id(self) -> &'static str {
        match self {
            Difficulty::Recruit => "recruit",
            Difficulty::Regular => "regular",
            Difficulty::Veteran => "veteran",
            Difficulty::Elite => "elite",
        }
    }
}

// ===========================================================================
// The node graph
// ===========================================================================

/// A stable index for an [`OperationNode`] in a [`Campaign`]. By construction `NodeId(i)` is the
/// node at position `i` in the authored node list — a dense, deterministic key (no `HashMap`, so
/// no process-randomised iteration; invariant #1's determinism discipline applies to host data
/// here too).
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug, Default)]
pub struct NodeId(pub u32);

impl NodeId {
    #[inline]
    fn index(self) -> usize {
        self.0 as usize
    }
}

/// One operation in the campaign graph: a mission, the prerequisites that gate it, and the
/// authored briefing copy a shell shows on the mission-select / briefing surface. This is **static
/// authored topology** — it carries no progress (that lives in [`Campaign`], so progress can be
/// persisted without re-shipping the graph).
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct OperationNode {
    /// This node's stable id (must equal its position in the authored list).
    pub id: NodeId,
    /// The mission this node launches — opaque (the WS-A seam, see [`MissionId`]).
    pub mission: MissionId,
    /// The nodes that must be **cleared** before this one unlocks. Empty ⇒ a root (unlocked from
    /// the start). "Clearing a node opens its successors" is exactly this relation read forward:
    /// clearing the last prerequisite of a node flips it from [`NodeProgress::Locked`] to
    /// [`NodeProgress::Available`].
    pub prerequisites: Vec<NodeId>,
    /// Short title for the mission-select tile.
    pub title: String,
    /// Briefing copy for the briefing surface (the light narrative framing WS-E expands; here it
    /// is authored data the hub carries).
    pub briefing: String,
}

impl OperationNode {
    /// A **root** node (no prerequisites — unlocked from the start).
    pub fn new(
        id: NodeId,
        mission: MissionId,
        title: impl Into<String>,
        briefing: impl Into<String>,
    ) -> OperationNode {
        OperationNode {
            id,
            mission,
            prerequisites: Vec::new(),
            title: title.into(),
            briefing: briefing.into(),
        }
    }

    /// Builder: gate this node behind the given prerequisite nodes (all must be cleared).
    pub fn requires(mut self, prerequisites: impl IntoIterator<Item = NodeId>) -> OperationNode {
        self.prerequisites = prerequisites.into_iter().collect();
        self
    }
}

/// The derived unlock/clear state of a node, as the shell reads it. **Derived**, never stored:
/// [`Locked`](NodeProgress::Locked) vs [`Available`](NodeProgress::Available) is recomputed from
/// the prerequisite clears each read, so it can never drift from the persisted `cleared` set.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum NodeProgress {
    /// At least one prerequisite is not yet cleared — the node cannot be played.
    Locked,
    /// Every prerequisite is cleared (or there are none) but the node itself has not been cleared.
    Available,
    /// The node has been cleared; `best` is the highest difficulty it was cleared at (for the
    /// replay-at-higher-difficulty surface).
    Cleared { best: Difficulty },
}

impl NodeProgress {
    /// Whether the node can be launched now (Available **or** already Cleared — a cleared node is
    /// replayable). Locked nodes are the only un-launchable ones.
    pub fn is_playable(self) -> bool {
        !matches!(self, NodeProgress::Locked)
    }

    /// The best difficulty this node was cleared at, if cleared.
    pub fn best_cleared(self) -> Option<Difficulty> {
        match self {
            NodeProgress::Cleared { best } => Some(best),
            _ => None,
        }
    }
}

/// Why a [`Campaign::clear`] was rejected. Clearing is the only state mutation, so this is the only
/// fallible operation on the model.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum CampaignError {
    /// The node id is out of range for this campaign.
    UnknownNode(NodeId),
    /// The node is still [`Locked`](NodeProgress::Locked) — its prerequisites are not all cleared,
    /// so it cannot be cleared.
    NodeLocked(NodeId),
}

/// The result of a successful [`Campaign::clear`]: which successor nodes this clear *newly*
/// unlocked, and whether it raised the node's best difficulty. The shell uses `newly_unlocked` to
/// animate freshly-opened tiles on the hub.
#[derive(Clone, PartialEq, Eq, Debug, Default)]
pub struct ClearOutcome {
    /// Nodes that were [`Locked`](NodeProgress::Locked) before this clear and are
    /// [`Available`](NodeProgress::Available) after it (their last prerequisite was this node).
    pub newly_unlocked: Vec<NodeId>,
    /// `true` if this clear raised the node's recorded best difficulty (a first clear, or a replay
    /// at a strictly higher tier). `false` if it was a replay at an equal-or-lower tier.
    pub raised_difficulty: bool,
}

/// The Operations-hub campaign: the authored node graph plus the player's progress. Construct with
/// [`Campaign::new`]; advance with [`Campaign::clear`]; read for the shell with
/// [`Campaign::mission_select`] / [`Campaign::briefing`]; persist with
/// [`Campaign::serialize_progress`] / [`Campaign::apply_progress`].
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Campaign {
    /// Authored topology, indexed by `NodeId` (position `i` holds `NodeId(i)`).
    nodes: Vec<OperationNode>,
    /// Per-node progress, the **only** persisted state: `None` = not cleared, `Some(best)` =
    /// cleared at best difficulty `best`. Indexed identically to `nodes`. (Unlock state is
    /// *derived* from this, not stored — see [`NodeProgress`].)
    cleared: Vec<Option<Difficulty>>,
}

/// Version byte for the progress blob format. Bump on any layout change so an old/foreign blob is
/// rejected loudly ([`DeserializeError::BadVersion`]) rather than silently misparsed.
const PROGRESS_VERSION: u8 = 1;

impl Campaign {
    /// Build a campaign from its authored nodes, with all progress cleared (nothing cleared yet;
    /// roots [`Available`](NodeProgress::Available), the rest [`Locked`](NodeProgress::Locked)).
    ///
    /// Panics if the authoring is malformed — `nodes[i].id != NodeId(i)`, or a prerequisite names
    /// an out-of-range node. These are authoring bugs in committed content (caught by the content's
    /// own tests), not runtime conditions, so a panic is the right, loud failure.
    pub fn new(nodes: Vec<OperationNode>) -> Campaign {
        for (i, node) in nodes.iter().enumerate() {
            assert_eq!(
                node.id,
                NodeId(i as u32),
                "OperationNode at position {i} must have id NodeId({i})"
            );
            for &prereq in &node.prerequisites {
                assert!(
                    prereq.index() < nodes.len(),
                    "node {i} has out-of-range prerequisite {prereq:?}"
                );
            }
        }
        let cleared = vec![None; nodes.len()];
        Campaign { nodes, cleared }
    }

    /// Number of nodes in the campaign.
    pub fn len(&self) -> usize {
        self.nodes.len()
    }

    /// Whether the campaign has no nodes.
    pub fn is_empty(&self) -> bool {
        self.nodes.is_empty()
    }

    /// The authored node for an id, or `None` if out of range.
    pub fn node(&self, id: NodeId) -> Option<&OperationNode> {
        self.nodes.get(id.index())
    }

    /// Whether a node is cleared (at any difficulty).
    pub fn is_cleared(&self, id: NodeId) -> bool {
        self.cleared
            .get(id.index())
            .map(Option::is_some)
            .unwrap_or(false)
    }

    /// The best difficulty a node was cleared at, or `None` (out of range or not cleared).
    pub fn best_cleared(&self, id: NodeId) -> Option<Difficulty> {
        self.cleared.get(id.index()).copied().flatten()
    }

    /// Whether a node is **unlocked** — every prerequisite cleared (a root with no prerequisites is
    /// always unlocked). This is the derivation that makes "clearing a node opens its successors"
    /// hold without storing edge state. An out-of-range id is not unlocked.
    pub fn is_unlocked(&self, id: NodeId) -> bool {
        match self.node(id) {
            None => false,
            Some(node) => node.prerequisites.iter().all(|&p| self.is_cleared(p)),
        }
    }

    /// The derived [`NodeProgress`] for a node (Locked / Available / Cleared). The single source
    /// the mission-select and briefing surfaces read.
    pub fn progress(&self, id: NodeId) -> NodeProgress {
        match self.best_cleared(id) {
            Some(best) => NodeProgress::Cleared { best },
            None if self.is_unlocked(id) => NodeProgress::Available,
            None => NodeProgress::Locked,
        }
    }

    /// The forward edges of `id` — every node that lists `id` as a prerequisite. Computed by scan
    /// (the graphs are small and authored), so the model stores no redundant reverse index to keep
    /// in sync.
    pub fn successors(&self, id: NodeId) -> Vec<NodeId> {
        self.nodes
            .iter()
            .filter(|n| n.prerequisites.contains(&id))
            .map(|n| n.id)
            .collect()
    }

    /// Record a clear of `id` at `difficulty`, opening any successors whose last prerequisite this
    /// was. **Replay-aware:** clearing an already-cleared node is allowed (replay) and keeps only
    /// the *best* (highest) difficulty — a lower-tier replay never demotes a node.
    ///
    /// Fails if the node is unknown ([`CampaignError::UnknownNode`]) or still locked
    /// ([`CampaignError::NodeLocked`] — you cannot clear what you cannot play). On success returns
    /// a [`ClearOutcome`] describing the newly-unlocked successors and whether the best difficulty
    /// rose.
    pub fn clear(
        &mut self,
        id: NodeId,
        difficulty: Difficulty,
    ) -> Result<ClearOutcome, CampaignError> {
        if self.node(id).is_none() {
            return Err(CampaignError::UnknownNode(id));
        }
        if !self.is_unlocked(id) {
            return Err(CampaignError::NodeLocked(id));
        }

        // Snapshot which successors were locked before, so we can report the ones this clear opens.
        let successors = self.successors(id);
        let was_locked: Vec<NodeId> = successors
            .iter()
            .copied()
            .filter(|&s| !self.is_unlocked(s))
            .collect();

        // Record the clear, keeping the best (max) difficulty.
        let slot = &mut self.cleared[id.index()];
        let raised_difficulty = match *slot {
            Some(prev) => {
                if difficulty > prev {
                    *slot = Some(difficulty);
                    true
                } else {
                    false
                }
            }
            None => {
                *slot = Some(difficulty);
                true
            }
        };

        // A successor is newly unlocked iff it was locked before and is unlocked now.
        let newly_unlocked: Vec<NodeId> = was_locked
            .into_iter()
            .filter(|&s| self.is_unlocked(s))
            .collect();

        Ok(ClearOutcome {
            newly_unlocked,
            raised_difficulty,
        })
    }

    // -----------------------------------------------------------------------
    // Mission-select + briefing read surface (reached through `core::shell`)
    // -----------------------------------------------------------------------

    /// The mission-select surface: one [`MissionSelectEntry`] per node, in authored (`NodeId`)
    /// order, carrying the derived [`NodeProgress`] the hub renders tiles from. Presentation-safe
    /// data only — no sim state, never checksummed.
    pub fn mission_select(&self) -> Vec<MissionSelectEntry> {
        self.nodes
            .iter()
            .map(|n| MissionSelectEntry {
                node: n.id,
                mission: n.mission,
                title: n.title.clone(),
                progress: self.progress(n.id),
            })
            .collect()
    }

    /// The briefing surface for one node — the data a "launch this mission" screen shows. `None`
    /// for an out-of-range id. `replayable` is true once the node is cleared (a cleared node can be
    /// replayed, including at a higher difficulty); `available_difficulties` is every tier the
    /// shell may offer for the launch.
    pub fn briefing(&self, id: NodeId) -> Option<Briefing> {
        let node = self.node(id)?;
        let progress = self.progress(id);
        Some(Briefing {
            node: node.id,
            mission: node.mission,
            title: node.title.clone(),
            briefing: node.briefing.clone(),
            progress,
            replayable: progress.best_cleared().is_some(),
            available_difficulties: Difficulty::ALL.to_vec(),
        })
    }

    // -----------------------------------------------------------------------
    // Persistence — a HOST blob, OUTSIDE the sim checksum fold (invariants #1/#7)
    // -----------------------------------------------------------------------

    /// Serialize **only the progress** (the `cleared` set) to a host blob, using the same
    /// little-endian [`persist`](crate::persist) codec the snapshot uses — but as a **separate**
    /// host file, never part of [`Sim::serialize`](crate::sim::Sim::serialize) and never folded
    /// into the checksum. The authored topology is *not* written (the build re-supplies it via
    /// [`Campaign::new`]); only the per-node best-difficulty progress is.
    ///
    /// Layout: `[version:u8][node_count:u32]` then one `u8` per node — `0` = not cleared, else
    /// `tier+1` (so `1..=4` map to the four difficulty tiers). The count is written so a load can
    /// detect topology skew before applying.
    pub fn serialize_progress(&self) -> Vec<u8> {
        let mut w = Writer::new();
        w.write_u8(PROGRESS_VERSION);
        w.write_u32(self.nodes.len() as u32);
        for slot in &self.cleared {
            match slot {
                None => w.write_u8(0),
                Some(d) => w.write_u8(d.tier() + 1),
            }
        }
        w.into_bytes()
    }

    /// Apply a progress blob produced by [`serialize_progress`](Campaign::serialize_progress) onto
    /// this campaign's topology. The exact inverse of the writer; **never panics** — a malformed or
    /// skewed blob is a [`DeserializeError`] to handle, mirroring the snapshot codec's discipline
    /// (D28): a bad version, a node-count that disagrees with this build's topology
    /// ([`DeserializeError::CorruptState`]), a difficulty byte out of range
    /// ([`DeserializeError::BadTag`]), a short buffer, or trailing bytes are all rejected rather
    /// than silently producing a wrong progress state.
    ///
    /// On success the `cleared` set is replaced wholesale; unlock state is re-derived on the next
    /// read, so no separate "recompute" step is needed.
    pub fn apply_progress(&mut self, bytes: &[u8]) -> Result<(), DeserializeError> {
        let mut r = Reader::new(bytes);
        let ver = r.read_u8()?;
        if ver != PROGRESS_VERSION {
            return Err(DeserializeError::BadVersion(ver));
        }
        let count = r.read_u32()? as usize;
        // Topology skew: a blob for a different campaign shape is rejected, not partially applied.
        if count != self.nodes.len() {
            return Err(DeserializeError::CorruptState);
        }
        let mut cleared = Vec::with_capacity(count);
        for _ in 0..count {
            let byte = r.read_u8()?;
            let slot = match byte {
                0 => None,
                k => Some(Difficulty::from_tier(k - 1).ok_or(DeserializeError::BadTag(k))?),
            };
            cleared.push(slot);
        }
        // Reject trailing bytes (format/version skew) before committing the parsed state.
        r.expect_end()?;
        self.cleared = cleared;
        Ok(())
    }
}

/// One tile on the mission-select hub: a node's identity, its mission handle (the WS-A seam), its
/// title, and the derived progress the tile renders from. Presentation-safe; never checksummed.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct MissionSelectEntry {
    pub node: NodeId,
    pub mission: MissionId,
    pub title: String,
    pub progress: NodeProgress,
}

/// The briefing surface for a single node — what a "launch this mission" screen reads. All
/// presentation data; the launch itself resolves [`mission`](Briefing::mission) to a real mission
/// via the host-side WS-A registry (the integration seam), outside this model.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Briefing {
    pub node: NodeId,
    pub mission: MissionId,
    pub title: String,
    pub briefing: String,
    pub progress: NodeProgress,
    /// True once cleared — the node may be replayed (including at a higher difficulty).
    pub replayable: bool,
    /// Every difficulty tier the shell may offer for this launch (lowest-to-highest).
    pub available_difficulties: Vec<Difficulty>,
}

#[cfg(test)]
mod tests {
    use super::*;

    // A small chain campaign: A -> B -> C (each gates the next). Missions are opaque ids.
    fn chain() -> Campaign {
        Campaign::new(vec![
            OperationNode::new(NodeId(0), MissionId(100), "A", "take the outpost"),
            OperationNode::new(NodeId(1), MissionId(101), "B", "hold the ridge").requires([NodeId(0)]),
            OperationNode::new(NodeId(2), MissionId(102), "C", "seize the base").requires([NodeId(1)]),
        ])
    }

    // A diamond: A unlocks B and C; D needs BOTH B and C cleared.
    fn diamond() -> Campaign {
        Campaign::new(vec![
            OperationNode::new(NodeId(0), MissionId(0), "A", ""),
            OperationNode::new(NodeId(1), MissionId(1), "B", "").requires([NodeId(0)]),
            OperationNode::new(NodeId(2), MissionId(2), "C", "").requires([NodeId(0)]),
            OperationNode::new(NodeId(3), MissionId(3), "D", "").requires([NodeId(1), NodeId(2)]),
        ])
    }

    #[test]
    fn roots_unlocked_successors_locked_at_start() {
        let c = chain();
        assert_eq!(c.progress(NodeId(0)), NodeProgress::Available);
        assert_eq!(c.progress(NodeId(1)), NodeProgress::Locked);
        assert_eq!(c.progress(NodeId(2)), NodeProgress::Locked);
        assert!(c.is_unlocked(NodeId(0)));
        assert!(!c.is_unlocked(NodeId(1)));
    }

    #[test]
    fn clearing_a_node_opens_its_successor_and_only_it() {
        let mut c = chain();
        let outcome = c.clear(NodeId(0), Difficulty::Recruit).unwrap();
        // Clearing A opens exactly B (not C — C still needs B).
        assert_eq!(outcome.newly_unlocked, vec![NodeId(1)]);
        assert!(outcome.raised_difficulty);
        assert_eq!(
            c.progress(NodeId(0)),
            NodeProgress::Cleared { best: Difficulty::Recruit }
        );
        assert_eq!(c.progress(NodeId(1)), NodeProgress::Available);
        // C stays locked: its prerequisite B is not cleared yet.
        assert_eq!(c.progress(NodeId(2)), NodeProgress::Locked);

        // Now clear B → C opens.
        let outcome = c.clear(NodeId(1), Difficulty::Recruit).unwrap();
        assert_eq!(outcome.newly_unlocked, vec![NodeId(2)]);
        assert_eq!(c.progress(NodeId(2)), NodeProgress::Available);
    }

    #[test]
    fn locked_node_cannot_be_cleared() {
        let mut c = chain();
        // B is locked (A not cleared) — clearing it is rejected, and nothing changes.
        assert_eq!(
            c.clear(NodeId(1), Difficulty::Recruit),
            Err(CampaignError::NodeLocked(NodeId(1)))
        );
        assert_eq!(c.progress(NodeId(1)), NodeProgress::Locked);
        assert!(!c.is_cleared(NodeId(1)));
    }

    #[test]
    fn unknown_node_is_rejected() {
        let mut c = chain();
        assert_eq!(
            c.clear(NodeId(99), Difficulty::Recruit),
            Err(CampaignError::UnknownNode(NodeId(99)))
        );
        assert_eq!(c.node(NodeId(99)), None);
        assert_eq!(c.briefing(NodeId(99)), None);
    }

    #[test]
    fn diamond_requires_all_prerequisites_cleared() {
        let mut c = diamond();
        // Clearing A opens both B and C.
        let outcome = c.clear(NodeId(0), Difficulty::Regular).unwrap();
        assert_eq!(outcome.newly_unlocked, vec![NodeId(1), NodeId(2)]);
        // D still locked: needs both B and C.
        assert_eq!(c.progress(NodeId(3)), NodeProgress::Locked);
        // Clearing B alone does NOT open D (C still uncleared).
        let outcome = c.clear(NodeId(1), Difficulty::Regular).unwrap();
        assert!(outcome.newly_unlocked.is_empty());
        assert_eq!(c.progress(NodeId(3)), NodeProgress::Locked);
        // Clearing C — the last prerequisite — finally opens D.
        let outcome = c.clear(NodeId(2), Difficulty::Regular).unwrap();
        assert_eq!(outcome.newly_unlocked, vec![NodeId(3)]);
        assert_eq!(c.progress(NodeId(3)), NodeProgress::Available);
    }

    #[test]
    fn replay_keeps_best_difficulty_and_never_demotes() {
        let mut c = chain();
        // First clear at Regular.
        let o = c.clear(NodeId(0), Difficulty::Regular).unwrap();
        assert!(o.raised_difficulty);
        assert_eq!(c.best_cleared(NodeId(0)), Some(Difficulty::Regular));
        // Replay at a HIGHER tier raises the best.
        let o = c.clear(NodeId(0), Difficulty::Elite).unwrap();
        assert!(o.raised_difficulty);
        assert_eq!(c.best_cleared(NodeId(0)), Some(Difficulty::Elite));
        // Replay at a LOWER tier does NOT demote, and reports no raise.
        let o = c.clear(NodeId(0), Difficulty::Recruit).unwrap();
        assert!(!o.raised_difficulty);
        assert_eq!(c.best_cleared(NodeId(0)), Some(Difficulty::Elite));
        // A cleared node stays playable (replayable) — successors don't re-open spuriously.
        assert!(c.progress(NodeId(0)).is_playable());
        assert!(o.newly_unlocked.is_empty());
    }

    #[test]
    fn difficulty_tiers_are_ordered_and_round_trip_by_rank() {
        assert!(Difficulty::Recruit < Difficulty::Regular);
        assert!(Difficulty::Regular < Difficulty::Veteran);
        assert!(Difficulty::Veteran < Difficulty::Elite);
        for d in Difficulty::ALL {
            assert_eq!(Difficulty::from_tier(d.tier()), Some(d));
        }
        // Out-of-range rank is rejected (a corrupt persistence byte), not guessed.
        assert_eq!(Difficulty::from_tier(4), None);
        // Ids are unique and stable.
        let mut ids: Vec<&str> = Difficulty::ALL.iter().map(|d| d.id()).collect();
        ids.sort_unstable();
        ids.dedup();
        assert_eq!(ids.len(), Difficulty::ALL.len());
    }

    // -------- mission-select + briefing surface --------

    #[test]
    fn mission_select_reflects_derived_progress() {
        let mut c = chain();
        c.clear(NodeId(0), Difficulty::Veteran).unwrap();
        let entries = c.mission_select();
        assert_eq!(entries.len(), 3);
        // In authored order, carrying mission ids and derived progress.
        assert_eq!(entries[0].mission, MissionId(100));
        assert_eq!(
            entries[0].progress,
            NodeProgress::Cleared { best: Difficulty::Veteran }
        );
        assert_eq!(entries[1].progress, NodeProgress::Available);
        assert_eq!(entries[2].progress, NodeProgress::Locked);
    }

    #[test]
    fn briefing_exposes_replay_and_difficulty_options() {
        let mut c = chain();
        // Uncleared root: playable, not replayable.
        let b = c.briefing(NodeId(0)).unwrap();
        assert_eq!(b.mission, MissionId(100));
        assert_eq!(b.title, "A");
        assert!(!b.replayable);
        assert_eq!(b.available_difficulties, Difficulty::ALL.to_vec());
        // After a clear: replayable.
        c.clear(NodeId(0), Difficulty::Recruit).unwrap();
        let b = c.briefing(NodeId(0)).unwrap();
        assert!(b.replayable);
        assert_eq!(b.progress, NodeProgress::Cleared { best: Difficulty::Recruit });
    }

    // -------- persistence round-trip (host blob, outside the checksum) --------

    #[test]
    fn progress_round_trips_through_the_host_blob() {
        let mut c = diamond();
        c.clear(NodeId(0), Difficulty::Elite).unwrap();
        c.clear(NodeId(1), Difficulty::Regular).unwrap();
        c.clear(NodeId(2), Difficulty::Recruit).unwrap();
        let bytes = c.serialize_progress();

        // A FRESH campaign with the same topology, then load the blob — state must match exactly.
        let mut restored = diamond();
        restored.apply_progress(&bytes).unwrap();
        assert_eq!(restored, c);
        // Derived unlock state survives the round-trip too.
        assert_eq!(restored.best_cleared(NodeId(0)), Some(Difficulty::Elite));
        assert_eq!(restored.best_cleared(NodeId(1)), Some(Difficulty::Regular));
        assert_eq!(restored.best_cleared(NodeId(2)), Some(Difficulty::Recruit));
        assert_eq!(restored.progress(NodeId(3)), NodeProgress::Available);
    }

    #[test]
    fn fresh_progress_round_trips_empty() {
        let c = chain();
        let bytes = c.serialize_progress();
        let mut restored = chain();
        restored.apply_progress(&bytes).unwrap();
        assert_eq!(restored, c);
        assert!(!restored.is_cleared(NodeId(0)));
    }

    #[test]
    fn apply_progress_rejects_bad_version() {
        let mut c = chain();
        let mut bytes = c.serialize_progress();
        bytes[0] = 0xFF; // corrupt the version byte
        assert_eq!(
            chain().apply_progress(&bytes),
            Err(DeserializeError::BadVersion(0xFF))
        );
        // Original is untouched on a rejected load.
        assert_eq!(c.apply_progress(&c.clone().serialize_progress()), Ok(()));
    }

    #[test]
    fn apply_progress_rejects_topology_skew() {
        // A blob for the 3-node chain applied to the 4-node diamond is rejected (count mismatch),
        // not partially applied — the load is all-or-nothing.
        let mut chain_c = chain();
        chain_c.clear(NodeId(0), Difficulty::Regular).unwrap();
        let bytes = chain_c.serialize_progress();
        let mut diamond_c = diamond();
        assert_eq!(
            diamond_c.apply_progress(&bytes),
            Err(DeserializeError::CorruptState)
        );
        // Untouched: nothing got applied.
        assert!(!diamond_c.is_cleared(NodeId(0)));
    }

    #[test]
    fn apply_progress_rejects_bad_difficulty_byte() {
        let c = chain();
        let mut bytes = c.serialize_progress();
        // Last byte is node 2's progress slot — set it to a difficulty rank that doesn't exist
        // (tier+1 == 9 ⇒ tier 8). Must be rejected, not clamped.
        *bytes.last_mut().unwrap() = 9;
        assert_eq!(
            chain().apply_progress(&bytes),
            Err(DeserializeError::BadTag(9))
        );
    }

    #[test]
    fn apply_progress_rejects_short_and_trailing() {
        let c = chain();
        let bytes = c.serialize_progress();
        // Truncated mid-stream → UnexpectedEof.
        assert_eq!(
            chain().apply_progress(&bytes[..bytes.len() - 1]),
            Err(DeserializeError::UnexpectedEof)
        );
        // Trailing junk → TrailingBytes.
        let mut extra = bytes.clone();
        extra.push(0);
        assert_eq!(
            chain().apply_progress(&extra),
            Err(DeserializeError::TrailingBytes)
        );
    }
}
