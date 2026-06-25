//! The in-engine **in-session shell** (Phase 4 WS-B, D32 carve-out): pause, surrender/leave,
//! post-match summary, and the reconnect prompt — the only shell surface that is in-engine
//! (`engine`/`render`) rather than native, because it renders under the same avatar-only fog as
//! the match (invariant #6, "the world goes dark").
//!
//! ## What this module is — and is NOT
//!
//! It is a **pure presentation/session state machine** plus the host-side **summary assembler**.
//! It holds *no* sim state and never touches `&mut Sim`: every transition here is a host/session
//! concern (stop/start stepping the local tick accumulator, tear the session down, kick off a
//! reconnect) — exactly the [`SessionAction`](gonedark_core::shell::SessionAction) half of the
//! `core::shell` seam, which by construction never enters the lockstep stream and so can never
//! desync (invariant #1/#7). The control flow in is the seam:
//! [`ShellIntent`](gonedark_core::shell::ShellIntent) → `resolve_intent` →
//! [`ResolvedIntent::Session`](gonedark_core::shell::ResolvedIntent) → [`InSessionShell::apply`].
//!
//! ## The load-bearing pause rule (single-player vs lockstep)
//!
//! In **single-player** a pause may halt the local fixed-tick accumulator — the sim simply stops
//! advancing and resumes bit-identically (a paused sim is byte-identical to a never-paused one
//! once stepping resumes: pause mutates no sim state, `core::shell::MatchPhase::Paused`). In
//! **lockstep** a local pause must NOT stall the shared clock: it is a *local overlay only* — the
//! sim keeps stepping from the lockstep gate while the overlay is up, or every peer would have to
//! agree to pause (which the protocol has no concept of). [`InSessionShell::halts_local_tick`]
//! encodes this distinction and is the single point the host reads to decide whether to drain the
//! accumulator while paused.
//!
//! ## Fairness (invariant #6) — what this module does and does not surface
//!
//! This state machine only carries *which shell surface is up* and the (already-presentation)
//! match summary. It holds no world, no fog mask, and no off-screen unit state, so it cannot leak
//! strategic intel by itself. The fog/visibility for whatever is drawn underneath stays the
//! avatar-only mask the engine already computes while embodied — drawing the overlay never widens
//! it. The full-info post-match summary becomes available only in the [`Ended`](ShellSurface::Ended)
//! surface, which is reached after the match is over (you are no longer embodied), per WS-B.

use gonedark_core::components::{Faction, FACTION_COUNT};
use gonedark_core::event::SimEvent;
use gonedark_core::shell::{
    ConnectionStatus, FactionStats, LinkState, MatchOutcome, MatchSummary, SessionAction,
};

/// Which in-session shell surface is currently up. A small, flat state machine — the in-engine
/// counterpart to `core::shell::MatchPhase`, but tracking the *overlay* the player sees rather than
/// the sim lifecycle. Presentation/session state only; never folded into the checksum.
#[derive(Clone, PartialEq, Eq, Debug, Default)]
pub enum ShellSurface {
    /// No overlay — the match is playing (command view or embodied). The default.
    #[default]
    Playing,
    /// The pause overlay is up. Whether this halts the local tick is the host's read of
    /// [`InSessionShell::halts_local_tick`] (single-player yes, lockstep no).
    Paused,
    /// The match has ended; the post-match summary is shown. Carries the assembled, presentation-
    /// safe [`MatchSummary`] (all integer/`Fixed`, never float — invariant #1).
    Ended(MatchSummary),
    /// The reconnect prompt is up (lockstep stalled / desynced). Offers resume-from-snapshot or
    /// leave. The `state` is why it appeared (Reconnecting vs Desynced), for the prompt copy.
    ReconnectPrompt(LinkState),
}

/// The in-engine in-session shell: the current overlay surface plus whether the local session is
/// single-player (which decides the pause-halts-tick rule). Pure presentation/session state — it
/// owns no `Sim` and can never be handed `&mut Sim`, so it structurally cannot desync lockstep.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct InSessionShell {
    surface: ShellSurface,
    /// True for a single-player session (one peer): a pause may halt the local tick accumulator.
    /// False for a lockstep (multi-peer) session: a local pause is overlay-only and must not stall
    /// the shared clock.
    single_player: bool,
}

impl InSessionShell {
    /// A fresh shell for a session. `single_player` is whether this is a one-peer session (host
    /// reads it from the lockstep peer count): it governs the pause rule and nothing else.
    pub fn new(single_player: bool) -> Self {
        InSessionShell {
            surface: ShellSurface::Playing,
            single_player,
        }
    }

