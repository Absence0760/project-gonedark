//! Host-side **onboarding / first-possession teach** layer (CP-7) — the OBSERVE-only scripted-beat
//! state machine that teaches the "going dark" twist inside PvE mission 1.
//!
//! ## The discipline (mirrors `engine::objectives`)
//!
//! An [`Onboarding`] is advanced **after `Sim::step`** from the same already-checksummed signals the
//! objective layer reads — the embody/surface/death edges + the elapsed tick. It owns no [`Sim`], can
//! never be handed `&mut Sim`, and mutates only its own small state, so it adds **zero** per-tick
//! checksum surface (invariants #1/#7): the teach layer is authored and tuned with no lockstep risk.
//! It is the exact footing [`ObjectiveSet`](crate::objectives::ObjectiveSet) and
//! [`InSessionShell`](crate::session_shell::InSessionShell) stand on.
//!
//! ## Fairness (invariant #6) is structural
//!
//! Every beat telegraphs the **cost** and the **controls** — never the enemy's position. The copy is
//! static strings carrying no world state, so it structurally cannot leak intel while the map is dark.
//! The first beat fires the instant you go dark (it telegraphs the cost *before* it bites); the death
//! payoff frames the auto-surface as **your** overstay, so the loss reads as *"I stayed too long,"*
//! not *"the game robbed me."*
//!
//! ## The model
//!
//! A tiny three-beat state machine, each beat **fired once** per match:
//!   * [`TeachBeat::WentDark`] — first embody: the map is blind, you see only what your soldier sees,
//!     press Surface to return to command.
//!   * [`TeachBeat::Lingering`] — embodied past [`LINGER_TICKS`]: a time-cost nudge (your squad fights
//!     without you), **no intel**.
//!   * [`TeachBeat::StayedTooLong`] — first embodied death / auto-surface: the framing payoff.
//!
//! [`Onboarding::observe`] folds one tick of [`TeachInput`] into the machine and returns the beat that
//! fired this tick (if any); [`Onboarding::current_prompt`] derives the [`Prompt`] the embodied-safe
//! [`render::prompt`](gonedark_render::prompt) pass draws, faded toward the end of its window. Both the
//! transition logic and the copy mapping are pure free-fn-backed, so they are unit-tested without a GPU.

use gonedark_render::prompt::{Prompt, PromptTone};

/// How long (sim ticks at 60 Hz) the player must stay embodied before the lingering nudge fires.
/// ~6 s — long enough that a quick in-and-out raid never trips it, short enough to bite an overstay.
pub const LINGER_TICKS: u64 = 360;
/// How long (sim ticks) a raised prompt stays on screen before it expires. ~5 s — time to read it.
pub const PROMPT_TICKS: u64 = 300;
/// The trailing slice of the display window (sim ticks) over which a prompt fades out. ~1 s.
pub const FADE_TICKS: u64 = 60;

/// Which teaching beat. Each fires at most once per match.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum TeachBeat {
    /// First embody — telegraph the cost + the Surface control the instant the world goes dark.
    WentDark,
    /// Embodied past [`LINGER_TICKS`] — a time-cost nudge (no intel, invariant #6).
    Lingering,
    /// First embodied death / auto-surface — frame it as the player's own overstay.
    StayedTooLong,
}

/// The per-tick signals the teach layer observes — all derived from already-checksummed state or
/// transient host flags, exactly like [`ObserveCtx`](crate::objectives::ObserveCtx). No `&Sim`, no
/// mutation, so observing it folds nothing into the checksum (invariants #1/#7).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct TeachInput {
    /// Is the local player embodied right now — i.e. AFTER this frame's embody/surface/death flips?
    pub embodied: bool,
    /// Did the possessed avatar **die** this frame (auto-surface on death), as opposed to a manual
    /// Surface? Distinguishes the "you stayed too long" payoff from a clean voluntary return.
    pub avatar_died: bool,
    /// Ticks elapsed in the match (`Sim::tick_count()`) — the dark-dwell clock + the fade clock.
    pub tick: u64,
}

