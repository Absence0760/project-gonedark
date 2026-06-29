//! The shell↔sim seam (D32 / Q12) — the narrow, GPU-free, **logic-free** boundary the
//! per-platform app shells reach the shared `core` through.
//!
//! D32 split the meta-UI two ways: the **out-of-match** app shell (title, settings, match
//! setup, store) is **native per-platform** (SwiftUI / Jetpack Compose / desktop), and the
//! **in-session** shell (pause / surrender / post-match summary / reconnect prompt) stays
//! **in-engine** because it draws under avatar-only fog while embodied (invariant #6). BOTH
//! reach `core` only through this module — so the sim, netcode, and order/stance vocabulary
//! stay **single-sourced** in `core` (invariant #2): only the chrome forks, never the game.
//!
//! ## What this module is — and is NOT
//!
//! It is a typed **façade / DTO** boundary, on exactly the same footing as the PAL: it carries
//! no game logic, makes no unit decisions, runs no AI, touches no GPU, and never mutates sim
//! state except by **forwarding a validated intent** to an existing `core` operation. Its job
//! is to *shape* the shell's coarse intents into `core` calls and to *expose* `core`'s state as
//! presentation-safe data — nothing more. The depth of the game lives in `core`'s systems and
//! the order vocabulary (invariant #3), never here.
//!
//! ## Two directions
//!
//! - **Read side (`core` → shell), presentation-safe.** Match lifecycle ([`MatchStatus`]), the
//!   post-match summary ([`MatchSummary`] — every numeric field is `i64`/[`Fixed`], **never** a
//!   float, invariant #1), the order/stance vocabulary as data ([`order_vocabulary`] /
//!   [`stance_vocabulary`], single-sourced from [`crate::components`]), and the lockstep
//!   connection surface ([`ConnectionStatus`], derived from [`crate::lockstep`] — pure data, no
//!   sockets). The **in-session** read view ([`InSessionView`]) is the fairness-critical one
//!   (see below).
//! - **Control side (shell → `core`).** A typed intent enum ([`ShellIntent`]) the host maps into
//!   a [`core` command](crate::sim::Command) (or a non-sim session-control action) via
//!   [`resolve_intent`]. It validates/shapes; it never invents gameplay.
//!
//! ## Checksum neutrality (invariants #1/#7)
//!
//! Everything on the read side is a **derived presentation view** — exactly like
//! [`fog`](crate::fog), [`detection`](crate::detection), and [`alerts`](crate::alerts). It reads
//! state (or already-derived views) and is **never folded into the per-tick checksum**, so
//! computing any of it can never desync lockstep. The seam adds **no** sim-state field: it holds
//! nothing that a tick mutates. (If it ever needed to, that field would have to live in
//! [`crate::ecs::World`] and fold into [`Sim::fold`](crate::sim) — but a *boundary* never owns
//! sim state.)
//!
//! ## Fairness — "the world goes dark" (invariant #6) is STRUCTURAL here
//!
//! The in-session read view ([`InSessionView`]) is what an embodied player's HUD sees, so it can
//! NEVER reveal beyond avatar-only visibility. That guarantee is made **structural, not
//! disciplined**: [`InSessionView::compose`] does not take `&World`. It takes the *already-derived*
//! presentation state — the avatar's [`fog::Visibility`](crate::fog), the [`alerts`](crate::alerts)
//! channel (the only "thread back", game-design §6), and the [`detection`](crate::detection) tells
//! — and merely bundles them for the host. Because the raw world is not in scope, this view
//! *cannot* leak strategic intel even by accident: there is no world to read. The host's
//! contract is to pass [`fog::embodied_visibility`](crate::fog::embodied_visibility) (avatar-only)
//! while embodied, never [`command_visibility`](crate::fog::command_visibility) — and even if it
//! passed the command mask, the seam itself adds zero new disclosure.

use crate::alerts::AlertChannel;
use crate::components::{Faction, Order, Stance, FACTION_COUNT};
use crate::detection::Tell;
use crate::ecs::Entity;
use crate::fog::Visibility;
use crate::lockstep::{Desync, Lockstep};
use crate::sim::Command;