    /// The current overlay surface.
    pub fn surface(&self) -> &ShellSurface {
        &self.surface
    }

    /// Whether the pause overlay is currently up.
    pub fn is_paused(&self) -> bool {
        matches!(self.surface, ShellSurface::Paused)
    }

    /// Whether the match has ended (the post-match summary surface is up).
    pub fn is_ended(&self) -> bool {
        matches!(self.surface, ShellSurface::Ended(_))
    }

    /// The post-match summary, if the [`Ended`](ShellSurface::Ended) surface is up.
    pub fn summary(&self) -> Option<&MatchSummary> {
        match &self.surface {
            ShellSurface::Ended(s) => Some(s),
            _ => None,
        }
    }

    /// **THE load-bearing pause rule.** Whether the host should halt the local fixed-tick
    /// accumulator this frame. True ONLY when paused *and* single-player: a lockstep pause is a
    /// local overlay that must keep stepping from the shared gate (the protocol has no peer-agreed
    /// pause), so it never halts the tick. When not paused, the tick always advances.
    pub fn halts_local_tick(&self) -> bool {
        self.is_paused() && self.single_player
    }

    /// Apply a host-side [`SessionAction`] (the `core::shell` seam's non-sim half — Pause / Resume
    /// / Surrender / RequestReconnect resolved from a [`ShellIntent`](gonedark_core::shell::ShellIntent)).
    /// Pure state transition; mutates only `self`, never any sim state — that is what keeps a
    /// session-control action from ever masquerading as a lockstep-ordered sim command.
    ///
    /// Transition table (anything not listed is a no-op — e.g. Pause while already Ended is
    /// ignored: a finished match cannot be paused):
    /// - `Pause`:   `Playing` → `Paused`.
    /// - `Resume`:  `Paused` → `Playing` (also dismisses a `ReconnectPrompt` — "resume" the link).
    /// - `Surrender`: from `Playing`/`Paused`/`ReconnectPrompt` → `Ended(summary)`. The caller must
    ///   pass the assembled `summary` (the host fills it; see [`assemble_summary`]). Surrender ends
    ///   the *session*; it is **not** a sim "give up" command (invariant #5: no respawn/lives
    ///   machinery, no sim mutation — D34 makes it a host-side `SessionAction`).
    /// - `RequestReconnect`: surfaced via [`request_reconnect`](InSessionShell::request_reconnect),
    ///   not here, because it needs the [`LinkState`] for the prompt copy; this arm is a no-op so a
    ///   bare resolve of `RequestReconnect` can't silently swallow it.
    ///
    /// `surrender_summary` is consumed only on the `Surrender` arm; pass the host's assembled
    /// summary (it is cloned into the `Ended` surface).
    pub fn apply(&mut self, action: SessionAction, surrender_summary: &MatchSummary) {
        match action {
            SessionAction::Pause => {
                if matches!(self.surface, ShellSurface::Playing) {
                    self.surface = ShellSurface::Paused;
                }
            }
            SessionAction::Resume => {
                if matches!(
                    self.surface,
                    ShellSurface::Paused | ShellSurface::ReconnectPrompt(_)
                ) {
                    self.surface = ShellSurface::Playing;
                }
            }
            SessionAction::Surrender => {
                // Surrender/leave ends the session from any live overlay (not from an already-
                // ended match). It tears the session down and shows the summary — no sim command.
                if !self.is_ended() {
                    self.surface = ShellSurface::Ended(surrender_summary.clone());
                }
            }
            SessionAction::RequestReconnect => {
                // Handled by `request_reconnect` (needs the LinkState). No-op here so this arm is
                // explicit and never silently changes state.
            }
        }
    }

    /// Raise (or refresh) the reconnect prompt for the given link `state`. The host calls this when
    /// [`should_prompt_reconnect`] says the link warrants it. Never shown over an ended match.
    /// Returns whether the prompt is now up.
    pub fn request_reconnect(&mut self, state: LinkState) -> bool {
        if self.is_ended() {
            return false;
        }
        self.surface = ShellSurface::ReconnectPrompt(state);
        true
    }

    /// End the match and show the post-match summary (the natural match-over path, distinct from a
    /// player surrender). Idempotent: an already-ended match keeps its first summary.
    pub fn end_match(&mut self, summary: MatchSummary) {
        if !self.is_ended() {
            self.surface = ShellSurface::Ended(summary);
        }
    }
}

