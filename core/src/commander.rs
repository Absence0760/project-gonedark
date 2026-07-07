//! The enemy commander — a deterministic, *commander-level* scripted AI (invariant #3).
//!
//! This is the strategic brain a human opponent would be: it surveys the (identical,
//! checksummed) world and **issues orders** — exactly the orders a player issues through the
//! command UI (`AttackMove` / `SetOrder` / `SetStance` / `Build` / `QueueProduction`). It does
//! **not** give units autonomous smarts. Units remain pure literal executors (invariant #3,
//! [`orders`](crate::orders)): a unit still does *exactly* its last order, every tick, forever.
//! All "intelligence" here is the commander *choosing* which order to hand a unit, never the
//! unit deciding for itself. A *commander* issuing orders is explicitly distinct from, and
//! allowed alongside, the literal-executor rule.
//!
//! Determinism (invariants #1, #7):
//! - **No floats.** Every comparison is on squared `Fixed` magnitudes ([`Vec2::len_sq`]) — no
//!   `sqrt`/`normalize`/transcendental. The determinism guard greps this file (incl. tests).
//! - **Stable iteration + tie-breaks.** Every scan walks the world in `0..capacity()` index
//!   order; "nearest" ties break toward the lowest index (`<` never replaces an equal-distance
//!   earlier candidate). Where the plan is deliberately *varied* (the exploration roll that lets
//!   the commander pick the second-nearest objective), the choice is drawn from the seeded RNG —
//!   so the produced command list is still a pure function of `(world, territory, resources,
//!   rng-state, config, faction, tick)`, identical on every peer, but a *different seed* yields a
//!   different (still reproducible) game. That is the whole point: a human can no longer learn one
//!   fixed opening and beat it every match.
//! - **Own RNG stream.** The commander draws from a RNG owned by the *host* (`engine::Game`),
//!   seeded `sim_seed ^ faction`, **never** `Sim::rng()` (that stream is folded into the
//!   checksum; a host-side draw would advance it and desync). The host pushes the returned
//!   commands into the same lockstep stream player commands travel, so they are applied
//!   bit-identically on every peer — the commander itself stays peer-agnostic. Because the draw
//!   count is a pure function of already-checksummed state (how many free units face a real
//!   objective *choice* this cycle), every peer advances the RNG identically.
//!
//! The host calls [`commander_orders`] on a `tick % PERIOD == 0` cadence (see
//! [`COMMANDER_PERIOD`]); on off-ticks it issues nothing. Returning a `Vec<Command>` (not
//! mutating the world) keeps it a *pure planner* — the sim still applies every command through
//! the one authoritative `Sim::apply` path.

use crate::components::{Army, EntityKind, Faction, Order, Stance, UnitKind, Vec2};
use crate::detection::Tell;
use crate::economy::{self, Resources};
use crate::ecs::World;
use crate::fixed::Fixed;
use crate::mission_tuning::{Difficulty, DifficultyParams};
use crate::rng::Rng;
use crate::sim::Command;
use crate::territory::Territory;

/// How often (in ticks) the host invokes the commander. 60 ticks = 1 s at the locked 60 Hz
/// ([`crate::sim::TICK_HZ`]): a deliberate, human-cadence re-plan, not a per-tick micro. Cheap
/// (a per-second linear scan), and slow enough that order churn reads as decisions, not jitter.
pub const COMMANDER_PERIOD: u64 = 60;

/// Radius (world units) within which a unit is considered "already committed" to a control
/// point and is not re-tasked. Matches the territory capture radius so a unit sitting on a
/// point it is capturing is left to finish the job. Squared at the use site (no sqrt).
const POINT_COMMIT_RADIUS: Fixed = crate::territory::CAPTURE_RADIUS;

/// Radius (world units) within which a hostile unit is considered to be *threatening* a control
/// point this faction owns — twice the [capture radius](crate::territory::CAPTURE_RADIUS), so the
/// commander reacts as an enemy *closes on* a held point, not only once it is already inside the
/// capture ring. Squared at the use site (no sqrt). Only consulted when the tier
/// [`defends`](PlanStyle::defend).
const DEFEND_RADIUS: Fixed = Fixed::from_int(12);

/// The **tactical style** the commander plays at, derived purely from its difficulty tier
/// ([`plan_style`]). This is the "behavior quality" axis (invariant #3: it changes only which
/// orders the *commander* chooses — units stay literal executors): a harder tier explores less
/// wildly, concentrates its fire, defends what it holds, and preserves damaged bodies; an easier
/// tier scatters more, never concentrates or defends, and feeds wounded units back in.
///
/// It is deliberately *separate* from [`DifficultyParams`] (which carries the economic / cadence
/// knobs the sim already checksums through the command stream): the style is commander-local
/// tactical policy, all integer, float-free (invariant #1), so it is deterministic and bit-
/// identical on every peer. The RNG only ever enters through [`explore_pct`](Self::explore_pct).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
struct PlanStyle {
    /// Chance, in **percent**, that a free unit facing a real *choice* of open control points
    /// takes the SECOND-nearest instead of the nearest. The unpredictability knob: `0` ⇒ pure
    /// greedy-nearest (the old, learnable opening); higher ⇒ the commander's route/priority
    /// varies by seed. Rolled from the seeded RNG, so it stays reproducible per seed.
    explore_pct: u32,
    /// Concentrate free attackers on ONE priority target (focus-fire) rather than each picking its
    /// own nearest foe. Off ⇒ the old per-unit nearest-hostile spread.
    concentrate: bool,
    /// Peel free units back to defend a held control point a hostile is closing on
    /// ([`DEFEND_RADIUS`]) before taking new ground. Off ⇒ never defends (the old always-advance).
    defend: bool,
    /// Health fraction, in **percent**, below which a unit is pulled back to a friendly building to
    /// preserve it (`0` ⇒ never retreat — feed them in, the old behavior).
    retreat_hp_pct: u32,
}

/// The tactical [`PlanStyle`] for a difficulty tier — the "difficulty changes *behavior quality*"
/// mapping ([D-AI]). Easiest → hardest: exploration tightens (a sharper commander wanders less),
/// while concentration, defense, and force-preservation switch on and deepen. Every value is an
/// integer, so the whole thing is `const`-evaluable and float-free (invariant #1).
const fn plan_style(difficulty: Difficulty) -> PlanStyle {
    match difficulty {
        // Forgiving: scatters its objectives most, never concentrates or defends, and throws
        // wounded units back into the fight — the tier a new player learns the loop against.
        Difficulty::Recruit => PlanStyle {
            explore_pct: 40,
            concentrate: false,
            defend: false,
            retreat_hp_pct: 0,
        },
        // The baseline commander: explores moderately, focus-fires, defends what it holds, and
        // pulls a badly hurt unit (under a quarter health) back rather than trading it away.
        Difficulty::Veteran => PlanStyle {
            explore_pct: 25,
            concentrate: true,
            defend: true,
            retreat_hp_pct: 25,
        },
        // Sharp: wanders least (still varies by seed), always concentrates and defends, and
        // preserves bodies sooner — a better commander, never an omniscient one.
        Difficulty::Elite => PlanStyle {
            explore_pct: 15,
            concentrate: true,
            defend: true,
            retreat_hp_pct: 35,
        },
    }
}

// The production backlog cap and the Heavy-purchase reserve are no longer fixed constants: they
// are **difficulty knobs** ([`mission_tuning::DifficultyParams`]), so a tier scales how deep the
// commander queues and how big a cushion it keeps. The default tier ([`Difficulty::Veteran`])
// returns the original values (`max_queue_depth = 2`, `heavy_reserve = RIFLEMAN_COST`), so the
// default scenes' command streams are byte-identical (see `mission_tuning`).

/// Tunable knobs for the commander — a *mechanism*, not a frozen design (the D23/D26/D33 house
/// style). Defaults reproduce the original, golden-checksum-stable behavior **byte-for-byte**, so
/// adding a knob never perturbs the default `phase2`/`stress`/demo command streams.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct CommanderConfig {
    /// How aggressively this commander plays — a deterministic difficulty tier
    /// (`mission_tuning::Difficulty`) that scales the planner's **choices** (production backlog
    /// depth, the Heavy reserve, and the army re-plan cadence), never its **knowledge**. A harder
    /// tier issues orders sooner and spends more freely; it reads *nothing* about the player going
    /// dark — invariant #6 is structural, not a discipline (the gone-dark consult stays gated
    /// solely by [`hunt_embodied`](Self::hunt_embodied), independent of tier).
    ///
    /// **Defaults to [`Veteran`](Difficulty::Veteran)**, whose knobs reproduce the commander's
    /// original constants exactly, so the default scenes' lockstep/checksum streams are untouched.
    pub difficulty: Difficulty,

    /// When `true`, the commander may **consult the detection channel** and chase a hostile that
    /// has "gone dark" (embodied). It reads ONLY the `tells` the caller derived from
    /// [`detection::detectable_embodiment`](crate::detection::detectable_embodiment) for *this*
    /// faction as observer — so it learns exactly what detection honestly permits (range +
    /// line-of-sight bounded, with the `Subtle` linger) and **nothing more**: in `Hidden` mode, or
    /// out of range / no LoS, the slice is empty and the commander reacts to nothing it could not
    /// legitimately know. That structural bound — the commander cannot peek at `&World` for embodied
    /// enemies itself, only consume the channel — is the point (invariant #6 fairness, "no
    /// omniscient peek").
    ///
    /// **Defaults `false`** so the default scenes' lockstep command streams stay byte-identical;
    /// enable it per-scene/per-difficulty to make the AI hunt a gone-dark player.
    pub hunt_embodied: bool,

    /// Per-node override of the tier's production backlog depth (the **aggression** knob,
    /// [`DifficultyParams::max_queue_depth`]). `None` ⇒ the tier's value — the byte-identical
    /// default. A campaign node dials this in its per-node battle spec so it can field a keener or
    /// slacker commander than its replay-tier band alone; it is host-side planning **config**, not
    /// new AI logic (invariant #3: units stay literal executors; only the commander's *choices*
    /// change). Applied deterministically at launch, so two peers at the same override stay
    /// bit-identical (invariant #7).
    pub max_queue_depth: Option<usize>,

    /// Per-node override of the tier's Heavy-purchase reserve cushion
    /// ([`DifficultyParams::heavy_reserve`], the **reserve / unit-mix** knob). `None` ⇒ the tier's
    /// value. Clamped `>= 0` on apply (a negative reserve is meaningless). Same config-only,
    /// deterministic contract as [`max_queue_depth`](Self::max_queue_depth).
    pub heavy_reserve: Option<i64>,

    /// Per-node override of the tier's re-plan **cadence** stride
    /// ([`DifficultyParams::command_stride`]). `None` ⇒ the tier's value. Clamped `>= 1` on apply
    /// (stride 0 would divide by zero in the cadence gate). Same config-only, deterministic
    /// contract as [`max_queue_depth`](Self::max_queue_depth).
    pub command_stride: Option<u64>,
}