/// The faction-identity type ([`Army`](crate::components::Army)) re-exported through the seam, so a
/// native match-setup shell reaches the US/FR selection vocabulary from the single `core::shell`
/// import surface (factions-plan WS-A, D68) — the same single-sourcing as the order/stance vocab and
/// the campaign types. It is plain presentation-safe data (a `repr`-stable tag, no float, no sim
/// state); the actual per-side choice travels as a [`ShellIntent::SelectArmy`] → [`Command::SelectArmy`].
/// (A `pub use` is also in scope locally, so the seam's [`ShellIntent`] names it directly.)
pub use crate::components::Army;

// ===========================================================================
// READ SIDE — match lifecycle
// ===========================================================================

/// Where a match is in its lifecycle, for the shells that frame it (a native lobby before it
/// starts; the in-engine pause/summary while/after it runs). Presentation only — the sim has no
/// "phase" field; the host owns this enum and drives it from session events.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum MatchPhase {
    /// No match running yet (the native out-of-match shell is in front — title/lobby/setup).
    #[default]
    NotStarted,
    /// A match is live and ticking.
    Running,
    /// A match is live but the in-engine pause shell is up (the sim is not advancing). Pause is
    /// a host/session concern — it stops calling [`Sim::step`](crate::sim), it does not mutate
    /// sim state — so it never desyncs (it is not even a `core` command).
    Paused,
    /// The match is over; [`MatchStatus::summary`] carries the post-match summary.
    Ended,
}

/// The lifecycle status the shells read each frame. `tick` is the sim's current tick (so a HUD
/// can show match time); `summary` is present iff `phase == Ended`.
#[derive(Clone, PartialEq, Eq, Debug, Default)]
pub struct MatchStatus {
    pub phase: MatchPhase,
    /// The sim tick this status reflects (presentation/time display only).
    pub tick: u64,
    /// The post-match summary — `Some` exactly when `phase == MatchPhase::Ended`.
    pub summary: Option<MatchSummary>,
}

// ===========================================================================
// READ SIDE — post-match summary (all integer / fixed-point, NEVER float)
// ===========================================================================

/// Who won, from the perspective of the local player's faction. A draw is its own variant rather
/// than a sentinel so the shell never has to special-case a magic value.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum MatchOutcome {
    /// The given faction won the match.
    Victory(Faction),
    /// No faction met the win condition (timeout / mutual elimination).
    Draw,
}

/// Per-faction aggregate stats for the post-match summary. Every field is an **integer count** —
/// there is no float in the summary (invariant #1); a derived ratio a shell wants (e.g. K/D) is
/// the shell's own presentation math, computed from these integers above the seam.
///
/// These are aggregates the host accumulates over the match (counting [`SimEvent`](crate::event)s
/// is the natural source) plus end-of-match reads of checksummed sim state (territory held,
/// resource total). The seam defines the *shape*; the host fills it — the seam invents no
/// gameplay tally of its own.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct FactionStats {
    pub faction: FactionTag,
    /// Units this faction produced over the match.
    pub units_produced: u32,
    /// Units of this faction lost (killed) over the match.
    pub units_lost: u32,
    /// Enemy units this faction killed over the match.
    pub units_killed: u32,
    /// Territory control points this faction holds at match end.
    pub territory_held: u32,
    /// Total resources this faction banked over the match (the economy purse is `i64`, never
    /// float money — invariant #1).
    pub resources_total: i64,
}

/// A faction tag carried inside [`FactionStats`] so the struct is `Default`-able for fixed-size
/// arrays. Mirrors [`Faction`] one-to-one; converted at the boundary.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum FactionTag {
    #[default]
    Player,
    Enemy,
    Neutral,
}

impl From<Faction> for FactionTag {
    fn from(f: Faction) -> Self {
        match f {
            Faction::Player => FactionTag::Player,
            Faction::Enemy => FactionTag::Enemy,
            Faction::Neutral => FactionTag::Neutral,
        }
    }
}

impl From<FactionTag> for Faction {
    fn from(f: FactionTag) -> Self {
        match f {
            FactionTag::Player => Faction::Player,
            FactionTag::Enemy => Faction::Enemy,
            FactionTag::Neutral => Faction::Neutral,
        }
    }
}

/// The post-match summary the in-engine end-screen renders and (a digest of) the native shell
/// shows in the profile/history. All numerics are integer/`Fixed` (invariant #1).
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct MatchSummary {
    pub outcome: MatchOutcome,
    /// The tick the match ended on (its length).
    pub end_tick: u64,
    /// Per-faction stats in fixed [`Faction::ALL`] order — a fixed-size array, so it is
    /// allocation-free and its order is deterministic by construction.
    pub per_faction: [FactionStats; FACTION_COUNT],
}

