//! Host-side **mission objectives** (PvE WS-A) — the OBSERVE-not-mutate layer.
//!
//! ## The load-bearing architecture call (pve-campaign-plan)
//!
//! An [`ObjectiveSet`] is evaluated **after `Sim::step`**, reading the per-tick deterministic
//! [`SimEvent`] stream + the already-derived per-faction [`FactionForces`] — the **exact footing**
//! the win/lose evaluator already stands on
//! ([`evaluate_outcome`](crate::session_shell::evaluate_outcome), D38) and the same footing as
//! fog/alerts/tell. Because objectives **observe** the sim and never **change** it, folding them
//! into the checksum would add desync surface for zero benefit: keeping them host-side means
//! missions are authored, tuned, and reshuffled with **no lockstep risk** (invariant #7) and **no
//! new cross-arch coverage** for the objective layer itself. This type owns no `Sim`, can never be
//! handed `&mut Sim`, and so structurally cannot perturb the per-tick checksum (invariants #1/#7) —
//! exactly the discipline [`InSessionShell`](crate::session_shell::InSessionShell) follows.
//!
//! ## The model
//!
//! An [`Objective`] is `{ kind, owner, progress, state }` (the WS-A shape; the *target* is carried
//! inside the [`ObjectiveKind`] variant, since each kind targets a different thing — a point, an
//! entity, a faction, a duration). [`ObjectiveKind`] ∈ `{Capture, Eliminate(entity|faction),
//! Survive(ticks), Reach, Escort}`. [`Objective::observe`] folds one tick's events + faction reads
//! into [`Objective::progress`] and may flip [`Objective::state`] → `Completed`/`Failed`, returning
//! the transition so the host can drive the summary + HUD.
//!
//! ## Reuse, not duplication
//!
//! The elimination rule is `evaluate_outcome`'s, generalized: every objective fails the moment its
//! **owner** faction is wiped out (zero living units *and* buildings —
//! [`FactionForces::is_eliminated`]), and an `Eliminate(Faction)` objective completes when its
//! *target* faction is. That is the same `is_eliminated` predicate the host's win-condition
//! evaluator reads — there is one elimination rule in the codebase, not two.

use gonedark_core::components::{Faction, Vec2, FACTION_COUNT};
use gonedark_core::ecs::Entity;
use gonedark_core::event::SimEvent;
use gonedark_core::fixed::Fixed;
use gonedark_core::sim::Sim;

use crate::session_shell::FactionForces;
use gonedark_render::objective_hud::{ObjectiveHudView, ObjectiveStateView};

/// What an objective targets to ELIMINATE.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum EliminateTarget {
    /// A single VIP entity (e.g. an enemy officer) — done the tick it is `Killed`.
    Entity(Entity),
    /// An entire faction — done when it has zero living units AND buildings
    /// ([`FactionForces::is_eliminated`], the generalized `evaluate_outcome` rule).
    Faction(Faction),
}

/// The kind of objective + its target parameters. `kind ∈ {Capture, Eliminate, Survive, Reach,
/// Escort}` (WS-A). The *target* lives inside the variant rather than as a separate field, because
/// each kind targets a different thing.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ObjectiveKind {
    /// Capture (flip ownership to `who`) the control point at `point` — completed on a
    /// [`SimEvent::Captured`] whose `to == who` at that point.
    Capture { who: Faction, point: Vec2 },
    /// Eliminate a VIP entity or a whole faction.
    Eliminate(EliminateTarget),
    /// Keep `who` alive until `until_tick` (survive-to-timeout): completed at the tick, failed if
    /// `who` is wiped out before then (the universal owner-eliminated rule — set `owner == who`).
    Survive { who: Faction, until_tick: u64 },
    /// Move `who` within `radius` of `dest` (read from the host-supplied tracked positions).
    Reach { who: Entity, dest: Vec2, radius: Fixed },
    /// Escort `vip` to within `radius` of `dest` — it must arrive ALIVE (a `Killed` for it fails).
    Escort { vip: Entity, dest: Vec2, radius: Fixed },
}

/// Where an objective is in its lifecycle. Terminal states (`Completed`/`Failed`) stick.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum ObjectiveState {
    /// Still in progress.
    #[default]
    Active,
    /// Achieved.
    Completed,
    /// Failed (the owner was wiped out, or an escortee died).
    Failed,
}