impl CommanderConfig {
    /// The integer knobs the planner actually uses this tick: the [`difficulty`](Self::difficulty)
    /// tier's [`params`](Difficulty::params), with any per-node `Some(_)` override applied on top
    /// (the reserve clamped `>= 0`, the stride clamped `>= 1`). With **no** overrides (the default)
    /// this is *exactly* `self.difficulty.params()`, so a default config is byte-identical to the
    /// pre-override commander — the property that keeps the golden-checksum streams untouched.
    /// Pure, `const`-friendly, float-free (invariant #1).
    pub fn resolved_params(&self) -> DifficultyParams {
        let mut p = self.difficulty.params();
        if let Some(q) = self.max_queue_depth {
            p.max_queue_depth = q;
        }
        if let Some(r) = self.heavy_reserve {
            p.heavy_reserve = r.max(0);
        }
        if let Some(s) = self.command_stride {
            p.command_stride = s.max(1);
        }
        p
    }
}

/// Survey the world and return the orders to feed the lockstep stream this tick — possibly empty
/// (nothing affordable, no idle units, no targets). The host owns the RNG (its own stream,
/// seeded `sim_seed ^ faction`) and passes it in by `&mut`; everything else is a read-only view
/// of already-checksummed sim state. The caller pushes the result into the same `commands` Vec
/// that drives `drive_lockstep`, *before* the lockstep step.
///
/// `config` gates optional behavior; `tells` is the detection channel's output for `faction` as
/// observer (the caller derives it from [`detection::detectable_embodiment`](crate::detection)).
/// With `CommanderConfig::default()` and `tells == &[]`, the returned command list is **identical,
/// byte-for-byte, to the original commander** — the default scenes' checksum streams are untouched.
///
/// Behavior loop (all "only existing order/economy commands", invariant #3 — the *commander*
/// chooses; units still execute literally). The tactical style — how much it explores, whether it
/// concentrates, defends, and preserves damaged units — is a pure function of the difficulty tier
/// (`plan_style`), so a harder tier is a genuinely *better* commander, never an omniscient one:
/// 1. **Reinforce.** For each built friendly camp, if the faction can afford a unit, queue one
///    (`QueueProduction`). Heavy when flush, else Rifleman — pure resource thresholds, no float.
/// 2. **Posture.** Any unit on `HoldFire` is bumped to `FireAtWill` so the commander's army
///    actually engages (a one-shot stance fix; idempotent thereafter). `ReturnFire` would not do:
///    a `HoldFire`/`ReturnFire` unit only shoots once *it* is hit, so a defending line would never
///    open up on an attacker — it must `FireAtWill` to fight on its own.
/// 3. **Preserve (retreat).** A unit whose health has dropped under the tier's `retreat_hp_pct`
///    is pulled back to its nearest friendly building (`SetOrder` → `FallBack`) instead of being
///    fed in to die — once, not every cycle (FallBack is terminal, so the guard never re-fires).
///    `retreat_hp_pct == 0` (Recruit) keeps the old feed-them-in behavior — easier to beat.
/// 4. **Hunt the dark** *(only when `config.hunt_embodied`)*. If a hostile has gone dark
///    (embodied) within what the detection channel HONESTLY reveals (a non-empty `tells`), a free
///    unit is pressed toward its nearest tell's (last-seen) position ABOVE taking ground — a
///    gone-dark player is the juiciest target. Empty `tells` (out of range / no LoS / `Hidden`) ⇒
///    no reaction, so the AI never knows more than detection grants (invariant #6, no omniscient
///    peek). Off by default → no effect on the default streams.
/// 5. **Defend** *(tiers with `defend`)*. A free unit will hold a control point the faction owns
///    that a hostile is closing on (within `DEFEND_RADIUS`) BEFORE grabbing new ground — don't
///    lose what you hold. Multiple free units near the same threatened point converge on it, so a
///    real defending force peels back.
/// 6. **Capture.** Free units are otherwise sent to an open (neutral/enemy) control point. The
///    target is usually the nearest, but with the tier's `explore_pct` chance the commander picks
///    the *second*-nearest instead (a seeded roll) — so it does not march the identical greedy
///    opening every match. Taking ground is how you out-produce the player.
/// 7. **Attack.** Units with no point to take press the enemy. With `concentrate` on, they all
///    converge on ONE priority target — the hostile nearest to the commander's own line
///    (`nearest_contact`) — so the army focus-fires instead of each unit dribbling toward its
///    own nearest foe. Without it (Recruit) each unit picks its own nearest hostile, as before.
#[allow(clippy::too_many_arguments)] // honest read-only inputs; bundling them buys no clarity
pub fn commander_orders(
    world: &World,
    territory: &Territory,
    resources: &Resources,
    rng: &mut Rng,
    config: &CommanderConfig,
    tells: &[Tell],
    faction: Faction,
    army: Army,
    tick: u64,
) -> Vec<Command> {
    // Difficulty tier → the integer knobs that scale this plan (aggression / reserve / cadence),
    // with any per-node override the caller dialed in (`resolved_params`). The default tier
    // (`Veteran`) with no overrides returns the commander's original constants, so a default-config
    // call is byte-identical to the pre-difficulty commander.
    let params = config.resolved_params();

    // Re-plan **cadence** (the `command_stride` knob): the army-tasking + posture pass runs only on
    // cycles where `cycle % stride == 0`, where `cycle = tick / COMMANDER_PERIOD` is a pure function
    // of sim state (so it is identical on every peer regardless of frame pacing). Stride `1`
    // (Veteran) ⇒ every cycle ⇒ the original behavior; a larger stride makes an easier commander
    // reconsider its orders less often. Reinforcement is intentionally *not* strided.
    let retask_this_cycle = params.command_stride <= 1
        || (tick / COMMANDER_PERIOD).is_multiple_of(params.command_stride);

    let mut commands = Vec::new();

    // --- 1. Reinforce: spend banked resources on production at each built camp. ----------------
    // A running purse so we never over-commit beyond what we can afford THIS plan (the sim's
    // `try_spend` is the final authority, but planning against a local purse keeps us from
    // queueing five units we can pay for once).
    let mut purse = resources.get(faction);
    for i in 0..world.capacity() {
        if !world.is_index_alive(i)
            || world.kind[i] != EntityKind::Building
            || world.faction[i] != faction
        {
            continue;
        }
        let b = &world.building[i];
        // Only a finished camp produces.
        if b.build_ticks_left != 0 {
            continue;
        }
        // Don't pile the queue arbitrarily deep — at most the tier's small backlog so resources
        // also fund captures/expansion, and so a stalled front item doesn't hoard the whole purse.
        // (`max_queue_depth` is the difficulty **aggression** knob; Veteran = 2 as before.)
        if b.queue.len() >= params.max_queue_depth {
            continue;
        }
        let Some(camp) = world.entity(i) else {
            continue;
        };
        // Flush → buy a Heavy (the expensive bruiser); otherwise the cheap, spammable Rifleman. The
        // cushion is the difficulty **reserve / unit-mix** knob: a fat reserve keeps the mix rifle-
        // heavy, a zero reserve (Elite) buys the Heavy the instant it is affordable. Veteran's
        // reserve is one Rifleman — the original threshold.
        let unit = if purse >= economy::HEAVY_COST + params.heavy_reserve {
            UnitKind::Heavy
        } else {
            UnitKind::Rifleman
        };
        // Charge the faction's ARMY price (D120): the WW2 cost-vs-power armies price the same
        // archetype differently, so a WW2-army commander plans its purse against its own costs. For
        // every non-WW2 army (and for the Rifleman/Heavy this loop queues, which no army tilts) this
        // is byte-identical to the shared `unit_cost`.
        let cost = economy::unit_cost_for(army, unit);
        if purse >= cost {
            purse -= cost;
            commands.push(Command::QueueProduction { camp, unit });
        }
    }

    // --- 2..7. Task the army: preserve, defend, capture (with variation), and concentrate. -----
    // Gated by the difficulty cadence: an easier commander (stride > 1) skips re-tasking on
    // off-cycles, so its army reconsiders orders less often. At Veteran stride this runs every
    // cycle. The tactical *style* (explore / concentrate / defend / preserve) is the difficulty
    // tier's — the "harder tier = better commander" axis (invariant #3: only the commander's
    // *choices* change; units still execute literally).
    if retask_this_cycle {
        let style = plan_style(config.difficulty);

        // The single priority target for a concentrated attack, computed ONCE so every free
        // attacker converges on it (focus-fire). `None` when the tier does not concentrate or
        // there is no contact — then each unit falls back to its own nearest hostile below.
        let focus = if style.concentrate {
            nearest_contact(world, faction)
        } else {
            None
        };

        for i in 0..world.capacity() {
            if !world.is_index_alive(i)
                || world.kind[i] != EntityKind::Unit
                || world.faction[i] != faction
            {
                continue;
            }
            // Posture fix: an idle army that won't shoot is useless. Bump HoldFire → FireAtWill once
            // so the unit engages enemies in range on its own (ReturnFire would only ever shoot back
            // after being hit, never opening the fight — the AI-vs-AI first-shot deadlock).
            if world.stance[i] == Stance::HoldFire {
                if let Some(e) = world.entity(i) {
                    commands.push(Command::SetStance {
                        entity: e,
                        stance: Stance::FireAtWill,
                    });
                }
            }

            let pos = world.pos[i];

            // Preserve (retreat): a badly hurt unit is pulled back to its nearest friendly building
            // rather than fed in to die. This applies even to a unit mid-order (it is *losing* the
            // fight it is in) — so it comes BEFORE the "leave a busy unit alone" gate. Issued once:
            // FallBack is terminal ([`orders`]), so a unit already falling back is skipped and the
            // order never thrashes. `retreat_hp_pct == 0` (Recruit) disables this entirely.
            if style.retreat_hp_pct > 0
                && !matches!(world.order[i], Order::FallBack(_))
                && world.health[i].fraction() < Fixed::from_ratio(style.retreat_hp_pct as i32, 100)
            {
                if let (Some(e), Some(rally)) = (
                    world.entity(i),
                    nearest_friendly_building(world, pos, faction),
                ) {
                    commands.push(Command::SetOrder {
                        entity: e,
                        order: Order::FallBack(rally),
                    });
                    continue;
                }
            }

            // Only (re-)task units free to take a new objective: Idle / HoldPosition. A unit mid-
            // MoveTo/AttackMove/Patrol/FallBack is left to finish its current order (re-issuing every
            // period would thrash it).
            if !matches!(world.order[i], Order::Idle | Order::HoldPosition) {
                continue;
            }

            // Already standing on a not-yet-ours point? Leave it to capture (don't re-issue).
            if sitting_on_open_point(territory, pos, faction) {
                continue;
            }

            let Some(e) = world.entity(i) else {
                continue;
            };

            // Hunt the dark (config-gated, default OFF): a hostile that has gone dark (embodied) and is
            // HONESTLY detectable — i.e. present in `tells`, which the caller bounded to range + LoS via
            // the detection channel — is the priority target, above taking ground. `tells` is empty when
            // detection reveals nothing (out of range / no LoS / `Hidden`), so this is a no-op then: the
            // commander reacts only to what it could legitimately know (invariant #6, no omniscient peek).
            if config.hunt_embodied {
                if let Some(target) = nearest_tell(tells, pos) {
                    commands.push(Command::AttackMove { entity: e, target });
                    continue;
                }
            }

            // Defend (tiers with `defend`): hold a point we own that an enemy is closing on before
            // going to grab new ground — free units near the same threatened point converge on it.
            if style.defend {
                if let Some(target) = nearest_threatened_owned_point(territory, world, pos, faction)
                {
                    commands.push(Command::AttackMove { entity: e, target });
                    continue;
                }
            }

            // Take ground: an open (neutral/enemy) control point. Usually the nearest, but with the
            // tier's `explore_pct` chance the seeded RNG picks the second-nearest instead — so the
            // opening/priority varies by seed and can't be learned as one fixed pattern.
            if let Some(target) = choose_open_point(territory, pos, faction, style.explore_pct, rng)
            {
                commands.push(Command::AttackMove { entity: e, target });
                continue;
            }

            // No point to take → press the enemy. Concentrate on the one priority target when the
            // tier focus-fires; otherwise each unit presses its own nearest hostile (the old spread).
            let target = focus.or_else(|| nearest_hostile(world, pos, faction));
            if let Some(target) = target {
                commands.push(Command::AttackMove { entity: e, target });
            }
        }
    }

    commands
}