impl MatchSummary {
    /// The stats for one faction (a convenience read for a shell that wants just one side).
    pub fn faction(&self, faction: Faction) -> &FactionStats {
        &self.per_faction[faction.index()]
    }
}

// ===========================================================================
// READ SIDE — the order / stance vocabulary, exposed as DATA (single-source)
// ===========================================================================

/// The order *vocabulary* as data — one variant per [`Order`] shape, WITHOUT the per-instance
/// payloads (target points, patrol legs). A native match-setup/settings shell lists these to
/// build its command palette without re-declaring the vocabulary (invariant #2: the vocab is
/// single-sourced in [`crate::components::Order`]). The actual order a player issues still travels
/// as a typed [`Command`] with its real fixed-point payload through [`resolve_intent`].
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum OrderKind {
    Idle,
    MoveTo,
    AttackMove,
    Patrol,
    HoldPosition,
    FallBack,
}

impl OrderKind {
    /// The kind of an actual [`Order`] — the single mapping point, so adding an `Order` variant
    /// is a compile error here until the vocabulary list is updated (it keeps the data view and
    /// the real enum from drifting).
    pub fn of(order: &Order) -> OrderKind {
        match order {
            Order::Idle => OrderKind::Idle,
            Order::MoveTo(_) => OrderKind::MoveTo,
            Order::AttackMove(_) => OrderKind::AttackMove,
            Order::Patrol { .. } => OrderKind::Patrol,
            Order::HoldPosition => OrderKind::HoldPosition,
            Order::FallBack(_) => OrderKind::FallBack,
        }
    }

    /// A stable, human-readable id a native shell can key a localized label off (never the label
    /// itself — localization is the shell's job, above the seam).
    pub fn id(self) -> &'static str {
        match self {
            OrderKind::Idle => "idle",
            OrderKind::MoveTo => "move_to",
            OrderKind::AttackMove => "attack_move",
            OrderKind::Patrol => "patrol",
            OrderKind::HoldPosition => "hold_position",
            OrderKind::FallBack => "fall_back",
        }
    }
}

/// The stance vocabulary as data — one per [`Stance`].
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum StanceKind {
    HoldFire,
    ReturnFire,
    FireAtWill,
}

impl StanceKind {
    pub fn of(stance: Stance) -> StanceKind {
        match stance {
            Stance::HoldFire => StanceKind::HoldFire,
            Stance::ReturnFire => StanceKind::ReturnFire,
            Stance::FireAtWill => StanceKind::FireAtWill,
        }
    }

    /// The real [`Stance`] this kind denotes (the inverse of [`of`](StanceKind::of)).
    pub fn to_stance(self) -> Stance {
        match self {
            StanceKind::HoldFire => Stance::HoldFire,
            StanceKind::ReturnFire => Stance::ReturnFire,
            StanceKind::FireAtWill => Stance::FireAtWill,
        }
    }

    pub fn id(self) -> &'static str {
        match self {
            StanceKind::HoldFire => "hold_fire",
            StanceKind::ReturnFire => "return_fire",
            StanceKind::FireAtWill => "fire_at_will",
        }
    }
}

/// The complete order vocabulary, in a fixed declaration order. A shell iterates this to build
/// its order palette — single-sourced, so it can never drift from [`Order`].
pub const fn order_vocabulary() -> [OrderKind; 6] {
    [
        OrderKind::Idle,
        OrderKind::MoveTo,
        OrderKind::AttackMove,
        OrderKind::Patrol,
        OrderKind::HoldPosition,
        OrderKind::FallBack,
    ]
}

/// The complete stance vocabulary, in a fixed declaration order.
pub const fn stance_vocabulary() -> [StanceKind; 3] {
    [
        StanceKind::HoldFire,
        StanceKind::ReturnFire,
        StanceKind::FireAtWill,
    ]
}

// ===========================================================================
// READ SIDE — lockstep / connection status (plain data; NO I/O, NO sockets)
// ===========================================================================

/// How the local peer's lockstep session is faring, as plain data for the reconnect-prompt /
/// connection HUD. Derived purely from [`Lockstep`] state — **no networking** (the sockets live
/// in the PAL transport, D27); this is a read-only projection (invariant #2).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum LinkState {
    /// Stepping normally — every peer's input for the next tick is in hand.
    Connected,
    /// Stalled waiting on a peer's input (the gate has not cleared). The host surfaces a
    /// "reconnecting / waiting for players" prompt; whether to actually reconnect is the host's
    /// call (this is detection, not policy).
    Reconnecting,
    /// A cross-client checksum disagreement was detected — a real desync (invariant #7). Pure
    /// detection: surfacing it never alters stepping (that policy is the host's, D27).
    Desynced,
}