/// A `0..=goal` progress bar for the HUD. `goal == 0` means a binary objective (no numeric bar).
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct Progress {
    pub current: u32,
    pub goal: u32,
}

/// One mission objective: its kind + target, the faction **pursuing** it (whose wipe-out fails it),
/// a HUD label, whether it is REQUIRED (its failure fails the mission), and its live progress/state.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Objective {
    pub kind: ObjectiveKind,
    /// The faction pursuing this objective. The generalized "lose all your units" rule: if `owner`
    /// is eliminated ([`FactionForces::is_eliminated`]), the objective fails — you cannot finish any
    /// objective once your whole force is gone.
    pub owner: Faction,
    /// Short HUD label ("Take the enemy base").
    pub label: String,
    /// Whether this objective is REQUIRED — its failure fails the mission, and the mission is won
    /// only when every required objective is completed. A non-required objective is a bonus.
    pub required: bool,
    pub progress: Progress,
    pub state: ObjectiveState,
}

impl Objective {
    fn new(kind: ObjectiveKind, owner: Faction, label: impl Into<String>, goal: u32) -> Self {
        Objective {
            kind,
            owner,
            label: label.into(),
            required: true,
            progress: Progress { current: 0, goal },
            state: ObjectiveState::Active,
        }
    }

    /// Eliminate an entire `target` faction. `goal` is that faction's starting destroyable strength
    /// (units + buildings) for the HUD progress bar.
    pub fn eliminate_faction(
        owner: Faction,
        target: Faction,
        label: impl Into<String>,
        goal: u32,
    ) -> Self {
        Objective::new(ObjectiveKind::Eliminate(EliminateTarget::Faction(target)), owner, label, goal)
    }

    /// Eliminate a single VIP `entity` (binary — done the tick it is killed).
    pub fn eliminate_entity(owner: Faction, entity: Entity, label: impl Into<String>) -> Self {
        Objective::new(ObjectiveKind::Eliminate(EliminateTarget::Entity(entity)), owner, label, 1)
    }

    /// Capture (flip to `who`) the control point at `point` (binary).
    pub fn capture(owner: Faction, who: Faction, point: Vec2, label: impl Into<String>) -> Self {
        Objective::new(ObjectiveKind::Capture { who, point }, owner, label, 1)
    }

    /// Keep `who` alive until `until_tick`. `owner == who` so a wipe before the timer fails it.
    pub fn survive(who: Faction, until_tick: u64, label: impl Into<String>) -> Self {
        // The progress bar counts ticks survived toward the goal (clamped to fit a u32).
        let goal = until_tick.min(u32::MAX as u64) as u32;
        Objective::new(ObjectiveKind::Survive { who, until_tick }, who, label, goal)
    }

    /// Move `who` within `radius` of `dest` (binary).
    pub fn reach(owner: Faction, who: Entity, dest: Vec2, radius: Fixed, label: impl Into<String>) -> Self {
        Objective::new(ObjectiveKind::Reach { who, dest, radius }, owner, label, 1)
    }

    /// Escort `vip` to within `radius` of `dest` — alive (binary).
    pub fn escort(owner: Faction, vip: Entity, dest: Vec2, radius: Fixed, label: impl Into<String>) -> Self {
        Objective::new(ObjectiveKind::Escort { vip, dest, radius }, owner, label, 1)
    }