/// Is `pos` within the commit radius of a control point this `faction` does NOT yet own? Such a
/// unit is left alone to finish capturing (re-tasking it would interrupt its own capture).
/// Squared-magnitude comparison only (no sqrt). Stable: any matching point in index order.
fn sitting_on_open_point(territory: &Territory, pos: Vec2, faction: Faction) -> bool {
    let r_sq = POINT_COMMIT_RADIUS * POINT_COMMIT_RADIUS;
    territory
        .points
        .iter()
        .any(|p| p.owner != faction && (p.pos - pos).len_sq() <= r_sq)
}

/// Nearest hostile (different, non-`Neutral` faction) entity — unit OR building — to `pos`, by
/// squared distance. `None` if there is no hostile alive. Deterministic: stable index-order
/// scan, ties break toward the lowest index.
fn nearest_hostile(world: &World, pos: Vec2, faction: Faction) -> Option<Vec2> {
    let mut best: Option<(Fixed, Vec2)> = None;
    for j in 0..world.capacity() {
        if !world.is_index_alive(j) {
            continue;
        }
        let f = world.faction[j];
        if f == faction || f == Faction::Neutral {
            continue;
        }
        let d = (world.pos[j] - pos).len_sq();
        match best {
            Some((bd, _)) if d >= bd => {}
            _ => best = Some((d, world.pos[j])),
        }
    }
    best.map(|(_, t)| t)
}

/// Choose which open (neutral/enemy) control point a free unit at `pos` heads for — usually the
/// nearest, but with `explore_pct`% chance the **second**-nearest instead. This is the
/// unpredictability knob (invariant #3 style): a fixed seed replays identically, but a different
/// seed yields a different opening/priority, so a human can't learn one greedy pattern and beat it
/// every match. The RNG is drawn **only when there is a real choice** (two or more open points),
/// so a single-candidate scene is byte-identical to plain nearest and advances the stream by
/// nothing. Float-free: squared-distance comparisons, an integer percent roll from the seeded RNG.
///
/// Deterministic: the two nearest are found by a stable point-order scan with lowest-index tie-
/// breaks; the roll is `rng.below(100) < explore_pct`.
fn choose_open_point(
    territory: &Territory,
    pos: Vec2,
    faction: Faction,
    explore_pct: u32,
    rng: &mut Rng,
) -> Option<Vec2> {
    // Track the two nearest open points (by squared distance), stable lowest-index tie-break.
    let mut best: Option<(Fixed, Vec2)> = None;
    let mut second: Option<(Fixed, Vec2)> = None;
    for p in &territory.points {
        if p.owner == faction {
            continue;
        }
        let d = (p.pos - pos).len_sq();
        match best {
            // Strictly closer than the current best → best becomes second, this becomes best.
            Some((bd, _)) if d < bd => {
                second = best;
                best = Some((d, p.pos));
            }
            // Not better than best, but better than second (or there is no second yet).
            Some(_) => match second {
                Some((sd, _)) if d >= sd => {}
                _ => second = Some((d, p.pos)),
            },
            None => best = Some((d, p.pos)),
        }
    }
    let (_, best_pos) = best?;
    // A real choice (a distinct second option) → roll for exploration; otherwise take the nearest.
    match second {
        Some((_, second_pos)) if explore_pct > 0 && rng.below(100) < explore_pct => {
            Some(second_pos)
        }
        _ => Some(best_pos),
    }
}

/// The single **priority target** for a concentrated (focus-fire) attack: the hostile — unit OR
/// building — sitting closest to *any* of this faction's own units, i.e. where the lines actually
/// meet. Sending every free attacker here makes the army converge instead of each unit dribbling
/// toward its own nearest foe. `None` when the faction has no units or there is no hostile.
///
/// Deterministic and float-free: for each hostile (stable index order) take its squared distance
/// to the nearest of our units (stable inner scan), then keep the hostile with the smallest such
/// distance, ties breaking toward the lowest hostile index (`>=` never displaces an earlier one).
fn nearest_contact(world: &World, faction: Faction) -> Option<Vec2> {
    let mut best: Option<(Fixed, Vec2)> = None;
    for j in 0..world.capacity() {
        if !world.is_index_alive(j) {
            continue;
        }
        let f = world.faction[j];
        if f == faction || f == Faction::Neutral {
            continue;
        }
        // Distance from this hostile to the nearest of our own units.
        let mut nearest_ours: Option<Fixed> = None;
        for i in 0..world.capacity() {
            if !world.is_index_alive(i)
                || world.kind[i] != EntityKind::Unit
                || world.faction[i] != faction
            {
                continue;
            }
            let d = (world.pos[i] - world.pos[j]).len_sq();
            nearest_ours = Some(match nearest_ours {
                Some(x) if x <= d => x,
                _ => d,
            });
        }
        let Some(d) = nearest_ours else {
            continue; // we have no units to measure from
        };
        match best {
            Some((bd, _)) if d >= bd => {}
            _ => best = Some((d, world.pos[j])),
        }
    }
    best.map(|(_, p)| p)
}

/// The nearest control point this `faction` owns that a hostile **unit** is closing on (within
/// [`DEFEND_RADIUS`]) — the point to peel a defender back to. `None` if no owned point is
/// threatened. Deterministic: stable point order for the nearest pick (lowest-index tie-break),
/// stable index-order threat scan; squared-distance only (no sqrt), float-free (invariant #1).
fn nearest_threatened_owned_point(
    territory: &Territory,
    world: &World,
    pos: Vec2,
    faction: Faction,
) -> Option<Vec2> {
    let threat_sq = DEFEND_RADIUS * DEFEND_RADIUS;
    let mut best: Option<(Fixed, Vec2)> = None;
    for p in &territory.points {
        if p.owner != faction {
            continue;
        }
        // Is any hostile unit closing on this point?
        let threatened = (0..world.capacity()).any(|j| {
            world.is_index_alive(j)
                && world.kind[j] == EntityKind::Unit
                && world.faction[j] != faction
                && world.faction[j] != Faction::Neutral
                && (world.pos[j] - p.pos).len_sq() <= threat_sq
        });
        if !threatened {
            continue;
        }
        let d = (p.pos - pos).len_sq();
        match best {
            Some((bd, _)) if d >= bd => {}
            _ => best = Some((d, p.pos)),
        }
    }
    best.map(|(_, t)| t)
}

/// Nearest alive friendly (`same faction`) **building** to `pos` — the rally a retreating unit
/// falls back to. `None` if the faction has no building. Deterministic: stable index-order scan,
/// ties break toward the lowest index. Squared-distance only (no sqrt), float-free (invariant #1).
/// (Mirrors [`orders`](crate::orders)'s private rally scan, but returns `Option` so the commander
/// can decline to retreat a unit that has nowhere to fall back to, rather than sending it to the
/// origin.)
fn nearest_friendly_building(world: &World, pos: Vec2, faction: Faction) -> Option<Vec2> {
    let mut best: Option<(Fixed, Vec2)> = None;
    for j in 0..world.capacity() {
        if !world.is_index_alive(j)
            || world.kind[j] != EntityKind::Building
            || world.faction[j] != faction
        {
            continue;
        }
        let d = (world.pos[j] - pos).len_sq();
        match best {
            Some((bd, _)) if d >= bd => {}
            _ => best = Some((d, world.pos[j])),
        }
    }
    best.map(|(_, p)| p)
}