/// The connection surface the shell reads: the link state, the current input delay (ticks), and
/// the next tick the sim will execute. All integers — no float, no clock (the host owns RTT;
/// `core` reads no wall-clock, invariant #1/#2).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct ConnectionStatus {
    pub state: LinkState,
    /// The session's current input delay in ticks (adaptive via the lockstep delay protocol).
    pub input_delay: u64,
    /// The next tick the sim will execute (lets a HUD show how far behind the gate is).
    pub next_tick: u64,
}

impl ConnectionStatus {
    /// Project a [`Lockstep`]'s public state into the shell's connection surface.
    ///
    /// `stalled` is whether the host's last [`Lockstep::try_advance`](crate::lockstep::Lockstep::try_advance)
    /// returned `None` (waiting on a peer) — a host observation, since `try_advance` consumes the
    /// gate and the seam must stay read-only here. `recent_desync` is the most recent
    /// [`Desync`] the host drained via
    /// [`take_desyncs`](crate::lockstep::Lockstep::take_desyncs), if any. Desync dominates a
    /// stall (a confirmed divergence is the more severe signal).
    pub fn project(live: &Lockstep, stalled: bool, recent_desync: Option<Desync>) -> ConnectionStatus {
        let state = if recent_desync.is_some() {
            LinkState::Desynced
        } else if stalled {
            LinkState::Reconnecting
        } else {
            LinkState::Connected
        };
        ConnectionStatus {
            state,
            input_delay: live.delay(),
            next_tick: live.next_tick(),
        }
    }
}

// ===========================================================================
// READ SIDE — the IN-SESSION view (fairness-critical: invariant #6 is STRUCTURAL)
// ===========================================================================

/// What the **in-engine in-session shell sees while embodied** — bundled *already-derived*
/// presentation state, never the raw world. This is the load-bearing fairness boundary
/// (invariant #6, "the world goes dark"): an embodied player's HUD must reveal nothing beyond
/// avatar-only visibility.
///
/// That guarantee is **structural**: [`compose`](InSessionView::compose) does not take `&World`.
/// It takes:
/// - `visibility` — the avatar's fog mask. The host MUST pass
///   [`fog::embodied_visibility`](crate::fog::embodied_visibility) (avatar-only) while embodied,
///   not [`command_visibility`](crate::fog::command_visibility) (full strategic). The seam holds
///   only what it is handed, so it cannot widen the disclosure.
/// - `alerts` — the [`AlertChannel`], the *only* thread back to command (game-design §6: "alerts,
///   not intel" — a direction, never a map reveal).
/// - `tells` — the [`detection`](crate::detection) tells about enemy embodied units, themselves a
///   fog/LoS-gated, checksum-excluded derivation (D33). In `Hidden` mode this is empty.
///
/// Because the raw world is not in scope, this view literally *cannot* leak strategic intel — it
/// has no world to read. It is a presentation bundle, never folded into the checksum.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct InSessionView {
    /// The avatar-only fog mask (the host's contract: embodied visibility, not command).
    pub visibility: Visibility,
    /// The thread back to command — directional alerts, not intel.
    pub alerts: AlertChannel,
    /// Enemy embodied-unit tells (D33), already fog/LoS-gated and aged. Empty in `Hidden`.
    pub tells: Vec<Tell>,
}

impl InSessionView {
    /// Bundle the already-derived presentation state for the embodied HUD. Takes ONLY derived
    /// views — never `&World` — so it is structurally incapable of revealing beyond what the host
    /// already computed for the avatar (invariant #6). This is a move/clone of presentation data;
    /// it runs no logic and touches no sim state.
    pub fn compose(visibility: Visibility, alerts: AlertChannel, tells: Vec<Tell>) -> InSessionView {
        InSessionView {
            visibility,
            alerts,
            tells,
        }
    }
}

// ===========================================================================
// CONTROL SIDE — typed intents (shell → core). Validates/shapes; NO game logic.
// ===========================================================================