    /// Fold one tick's `ctx` (events + faction reads + elapsed tick + tracked positions) into this
    /// objective. Returns `Some(new_state)` on the tick it transitions to a terminal state, else
    /// `None`. A no-op once terminal (the first result sticks). Reads only `Copy` snapshots + the
    /// event stream — it mutates only `self`, never any sim state, so it cannot desync.
    pub fn observe(&mut self, ctx: &ObserveCtx) -> Option<ObjectiveState> {
        if self.state != ObjectiveState::Active {
            return None;
        }

        // Universal fail (generalized `evaluate_outcome` rule 1): the pursuing faction has been wiped
        // out — zero living units AND buildings — so no objective it owns can still be finished.
        if ctx.forces[self.owner.index()].is_eliminated() {
            return Some(self.fail());
        }

        let done = match self.kind {
            ObjectiveKind::Capture { who, point } => ctx
                .events
                .iter()
                .any(|e| matches!(*e, SimEvent::Captured { to, pos, .. } if to == who && pos == point)),

            ObjectiveKind::Eliminate(EliminateTarget::Faction(f)) => {
                let force = &ctx.forces[f.index()];
                // Progress bar: how much of the target's starting strength is gone.
                let remaining = force.alive_units + force.buildings;
                self.progress.current = self.progress.goal.saturating_sub(remaining);
                force.is_eliminated()
            }

            ObjectiveKind::Eliminate(EliminateTarget::Entity(e)) => ctx
                .events
                .iter()
                .any(|ev| matches!(*ev, SimEvent::Killed { entity, .. } if entity == e)),

            ObjectiveKind::Survive { until_tick, .. } => {
                self.progress.current = ctx.elapsed_ticks.min(until_tick).min(u32::MAX as u64) as u32;
                ctx.elapsed_ticks >= until_tick
            }

            ObjectiveKind::Reach { who, dest, radius } => within(ctx.tracked, who, dest, radius),

            ObjectiveKind::Escort { vip, dest, radius } => {
                // A dead escortee is a hard fail — distinct from the owner-wipe fail above.
                if ctx
                    .events
                    .iter()
                    .any(|ev| matches!(*ev, SimEvent::Killed { entity, .. } if entity == vip))
                {
                    return Some(self.fail());
                }
                within(ctx.tracked, vip, dest, radius)
            }
        };

        if done {
            // Fill the bar on completion (binary objectives have goal 0 → no bar).
            self.progress.current = self.progress.goal;
            self.state = ObjectiveState::Completed;
            return Some(ObjectiveState::Completed);
        }
        None
    }

    fn fail(&mut self) -> ObjectiveState {
        self.state = ObjectiveState::Failed;
        ObjectiveState::Failed
    }

    /// Map this objective's state to the render HUD's tint enum.
    fn state_view(&self) -> ObjectiveStateView {
        match self.state {
            ObjectiveState::Active => ObjectiveStateView::Active,
            ObjectiveState::Completed => ObjectiveStateView::Completed,
            ObjectiveState::Failed => ObjectiveStateView::Failed,
        }
    }
}

/// Whether tracked entity `who` is within `radius` of `dest`. Squared-distance fixed-point compare
/// (no `sqrt`); `None` (the entity has no tracked position) reads as "not there yet".
fn within(tracked: &[(Entity, Vec2)], who: Entity, dest: Vec2, radius: Fixed) -> bool {
    tracked
        .iter()
        .find(|(e, _)| *e == who)
        .is_some_and(|(_, pos)| (*pos - dest).len_sq() <= radius * radius)
}

/// The per-tick read window an [`ObjectiveSet`] observes — events + the already-derived per-faction
/// [`FactionForces`] + the elapsed tick + (for Reach/Escort) tracked entity positions. All are
/// snapshots of already-checksummed state or transient event copies; observing them folds nothing.
pub struct ObserveCtx<'a> {
    /// This tick's deterministic [`SimEvent`] stream (`Sim::events`).
    pub events: &'a [SimEvent],
    /// Each faction's standing forces, indexed by [`Faction::index`] — derive with
    /// [`faction_forces_all`].
    pub forces: &'a [FactionForces; FACTION_COUNT],
    /// Ticks elapsed in the match (`Sim::tick_count`).
    pub elapsed_ticks: u64,
    /// Positions of entities a Reach/Escort objective tracks; the host fills it from the world.
    /// Empty for missions that use none (the *Seize* mission uses none).
    pub tracked: &'a [(Entity, Vec2)],
}

impl<'a> ObserveCtx<'a> {
    /// The common case: no tracked Reach/Escort entities.
    pub fn new(
        events: &'a [SimEvent],
        forces: &'a [FactionForces; FACTION_COUNT],
        elapsed_ticks: u64,
    ) -> Self {
        ObserveCtx {
            events,
            forces,
            elapsed_ticks,
            tracked: &[],
        }
    }
}

/// A transition reported by [`ObjectiveSet::observe`]: which objective flipped, and to what.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct ObjectiveEvent {
    /// Index of the objective in the set.
    pub index: usize,
    /// The terminal state it reached this tick (`Completed` or `Failed`).
    pub state: ObjectiveState,
}