/// Nearest gone-dark tell to `pos` by squared distance (no sqrt). `None` for an empty slice.
/// Deterministic: stable slice order, ties break toward the earliest tell (`<` never displaces an
/// equal-distance earlier one) — exactly the tie-break the other "nearest" scans use. Reads only
/// the (presentation-derived but float-free) tell positions, never `&World` — so the commander's
/// gone-dark knowledge is bounded by the detection channel that produced `tells`.
fn nearest_tell(tells: &[Tell], pos: Vec2) -> Option<Vec2> {
    let mut best: Option<(Fixed, Vec2)> = None;
    for t in tells {
        let d = (t.pos - pos).len_sq();
        match best {
            Some((bd, _)) if d >= bd => {}
            _ => best = Some((d, t.pos)),
        }
    }
    best.map(|(_, p)| p)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::components::{Building, BuildingKind, Health, InputSource};
    use crate::detection::{detectable_embodiment, DetectionConfig, DetectionMemory, TellMode};
    use crate::ecs::{Entity, World};
    use crate::terrain::{Cover, Terrain};
    use crate::territory::ControlPoint;

    fn at(x: i32, y: i32) -> Vec2 {
        Vec2::new(Fixed::from_int(x), Fixed::from_int(y))
    }

    fn spawn_unit(world: &mut World, faction: Faction, pos: Vec2) -> Entity {
        let e = world.spawn();
        let i = e.index as usize;
        world.kind[i] = EntityKind::Unit;
        world.faction[i] = faction;
        world.pos[i] = pos;
        world.order[i] = Order::Idle;
        world.stance[i] = Stance::ReturnFire;
        e
    }

    fn spawn_built_camp(world: &mut World, faction: Faction, pos: Vec2) -> Entity {
        let e = world.spawn();
        let i = e.index as usize;
        world.kind[i] = EntityKind::Building;
        world.faction[i] = faction;
        world.pos[i] = pos;
        world.health[i] = Health::full(Fixed::from_int(1000));
        world.building[i] = Building {
            kind: BuildingKind::Camp,
            level: 0,
            build_ticks_left: 0, // finished → can produce
            queue: Vec::new(),
            rally: None,
        };
        e
    }

    /// Same (seed, tick, world, territory, resources) ⇒ identical command list, twice over.
    #[test]
    fn deterministic_for_identical_inputs() {
        let mut world = World::new();
        spawn_unit(&mut world, Faction::Enemy, at(20, 0));
        spawn_unit(&mut world, Faction::Enemy, at(22, 3));
        spawn_built_camp(&mut world, Faction::Enemy, at(30, 0));
        spawn_unit(&mut world, Faction::Player, at(-5, 0)); // a hostile to target
        let terr = Territory {
            points: vec![ControlPoint::neutral(at(0, 0))],
        };
        let res = Resources::new(500);

        let mut rng_a = Rng::new(123);
        let a = commander_orders(
            &world,
            &terr,
            &res,
            &mut rng_a,
            &CommanderConfig::default(),
            &[],
            Faction::Enemy,
            Army::Neutral,
            60,
        );
        let mut rng_b = Rng::new(123);
        let b = commander_orders(
            &world,
            &terr,
            &res,
            &mut rng_b,
            &CommanderConfig::default(),
            &[],
            Faction::Enemy,
            Army::Neutral,
            60,
        );

        assert_eq!(a.len(), b.len(), "same inputs → same number of commands");
        // Commands are Copy/Debug; compare their debug forms field-for-field.
        for (x, y) in a.iter().zip(b.iter()) {
            assert_eq!(
                format!("{x:?}"),
                format!("{y:?}"),
                "command streams diverged"
            );
        }
    }

    /// An idle unit + a neutral control point ⇒ the commander issues an AttackMove ONTO the point
    /// (capture order), not toward the enemy.
    #[test]
    fn idle_unit_gets_capture_order_for_neutral_point() {
        let mut world = World::new();
        let u = spawn_unit(&mut world, Faction::Enemy, at(20, 0));
        // A distant player unit also exists, but the open point is the priority target.
        spawn_unit(&mut world, Faction::Player, at(-50, 0));
        let point = at(5, 0);
        let terr = Territory {
            points: vec![ControlPoint::neutral(point)],
        };
        let res = Resources::new(0); // no money → no production noise

        let mut rng = Rng::new(1);
        let cmds = commander_orders(
            &world,
            &terr,
            &res,
            &mut rng,
            &CommanderConfig::default(),
            &[],
            Faction::Enemy,
            Army::Neutral,
            60,
        );

        let captured = cmds.iter().any(|c| {
            matches!(c, Command::AttackMove { entity, target }
                if *entity == u && *target == point)
        });
        assert!(
            captured,
            "idle unit should be sent to capture the open point: {cmds:?}"
        );
        // And it must NOT have been pointed at the far player instead.
        assert!(
            !cmds
                .iter()
                .any(|c| matches!(c, Command::AttackMove { target, .. } if *target == at(-50, 0))),
            "the open point outranks the distant foe as a target"
        );
    }

    /// With no open point but a hostile present, the idle unit is pointed at the nearest foe.
    #[test]
    fn idle_unit_attacks_nearest_foe_when_no_open_point() {
        let mut world = World::new();
        let u = spawn_unit(&mut world, Faction::Enemy, at(20, 0));
        let near = at(10, 0);
        let far = at(-30, 0);
        spawn_unit(&mut world, Faction::Player, near);
        spawn_unit(&mut world, Faction::Player, far);
        // The only point is already owned by us → not "open", so the attack step applies. It sits
        // far from both foes (well beyond DEFEND_RADIUS) so the defensive-reaction step does not
        // preempt the attack we are asserting here — this test isolates the attack fallback.
        let terr = Territory {
            points: vec![ControlPoint {
                pos: at(0, 40),
                owner: Faction::Enemy,
                progress: Fixed::ZERO,
            }],
        };
        let res = Resources::new(0);

        let mut rng = Rng::new(1);
        let cmds = commander_orders(
            &world,
            &terr,
            &res,
            &mut rng,
            &CommanderConfig::default(),
            &[],
            Faction::Enemy,
            Army::Neutral,
            60,
        );
        let attacked_near = cmds.iter().any(|c| {
            matches!(c, Command::AttackMove { entity, target } if *entity == u && *target == near)
        });
        assert!(attacked_near, "should target the NEAREST foe: {cmds:?}");
    }

    /// Nearest-foe targeting picks the closer of two by squared distance, with a stable tie-break
    /// toward the lower index when distances are exactly equal.
    #[test]
    fn targeting_picks_nearest_with_stable_tiebreak() {
        // Two equidistant foes: the lower-index one wins.
        let mut world = World::new();
        let _u = spawn_unit(&mut world, Faction::Enemy, at(0, 0));
        let first = spawn_unit(&mut world, Faction::Player, at(10, 0)); // index lower
        let _second = spawn_unit(&mut world, Faction::Player, at(-10, 0)); // same dist
        let chosen = nearest_hostile(&world, at(0, 0), Faction::Enemy).unwrap();
        assert_eq!(
            chosen, world.pos[first.index as usize],
            "equal distance → earliest index wins (stable tie-break)"
        );

        // And a strictly closer foe is preferred regardless of order.
        let mut w2 = World::new();
        spawn_unit(&mut w2, Faction::Player, at(40, 0));
        spawn_unit(&mut w2, Faction::Player, at(5, 0));
        let chosen2 = nearest_hostile(&w2, at(0, 0), Faction::Enemy).unwrap();
        assert_eq!(chosen2, at(5, 0), "strictly nearer foe wins");
    }

    /// Production is queued only when the faction can afford a unit; never when broke.
    #[test]
    fn queues_production_only_when_affordable() {
        let mut world = World::new();
        let camp = spawn_built_camp(&mut world, Faction::Enemy, at(0, 0));

        // Broke: no production command at all.
        let terr = Territory::empty();
        let mut rng = Rng::new(1);
        let broke = commander_orders(
            &world,
            &terr,
            &Resources::new(economy::RIFLEMAN_COST - 1),
            &mut rng,
            &CommanderConfig::default(),
            &[],
            Faction::Enemy,
            Army::Neutral,
            60,
        );
        assert!(
            !broke
                .iter()
                .any(|c| matches!(c, Command::QueueProduction { .. })),
            "must not queue when it can't afford a unit: {broke:?}"
        );

        // Exactly a Rifleman's worth → queue one Rifleman.
        let mut rng = Rng::new(1);
        let afford = commander_orders(
            &world,
            &terr,
            &Resources::new(economy::RIFLEMAN_COST),
            &mut rng,
            &CommanderConfig::default(),
            &[],
            Faction::Enemy,
            Army::Neutral,
            60,
        );
        let queued_rifle = afford.iter().any(|c| {
            matches!(c, Command::QueueProduction { camp: cc, unit: UnitKind::Rifleman }
                if *cc == camp)
        });
        assert!(
            queued_rifle,
            "should queue a Rifleman when just affordable: {afford:?}"
        );
        assert!(
            !afford.iter().any(|c| matches!(
                c,
                Command::QueueProduction {
                    unit: UnitKind::Heavy,
                    ..
                }
            )),
            "not flush enough for a Heavy"
        );
    }

    /// When flush, the commander splurges on the Heavy bruiser.
    #[test]
    fn queues_heavy_when_flush() {
        let mut world = World::new();
        let _camp = spawn_built_camp(&mut world, Faction::Enemy, at(0, 0));
        let terr = Territory::empty();
        let mut rng = Rng::new(1);
        let cmds = commander_orders(
            &world,
            &terr,
            // The default (Veteran) tier's Heavy reserve is one Rifleman — the original threshold.
            &Resources::new(economy::HEAVY_COST + economy::RIFLEMAN_COST),
            &mut rng,
            &CommanderConfig::default(),
            &[],
            Faction::Enemy,
            Army::Neutral,
            60,
        );
        assert!(
            cmds.iter().any(|c| matches!(
                c,
                Command::QueueProduction {
                    unit: UnitKind::Heavy,
                    ..
                }
            )),
            "flush commander should buy a Heavy: {cmds:?}"
        );
    }

    /// An unbuilt (still-constructing) camp never produces.
    #[test]
    fn unbuilt_camp_does_not_produce() {
        let mut world = World::new();
        let e = spawn_built_camp(&mut world, Faction::Enemy, at(0, 0));
        world.building[e.index as usize].build_ticks_left = 100; // under construction
        let terr = Territory::empty();
        let mut rng = Rng::new(1);
        let cmds = commander_orders(
            &world,
            &terr,
            &Resources::new(10_000),
            &mut rng,
            &CommanderConfig::default(),
            &[],
            Faction::Enemy,
            Army::Neutral,
            60,
        );
        assert!(
            !cmds
                .iter()
                .any(|c| matches!(c, Command::QueueProduction { .. })),
            "a camp under construction must not produce"
        );
    }

    /// A unit standing on the open point it is capturing is NOT re-tasked (don't interrupt it).
    #[test]
    fn unit_on_open_point_is_not_retasked() {
        let mut world = World::new();
        let _u = spawn_unit(&mut world, Faction::Enemy, at(0, 0));
        let terr = Territory {
            points: vec![ControlPoint::neutral(at(0, 0))], // unit sits exactly on it
        };
        let mut rng = Rng::new(1);
        let cmds = commander_orders(
            &world,
            &terr,
            &Resources::new(0),
            &mut rng,
            &CommanderConfig::default(),
            &[],
            Faction::Enemy,
            Army::Neutral,
            60,
        );
        assert!(
            !cmds.iter().any(|c| matches!(c, Command::AttackMove { .. })),
            "a unit already on its capture point should be left alone: {cmds:?}"
        );
    }

    /// A unit already mid-order (AttackMove) is not re-tasked every period (no thrash).
    #[test]
    fn busy_unit_is_not_retasked() {
        let mut world = World::new();
        let u = spawn_unit(&mut world, Faction::Enemy, at(20, 0));
        world.order[u.index as usize] = Order::AttackMove(at(5, 0));
        let terr = Territory {
            points: vec![ControlPoint::neutral(at(5, 0))],
        };
        let mut rng = Rng::new(1);
        let cmds = commander_orders(
            &world,
            &terr,
            &Resources::new(0),
            &mut rng,
            &CommanderConfig::default(),
            &[],
            Faction::Enemy,
            Army::Neutral,
            60,
        );
        assert!(
            !cmds
                .iter()
                .any(|c| matches!(c, Command::AttackMove { entity, .. } if *entity == u)),
            "a unit already executing an AttackMove must not be re-issued: {cmds:?}"
        );
    }

    /// A HoldFire unit is bumped to FireAtWill so the army actually fights (engages on sight, not
    /// merely shoots back once hit — the latter would deadlock two opposing defensive lines).
    #[test]
    fn hold_fire_unit_is_bumped_to_fire_at_will() {
        let mut world = World::new();
        let u = spawn_unit(&mut world, Faction::Enemy, at(20, 0));
        world.stance[u.index as usize] = Stance::HoldFire;
        let terr = Territory::empty();
        let mut rng = Rng::new(1);
        let cmds = commander_orders(
            &world,
            &terr,
            &Resources::new(0),
            &mut rng,
            &CommanderConfig::default(),
            &[],
            Faction::Enemy,
            Army::Neutral,
            60,
        );
        assert!(
            cmds.iter().any(
                |c| matches!(c, Command::SetStance { entity, stance: Stance::FireAtWill }
                if *entity == u)
            ),
            "a HoldFire unit should be set to FireAtWill: {cmds:?}"
        );
    }

    /// The commander only ever touches its own faction's units/camps — never the player's.
    #[test]
    fn never_orders_other_factions() {
        let mut world = World::new();
        let player_unit = spawn_unit(&mut world, Faction::Player, at(0, 0));
        let player_camp = spawn_built_camp(&mut world, Faction::Player, at(3, 0));
        let terr = Territory {
            points: vec![ControlPoint::neutral(at(10, 0))],
        };
        let mut rng = Rng::new(1);
        let cmds = commander_orders(
            &world,
            &terr,
            &Resources::new(10_000),
            &mut rng,
            &CommanderConfig::default(),
            &[],
            Faction::Enemy,
            Army::Neutral,
            60,
        );
        for c in &cmds {
            match c {
                Command::AttackMove { entity, .. }
                | Command::SetStance { entity, .. }
                | Command::SetOrder { entity, .. } => {
                    assert_ne!(*entity, player_unit, "must not order a player unit");
                }
                Command::QueueProduction { camp, .. } => {
                    assert_ne!(*camp, player_camp, "must not produce at a player camp");
                }
                _ => {}
            }
        }
    }

    // --- Gone-dark hunt (config-gated detection-channel consult) -----------------------------
    //
    // The commander may CONSULT the detection channel to chase a hostile that has gone dark
    // (embodied) — but only within what detection HONESTLY permits (range + LoS, or `Hidden` →
    // nothing). The behavior is gated behind `CommanderConfig::hunt_embodied`, default OFF, so the
    // default scenes' command streams stay byte-identical (no golden-checksum churn).

    /// Embodied (gone-dark) variant of `spawn_unit`: a possessed hero the detection channel can tell.
    fn spawn_embodied(world: &mut World, faction: Faction, pos: Vec2) -> Entity {
        let e = spawn_unit(world, faction, pos);
        world.input_source[e.index as usize] = InputSource::Embodied;
        e
    }

    /// A scene where an idle Enemy unit (which doubles as the detection observer) sits in plain,
    /// in-range sight of a gone-dark Player hero, with a neutral point as the baseline objective.
    /// Returns `(world, terrain, territory, hero_pos, point_pos)`.
    fn hunt_scene() -> (World, Terrain, Territory, Vec2, Vec2) {
        let mut world = World::new();
        // The Enemy unit at the origin is BOTH the unit we task AND the faction's detection observer.
        spawn_unit(&mut world, Faction::Enemy, at(0, 0));
        let hero_pos = at(5, 0); // within the default tell_range (28), open LoS → detectable
        spawn_embodied(&mut world, Faction::Player, hero_pos);
        let point_pos = at(10, 0);
        let terr = Territory {
            points: vec![ControlPoint::neutral(point_pos)],
        };
        (world, Terrain::open(), terr, hero_pos, point_pos)
    }

    /// Derive the detection channel exactly as the host would, for `observer` over `world`/`terrain`.
    fn tells_for(world: &World, terrain: &Terrain, mode: TellMode, observer: Faction) -> Vec<Tell> {
        let config = DetectionConfig {
            tell_mode: mode,
            ..DetectionConfig::default()
        };
        let mut mem = DetectionMemory::new();
        detectable_embodiment(world, terrain, &config, observer, 0, &mut mem)
    }

    /// 1. **Default-off → byte-identical.** With `hunt_embodied = false`, the commander emits the
    ///    EXACT same command list whether or not detection tells are supplied — the gone-dark code
    ///    is fully bypassed, so the default scenes' lockstep/checksum streams are untouched.
    #[test]
    fn hunt_disabled_is_byte_identical_regardless_of_tells() {
        let (world, terrain, terr, _hero, _point) = hunt_scene();
        let res = Resources::new(0); // no production noise
        let tells = tells_for(&world, &terrain, TellMode::Subtle, Faction::Enemy);
        assert!(
            !tells.is_empty(),
            "scene precondition: the hero IS detectable"
        );

        let mut rng = Rng::new(7);
        let baseline = commander_orders(
            &world,
            &terr,
            &res,
            &mut rng,
            &CommanderConfig::default(),
            &[],
            Faction::Enemy,
            Army::Neutral,
            60,
        );
        // Same default (off) config, but now WITH a live tell present: must be ignored entirely.
        let mut rng = Rng::new(7);
        let with_tells_off = commander_orders(
            &world,
            &terr,
            &res,
            &mut rng,
            &CommanderConfig {
                hunt_embodied: false,
                ..CommanderConfig::default()
            },
            &tells,
            Faction::Enemy,
            Army::Neutral,
            60,
        );
        assert_eq!(
            baseline.len(),
            with_tells_off.len(),
            "flag off must ignore tells → identical command count"
        );
        for (x, y) in baseline.iter().zip(with_tells_off.iter()) {
            assert_eq!(
                format!("{x:?}"),
                format!("{y:?}"),
                "flag off must emit a byte-identical command stream even with tells present"
            );
        }
        // And the baseline genuinely heads for the capture point (so this test has real teeth).
        assert!(
            baseline
                .iter()
                .any(|c| matches!(c, Command::AttackMove { target, .. } if *target == _point)),
            "baseline should capture the open point: {baseline:?}"
        );
    }

    /// 2. **Enabled → reacts.** With `hunt_embodied = true` and a detectable gone-dark hostile, a
    ///    free unit is pressed toward the hero's revealed position INSTEAD of the capture point — a
    ///    different, sensible (honest) order responding to the tell.
    #[test]
    fn hunt_enabled_chases_detectable_gone_dark_hostile() {
        let (world, terrain, terr, hero, point) = hunt_scene();
        let res = Resources::new(0);
        let tells = tells_for(&world, &terrain, TellMode::Subtle, Faction::Enemy);
        assert!(
            !tells.is_empty(),
            "scene precondition: the hero IS detectable"
        );

        let mut rng = Rng::new(7);
        let cmds = commander_orders(
            &world,
            &terr,
            &res,
            &mut rng,
            &CommanderConfig {
                hunt_embodied: true,
                ..CommanderConfig::default()
            },
            &tells,
            Faction::Enemy,
            Army::Neutral,
            60,
        );
        assert!(
            cmds.iter()
                .any(|c| matches!(c, Command::AttackMove { target, .. } if *target == hero)),
            "the commander should press toward the gone-dark hero at {hero:?}: {cmds:?}"
        );
        assert!(
            !cmds
                .iter()
                .any(|c| matches!(c, Command::AttackMove { target, .. } if *target == point)),
            "chasing the hero outranks capturing the point: {cmds:?}"
        );
    }

    /// 3a. **Honest bound — out of range.** Flag ON, but the hero is beyond `tell_range`, so the
    ///     detection channel reveals NOTHING (empty tells) and the commander does NOT react — it
    ///     falls back to the ordinary capture plan. No omniscient peek.
    #[test]
    fn hunt_does_not_react_when_hostile_out_of_detection_range() {
        let mut world = World::new();
        spawn_unit(&mut world, Faction::Enemy, at(0, 0)); // observer + the unit we task
        spawn_embodied(&mut world, Faction::Player, at(60, 0)); // far beyond default tell_range 28
        let point = at(10, 0);
        let terr = Territory {
            points: vec![ControlPoint::neutral(point)],
        };
        let terrain = Terrain::open();
        let tells = tells_for(&world, &terrain, TellMode::Subtle, Faction::Enemy);
        assert!(
            tells.is_empty(),
            "out of range → detection legitimately reveals nothing"
        );

        let mut rng = Rng::new(7);
        let cmds = commander_orders(
            &world,
            &terr,
            &Resources::new(0),
            &mut rng,
            &CommanderConfig {
                hunt_embodied: true,
                ..CommanderConfig::default()
            },
            &tells,
            Faction::Enemy,
            Army::Neutral,
            60,
        );
        assert!(
            cmds.iter()
                .any(|c| matches!(c, Command::AttackMove { target, .. } if *target == point)),
            "with no tell, the commander reverts to capturing the point: {cmds:?}"
        );
        assert!(
            !cmds
                .iter()
                .any(|c| matches!(c, Command::AttackMove { target, .. } if *target == at(60, 0))),
            "the commander must NOT know the secret hero position (no omniscient peek): {cmds:?}"
        );
    }

    /// 3b. **Honest bound — line of sight blocked.** Flag ON and in range, but a wall blocks LoS, so
    ///     the channel reveals nothing and the commander does not react.
    #[test]
    fn hunt_does_not_react_when_line_of_sight_blocked() {
        let mut world = World::new();
        spawn_unit(&mut world, Faction::Enemy, at(0, 0));
        spawn_embodied(&mut world, Faction::Player, at(10, 0)); // in range, but...
        let mut terrain = Terrain::open();
        terrain.set_cover(69, 64, Cover::Heavy); // ...a wall strictly between (cells 64↔74)
        assert!(!terrain.line_of_sight(at(0, 0), at(10, 0)));
        let point = at(0, 12); // well outside the commit radius (6) so it IS a capture target
        let terr = Territory {
            points: vec![ControlPoint::neutral(point)],
        };
        let tells = tells_for(&world, &terrain, TellMode::Subtle, Faction::Enemy);
        assert!(tells.is_empty(), "no LoS → detection reveals nothing");

        let mut rng = Rng::new(7);
        let cmds = commander_orders(
            &world,
            &terr,
            &Resources::new(0),
            &mut rng,
            &CommanderConfig {
                hunt_embodied: true,
                ..CommanderConfig::default()
            },
            &tells,
            Faction::Enemy,
            Army::Neutral,
            60,
        );
        assert!(
            cmds.iter()
                .any(|c| matches!(c, Command::AttackMove { target, .. } if *target == point)),
            "LoS-blocked → no reaction, ordinary capture plan: {cmds:?}"
        );
    }

    /// 3c. **Honest bound — `Hidden` mode.** Even point-blank in plain sight, `TellMode::Hidden`
    ///     yields no tells, so a commander that consults the channel gains ZERO knowledge — the
    ///     "no omniscient peek" property is structural, not a discipline.
    #[test]
    fn hunt_gains_nothing_in_hidden_tell_mode() {
        let (world, terrain, terr, _hero, point) = hunt_scene(); // hero in plain, in-range sight
        let tells = tells_for(&world, &terrain, TellMode::Hidden, Faction::Enemy);
        assert!(tells.is_empty(), "Hidden mode reveals nothing, ever");

        let mut rng = Rng::new(7);
        let cmds = commander_orders(
            &world,
            &terr,
            &Resources::new(0),
            &mut rng,
            &CommanderConfig {
                hunt_embodied: true,
                ..CommanderConfig::default()
            },
            &tells,
            Faction::Enemy,
            Army::Neutral,
            60,
        );
        assert!(
            cmds.iter()
                .any(|c| matches!(c, Command::AttackMove { target, .. } if *target == point)),
            "Hidden mode → the commander chases nothing, captures as usual: {cmds:?}"
        );
    }

    /// 4. **Deterministic.** Identical inputs (world, tells, config, seed, tick) ⇒ identical command
    ///    list, twice over — the gone-dark path adds no float and no nondeterminism.
    #[test]
    fn hunt_is_deterministic_for_identical_inputs() {
        let (world, terrain, terr, _hero, _point) = hunt_scene();
        let tells = tells_for(&world, &terrain, TellMode::Subtle, Faction::Enemy);
        let cfg = CommanderConfig {
            hunt_embodied: true,
            ..CommanderConfig::default()
        };
        let run = || {
            let mut rng = Rng::new(99);
            commander_orders(
                &world,
                &terr,
                &Resources::new(0),
                &mut rng,
                &cfg,
                &tells,
                Faction::Enemy,
                Army::Neutral,
                60,
            )
        };
        let a = run();
        let b = run();
        assert_eq!(a.len(), b.len(), "same inputs → same command count");
        for (x, y) in a.iter().zip(b.iter()) {
            assert_eq!(
                format!("{x:?}"),
                format!("{y:?}"),
                "hunt command stream diverged"
            );
        }
    }

    /// The tell picker mirrors the other "nearest" scans: nearest by squared distance, stable
    /// tie-break toward the earliest tell in the slice. (No sqrt, no float.)
    #[test]
    fn nearest_tell_picks_nearest_with_stable_tiebreak() {
        let dummy = World::new().spawn(); // an entity handle; only `pos` matters to the picker
        let t = |x: i32, y: i32| Tell {
            unit: dummy,
            pos: at(x, y),
            age_ticks: 0,
        };
        // Strictly nearer wins regardless of order.
        let tells = [t(40, 0), t(5, 0)];
        assert_eq!(nearest_tell(&tells, at(0, 0)), Some(at(5, 0)));
        // Equal distance → the earlier slice entry wins (stable).
        let tied = [t(10, 0), t(-10, 0)];
        assert_eq!(nearest_tell(&tied, at(0, 0)), Some(at(10, 0)));
        // Empty slice → nothing.
        assert_eq!(nearest_tell(&[], at(0, 0)), None);
    }

    // --- Difficulty tiers (WS-E) -------------------------------------------------------------
    //
    // A tier scales the SEEDED planner's *choices* — production backlog depth, the Heavy reserve,
    // and the army re-plan cadence — never its *knowledge*. None of this reads the player's
    // embodiment/fog state (invariant #6 / §9): the gone-dark consult stays gated solely by
    // `hunt_embodied`, independent of tier. So a (mission, tier, seed) replays bit-identically,
    // and harder tiers are a *better commander*, not an omniscient one.

    use crate::components::ProductionItem;
    use crate::mission_tuning::Difficulty;

    /// A config at an explicit tier, hunt off (the difficulty axis in isolation).
    fn tier_cfg(difficulty: Difficulty) -> CommanderConfig {
        CommanderConfig {
            difficulty,
            ..CommanderConfig::default()
        }
    }

    /// The default config is the `Veteran` tier — and produces a byte-identical command stream to
    /// an explicitly-`Veteran` config. This is the property that keeps the default scenes' golden
    /// checksums untouched after the difficulty knob was added.
    #[test]
    fn default_config_is_veteran_and_byte_identical() {
        let mut world = World::new();
        spawn_unit(&mut world, Faction::Enemy, at(20, 0));
        spawn_built_camp(&mut world, Faction::Enemy, at(30, 0));
        spawn_unit(&mut world, Faction::Player, at(-5, 0));
        let terr = Territory {
            points: vec![ControlPoint::neutral(at(0, 0))],
        };
        let res = Resources::new(500);

        assert_eq!(CommanderConfig::default().difficulty, Difficulty::Veteran);

        let run = |cfg: &CommanderConfig| {
            let mut rng = Rng::new(42);
            commander_orders(
                &world,
                &terr,
                &res,
                &mut rng,
                cfg,
                &[],
                Faction::Enemy,
                Army::Neutral,
                0,
            )
        };
        let default = run(&CommanderConfig::default());
        let veteran = run(&tier_cfg(Difficulty::Veteran));
        assert_eq!(default.len(), veteran.len());
        for (x, y) in default.iter().zip(veteran.iter()) {
            assert_eq!(
                format!("{x:?}"),
                format!("{y:?}"),
                "default must equal explicit Veteran"
            );
        }
    }

    /// **Reserve / unit-mix knob.** At a purse that sits between the two thresholds, `Elite` (zero
    /// reserve) splurges on a Heavy while `Veteran` (a one-Rifleman reserve) buys the cheap body —
    /// the same honest survey, different spending discipline.
    #[test]
    fn elite_buys_heavy_where_veteran_buys_rifleman() {
        let mut world = World::new();
        let camp = spawn_built_camp(&mut world, Faction::Enemy, at(0, 0));
        let terr = Territory::empty();
        // 250: above HEAVY_COST (220) but below Veteran's HEAVY_COST + one Rifleman (320).
        let purse = economy::HEAVY_COST + 30;

        let queued = |cfg: &CommanderConfig| -> Vec<UnitKind> {
            let mut rng = Rng::new(1);
            commander_orders(
                &world,
                &terr,
                &Resources::new(purse),
                &mut rng,
                cfg,
                &[],
                Faction::Enemy,
                Army::Neutral,
                0,
            )
            .into_iter()
            .filter_map(|c| match c {
                Command::QueueProduction { camp: cc, unit } if cc == camp => Some(unit),
                _ => None,
            })
            .collect()
        };

        assert_eq!(
            queued(&tier_cfg(Difficulty::Veteran)),
            vec![UnitKind::Rifleman],
            "Veteran keeps a reserve → cheap body at this purse"
        );
        assert_eq!(
            queued(&tier_cfg(Difficulty::Elite)),
            vec![UnitKind::Heavy],
            "Elite keeps no reserve → splurges on the Heavy the moment it's affordable"
        );
    }

    /// **Aggression knob.** With the camp already holding one queued item, whether the commander
    /// stacks a *second* depends on the tier's `max_queue_depth`: `Recruit` (1) declines, `Veteran`
    /// (2) and `Elite` (3) add one. A pure, single-call read of the backlog cap.
    #[test]
    fn backlog_depth_scales_with_tier() {
        let queues_more = |difficulty: Difficulty| -> bool {
            let mut world = World::new();
            let _camp = spawn_built_camp(&mut world, Faction::Enemy, at(0, 0));
            // Pre-load one item so the depth check is what decides a second.
            world.building[_camp.index as usize]
                .queue
                .push(ProductionItem {
                    kind: UnitKind::Rifleman,
                    ticks_left: 10,
                });
            let terr = Territory::empty();
            let mut rng = Rng::new(1);
            commander_orders(
                &world,
                &terr,
                &Resources::new(10_000), // flush, so only the depth cap gates
                &mut rng,
                &tier_cfg(difficulty),
                &[],
                Faction::Enemy,
                Army::Neutral,
                0,
            )
            .iter()
            .any(|c| matches!(c, Command::QueueProduction { .. }))
        };
        assert!(
            !queues_more(Difficulty::Recruit),
            "Recruit (depth 1) won't stack a second"
        );
        assert!(
            queues_more(Difficulty::Veteran),
            "Veteran (depth 2) stacks a second"
        );
        assert!(
            queues_more(Difficulty::Elite),
            "Elite (depth 3) stacks a second"
        );
    }

    /// **Cadence knob.** `Recruit` (stride 2) re-tasks its army only on even commander cycles, so on
    /// an off-cycle it issues no movement order even with an idle unit and an open point; `Veteran`
    /// (stride 1) re-tasks every cycle. Reinforcement is unaffected (not strided). Pure function of
    /// `tick`, so it stays deterministic across peers.
    #[test]
    fn cadence_stride_skips_retask_on_off_cycle() {
        // An idle Enemy unit with a neutral point to take → a re-task cycle yields one AttackMove.
        let scene = || {
            let mut world = World::new();
            spawn_unit(&mut world, Faction::Enemy, at(20, 0));
            let terr = Territory {
                points: vec![ControlPoint::neutral(at(5, 0))],
            };
            (world, terr)
        };
        let has_attackmove = |difficulty: Difficulty, tick: u64| -> bool {
            let (world, terr) = scene();
            let mut rng = Rng::new(1);
            commander_orders(
                &world,
                &terr,
                &Resources::new(0), // no production noise
                &mut rng,
                &tier_cfg(difficulty),
                &[],
                Faction::Enemy,
                Army::Neutral,
                tick,
            )
            .iter()
            .any(|c| matches!(c, Command::AttackMove { .. }))
        };

        // Cycle 0 (tick 0): both tiers re-task.
        assert!(
            has_attackmove(Difficulty::Recruit, 0),
            "on-cycle: Recruit re-tasks"
        );
        assert!(
            has_attackmove(Difficulty::Veteran, 0),
            "Veteran always re-tasks"
        );
        // Cycle 1 (tick = one period): Recruit skips (stride 2), Veteran still acts (stride 1).
        assert!(
            !has_attackmove(Difficulty::Recruit, COMMANDER_PERIOD),
            "off-cycle: Recruit's sluggish cadence skips the re-task"
        );
        assert!(
            has_attackmove(Difficulty::Veteran, COMMANDER_PERIOD),
            "Veteran re-tasks every cycle regardless"
        );
    }

    /// The headline WS-E property: a given **mission + tier + seed** replays **bit-identically**.
    /// The commander draws from its own stream seeded `sim_seed ^ faction` (never `Sim::rng`); two
    /// runs at the same tier produce the identical command stream, and the tier genuinely changes
    /// the plan (so the knob has teeth) — all without any float or omniscient read.
    #[test]
    fn mission_tier_seed_replays_bit_identically() {
        const SIM_SEED: u64 = 0xD0E1;
        let scene = || {
            let mut world = World::new();
            spawn_unit(&mut world, Faction::Enemy, at(20, 0));
            spawn_unit(&mut world, Faction::Enemy, at(22, 3));
            spawn_built_camp(&mut world, Faction::Enemy, at(30, 0));
            spawn_unit(&mut world, Faction::Player, at(-5, 0));
            let terr = Territory {
                points: vec![ControlPoint::neutral(at(0, 0))],
            };
            (world, terr)
        };
        // The commander RNG is the host's own stream, seeded sim_seed ^ faction.
        let plan = |difficulty: Difficulty| -> Vec<String> {
            let (world, terr) = scene();
            let mut rng = Rng::new(SIM_SEED ^ Faction::Enemy.index() as u64);
            commander_orders(
                &world,
                &terr,
                // A purse that makes the unit-mix knob observable (250).
                &Resources::new(economy::HEAVY_COST + 30),
                &mut rng,
                &tier_cfg(difficulty),
                &[],
                Faction::Enemy,
                Army::Neutral,
                0,
            )
            .iter()
            .map(|c| format!("{c:?}"))
            .collect()
        };

        for d in Difficulty::ALL {
            assert_eq!(
                plan(d),
                plan(d),
                "same (mission, tier, seed) ⇒ identical stream"
            );
        }
        // ...and distinct tiers really do reshape the plan (Veteran rifle vs Elite heavy here).
        assert_ne!(
            plan(Difficulty::Veteran),
            plan(Difficulty::Elite),
            "the difficulty knob must change the command stream"
        );
    }

    // --- Per-node param overrides (the campaign battle-spec commander flavor) -----------------
    //
    // A per-node battle spec may dial the tier's integer knobs (aggression / reserve / cadence)
    // without inventing a new tier. `None` overrides reproduce the tier byte-for-byte; a `Some`
    // override replaces just that knob, clamped so a bad authored value can't break the planner.

    /// No overrides ⇒ `resolved_params` is *exactly* the tier's `params` for every tier — the
    /// byte-identical default that keeps the golden-checksum streams untouched. And each `Some`
    /// override replaces its knob, with the documented clamps (reserve `>= 0`, stride `>= 1`).
    #[test]
    fn resolved_params_default_matches_tier_and_overrides_clamp() {
        for d in Difficulty::ALL {
            let cfg = CommanderConfig {
                difficulty: d,
                ..CommanderConfig::default()
            };
            assert_eq!(
                cfg.resolved_params(),
                d.params(),
                "no override ⇒ the tier's params"
            );
        }

        let cfg = CommanderConfig {
            difficulty: Difficulty::Veteran,
            max_queue_depth: Some(5),
            heavy_reserve: Some(-10), // clamps to 0
            command_stride: Some(0),  // clamps to 1
            ..CommanderConfig::default()
        };
        let p = cfg.resolved_params();
        assert_eq!(p.max_queue_depth, 5, "backlog override applied");
        assert_eq!(p.heavy_reserve, 0, "negative reserve clamped to 0");
        assert_eq!(
            p.command_stride, 1,
            "stride clamped to >= 1 (no div-by-zero)"
        );
    }

    /// The override has real teeth in the planner: a per-node backlog override lets an otherwise
    /// shallow-queuing `Recruit` commander stack a second production item — the flavor a campaign
    /// node carries reaches the actual command stream, not just the config struct.
    #[test]
    fn backlog_override_lets_recruit_stack_a_second_item() {
        let mut world = World::new();
        let camp = spawn_built_camp(&mut world, Faction::Enemy, at(0, 0));
        // Pre-load one item so the depth cap is what decides a second.
        world.building[camp.index as usize]
            .queue
            .push(ProductionItem {
                kind: UnitKind::Rifleman,
                ticks_left: 10,
            });
        let terr = Territory::empty();
        // Recruit's depth cap is 1 (declines a second) — but a per-node override to 3 stacks one.
        let cfg = CommanderConfig {
            difficulty: Difficulty::Recruit,
            max_queue_depth: Some(3),
            ..CommanderConfig::default()
        };
        let mut rng = Rng::new(1);
        let cmds = commander_orders(
            &world,
            &terr,
            &Resources::new(10_000), // flush, so only the depth cap gates
            &mut rng,
            &cfg,
            &[],
            Faction::Enemy,
            Army::Neutral,
            0,
        );
        assert!(
            cmds.iter()
                .any(|c| matches!(c, Command::QueueProduction { .. })),
            "the override lets Recruit stack a second item: {cmds:?}"
        );
    }

    // --- Seeded variation: same seed replays, different seed can differ (the fun-killer fix) ----
    //
    // The headline change: the commander no longer marches the identical greedy opening every
    // match. It draws an exploration roll from its OWN seeded stream, so a fixed seed is perfectly
    // reproducible (lockstep-safe) while a different seed yields a different — but equally
    // deterministic — plan. A human can't learn one pattern and beat it forever.

    /// `choose_open_point` is greedy-nearest at `explore_pct == 0`, always the second-nearest at
    /// `100`, deterministic for a fixed `(seed, pct)`, and takes BOTH branches across seeds at a
    /// moderate percentage — the mechanism behind the per-seed variation.
    #[test]
    fn choose_open_point_explores_deterministically() {
        let near = at(5, 0);
        let far = at(20, 0);
        let terr = Territory {
            points: vec![ControlPoint::neutral(near), ControlPoint::neutral(far)],
        };
        let choose = |seed: u64, pct: u32| {
            let mut rng = Rng::new(seed);
            choose_open_point(&terr, at(0, 0), Faction::Enemy, pct, &mut rng)
        };

        // 0% ⇒ pure greedy-nearest (byte-identical to the old behavior); 100% ⇒ always the second.
        assert_eq!(
            choose(1, 0),
            Some(near),
            "explore_pct 0 is always the nearest"
        );
        assert_eq!(
            choose(1, 100),
            Some(far),
            "explore_pct 100 is always the second-nearest"
        );
        // Deterministic for a fixed (seed, pct).
        assert_eq!(
            choose(9, 25),
            choose(9, 25),
            "same seed + pct ⇒ same choice"
        );
        // A moderate percentage takes BOTH branches over a run of the seeded stream (drawn from one
        // evolving generator, the way the live commander does — a fresh generator's very first
        // draw is not well distributed across nearby seeds).
        let mut rng = Rng::new(0xC0FFEE);
        let (mut saw_near, mut saw_far) = (false, false);
        for _ in 0..200 {
            match choose_open_point(&terr, at(0, 0), Faction::Enemy, 25, &mut rng) {
                Some(p) if p == near => saw_near = true,
                Some(p) if p == far => saw_far = true,
                _ => {}
            }
        }
        assert!(
            saw_near && saw_far,
            "a moderate explore_pct must reach both the nearest and second"
        );
    }

    /// A single open point is not a *choice*, so `choose_open_point` returns it WITHOUT drawing —
    /// the RNG stream is untouched (a one-candidate scene advances the seed by nothing, which is
    /// what keeps single-point scenes reproducible and cheap).
    #[test]
    fn choose_open_point_single_candidate_draws_no_rng() {
        let only = at(7, 0);
        let terr = Territory {
            points: vec![ControlPoint::neutral(only)],
        };
        let mut rng = Rng::new(3);
        let before = rng.checksum_state();
        assert_eq!(
            choose_open_point(&terr, at(0, 0), Faction::Enemy, 40, &mut rng),
            Some(only)
        );
        assert_eq!(
            rng.checksum_state(),
            before,
            "no real choice → no draw → stream untouched"
        );
    }

    /// End-to-end: a fixed seed replays bit-identically, and *some* different seed produces a
    /// different plan (the whole army's command stream). Uses several units and several open
    /// points so every free unit faces a real objective choice.
    #[test]
    fn same_seed_replays_but_a_different_seed_varies_the_plan() {
        let scene = || {
            let mut world = World::new();
            spawn_unit(&mut world, Faction::Enemy, at(0, 0));
            spawn_unit(&mut world, Faction::Enemy, at(1, 0));
            spawn_unit(&mut world, Faction::Enemy, at(0, 1));
            spawn_unit(&mut world, Faction::Enemy, at(1, 1));
            let terr = Territory {
                points: vec![
                    ControlPoint::neutral(at(10, 0)),
                    ControlPoint::neutral(at(12, 3)),
                    ControlPoint::neutral(at(20, -2)),
                    ControlPoint::neutral(at(9, -5)),
                ],
            };
            (world, terr)
        };
        let plan = |seed: u64| -> Vec<String> {
            let (world, terr) = scene();
            let mut rng = Rng::new(seed);
            commander_orders(
                &world,
                &terr,
                &Resources::new(0),
                &mut rng,
                &CommanderConfig::default(),
                &[],
                Faction::Enemy,
                Army::Neutral,
                60,
            )
            .iter()
            .map(|c| format!("{c:?}"))
            .collect()
        };
        assert_eq!(
            plan(42),
            plan(42),
            "a fixed seed must replay bit-identically"
        );
        let base = plan(1);
        let varied = (2..512u64).any(|s| plan(s) != base);
        assert!(
            varied,
            "a different seed must be able to yield a different plan"
        );
    }

    // --- Preserve (retreat): pull damaged units back instead of feeding them in ----------------

    /// A badly hurt unit (below the tier's `retreat_hp_pct`) is ordered to FALL BACK to its nearest
    /// friendly building — even though it is mid-AttackMove — rather than being left to die.
    #[test]
    fn retreat_pulls_a_low_hp_unit_back_to_the_camp() {
        let mut world = World::new();
        let _camp = spawn_built_camp(&mut world, Faction::Enemy, at(0, 0));
        let u = spawn_unit(&mut world, Faction::Enemy, at(20, 0));
        world.order[u.index as usize] = Order::AttackMove(at(50, 0)); // busy — losing this fight
        world.health[u.index as usize] = Health {
            cur: Fixed::from_int(10),
            max: Fixed::from_int(100), // 10% < Veteran's 25% retreat threshold
        };
        let mut rng = Rng::new(1);
        let cmds = commander_orders(
            &world,
            &Territory::empty(),
            &Resources::new(0),
            &mut rng,
            &CommanderConfig::default(),
            &[],
            Faction::Enemy,
            Army::Neutral,
            60,
        );
        assert!(
            cmds.iter().any(|c| matches!(c,
                Command::SetOrder { entity, order: Order::FallBack(rally) }
                    if *entity == u && *rally == at(0, 0))),
            "a low-HP unit should be ordered to FallBack to the camp: {cmds:?}"
        );
        // And it must NOT also be handed an attack/capture order this cycle (retreat wins).
        assert!(
            !cmds
                .iter()
                .any(|c| matches!(c, Command::AttackMove { entity, .. } if *entity == u)),
            "a retreating unit is not simultaneously sent to attack: {cmds:?}"
        );
    }

    /// `Recruit` (retreat_hp_pct 0) never preserves — it feeds a hurt unit right back in, which is
    /// part of what makes the easy tier easy. Same wound, no FallBack.
    #[test]
    fn recruit_does_not_retreat_a_low_hp_unit() {
        let mut world = World::new();
        let _camp = spawn_built_camp(&mut world, Faction::Enemy, at(0, 0));
        let u = spawn_unit(&mut world, Faction::Enemy, at(20, 0));
        world.health[u.index as usize] = Health {
            cur: Fixed::from_int(5),
            max: Fixed::from_int(100),
        };
        let mut rng = Rng::new(1);
        let cmds = commander_orders(
            &world,
            &Territory::empty(),
            &Resources::new(0),
            &mut rng,
            &tier_cfg(Difficulty::Recruit),
            &[],
            Faction::Enemy,
            Army::Neutral,
            0,
        );
        assert!(
            !cmds.iter().any(|c| matches!(
                c,
                Command::SetOrder {
                    order: Order::FallBack(_),
                    ..
                }
            )),
            "Recruit never retreats — it has no preservation instinct: {cmds:?}"
        );
    }

    /// A hurt unit with NOWHERE to fall back to (no friendly building) is left to fight, not sent
    /// to some arbitrary origin — the commander only retreats when there is a real rally.
    #[test]
    fn no_retreat_without_a_friendly_building() {
        let mut world = World::new();
        let u = spawn_unit(&mut world, Faction::Enemy, at(20, 0));
        world.order[u.index as usize] = Order::HoldPosition;
        world.health[u.index as usize] = Health {
            cur: Fixed::from_int(5),
            max: Fixed::from_int(100),
        };
        let mut rng = Rng::new(1);
        let cmds = commander_orders(
            &world,
            &Territory::empty(),
            &Resources::new(0),
            &mut rng,
            &CommanderConfig::default(), // Veteran (would retreat if it could)
            &[],
            Faction::Enemy,
            Army::Neutral,
            60,
        );
        assert!(
            !cmds.iter().any(|c| matches!(
                c,
                Command::SetOrder {
                    order: Order::FallBack(_),
                    ..
                }
            )),
            "with no rally the commander must not retreat the unit: {cmds:?}"
        );
    }

    // --- Defend: hold a threatened owned point before grabbing new ground ----------------------

    /// When a hostile is closing on a control point the faction OWNS, a free unit is peeled back to
    /// hold it (an `AttackMove` onto the point) — the defensive reaction the greedy planner lacked.
    #[test]
    fn defends_a_threatened_owned_point() {
        let mut world = World::new();
        // A point we own, with a player unit closing on it (within DEFEND_RADIUS = 12).
        spawn_unit(&mut world, Faction::Player, at(5, 0));
        let held = at(0, 0);
        let terr = Territory {
            points: vec![ControlPoint {
                pos: held,
                owner: Faction::Enemy,
                progress: Fixed::ZERO,
            }],
        };
        // A free enemy unit well away from the point (not already on it).
        let defender = spawn_unit(&mut world, Faction::Enemy, at(30, 0));
        let mut rng = Rng::new(1);
        let cmds = commander_orders(
            &world,
            &terr,
            &Resources::new(0),
            &mut rng,
            &CommanderConfig::default(),
            &[],
            Faction::Enemy,
            Army::Neutral,
            60,
        );
        assert!(
            cmds.iter().any(|c| matches!(c,
                Command::AttackMove { entity, target } if *entity == defender && *target == held)),
            "a threatened owned point should pull a defender back to it: {cmds:?}"
        );
    }

    /// `Recruit` does not defend — the same threatened point is ignored and the free unit just
    /// presses the nearest foe. (The tier's defensive instinct is off.)
    #[test]
    fn recruit_does_not_defend_its_point() {
        let mut world = World::new();
        let foe = at(5, 0);
        spawn_unit(&mut world, Faction::Player, foe);
        let held = at(0, 0);
        let terr = Territory {
            points: vec![ControlPoint {
                pos: held,
                owner: Faction::Enemy,
                progress: Fixed::ZERO,
            }],
        };
        let unit = spawn_unit(&mut world, Faction::Enemy, at(30, 0));
        let mut rng = Rng::new(1);
        let cmds = commander_orders(
            &world,
            &terr,
            &Resources::new(0),
            &mut rng,
            &tier_cfg(Difficulty::Recruit),
            &[],
            Faction::Enemy,
            Army::Neutral,
            0,
        );
        // It presses the foe, not the held point (no defense at this tier).
        assert!(
            cmds.iter().any(|c| matches!(c,
                Command::AttackMove { entity, target } if *entity == unit && *target == foe)),
            "Recruit ignores the threat and presses the foe: {cmds:?}"
        );
        assert!(
            !cmds.iter().any(|c| matches!(c,
                Command::AttackMove { entity, target } if *entity == unit && *target == held)),
            "Recruit must not defend the point: {cmds:?}"
        );
    }

    // --- Concentrate (focus-fire): converge attackers on one priority target -------------------

    /// With no ground to take, a concentrating tier (`Veteran`) sends BOTH free units at the ONE
    /// priority target — the hostile nearest the commander's own line — instead of each dribbling
    /// toward its own nearest foe. `Recruit` (no concentration) scatters them.
    #[test]
    fn concentrate_converges_attackers_that_recruit_would_scatter() {
        // unit1 sits by foe A; unit2 sits far away by foe B. A is the closest contact overall.
        let mut world = World::new();
        let a_pos = at(4, 0); // near unit1
        let b_pos = at(0, -55); // near unit2
        spawn_unit(&mut world, Faction::Player, a_pos);
        spawn_unit(&mut world, Faction::Player, b_pos);
        let u1 = spawn_unit(&mut world, Faction::Enemy, at(0, 0)); // dist 4 to A
        let u2 = spawn_unit(&mut world, Faction::Enemy, at(0, -50)); // dist 5 to B

        let plan = |cfg: &CommanderConfig| {
            let mut rng = Rng::new(1);
            commander_orders(
                &world,
                &Territory::empty(), // no ground to take → the attack step runs
                &Resources::new(0),
                &mut rng,
                cfg,
                &[],
                Faction::Enemy,
                Army::Neutral,
                0,
            )
        };
        let target_of = |cmds: &[Command], u: Entity| -> Option<Vec2> {
            cmds.iter().find_map(|c| match c {
                Command::AttackMove { entity, target } if *entity == u => Some(*target),
                _ => None,
            })
        };

        // Veteran concentrates: both units converge on the nearest contact (foe A).
        let vet = plan(&CommanderConfig::default());
        assert_eq!(target_of(&vet, u1), Some(a_pos), "u1 → priority target A");
        assert_eq!(
            target_of(&vet, u2),
            Some(a_pos),
            "u2 also focus-fires the priority target A"
        );

        // Recruit scatters: each unit presses its own nearest foe (u1→A, u2→B).
        let rec = plan(&tier_cfg(Difficulty::Recruit));
        assert_eq!(
            target_of(&rec, u1),
            Some(a_pos),
            "Recruit u1 → its own nearest (A)"
        );
        assert_eq!(
            target_of(&rec, u2),
            Some(b_pos),
            "Recruit u2 → its own nearest (B), scattered"
        );
    }

    /// `nearest_contact` is the focus-fire target picker: the hostile closest to *any* of our units
    /// (where the lines meet), stable and float-free. `None` when we field no unit or see no foe.
    #[test]
    fn nearest_contact_finds_where_the_lines_meet() {
        let mut world = World::new();
        spawn_unit(&mut world, Faction::Enemy, at(0, 0));
        spawn_unit(&mut world, Faction::Enemy, at(0, -50));
        spawn_unit(&mut world, Faction::Player, at(3, 0)); // 3 from our first unit — the contact
        spawn_unit(&mut world, Faction::Player, at(0, -60)); // 10 from our second unit
        assert_eq!(nearest_contact(&world, Faction::Enemy), Some(at(3, 0)));

        // No hostiles → None. No units → None.
        let mut only_ours = World::new();
        spawn_unit(&mut only_ours, Faction::Enemy, at(0, 0));
        assert_eq!(nearest_contact(&only_ours, Faction::Enemy), None);
        let mut only_foes = World::new();
        spawn_unit(&mut only_foes, Faction::Player, at(0, 0));
        assert_eq!(nearest_contact(&only_foes, Faction::Enemy), None);
    }
}