/// The CP-7 onboarding state machine — pure host-side session state. Owns no `Sim` and can never be
/// handed one, so it cannot perturb the per-tick checksum (invariants #1/#7). Disabled (a total
/// no-op) for every scene but the campaign mission, so no tutorial prompt ever shows in skirmish or
/// the debug sandboxes.
#[derive(Clone, PartialEq, Debug)]
pub struct Onboarding {
    /// Only the campaign mission teaches.
    enabled: bool,
    went_dark_fired: bool,
    lingering_fired: bool,
    stayed_too_long_fired: bool,
    /// Was the player embodied last tick — edge-detects the first embody and a fresh dark stretch.
    was_embodied: bool,
    /// The tick the current dark stretch began, or `None` while in the command view.
    dark_since: Option<u64>,
    /// The currently-raised beat + the tick it was raised (for the timed fade-out), or `None`.
    active: Option<(TeachBeat, u64)>,
}

impl Onboarding {
    /// A teach machine for a scene. `enabled` is set only for the campaign mission ([`Scene::Mission1`]
    /// (crate::Scene::Mission1)); every other scene gets a disabled, no-op machine.
    pub fn new(enabled: bool) -> Self {
        Onboarding {
            enabled,
            went_dark_fired: false,
            lingering_fired: false,
            stayed_too_long_fired: false,
            was_embodied: false,
            dark_since: None,
            active: None,
        }
    }

    /// A disabled machine (every non-teaching scene).
    pub fn disabled() -> Self {
        Onboarding::new(false)
    }

    /// Whether this scene teaches (the host gates the observe/draw on it).
    pub fn is_enabled(&self) -> bool {
        self.enabled
    }

    /// Fold one tick of signals into the machine, raising at most one beat (fired once each) and
    /// expiring a shown prompt past its window. Returns the beat that fired this tick, if any. A
    /// no-op (returns `None`) when disabled. Reads only `Copy` snapshots — it mutates only `self`,
    /// never any sim state, so it cannot desync (invariants #1/#7).
    pub fn observe(&mut self, input: TeachInput) -> Option<TeachBeat> {
        if !self.enabled {
            return None;
        }

        let mut fired = None;

        if input.avatar_died && self.was_embodied && !self.stayed_too_long_fired {
            // The death payoff: frame the auto-surface as the player's own overstay. Takes priority,
            // and the dark stretch is over.
            self.stayed_too_long_fired = true;
            self.active = Some((TeachBeat::StayedTooLong, input.tick));
            self.dark_since = None;
            fired = Some(TeachBeat::StayedTooLong);
        } else if input.embodied {
            let since = *self.dark_since.get_or_insert(input.tick);
            if !self.was_embodied && !self.went_dark_fired {
                // First embody — telegraph the cost the instant the world goes dark.
                self.went_dark_fired = true;
                self.active = Some((TeachBeat::WentDark, input.tick));
                fired = Some(TeachBeat::WentDark);
            } else if !self.lingering_fired && input.tick.saturating_sub(since) >= LINGER_TICKS {
                // Lingered too long in the dark — a time-cost nudge.
                self.lingering_fired = true;
                self.active = Some((TeachBeat::Lingering, input.tick));
                fired = Some(TeachBeat::Lingering);
            }
        } else {
            // Back in the command view (a voluntary surface or already ejected): the dark clock
            // resets so a later re-embody re-arms the dwell measurement.
            self.dark_since = None;
        }

        // Expire a shown prompt once its window has elapsed (keeps `active` honest for queries/tests).
        if let Some((_, raised)) = self.active {
            if input.tick.saturating_sub(raised) >= PROMPT_TICKS {
                self.active = None;
            }
        }

        self.was_embodied = input.embodied;
        fired
    }

    /// The teach prompt to draw at `tick`, faded toward the end of its window, or `None` when no beat
    /// is live (or the window has elapsed). The host hands the result to the embodied-safe
    /// [`Renderer::render_prompt`](gonedark_render::Renderer::render_prompt). Pure read — never mutates.
    pub fn current_prompt(&self, tick: u64) -> Option<Prompt> {
        let (beat, raised) = self.active?;
        let elapsed = tick.saturating_sub(raised);
        let alpha = fade_alpha(elapsed);
        if alpha <= 0.0 {
            return None;
        }
        Some(prompt_for(beat, alpha))
    }
}

/// The fade `alpha` for a prompt raised `elapsed` ticks ago: full opacity until the trailing
/// [`FADE_TICKS`] of the [`PROMPT_TICKS`] window, then a linear ramp to 0. Pure → unit-tested.
fn fade_alpha(elapsed: u64) -> f32 {
    if elapsed >= PROMPT_TICKS {
        return 0.0;
    }
    let remaining = PROMPT_TICKS - elapsed;
    if remaining >= FADE_TICKS {
        1.0
    } else {
        remaining as f32 / FADE_TICKS as f32
    }
}