/// Whether the host should raise the reconnect prompt, given the lockstep [`ConnectionStatus`]
/// (a pure `core::shell` projection of `core::lockstep`, no sockets). The prompt is warranted when
/// the link is **not** healthy — either stalled waiting on a peer ([`LinkState::Reconnecting`]) or
/// a confirmed cross-client divergence ([`LinkState::Desynced`], invariant #7). A `Connected` link
/// never prompts.
///
/// Pure predicate (the testable "when to show the reconnect prompt" rule WS-B calls out): it makes
/// no I/O and reads only the already-derived status, so it cannot perturb anything.
pub fn should_prompt_reconnect(status: &ConnectionStatus) -> bool {
    matches!(status.state, LinkState::Reconnecting | LinkState::Desynced)
}

/// Assemble the presentation-safe [`MatchSummary`] the `Ended` surface renders, from the match's
/// `events` (the deterministic per-tick [`SimEvent`] stream the host accumulates over the match)
/// plus end-of-match reads the host supplies: `end_tick`, the declared `outcome`, and per-faction
/// `territory_held` / `resources_total` (checksummed end-state reads — the event stream carries
/// produced/lost/killed but not standing territory or the resource purse).
///
/// **Pure, host-side, float-free** (invariant #1): every tally is an integer count derived by
/// scanning the event stream; resources are `i64`. This is the WS-A note made concrete — "the host
/// fills `MatchSummary`; there is no win-condition evaluator in core" (D34). It invents no
/// gameplay: it only counts facts the sim already emitted. Unit-tested as a pure fn (CLAUDE.md
/// testing rule), so the awkward outer end-of-match glue is the only untested seam.
///
/// `end_reads` supplies the two non-event aggregates per faction, indexed by [`Faction::index`].
pub fn assemble_summary(
    events: &[SimEvent],
    end_tick: u64,
    outcome: MatchOutcome,
    end_reads: &[EndStateRead; FACTION_COUNT],
) -> MatchSummary {
    let mut per_faction: [FactionStats; FACTION_COUNT] = Default::default();
    // Seed each slot with its faction tag + the host's end-state reads (territory/resources).
    for f in Faction::ALL {
        let i = f.index();
        per_faction[i] = FactionStats {
            faction: f.into(),
            units_produced: 0,
            units_lost: 0,
            units_killed: 0,
            territory_held: end_reads[i].territory_held,
            resources_total: end_reads[i].resources_total,
        };
    }

    // Count the event-derived tallies. Each is a copy of already-checksummed state (event.rs), so
    // scanning it here is checksum-neutral — it never touches sim state.
    for ev in events {
        match *ev {
            SimEvent::UnitProduced { faction, .. } => {
                per_faction[faction.index()].units_produced += 1;
            }
            SimEvent::Killed {
                faction, source, ..
            } => {
                // The victim's faction loses a unit; the source's faction scores a kill. The
                // source faction is read from the killer entity's faction via the host — but the
                // event only carries the source *entity*, not its faction, so a kill is credited
                // by the host's faction lookup. To stay self-contained and float-free here we
                // credit the kill to the *opposing* side of the victim only when unambiguous
                // (two-faction match); see note below.
                per_faction[faction.index()].units_lost += 1;
                let _ = source;
            }
            _ => {}
        }
    }

    // Kills: in the shipped two-faction skirmish, an enemy unit lost is a player kill and vice
    // versa, so `units_killed[f] = units_lost[opponent]`. This keeps the assembler self-contained
    // (the event carries the source entity, not its faction) without a world read; a true
    // free-for-all would instead resolve `source`'s faction host-side. Neutral kills nobody.
    let player_lost = per_faction[Faction::Player.index()].units_lost;
    let enemy_lost = per_faction[Faction::Enemy.index()].units_lost;
    per_faction[Faction::Player.index()].units_killed = enemy_lost;
    per_faction[Faction::Enemy.index()].units_killed = player_lost;

    MatchSummary {
        outcome,
        end_tick,
        per_faction,
    }
}

/// The two non-event end-of-match aggregates the host reads from checksummed sim state for one
/// faction (the event stream does not carry standing territory or the resource purse). Plain
/// integers/`i64` — no float (invariant #1).
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct EndStateRead {
    /// Territory control points this faction holds at match end.
    pub territory_held: u32,
    /// Total resources this faction has banked at match end (the purse is `i64`).
    pub resources_total: i64,
}