/// A coarse intent a shell raises, to be resolved into a `core` operation. These are the *only*
/// things a shell may ask the sim to do; the host translates each via [`resolve_intent`].
///
/// All payloads are `Copy` handle/fixed-point data — **no float crosses this boundary into the
/// sim** (invariant #1). The intent vocabulary is deliberately the in-session shell's surface
/// (pause/resume/surrender/reconnect + the embodiment toggle); the full RTS order palette is
/// issued as [`Command`]s directly by the command-layer input path, not re-wrapped here.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ShellIntent {
    /// Pause the match (in-engine pause shell). A **session-control** action — it stops the host
    /// from stepping the sim; it is NOT a sim command and mutates no sim state, so it cannot
    /// desync (the sim is bit-identical to a never-paused peer once stepping resumes).
    Pause,
    /// Resume from pause (session-control, as above).
    Resume,
    /// Surrender / leave the match. Session-control: the host tears the session down and shows
    /// the summary; there is no "surrender" sim command (the sim has no concept of giving up).
    Surrender,
    /// Possess a unit — flips its input source to live player input + goes dark (invariant #5).
    /// Maps to [`Command::Embody`].
    Embody { entity: Entity },
    /// Release a possessed unit back to order-driven control. Maps to [`Command::Surface`].
    Surface { entity: Entity },
    /// Request a reconnect/resync from the last authoritative snapshot. Session-control: the host
    /// drives [`reconnect`](crate::reconnect); the seam does no I/O.
    RequestReconnect,
    /// **Match setup**: select which [`Army`] identity a [`Faction`] fields (US vs FR — factions-plan
    /// WS-A, D68). The native lobby/army-select shell (WS-D) raises this; it resolves to a sim
    /// [`Command::SelectArmy`] the host feeds the lockstep stream, so the matchup is set identically
    /// on every peer (invariant #7). A coarse intent of `Copy` tag data — no float crosses the seam.
    SelectArmy { faction: Faction, army: Army },
}

/// What a [`ShellIntent`] resolves to. Either a sim [`Command`] the host feeds the lockstep
/// stream, or a **session-control** action the host performs *around* the sim (pause/resume/
/// teardown/reconnect) — never a sim mutation. Keeping the two arms distinct is what stops a
/// session-control action (pause) from ever masquerading as a sim command (which would have to be
/// lockstep-ordered and could desync).
///
/// (`Command` deliberately has no `PartialEq` — the lockstep codec compares re-encoded bytes
/// instead — so this enum is not `PartialEq` either; tests pattern-match the arms.)
#[derive(Clone, Copy, Debug)]
pub enum ResolvedIntent {
    /// Forward this command to the sim through the lockstep stream (the host stamps + sends it).
    Command(Command),
    /// Perform this session-control action host-side; it never touches sim state.
    Session(SessionAction),
}

/// A host-side session-control action — the non-sim half of [`ResolvedIntent`]. The seam names
/// these; the host carries them out (stop/start stepping, tear down, kick off a reconnect). None
/// of them mutate sim state, so none can desync lockstep.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum SessionAction {
    Pause,
    Resume,
    Surrender,
    RequestReconnect,
}

/// Resolve a [`ShellIntent`] into the `core` operation it denotes. Pure shaping: it maps an
/// intent to a [`Command`] or a [`SessionAction`] and does **nothing else** — no validation of
/// game rules, no unit decisions, no sim access (the literal-executor brain stays in `core`,
/// invariant #3). Whether the entity is alive / embodiable is the sim's call when it applies the
/// command ([`Sim::apply`](crate::sim) already guards `is_alive`), exactly as for any other
/// command — the seam does not duplicate that logic.
pub fn resolve_intent(intent: ShellIntent) -> ResolvedIntent {
    match intent {
        ShellIntent::Embody { entity } => ResolvedIntent::Command(Command::Embody { entity }),
        ShellIntent::Surface { entity } => ResolvedIntent::Command(Command::Surface { entity }),
        ShellIntent::Pause => ResolvedIntent::Session(SessionAction::Pause),
        ShellIntent::Resume => ResolvedIntent::Session(SessionAction::Resume),
        ShellIntent::Surrender => ResolvedIntent::Session(SessionAction::Surrender),
        ShellIntent::RequestReconnect => ResolvedIntent::Session(SessionAction::RequestReconnect),
        // Match-setup army pick → a sim command (it must be lockstep-ordered so every peer agrees on
        // the matchup), exactly like Embody/Surface — not a host-side session action.
        ShellIntent::SelectArmy { faction, army } => {
            ResolvedIntent::Command(Command::SelectArmy { faction, army })
        }
    }
}