/// The static teaching copy for a beat, at fade `alpha`. Short, punchy, design-voice — and carrying
/// no world state (invariant #6: it telegraphs cost + controls, never the enemy). Pure → unit-tested.
fn prompt_for(beat: TeachBeat, alpha: f32) -> Prompt {
    match beat {
        TeachBeat::WentDark => Prompt {
            title: "GOING DARK".into(),
            body: vec![
                "The strategic map is blind — you see only what your soldier sees.".into(),
                "SURFACE to take command again.".into(),
            ],
            tone: PromptTone::Caution,
            alpha,
        },
        TeachBeat::Lingering => Prompt {
            title: "STILL DARK".into(),
            body: vec![
                "Your squad is fighting without you.".into(),
                "Surface soon — or pick your moment to fall back.".into(),
            ],
            tone: PromptTone::Danger,
            alpha,
        },
        TeachBeat::StayedTooLong => Prompt {
            title: "YOU STAYED TOO LONG".into(),
            body: vec![
                "The map went dark and you didn't come back in time.".into(),
                "Pick another unit — and watch the clock.".into(),
            ],
            tone: PromptTone::Reflect,
            alpha,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Embodied at `tick`, no death.
    fn embodied(tick: u64) -> TeachInput {
        TeachInput { embodied: true, avatar_died: false, tick }
    }
    /// In command view at `tick`, no death.
    fn command(tick: u64) -> TeachInput {
        TeachInput { embodied: false, avatar_died: false, tick }
    }
    /// The avatar died this tick (auto-surface → command view).
    fn died(tick: u64) -> TeachInput {
        TeachInput { embodied: false, avatar_died: true, tick }
    }

    // --- gating ---------------------------------------------------------------------------------

    #[test]
    fn disabled_machine_never_fires_or_draws() {
        let mut o = Onboarding::disabled();
        assert!(!o.is_enabled());
        assert_eq!(o.observe(embodied(1)), None);
        assert_eq!(o.observe(embodied(1000)), None);
        assert_eq!(o.observe(died(1001)), None);
        assert_eq!(o.current_prompt(1001), None, "disabled draws nothing");
    }

    // --- the went-dark beat ---------------------------------------------------------------------

    #[test]
    fn first_embody_fires_went_dark_once_and_telegraphs_the_cost() {
        let mut o = Onboarding::new(true);
        // A command-view tick before embody does nothing.
        assert_eq!(o.observe(command(0)), None);
        // The instant we go dark, the cost is telegraphed.
        assert_eq!(o.observe(embodied(1)), Some(TeachBeat::WentDark));
        // The prompt is live and tells you how to come back.
        let p = o.current_prompt(1).expect("a prompt is up");
        assert_eq!(p.tone, PromptTone::Caution);
        assert!(p.body.iter().any(|l| l.contains("SURFACE")), "teaches the control");
        // It does not re-fire on subsequent embodied ticks.
        assert_eq!(o.observe(embodied(2)), None);
    }

    #[test]
    fn went_dark_does_not_re_fire_after_a_surface_and_re_embody() {
        let mut o = Onboarding::new(true);
        assert_eq!(o.observe(embodied(1)), Some(TeachBeat::WentDark));
        assert_eq!(o.observe(command(2)), None); // voluntary surface
        // Re-embodying later must NOT replay the intro (fired once per match).
        assert_eq!(o.observe(embodied(3)), None);
    }

    // --- the lingering beat ---------------------------------------------------------------------

    #[test]
    fn lingering_fires_after_the_dwell_and_carries_no_intel() {
        let mut o = Onboarding::new(true);
        assert_eq!(o.observe(embodied(0)), Some(TeachBeat::WentDark));
        // Just short of the dwell: nothing.
        assert_eq!(o.observe(embodied(LINGER_TICKS - 1)), None);
        // At the dwell: the nudge fires.
        assert_eq!(o.observe(embodied(LINGER_TICKS)), Some(TeachBeat::Lingering));
        let p = o.current_prompt(LINGER_TICKS).expect("nudge is up");
        // Fairness (invariant #6): the copy is a time-cost, never a position. A crude intel guard:
        // none of the danger words a leak would use appear.
        let blob = format!("{} {}", p.title, p.body.join(" ")).to_lowercase();
        for banned in ["enemy at", "position", "coordinates", "spotted at", "north", "south"] {
            assert!(!blob.contains(banned), "no intel leak: {banned:?}");
        }
        // Fires once.
        assert_eq!(o.observe(embodied(LINGER_TICKS + 5)), None);
    }

    #[test]
    fn the_dwell_clock_resets_on_a_voluntary_surface() {
        let mut o = Onboarding::new(true);
        o.observe(embodied(0)); // WentDark
        // Dip in and out before the dwell elapses.
        o.observe(embodied(100));
        o.observe(command(110)); // surface — clock resets
        o.observe(embodied(120)); // re-embody — dwell restarts here
        // 120 + (LINGER_TICKS - 1) is short of the *new* dwell window → no nudge yet.
        assert_eq!(o.observe(embodied(120 + LINGER_TICKS - 1)), None);
        // One more tick clears the restarted window.
        assert_eq!(o.observe(embodied(120 + LINGER_TICKS)), Some(TeachBeat::Lingering));
    }

    // --- the death payoff -----------------------------------------------------------------------

    #[test]
    fn first_embodied_death_fires_the_framing_payoff() {
        let mut o = Onboarding::new(true);
        o.observe(embodied(10)); // WentDark
        // The avatar dies → auto-surface. The payoff frames it as the player's overstay.
        assert_eq!(o.observe(died(50)), Some(TeachBeat::StayedTooLong));
        let p = o.current_prompt(50).expect("payoff is up");
        assert_eq!(p.tone, PromptTone::Reflect);
        assert!(p.title.contains("STAYED TOO LONG"), "frames it as the player's choice");
        // It shows in the command view (you're ejected) and does not re-fire.
        assert_eq!(o.observe(died(200)), None);
    }

    #[test]
    fn a_manual_surface_is_not_a_death_payoff() {
        let mut o = Onboarding::new(true);
        o.observe(embodied(10)); // WentDark
        // A clean voluntary return is NOT framed as overstaying.
        assert_eq!(o.observe(command(40)), None);
        assert_eq!(o.current_prompt(40), o.current_prompt(40)); // (no panic) — WentDark may still fade
        // A real death later still earns the payoff.
        o.observe(embodied(60));
        assert_eq!(o.observe(died(80)), Some(TeachBeat::StayedTooLong));
    }

    // --- the fade / display window --------------------------------------------------------------

    #[test]
    fn fade_alpha_is_full_then_ramps_to_zero() {
        assert_eq!(fade_alpha(0), 1.0);
        assert_eq!(fade_alpha(PROMPT_TICKS - FADE_TICKS), 1.0, "full until the fade slice");
        let mid = fade_alpha(PROMPT_TICKS - FADE_TICKS / 2);
        assert!(mid > 0.0 && mid < 1.0, "ramping down");
        assert_eq!(fade_alpha(PROMPT_TICKS), 0.0, "gone at the window end");
        assert_eq!(fade_alpha(PROMPT_TICKS + 100), 0.0, "stays gone");
    }

    #[test]
    fn a_prompt_expires_after_its_window() {
        let mut o = Onboarding::new(true);
        o.observe(embodied(0)); // WentDark raised at tick 0
        assert!(o.current_prompt(0).is_some());
        assert!(o.current_prompt(PROMPT_TICKS - 1).is_some(), "still up near the end");
        assert!(o.current_prompt(PROMPT_TICKS).is_none(), "expired at the window end");
        // And `observe` drops it from `active` once elapsed (no stale prompt lingers).
        o.observe(embodied(PROMPT_TICKS + 1));
        assert!(o.current_prompt(PROMPT_TICKS + 1).is_none());
    }

    #[test]
    fn each_beat_maps_to_a_distinct_tone() {
        assert_ne!(prompt_for(TeachBeat::WentDark, 1.0).tone, prompt_for(TeachBeat::Lingering, 1.0).tone);
        assert_ne!(
            prompt_for(TeachBeat::Lingering, 1.0).tone,
            prompt_for(TeachBeat::StayedTooLong, 1.0).tone
        );
        // Alpha threads straight through.
        assert_eq!(prompt_for(TeachBeat::WentDark, 0.4).alpha, 0.4);
    }
}