/// Where the whole mission stands.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum MissionStatus {
    /// In progress (or no objectives at all).
    Active,
    /// Every required objective is completed.
    Won,
    /// A required objective failed.
    Lost,
}

/// A mission's set of objectives — the host-side layer that watches a match. Pure session/
/// presentation state: it owns no `Sim` and can never be handed one, so observing it can never
/// desync lockstep (invariants #1/#7).
#[derive(Clone, PartialEq, Eq, Debug, Default)]
pub struct ObjectiveSet {
    pub objectives: Vec<Objective>,
}

impl ObjectiveSet {
    pub fn new(objectives: Vec<Objective>) -> Self {
        ObjectiveSet { objectives }
    }

    /// No objectives — the skirmish/sandbox scenes (no HUD, no win/lose from this layer).
    pub fn is_empty(&self) -> bool {
        self.objectives.is_empty()
    }

    /// The WS-A *Seize* mission ("10 troops, take the base"): one required objective — eliminate the
    /// Enemy faction (the camp + its garrison). It FAILS when the Player force is wiped out (lose all
    /// ten) via the universal owner-eliminated rule. `enemy_strength` is the enemy's starting
    /// destroyable count (garrison + base), for the HUD progress bar
    /// ([`SeizeMission::enemy_strength`](gonedark_core::scenario::SeizeMission::enemy_strength)).
    pub fn mission_one(enemy_strength: u32) -> Self {
        ObjectiveSet::new(vec![Objective::eliminate_faction(
            Faction::Player,
            Faction::Enemy,
            "Take the enemy base",
            enemy_strength,
        )])
    }

    /// Fold one tick into every active objective, returning the transitions that fired this tick (in
    /// objective order) for the host to surface (summary log / HUD flash).
    pub fn observe(&mut self, ctx: &ObserveCtx) -> Vec<ObjectiveEvent> {
        let mut out = Vec::new();
        for (index, o) in self.objectives.iter_mut().enumerate() {
            if let Some(state) = o.observe(ctx) {
                out.push(ObjectiveEvent { index, state });
            }
        }
        out
    }

    /// Where the mission stands: `Lost` if any required objective failed; `Won` once every required
    /// objective is completed (and there is at least one); else `Active`.
    pub fn status(&self) -> MissionStatus {
        let mut any_required = false;
        let mut all_required_complete = true;
        for o in &self.objectives {
            if !o.required {
                continue;
            }
            any_required = true;
            match o.state {
                ObjectiveState::Failed => return MissionStatus::Lost,
                ObjectiveState::Completed => {}
                ObjectiveState::Active => all_required_complete = false,
            }
        }
        if any_required && all_required_complete {
            MissionStatus::Won
        } else {
            MissionStatus::Active
        }
    }

    /// The PRIMARY objective for the HUD: the first still-active required objective, else the first
    /// objective (so a finished mission still shows its terminal state). `None` for an empty set.
    pub fn current(&self) -> Option<&Objective> {
        self.objectives
            .iter()
            .find(|o| o.required && o.state == ObjectiveState::Active)
            .or_else(|| self.objectives.first())
    }
}

/// Derive one faction's standing [`FactionForces`] from a [`Sim`] — alive units/buildings +
/// territory held + the banked purse, in the stable [`Faction::index`] space. A read-only scan of
/// already-checksummed state (it folds nothing, so it cannot perturb the checksum / desync,
/// invariants #1/#7). The single source of this derivation, shared by the host's win-condition
/// evaluator and the objective layer.
pub fn faction_forces(sim: &Sim, faction: Faction) -> FactionForces {
    use gonedark_core::components::EntityKind;
    let w = &sim.world;
    let mut alive_units = 0u32;
    let mut buildings = 0u32;
    for i in 0..w.capacity() {
        if !w.is_index_alive(i) || w.faction[i] != faction {
            continue;
        }
        match w.kind[i] {
            EntityKind::Unit => alive_units += 1,
            EntityKind::Building => buildings += 1,
        }
    }
    FactionForces {
        alive_units,
        buildings,
        territory_held: sim
            .territory
            .points
            .iter()
            .filter(|cp| cp.owner == faction)
            .count() as u32,
        resources_total: sim.resources.get(faction),
    }
}