// ===========================================================================
// READ SIDE — Operations-hub / campaign meta-progression (host-side; NOT checksummed)
// ===========================================================================
//
// The out-of-match (native) shell reaches the campaign node-graph through this seam, exactly as it
// reaches the order/stance vocabulary and match summary above. The model itself lives in
// [`crate::campaign`] (it owns the persistence codec and the unlock graph); these re-exports make
// `core::shell` the single import surface the shell uses for *all* meta-UI data.
//
// Like everything else on this read side it is **host-side, derived/owned state — never sim state,
// never folded into the per-tick checksum** (invariants #1/#7). Campaign progress persists to its
// own host blob ([`Campaign::serialize_progress`](crate::campaign::Campaign::serialize_progress)),
// separate from the authoritative [`Sim::serialize`](crate::sim::Sim::serialize) snapshot, so
// meta-progression can never leak into the checksum fold. See [`crate::campaign`] for the full
// rationale and the **WS-A integration seam** ([`MissionId`] is opaque until the mission/objective
// core lands).
pub use crate::campaign::{
    Briefing, Campaign, CampaignError, ClearOutcome, Difficulty, MissionId, MissionSelectEntry,
    NodeId, NodeProgress, OperationNode,
};

#[cfg(test)]
mod tests {
    use super::*;
    use crate::components::{EntityKind, InputSource, Vec2};
    use crate::detection::{detectable_embodiment, DetectionConfig, DetectionMemory};
    use crate::fixed::Fixed;
    use crate::ecs::World;
    use crate::fog::{command_visibility, embodied_visibility};
    use crate::terrain::Terrain;

    fn fx(n: i32) -> Fixed {
        Fixed::from_int(n)
    }
    fn at(x: i32, y: i32) -> Vec2 {
        Vec2::new(fx(x), fx(y))
    }

    fn spawn_unit(world: &mut World, faction: Faction, pos: Vec2, vision: Fixed) -> Entity {
        let e = world.spawn();
        let i = e.index as usize;
        world.pos[i] = pos;
        world.faction[i] = faction;
        world.kind[i] = EntityKind::Unit;
        world.vision[i] = vision;
        e
    }

    // ---- order / stance vocabulary is single-sourced from core (invariant #2) ----

    #[test]
    fn order_vocabulary_covers_every_order_variant() {
        // Build one of every Order variant and assert each maps to a distinct OrderKind that is
        // present in the exported vocabulary. If a new Order variant is added, `OrderKind::of`
        // stops compiling until it's handled — and this test ensures the exported list lists it.
        let samples = [
            Order::Idle,
            Order::MoveTo(at(1, 1)),
            Order::AttackMove(at(2, 2)),
            Order::Patrol {
                a: at(0, 0),
                b: at(3, 3),
                toward_b: true,
            },
            Order::HoldPosition,
            Order::FallBack(at(4, 4)),
        ];
        let vocab = order_vocabulary();
        assert_eq!(
            samples.len(),
            vocab.len(),
            "every Order variant must have exactly one OrderKind in the vocabulary"
        );
        for order in &samples {
            let kind = OrderKind::of(order);
            assert!(
                vocab.contains(&kind),
                "OrderKind {kind:?} for {order:?} missing from the exported vocabulary"
            );
        }
        // Ids are unique and stable (a shell keys localized labels off them).
        let mut ids: Vec<&str> = vocab.iter().map(|k| k.id()).collect();
        ids.sort_unstable();
        ids.dedup();
        assert_eq!(ids.len(), vocab.len(), "order ids must be unique");
    }

    #[test]
    fn stance_vocabulary_round_trips_every_stance() {
        let samples = [Stance::HoldFire, Stance::ReturnFire, Stance::FireAtWill];
        let vocab = stance_vocabulary();
        assert_eq!(samples.len(), vocab.len());
        for stance in samples {
            let kind = StanceKind::of(stance);
            assert!(vocab.contains(&kind));
            // The kind round-trips back to the exact stance (so a shell selection is faithful).
            assert_eq!(kind.to_stance(), stance);
        }
        let mut ids: Vec<&str> = vocab.iter().map(|k| k.id()).collect();
        ids.sort_unstable();
        ids.dedup();
        assert_eq!(ids.len(), vocab.len(), "stance ids must be unique");
    }