/// One combatant faction's standing forces + score, as the host derives them by scanning
/// checksummed sim state (alive entities by kind, territory, purse) — the inputs the win-condition
/// evaluator reads. **Plain integers/`i64`, no float** (invariant #1), and — critically — this is a
/// *snapshot of already-checksummed state*, not new sim state: deriving it folds nothing new and so
/// cannot perturb the per-tick checksum or desync lockstep (invariants #1/#7). The evaluator takes
/// these (never `&World`), exactly the "extract a pure testable seam" pattern this repo uses, so it
/// is unit-testable without a GPU and structurally cannot reach into the sim.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct FactionForces {
    /// Living `Unit`-kind entities this faction has on the field.
    pub alive_units: u32,
    /// Living `Building`-kind entities this faction has on the field.
    pub buildings: u32,
    /// Territory control points this faction currently holds (the timeout primary tiebreak).
    pub territory_held: u32,
    /// Banked resources (the timeout secondary tiebreak — the purse is `i64`).
    pub resources_total: i64,
}

impl FactionForces {
    /// Whether this faction has been **eliminated**: zero living units *and* zero living buildings.
    /// A faction with even one building (which can still produce) or one unit is still in the match.
    pub fn is_eliminated(&self) -> bool {
        self.alive_units == 0 && self.buildings == 0
    }
}