/// Derive every faction's [`FactionForces`] at once, indexed by [`Faction::index`] — the
/// [`ObserveCtx::forces`] input.
pub fn faction_forces_all(sim: &Sim) -> [FactionForces; FACTION_COUNT] {
    let mut out: [FactionForces; FACTION_COUNT] = Default::default();
    for f in Faction::ALL {
        out[f.index()] = faction_forces(sim, f);
    }
    out
}

/// Build the render [`ObjectiveHudView`] for the current objective + progress (PURE → host-tested).
/// Empty (no current objective) ⇒ an empty view (nothing drawn). The progress pair is carried only
/// when the objective has a numeric goal (`goal > 0`); binary objectives show no bar.
pub fn objective_hud_view(set: &ObjectiveSet) -> ObjectiveHudView {
    match set.current() {
        None => ObjectiveHudView::default(),
        Some(o) => ObjectiveHudView {
            objective: o.label.clone(),
            state: Some(o.state_view()),
            progress: (o.progress.goal > 0).then_some((o.progress.current, o.progress.goal)),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gonedark_core::components::Vec2;
    use gonedark_core::ecs::Entity;
    use gonedark_core::sim::{Command, Sim};

    fn ent(i: u32) -> Entity {
        Entity { index: i, generation: 1 }
    }

    fn at(x: i32, y: i32) -> Vec2 {
        Vec2::new(Fixed::from_int(x), Fixed::from_int(y))
    }

    /// A faction-forces array with `player`/`enemy` set and Neutral empty.
    fn forces(player: FactionForces, enemy: FactionForces) -> [FactionForces; FACTION_COUNT] {
        let mut f: [FactionForces; FACTION_COUNT] = Default::default();
        f[Faction::Player.index()] = player;
        f[Faction::Enemy.index()] = enemy;
        f
    }

    fn alive(units: u32, buildings: u32) -> FactionForces {
        FactionForces {
            alive_units: units,
            buildings,
            territory_held: 0,
            resources_total: 0,
        }
    }

    fn wiped() -> FactionForces {
        alive(0, 0)
    }

    // --- per-kind evaluators against synthetic SimEvent streams ---------------------------------

    #[test]
    fn capture_completes_when_the_point_flips_to_the_owner() {
        let point = at(3, 4);
        let mut o = Objective::capture(Faction::Player, Faction::Player, point, "Take the hill");
        let f = forces(alive(2, 0), alive(2, 1));

        // A capture of a DIFFERENT point (or to a different faction) does not complete it.
        let other = [SimEvent::Captured { pos: at(9, 9), from: Faction::Neutral, to: Faction::Player }];
        assert_eq!(o.observe(&ObserveCtx::new(&other, &f, 1)), None);
        assert_eq!(o.state, ObjectiveState::Active);

        // The right point flipping to the owner completes it (capture flips).
        let evs = [SimEvent::Captured { pos: point, from: Faction::Neutral, to: Faction::Player }];
        assert_eq!(o.observe(&ObserveCtx::new(&evs, &f, 2)), Some(ObjectiveState::Completed));
        assert_eq!(o.state, ObjectiveState::Completed);
        // Terminal sticks: a later tick reports no further transition.
        assert_eq!(o.observe(&ObserveCtx::new(&evs, &f, 3)), None);
    }

    #[test]
    fn eliminate_entity_completes_when_the_vip_is_killed() {
        let vip = ent(7);
        let mut o = Objective::eliminate_entity(Faction::Player, vip, "Kill the officer");
        let f = forces(alive(3, 0), alive(2, 1));

        // Some OTHER entity dying does not complete it (VIP-killed keys on the named entity).
        let other = [SimEvent::Killed { entity: ent(8), faction: Faction::Enemy, source: ent(1), pos: at(0, 0) }];
        assert_eq!(o.observe(&ObserveCtx::new(&other, &f, 1)), None);

        let evs = [SimEvent::Killed { entity: vip, faction: Faction::Enemy, source: ent(1), pos: at(1, 1) }];
        assert_eq!(o.observe(&ObserveCtx::new(&evs, &f, 2)), Some(ObjectiveState::Completed));
    }

    #[test]
    fn survive_completes_at_the_timeout_and_tracks_tick_progress() {
        let mut o = Objective::survive(Faction::Player, 100, "Hold for 100 ticks");
        let f = forces(alive(4, 0), alive(2, 1));

        // Before the timer: still active, progress tracks elapsed ticks.
        assert_eq!(o.observe(&ObserveCtx::new(&[], &f, 40)), None);
        assert_eq!(o.progress.current, 40);
        assert_eq!(o.state, ObjectiveState::Active);

        // At the timer: completed (survive-to-timeout).
        assert_eq!(o.observe(&ObserveCtx::new(&[], &f, 100)), Some(ObjectiveState::Completed));
        assert_eq!(o.progress.current, o.progress.goal);
    }

    #[test]
    fn survive_fails_if_the_owner_is_wiped_before_the_timer() {
        // Overstaying — the protected force is wiped out before the clock runs out → an honest fail.
        let mut o = Objective::survive(Faction::Player, 1000, "Hold the line");
        let f = forces(wiped(), alive(3, 1));
        assert_eq!(o.observe(&ObserveCtx::new(&[], &f, 200)), Some(ObjectiveState::Failed));
        assert_eq!(o.state, ObjectiveState::Failed);
    }

    #[test]
    fn eliminate_faction_completes_when_the_target_is_wiped_and_tracks_progress() {
        // Eliminate the Enemy (goal 5 = 4 garrison + 1 base). Progress = how much is gone.
        let mut o = Objective::eliminate_faction(Faction::Player, Faction::Enemy, "Take the base", 5);

        // 2 enemy units + 1 building remain → 3 of 5 cleared, still active.
        let f = forces(alive(8, 0), alive(2, 1));
        assert_eq!(o.observe(&ObserveCtx::new(&[], &f, 1)), None);
        assert_eq!(o.progress.current, 2, "5 - (2 units + 1 building) = 2 cleared");

        // Enemy wiped → completed.
        let f = forces(alive(8, 0), wiped());
        assert_eq!(o.observe(&ObserveCtx::new(&[], &f, 2)), Some(ObjectiveState::Completed));
        assert_eq!(o.progress.current, o.progress.goal);
    }

    #[test]
    fn any_objective_fails_when_its_owner_loses_all_units() {
        // The mission-1 fail path ("lose all ten"): the Player owner is wiped → the eliminate-Enemy
        // objective fails (generalized `evaluate_outcome` elimination, reused not duplicated).
        let mut o = Objective::eliminate_faction(Faction::Player, Faction::Enemy, "Take the base", 5);
        let f = forces(wiped(), alive(3, 1));
        assert_eq!(o.observe(&ObserveCtx::new(&[], &f, 50)), Some(ObjectiveState::Failed));
        assert_eq!(o.state, ObjectiveState::Failed);
    }

    #[test]
    fn reach_and_escort_use_tracked_positions() {
        let runner = ent(2);
        let dest = at(10, 0);
        let mut reach = Objective::reach(Faction::Player, runner, dest, Fixed::from_int(2), "Reach the LZ");
        let f = forces(alive(1, 0), alive(1, 0));

        // Far away → not yet.
        let far = [(runner, at(0, 0))];
        assert_eq!(reach.observe(&ObserveCtx { events: &[], forces: &f, elapsed_ticks: 1, tracked: &far }), None);
        // Within radius → completed.
        let near = [(runner, at(11, 0))];
        assert_eq!(
            reach.observe(&ObserveCtx { events: &[], forces: &f, elapsed_ticks: 2, tracked: &near }),
            Some(ObjectiveState::Completed)
        );

        // Escort fails if the VIP dies en route.
        let vip = ent(5);
        let mut escort = Objective::escort(Faction::Player, vip, dest, Fixed::from_int(2), "Escort the VIP");
        let dead = [SimEvent::Killed { entity: vip, faction: Faction::Player, source: ent(9), pos: at(3, 3) }];
        assert_eq!(
            escort.observe(&ObserveCtx { events: &dead, forces: &f, elapsed_ticks: 3, tracked: &[(vip, at(3, 3))] }),
            Some(ObjectiveState::Failed)
        );
    }

    // --- ObjectiveSet status + the HUD mapper ---------------------------------------------------

    #[test]
    fn set_status_active_then_won() {
        let mut set = ObjectiveSet::mission_one(5);
        assert_eq!(set.status(), MissionStatus::Active);
        let f = forces(alive(8, 0), wiped());
        set.observe(&ObserveCtx::new(&[], &f, 10));
        assert_eq!(set.status(), MissionStatus::Won);
    }

    #[test]
    fn set_status_lost_on_required_failure() {
        let mut set = ObjectiveSet::mission_one(5);
        let f = forces(wiped(), alive(3, 1));
        set.observe(&ObserveCtx::new(&[], &f, 10));
        assert_eq!(set.status(), MissionStatus::Lost);
    }

    #[test]
    fn hud_view_reflects_the_current_objective_and_progress() {
        let mut set = ObjectiveSet::mission_one(5);
        // Active with partial progress.
        let f = forces(alive(8, 0), alive(2, 1));
        set.observe(&ObserveCtx::new(&[], &f, 1));
        let v = objective_hud_view(&set);
        assert_eq!(v.objective, "Take the enemy base");
        assert_eq!(v.state, Some(ObjectiveStateView::Active));
        assert_eq!(v.progress, Some((2, 5)));

        // Empty set → empty view (skirmish/sandbox).
        assert!(objective_hud_view(&ObjectiveSet::default()).is_empty());
    }

    // --- the engine-level integration: drive mission 1 to a win AND a loss ----------------------

    /// Drive the seeded *Seize* mission with the player troops `stance`d as given and attack-moving
    /// onto the enemy base, stepping the bare `Sim` (no GPU) while the host-side `ObjectiveSet`
    /// observes each tick — exactly the `Sim` + objective loop the live host runs, minus the
    /// renderer. Returns the final `MissionStatus` after up to `max_ticks`.
    fn run_mission_one(player_stance: gonedark_core::components::Stance, max_ticks: u64) -> MissionStatus {
        use gonedark_core::components::Order;
        let mut sim = Sim::new(0xA11CE);
        let m = gonedark_core::scenario::seed_seize_mission(&mut sim);
        let mut set = ObjectiveSet::mission_one(m.enemy_strength());

        // Order the ten troops to assault the base (stance decides whether they fire on the way in).
        let base = sim.world.pos[m.enemy_base.index as usize];
        let mut opening: Vec<Command> = Vec::with_capacity(m.troops.len() * 2);
        for &t in &m.troops {
            opening.push(Command::SetStance { entity: t, stance: player_stance });
            opening.push(Command::AttackMove { entity: t, target: base });
        }
        sim.step(&opening);

        for _ in 0..max_ticks {
            // Keep the assault pressed: any idle survivor re-attacks the base (mirrors a player
            // holding the order). Cheap, deterministic, issues nothing once the troop is committed.
            let mut cmds: Vec<Command> = Vec::new();
            for &t in &m.troops {
                let i = t.index as usize;
                if sim.world.is_alive(t) && matches!(sim.world.order[i], Order::Idle | Order::HoldPosition) {
                    cmds.push(Command::AttackMove { entity: t, target: base });
                }
            }
            sim.step(&cmds);

            let forces = faction_forces_all(&sim);
            set.observe(&ObserveCtx::new(sim.events(), &forces, sim.tick_count()));
            match set.status() {
                MissionStatus::Active => {}
                decided => return decided,
            }
        }
        set.status()
    }

    #[test]
    fn mission_one_is_won_when_the_assault_takes_the_base() {
        use gonedark_core::components::Stance;
        // Ten FireAtWill troops storm the base: they clear the garrison and raze the camp →
        // the Enemy is eliminated → the mission is WON.
        let status = run_mission_one(Stance::FireAtWill, 60 * 60);
        assert_eq!(status, MissionStatus::Won, "ten troops should take the base");
    }

    #[test]
    fn mission_one_is_lost_when_all_ten_die() {
        use gonedark_core::components::Stance;
        // Ten HoldFire troops march in without firing a shot: the FireAtWill garrison cuts them down
        // → the Player force is wiped → the mission is LOST (the "lose all ten" fail path).
        let status = run_mission_one(Stance::HoldFire, 60 * 60);
        assert_eq!(status, MissionStatus::Lost, "ten troops that won't fight are wiped out");
    }
}