    // ---- control intents map to the right core operation (no game logic) ----

    #[test]
    fn embody_surface_intents_map_to_sim_commands() {
        let e = Entity {
            index: 7,
            generation: 2,
        };
        match resolve_intent(ShellIntent::Embody { entity: e }) {
            ResolvedIntent::Command(Command::Embody { entity }) => assert_eq!(entity, e),
            other => panic!("Embody intent must map to Command::Embody, got {other:?}"),
        }
        match resolve_intent(ShellIntent::Surface { entity: e }) {
            ResolvedIntent::Command(Command::Surface { entity }) => assert_eq!(entity, e),
            other => panic!("Surface intent must map to Command::Surface, got {other:?}"),
        }
    }

    #[test]
    fn select_army_intent_maps_to_a_sim_command() {
        // The army-select intent (factions-plan WS-A) must resolve to a lockstep-ordered
        // Command::SelectArmy (not a host-side session action) so every peer agrees on the matchup.
        match resolve_intent(ShellIntent::SelectArmy {
            faction: Faction::Player,
            army: Army::Us,
        }) {
            ResolvedIntent::Command(Command::SelectArmy { faction, army }) => {
                assert_eq!(faction, Faction::Player);
                assert_eq!(army, Army::Us);
            }
            other => panic!("SelectArmy intent must map to Command::SelectArmy, got {other:?}"),
        }
    }

    #[test]
    fn session_control_intents_are_never_sim_commands() {
        // Pause/resume/surrender/reconnect must NOT become sim commands — they are host-side
        // session control (so they can never enter the lockstep stream or desync).
        for (intent, expected) in [
            (ShellIntent::Pause, SessionAction::Pause),
            (ShellIntent::Resume, SessionAction::Resume),
            (ShellIntent::Surrender, SessionAction::Surrender),
            (
                ShellIntent::RequestReconnect,
                SessionAction::RequestReconnect,
            ),
        ] {
            match resolve_intent(intent) {
                ResolvedIntent::Session(action) => assert_eq!(action, expected),
                ResolvedIntent::Command(c) => {
                    panic!("session-control intent {intent:?} leaked as sim command {c:?}")
                }
            }
        }
    }

    // ---- post-match summary is float-free and addressable by faction ----

    #[test]
    fn summary_is_integer_only_and_addressable() {
        let mut per_faction: [FactionStats; FACTION_COUNT] = Default::default();
        for f in Faction::ALL {
            per_faction[f.index()] = FactionStats {
                faction: f.into(),
                units_produced: 5,
                units_lost: 2,
                units_killed: 3,
                territory_held: 1,
                resources_total: 1234,
            };
        }
        let summary = MatchSummary {
            outcome: MatchOutcome::Victory(Faction::Player),
            end_tick: 3600,
            per_faction,
        };
        // Addressable by faction, in fixed order.
        assert_eq!(summary.faction(Faction::Enemy).units_killed, 3);
        assert_eq!(summary.faction(Faction::Player).resources_total, 1234);
        // resources_total is i64 (no float money) — a trivially true type check that documents
        // intent: this line would not compile if the field were a floating-point type.
        let _total: i64 = summary.faction(Faction::Player).resources_total;
        match summary.outcome {
            MatchOutcome::Victory(f) => assert_eq!(f, Faction::Player),
            MatchOutcome::Draw => panic!("expected a victory"),
        }
    }

    #[test]
    fn faction_tag_round_trips() {
        for f in Faction::ALL {
            let tag: FactionTag = f.into();
            let back: Faction = tag.into();
            assert_eq!(f, back);
        }
    }

    // ---- connection status is a pure projection of lockstep state ----

    #[test]
    fn connection_status_projects_lockstep_state() {
        let ls = Lockstep::new(2, 0, 4);
        // No stall, no desync → connected, with the configured delay surfaced.
        let s = ConnectionStatus::project(&ls, false, None);
        assert_eq!(s.state, LinkState::Connected);
        assert_eq!(s.input_delay, 4);
        assert_eq!(s.next_tick, 0);
        // Stalled → reconnecting.
        let s = ConnectionStatus::project(&ls, true, None);
        assert_eq!(s.state, LinkState::Reconnecting);
        // A detected desync dominates a stall (the more severe signal wins).
        let d = Desync {
            tick: 9,
            peer: 1,
            local: 0xAAAA,
            remote: 0xBBBB,
        };
        let s = ConnectionStatus::project(&ls, true, Some(d));
        assert_eq!(s.state, LinkState::Desynced);
    }