/// **The match-end / victory-condition evaluator.** A *pure* host-side function of already-derived,
/// already-checksummed state (D34: there is no win-condition evaluator in `core`; the host owns
/// this). It takes the two combatants' [`FactionForces`] plus the elapsed tick and a timeout limit,
/// and returns `Some(outcome)` once the match is decided, or `None` while it is still ongoing.
///
/// Because it reads only `Copy` integer snapshots and never `&World` / `&Sim`, computing it folds
/// nothing into the checksum and so cannot desync (invariants #1/#7); it invents no gameplay — it
/// only *reads off* a winner from facts the sim already settled.
///
/// ## Rules (in priority order)
///
/// 1. **Elimination.** A combatant with zero alive units *and* zero buildings has lost
///    ([`FactionForces::is_eliminated`]). If exactly one combatant survives, that one **wins**
///    immediately (regardless of the clock). If *both* are eliminated in the same evaluation
///    (mutual annihilation), it is a [`Draw`](MatchOutcome::Draw).
/// 2. **Timeout tiebreak.** If neither side is eliminated but `elapsed_ticks >= timeout_ticks`, the
///    match is decided on score: more **territory** held wins; on equal territory, more
///    **resources** wins; on a dead-equal score, [`Draw`](MatchOutcome::Draw).
/// 3. **Ongoing.** Otherwise the match is not over — return `None`.
///
/// `player` / `enemy` are the two combatant factions' forces (Neutral never wins or loses — it
/// holds no army). The caller derives them in the stable [`Faction::ALL`] index order so the inputs
/// are deterministic.
pub fn evaluate_outcome(
    player: FactionForces,
    enemy: FactionForces,
    elapsed_ticks: u64,
    timeout_ticks: u64,
) -> Option<MatchOutcome> {
    let player_out = player.is_eliminated();
    let enemy_out = enemy.is_eliminated();

    // Rule 1 — elimination dominates the clock: a wiped-out side has lost now.
    match (player_out, enemy_out) {
        (false, true) => return Some(MatchOutcome::Victory(Faction::Player)),
        (true, false) => return Some(MatchOutcome::Victory(Faction::Enemy)),
        (true, true) => return Some(MatchOutcome::Draw), // mutual annihilation
        (false, false) => {}                             // both alive — fall through
    }

    // Rule 2 — timeout tiebreak: territory first, then resources, else a true draw.
    if elapsed_ticks >= timeout_ticks {
        let outcome = match player.territory_held.cmp(&enemy.territory_held) {
            core::cmp::Ordering::Greater => MatchOutcome::Victory(Faction::Player),
            core::cmp::Ordering::Less => MatchOutcome::Victory(Faction::Enemy),
            core::cmp::Ordering::Equal => match player.resources_total.cmp(&enemy.resources_total) {
                core::cmp::Ordering::Greater => MatchOutcome::Victory(Faction::Player),
                core::cmp::Ordering::Less => MatchOutcome::Victory(Faction::Enemy),
                core::cmp::Ordering::Equal => MatchOutcome::Draw,
            },
        };
        return Some(outcome);
    }

    // Rule 3 — still ongoing.
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use gonedark_core::components::{Faction, Vec2};
    use gonedark_core::ecs::Entity;
    use gonedark_core::fixed::Fixed;

    fn ent(i: u32) -> Entity {
        Entity {
            index: i,
            generation: 1,
        }
    }
    fn at(x: i32, y: i32) -> Vec2 {
        Vec2::new(Fixed::from_int(x), Fixed::from_int(y))
    }

    fn empty_reads() -> [EndStateRead; FACTION_COUNT] {
        Default::default()
    }

    /// A minimal summary for the surrender-path tests (the assembler is tested separately).
    fn stub_summary() -> MatchSummary {
        assemble_summary(&[], 0, MatchOutcome::Draw, &empty_reads())
    }

    // ---- state machine transitions ----

    #[test]
    fn starts_playing_and_not_paused() {
        let s = InSessionShell::new(true);
        assert_eq!(*s.surface(), ShellSurface::Playing);
        assert!(!s.is_paused());
        assert!(!s.is_ended());
    }

    #[test]
    fn pause_then_resume_round_trips() {
        let mut s = InSessionShell::new(true);
        s.apply(SessionAction::Pause, &stub_summary());
        assert!(s.is_paused());
        assert_eq!(*s.surface(), ShellSurface::Paused);
        s.apply(SessionAction::Resume, &stub_summary());
        assert!(!s.is_paused());
        assert_eq!(*s.surface(), ShellSurface::Playing);
    }

    #[test]
    fn pause_is_ignored_once_ended() {
        // A finished match cannot be paused — Surrender ends it, Pause is then a no-op.
        let mut s = InSessionShell::new(true);
        s.apply(SessionAction::Surrender, &stub_summary());
        assert!(s.is_ended());
        s.apply(SessionAction::Pause, &stub_summary());
        assert!(s.is_ended(), "an ended match stays ended through a stray Pause");
    }

    #[test]
    fn resume_dismisses_reconnect_prompt() {
        let mut s = InSessionShell::new(false);
        assert!(s.request_reconnect(LinkState::Reconnecting));
        assert_eq!(
            *s.surface(),
            ShellSurface::ReconnectPrompt(LinkState::Reconnecting)
        );
        // "Resume" the link dismisses the prompt back to Playing.
        s.apply(SessionAction::Resume, &stub_summary());
        assert_eq!(*s.surface(), ShellSurface::Playing);
    }

    #[test]
    fn surrender_from_any_live_overlay_ends_the_match() {
        for setup in [
            // from Playing
            |s: &mut InSessionShell| {
                let _ = s;
            },
            // from Paused
            |s: &mut InSessionShell| s.apply(SessionAction::Pause, &stub_summary()),
            // from ReconnectPrompt
            |s: &mut InSessionShell| {
                s.request_reconnect(LinkState::Desynced);
            },
        ] {
            let mut s = InSessionShell::new(false);
            setup(&mut s);
            s.apply(SessionAction::Surrender, &stub_summary());
            assert!(s.is_ended(), "surrender must end the session");
            assert!(s.summary().is_some(), "ended surface carries the summary");
        }
    }

    #[test]
    fn surrender_is_not_a_sim_command_and_carries_the_passed_summary() {
        // Surrender ends the *session* (invariant #5: no sim mutation, no respawn machinery). The
        // shell holds no Sim and can't be handed one; here we just confirm it stores exactly the
        // host-assembled summary it was given.
        let mut s = InSessionShell::new(true);
        let summary = assemble_summary(
            &[],
            999,
            MatchOutcome::Victory(Faction::Enemy),
            &empty_reads(),
        );
        s.apply(SessionAction::Surrender, &summary);
        assert_eq!(s.summary(), Some(&summary));
    }

    #[test]
    fn request_reconnect_is_refused_after_match_end() {
        let mut s = InSessionShell::new(false);
        s.end_match(stub_summary());
        assert!(!s.request_reconnect(LinkState::Reconnecting));
        assert!(s.is_ended(), "an ended match never shows the reconnect prompt");
    }

    #[test]
    fn end_match_is_idempotent_keeps_first_summary() {
        let mut s = InSessionShell::new(true);
        let first = assemble_summary(&[], 10, MatchOutcome::Draw, &empty_reads());
        let second = assemble_summary(
            &[],
            20,
            MatchOutcome::Victory(Faction::Player),
            &empty_reads(),
        );
        s.end_match(first.clone());
        s.end_match(second);
        assert_eq!(s.summary(), Some(&first), "first summary wins");
    }

    #[test]
    fn request_reconnect_arm_of_apply_is_a_noop() {
        // A bare resolve of RequestReconnect through `apply` must not change state (it's handled by
        // `request_reconnect`, which carries the LinkState). This pins the explicit no-op.
        let mut s = InSessionShell::new(false);
        s.apply(SessionAction::RequestReconnect, &stub_summary());
        assert_eq!(*s.surface(), ShellSurface::Playing);
    }

    // ---- THE load-bearing pause rule: single-player vs lockstep ----

    #[test]
    fn single_player_pause_halts_the_local_tick() {
        let mut s = InSessionShell::new(/* single_player = */ true);
        assert!(!s.halts_local_tick(), "playing never halts the tick");
        s.apply(SessionAction::Pause, &stub_summary());
        assert!(
            s.halts_local_tick(),
            "single-player pause halts the local tick accumulator"
        );
        s.apply(SessionAction::Resume, &stub_summary());
        assert!(!s.halts_local_tick(), "resume re-advances the tick");
    }

    #[test]
    fn lockstep_pause_never_halts_the_shared_clock() {
        let mut s = InSessionShell::new(/* single_player = */ false);
        s.apply(SessionAction::Pause, &stub_summary());
        assert!(
            s.is_paused(),
            "the overlay is up (a local pause is a local overlay)"
        );
        assert!(
            !s.halts_local_tick(),
            "a lockstep pause must NOT stall the shared clock — overlay only"
        );
    }

    // ---- the "when to show the reconnect prompt" predicate ----

    fn status(state: LinkState) -> ConnectionStatus {
        ConnectionStatus {
            state,
            input_delay: 4,
            next_tick: 100,
        }
    }

    #[test]
    fn drained_desync_projects_to_desynced_and_prompts() {
        // A confirmed cross-client desync drained from the lockstep handle must project to
        // LinkState::Desynced (the most-severe signal — dominates a stall) and prompt (invariant
        // #7). This locks the wire-up: the call site drains take_desyncs() and passes it through.
        use gonedark_core::lockstep::{Desync, Lockstep};
        let ls = Lockstep::new(2, 0, 4);
        let desync = Desync {
            tick: 42,
            peer: 1,
            local: 0xAAAA,
            remote: 0xBBBB,
        };
        // Even with a concurrent stall, the desync dominates.
        let status = ConnectionStatus::project(&ls, /* stalled = */ true, Some(desync));
        assert_eq!(status.state, LinkState::Desynced);
        assert!(
            should_prompt_reconnect(&status),
            "a confirmed desync must raise the reconnect prompt"
        );
    }

    #[test]
    fn reconnect_prompt_supersedes_a_pause_overlay() {
        // A lockstep pause is a local-only overlay while the shared clock ticks, so a stall/desync
        // while the pause menu is open must still reach the player. request_reconnect transitions
        // Paused → ReconnectPrompt (it only refuses an ended match).
        let mut s = InSessionShell::new(/* single_player = */ false);
        s.apply(SessionAction::Pause, &stub_summary());
        assert!(s.is_paused());
        assert!(
            s.request_reconnect(LinkState::Desynced),
            "the prompt must supersede a pause overlay"
        );
        assert_eq!(
            *s.surface(),
            ShellSurface::ReconnectPrompt(LinkState::Desynced)
        );
    }

    #[test]
    fn reconnect_predicate_fires_only_on_unhealthy_links() {
        assert!(
            !should_prompt_reconnect(&status(LinkState::Connected)),
            "a healthy link never prompts"
        );
        assert!(
            should_prompt_reconnect(&status(LinkState::Reconnecting)),
            "a stalled link prompts"
        );
        assert!(
            should_prompt_reconnect(&status(LinkState::Desynced)),
            "a confirmed desync prompts"
        );
    }

    // ---- the post-match summary assembler (pure, float-free) ----

    #[test]
    fn assembler_counts_produced_lost_and_credits_kills_two_faction() {
        // 3 player units produced, 2 enemy produced; 2 player killed, 3 enemy killed. In a two-
        // faction match a side's kills == the opponent's losses.
        let events = vec![
            SimEvent::UnitProduced {
                faction: Faction::Player,
                pos: at(0, 0),
            },
            SimEvent::UnitProduced {
                faction: Faction::Player,
                pos: at(0, 0),
            },
            SimEvent::UnitProduced {
                faction: Faction::Player,
                pos: at(0, 0),
            },
            SimEvent::UnitProduced {
                faction: Faction::Enemy,
                pos: at(0, 0),
            },
            SimEvent::UnitProduced {
                faction: Faction::Enemy,
                pos: at(0, 0),
            },
            // 2 player units killed.
            SimEvent::Killed {
                entity: ent(1),
                faction: Faction::Player,
                source: ent(50),
                pos: at(1, 1),
            },
            SimEvent::Killed {
                entity: ent(2),
                faction: Faction::Player,
                source: ent(51),
                pos: at(1, 1),
            },
            // 3 enemy units killed.
            SimEvent::Killed {
                entity: ent(50),
                faction: Faction::Enemy,
                source: ent(3),
                pos: at(2, 2),
            },
            SimEvent::Killed {
                entity: ent(51),
                faction: Faction::Enemy,
                source: ent(3),
                pos: at(2, 2),
            },
            SimEvent::Killed {
                entity: ent(52),
                faction: Faction::Enemy,
                source: ent(3),
                pos: at(2, 2),
            },
        ];
        let mut reads = empty_reads();
        reads[Faction::Player.index()] = EndStateRead {
            territory_held: 2,
            resources_total: 1500,
        };
        reads[Faction::Enemy.index()] = EndStateRead {
            territory_held: 0,
            resources_total: 200,
        };

        let summary = assemble_summary(
            &events,
            3600,
            MatchOutcome::Victory(Faction::Player),
            &reads,
        );

        assert_eq!(summary.end_tick, 3600);
        let p = summary.faction(Faction::Player);
        let e = summary.faction(Faction::Enemy);
        assert_eq!(p.units_produced, 3);
        assert_eq!(e.units_produced, 2);
        assert_eq!(p.units_lost, 2);
        assert_eq!(e.units_lost, 3);
        // Two-faction kill credit: a side's kills == the opponent's losses.
        assert_eq!(p.units_killed, 3, "player killed the 3 lost enemies");
        assert_eq!(e.units_killed, 2, "enemy killed the 2 lost players");
        // End-state reads carried through.
        assert_eq!(p.territory_held, 2);
        assert_eq!(p.resources_total, 1500);
        assert_eq!(e.resources_total, 200);
        // Outcome carried through.
        match summary.outcome {
            MatchOutcome::Victory(f) => assert_eq!(f, Faction::Player),
            MatchOutcome::Draw => panic!("expected a player victory"),
        }
    }

    #[test]
    fn assembler_on_empty_events_is_zeroed_but_carries_reads_and_outcome() {
        let mut reads = empty_reads();
        reads[Faction::Player.index()] = EndStateRead {
            territory_held: 1,
            resources_total: 42,
        };
        let summary = assemble_summary(&[], 0, MatchOutcome::Draw, &reads);
        for f in Faction::ALL {
            let s = summary.faction(f);
            assert_eq!(s.units_produced, 0);
            assert_eq!(s.units_lost, 0);
            assert_eq!(s.units_killed, 0);
        }
        assert_eq!(summary.faction(Faction::Player).territory_held, 1);
        assert_eq!(summary.faction(Faction::Player).resources_total, 42);
        assert_eq!(summary.outcome, MatchOutcome::Draw);
    }

    #[test]
    fn assembler_resources_total_is_i64() {
        // A compile-time check documenting intent: resources_total is i64 (no float money).
        let summary = assemble_summary(&[], 0, MatchOutcome::Draw, &empty_reads());
        let _total: i64 = summary.faction(Faction::Player).resources_total;
    }

    #[test]
    fn assembler_ignores_non_lifecycle_events() {
        // Damaged/Captured must not perturb the produced/lost/killed tallies.
        let events = vec![
            SimEvent::Damaged {
                entity: ent(1),
                faction: Faction::Player,
                source: ent(2),
                amount: Fixed::from_int(5),
                pos: at(0, 0),
            },
            SimEvent::Captured {
                pos: at(0, 0),
                from: Faction::Neutral,
                to: Faction::Player,
            },
        ];
        let summary = assemble_summary(&events, 5, MatchOutcome::Draw, &empty_reads());
        for f in Faction::ALL {
            let s = summary.faction(f);
            assert_eq!(s.units_produced, 0);
            assert_eq!(s.units_lost, 0);
            assert_eq!(s.units_killed, 0);
        }
    }

    // ---- the match-end / victory-condition evaluator (pure, no &World) ----

    /// A long timeout so the elimination/ongoing tests never trip the clock branch.
    const NEVER: u64 = u64::MAX;

    fn forces(alive_units: u32, buildings: u32, territory_held: u32, resources_total: i64) -> FactionForces {
        FactionForces {
            alive_units,
            buildings,
            territory_held,
            resources_total,
        }
    }

    #[test]
    fn elimination_is_zero_units_and_zero_buildings() {
        // A faction with any unit OR any building is still in the match; only both-zero is out.
        assert!(forces(0, 0, 0, 0).is_eliminated());
        assert!(!forces(1, 0, 0, 0).is_eliminated(), "a unit keeps you in");
        assert!(!forces(0, 1, 0, 0).is_eliminated(), "a building keeps you in");
        assert!(!forces(3, 2, 0, 0).is_eliminated());
    }

    #[test]
    fn player_eliminated_enemy_survives_enemy_wins() {
        // Player wiped (0 units, 0 buildings), enemy still has forces → Enemy victory, now, even
        // though the clock is nowhere near the timeout.
        let out = evaluate_outcome(forces(0, 0, 1, 100), forces(2, 1, 0, 50), 10, NEVER);
        assert_eq!(out, Some(MatchOutcome::Victory(Faction::Enemy)));
    }

    #[test]
    fn enemy_eliminated_player_survives_player_wins() {
        // The plan's named case: P1 (player) drives P2 (enemy) to elimination → player wins.
        let out = evaluate_outcome(forces(4, 1, 2, 300), forces(0, 0, 0, 0), 10, NEVER);
        assert_eq!(out, Some(MatchOutcome::Victory(Faction::Player)));
    }

    #[test]
    fn mutual_elimination_is_a_draw() {
        // Both sides wiped in the same evaluation (e.g. simultaneous last-unit trade) → Draw.
        let out = evaluate_outcome(forces(0, 0, 0, 0), forces(0, 0, 0, 0), 10, NEVER);
        assert_eq!(out, Some(MatchOutcome::Draw));
    }

    #[test]
    fn mutual_survival_is_ongoing() {
        // Both factions still have forces and the timeout has not arrived → the match is not over.
        let out = evaluate_outcome(forces(3, 1, 1, 200), forces(2, 1, 2, 150), 100, NEVER);
        assert_eq!(out, None, "neither eliminated, before timeout → ongoing");
    }

    #[test]
    fn timeout_territory_tiebreak_decides_when_both_alive() {
        // Both alive at the timeout; player holds more territory → player wins (territory is the
        // primary tiebreak, ahead of resources even though the enemy is richer here).
        let out = evaluate_outcome(
            forces(2, 1, 3, 100), // player: more territory, fewer resources
            forces(2, 1, 1, 999), // enemy: less territory, more resources
            3600,
            3600,
        );
        assert_eq!(out, Some(MatchOutcome::Victory(Faction::Player)));

        // Mirror: enemy holds more territory → enemy wins.
        let out = evaluate_outcome(forces(2, 1, 1, 999), forces(2, 1, 3, 0), 3600, 3600);
        assert_eq!(out, Some(MatchOutcome::Victory(Faction::Enemy)));
    }

    #[test]
    fn timeout_resource_tiebreak_decides_on_equal_territory() {
        // Equal territory at the timeout → fall through to resources; player banked more → wins.
        let out = evaluate_outcome(forces(2, 1, 2, 500), forces(2, 1, 2, 200), 3600, 3600);
        assert_eq!(out, Some(MatchOutcome::Victory(Faction::Player)));

        // Mirror: equal territory, enemy richer → enemy wins.
        let out = evaluate_outcome(forces(2, 1, 2, 200), forces(2, 1, 2, 500), 3600, 3600);
        assert_eq!(out, Some(MatchOutcome::Victory(Faction::Enemy)));
    }

    #[test]
    fn timeout_exact_tie_is_a_draw() {
        // Dead-equal territory AND resources at the timeout → an honest Draw.
        let out = evaluate_outcome(forces(2, 1, 2, 200), forces(3, 0, 2, 200), 3600, 3600);
        assert_eq!(out, Some(MatchOutcome::Draw));
    }

    #[test]
    fn timeout_boundary_is_inclusive() {
        // The timeout fires at `elapsed == timeout` (>=), not only strictly past it. One tick
        // earlier it is still ongoing.
        assert_eq!(
            evaluate_outcome(forces(1, 0, 1, 0), forces(1, 0, 0, 0), 3599, 3600),
            None,
            "one tick before the limit → ongoing"
        );
        assert_eq!(
            evaluate_outcome(forces(1, 0, 1, 0), forces(1, 0, 0, 0), 3600, 3600),
            Some(MatchOutcome::Victory(Faction::Player)),
            "at the limit the tiebreak decides"
        );
    }

    #[test]
    fn elimination_beats_the_timeout_clock() {
        // Even past the timeout, an elimination is a clean victory — not the territory tiebreak.
        // Enemy is eliminated but would have LOST the territory tiebreak (player holds 0 vs 2);
        // elimination must still hand the player the win.
        let out = evaluate_outcome(forces(1, 0, 0, 0), forces(0, 0, 2, 999), 9000, 3600);
        assert_eq!(
            out,
            Some(MatchOutcome::Victory(Faction::Player)),
            "elimination wins outright; the clock/tiebreak never runs"
        );
    }
}