    // ---- THE load-bearing fairness test (invariant #6) ----

    #[test]
    fn in_session_view_cannot_reveal_beyond_avatar_visibility() {
        // Two friendly units far apart, plus a far enemy. While embodied in ONE unit, the
        // in-session view the shell sees must reveal ONLY the avatar's surroundings — never the
        // far friendly unit's area, never the far enemy. This is structural: `compose` is handed
        // the avatar-only fog mask and has no `&World` to widen it.
        let mut world = World::new();
        let avatar = spawn_unit(&mut world, Faction::Player, at(-30, -30), fx(12));
        world.input_source[avatar.index as usize] = InputSource::Embodied;
        let _far_friend = spawn_unit(&mut world, Faction::Player, at(30, 30), fx(12));
        let _far_enemy = spawn_unit(&mut world, Faction::Enemy, at(40, 40), fx(12));
        let terrain = Terrain::open();

        // The host's contract while embodied: pass the AVATAR-ONLY mask.
        let avatar_vis = embodied_visibility(&world, &terrain, avatar);
        let alerts = AlertChannel::new();
        // Detection tells, fog/LoS-gated (default Subtle). The far enemy is out of any observer's
        // sight range here, so there is nothing to leak through the tells either.
        let mut mem = DetectionMemory::new();
        let tells = detectable_embodiment(
            &world,
            &terrain,
            &DetectionConfig::default(),
            Faction::Player,
            0,
            &mut mem,
        );

        let view = InSessionView::compose(avatar_vis, alerts, tells);

        // Avatar's own cell: visible.
        assert!(view.visibility.is_visible(at(-30, -30)));
        // The far friendly unit's area: DARK (the strategic map went dark — invariant #6).
        assert!(
            !view.visibility.is_visible(at(30, 30)),
            "in-session view leaked the far friendly unit's area while embodied"
        );
        // The far enemy's area: DARK.
        assert!(
            !view.visibility.is_visible(at(40, 40)),
            "in-session view leaked an enemy area beyond avatar sight"
        );
        // And the tells reveal nothing here (the only embodied unit is the local avatar, which is
        // friendly — detection only tells HOSTILE embodied units).
        assert!(
            view.tells.is_empty(),
            "no hostile embodied unit in sight → no tell may appear"
        );

        // Contrast: the COMMAND view (what the seam must NOT hand the embodied shell) DOES light
        // the far friendly area — proving the difference is genuinely the avatar-only restriction,
        // not just an empty world.
        let command_vis = command_visibility(&world, &terrain, Faction::Player);
        assert!(
            command_vis.is_visible(at(30, 30)),
            "sanity: command view sees the far friendly area (so the dark above is meaningful)"
        );
    }

    #[test]
    fn in_session_view_is_checksum_neutral() {
        // Composing the in-session view every tick must never perturb the sim checksum — it is a
        // read-only presentation bundle over already-derived views (invariants #1/#7, #6). Mirror
        // detection.rs's checksum-neutrality guard.
        use crate::sim::Sim;
        let seed = 0x5_4E11_u64; // distinct seed for this test
        let mut with = Sim::new(seed);
        let mut without = Sim::new(seed);
        for sim in [&mut with, &mut without] {
            let e = sim.world.spawn();
            let i = e.index as usize;
            sim.world.kind[i] = EntityKind::Unit;
            sim.world.faction[i] = Faction::Player;
            sim.world.pos[i] = at(0, 0);
            sim.world.input_source[i] = InputSource::Embodied;
        }
        let avatar = with.world.entity(0).unwrap();
        let mut mem = DetectionMemory::new();
        for t in 0..30u64 {
            with.step(&[]);
            without.step(&[]);
            // Compute the full in-session view on `with` every tick.
            let vis = embodied_visibility(&with.world, &with.terrain, avatar);
            let mut alerts = AlertChannel::new();
            alerts.ingest(with.events(), &with.world, Faction::Player, t);
            let tells = detectable_embodiment(
                &with.world,
                &with.terrain,
                &DetectionConfig::default(),
                Faction::Player,
                t,
                &mut mem,
            );
            let _view = InSessionView::compose(vis, alerts, tells);
            assert_eq!(
                with.checksum(),
                without.checksum(),
                "composing the in-session view must not change the sim checksum at tick {t}"
            );
        }
    }
}
