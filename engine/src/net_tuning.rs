//! Host-side RTT estimator + input-delay hysteresis (Phase 3, workstream B).
//!
//! Measures network round-trip time and decides **when** to ask `core::lockstep` to change the
//! session input delay. The estimator — and all of its float / EWMA math — lives HERE in `engine`
//! (host glue), **never** in `core`/sim:
//!
//!  - **Invariant #1** forbids floats only in the *simulation*. RTT is a wall-clock measurement, so
//!    smoothing it with an EWMA in `f64` is exactly the kind of host-side math that belongs outside
//!    `core`. The ONLY thing this module hands to `core` is an **integer tick `delay`** (and an
//!    integer `guard`) the host passes to [`Lockstep::propose_delay`] — `core` reads no clock and
//!    sees no float.
//!  - **Invariant #2** keeps `core` platform-free. This estimator could equally live in
//!    `pal-desktop`, but the lockstep `propose_delay` call it drives is in `engine`'s `Game::frame`
//!    drive path, and `engine` may **not** depend on `pal-desktop` (the layering is
//!    `engine -> {core, render, pal}`). So the pure estimator lives next to the code that calls it.
//!    `engine` already uses host-side floats (`world_to_fixed`, the camera math), so this is the
//!    same allowance.
//!
//! **Testable seam.** The decision is a pure function ([`decide_delay`]) plus a pure projection
//! ([`target_delay_ticks`]) with **no timing, no IO, and no clock** inside — exactly the pattern
//! `engine`'s `map_input_commands` / camera math and `render::interpolate_instances` use to keep
//! logic unit-testable behind un-constructible platform glue. [`RttDelayEstimator`] wraps those
//! pure pieces with the EWMA accumulator and the last-change bookkeeping; the caller supplies the
//! current lockstep frontier tick, so even the dwell check stays clock-free.
//!
//! **RTT sample source (see [`RttDelayEstimator::observe_rtt`]).** A real RTT needs a transport-level
//! ping/pong — deliberately NOT a new `core::lockstep` wire frame (that would touch the protocol; RTT
//! is a host/transport wall-clock concern). The sample input is a clean host seam: the host feeds
//! measured RTTs in via `observe_rtt`, and the estimator → `propose_delay` path is complete and
//! tested independent of where the number comes from. The production source is
//! `pal_desktop::PingPongTransport` (a `pal-desktop` concern, not `core`): it multiplexes ping/pong
//! over the lockstep transport and the host drains its `RttSamples` into `observe_rtt`. On a session
//! with no transport (single-player) it is never fed, so the estimator stays inert (no samples → no
//! proposals) and never fabricates a delay change.

use gonedark_core::sim::TICK_HZ;

/// Tuning for the RTT → input-delay policy. All thresholds are host-side presentation/netcode
/// policy (never sim state): they decide *when* to propose an integer delay, never *what the sim
/// computes*. Defaults target the locked 60 Hz tick ([`TICK_HZ`]).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct DelayPolicy {
    /// EWMA smoothing factor in `(0, 1]`: `smoothed = alpha*sample + (1-alpha)*smoothed`. Lower =
    /// smoother / slower to react (rides out jitter spikes); higher = more responsive.
    pub ewma_alpha: f64,
    /// The sim tick rate the delay is denominated in (ticks per second).
    pub tick_hz: u32,
    /// Extra ticks added on top of the modelled one-way latency — jitter headroom so a burst of
    /// late packets doesn't immediately stall the gate.
    pub safety_margin_ticks: u64,
    /// Dead-band: the target must differ from the current delay by **strictly more** than this many
    /// ticks before a change is proposed. Absorbs sub-tick jitter so the delay doesn't thrash.
    pub dead_band_ticks: u64,
    /// Minimum ticks between two applied changes (anti-thrash dwell). A change is suppressed until
    /// at least this many ticks have elapsed since the last one committed.
    pub min_dwell_ticks: u64,
    /// Floor for the proposed delay (ticks). Never propose below this.
    pub min_delay: u64,
    /// Ceiling for the proposed delay (ticks). A pathological RTT can't push delay unbounded.
    pub max_delay: u64,
}

impl Default for DelayPolicy {
    fn default() -> Self {
        DelayPolicy {
            // Smooth but still responsive: ~5-sample memory. Rides out single-packet jitter.
            ewma_alpha: 0.2,
            tick_hz: TICK_HZ,
            // ~33 ms of jitter headroom at 60 Hz.
            safety_margin_ticks: 2,
            // Ignore a one-tick wobble; only react to a real >=2-tick move.
            dead_band_ticks: 1,
            // 2 seconds at 60 Hz — a delay change is disruptive, so don't do it often.
            min_dwell_ticks: 2 * TICK_HZ as u64,
            // Always keep at least one tick of delay on a networked session.
            min_delay: 1,
            // ~200 ms one-way cap; beyond this the link is unplayable regardless of delay.
            max_delay: 12,
        }
    }
}

/// Project a smoothed RTT (seconds) to the raw target input delay in **ticks**, before hysteresis.
///
/// Input delay must cover the **one-way** trip (≈ `RTT / 2`) so a peer receives this client's
/// input before the tick that executes it, plus [`safety_margin_ticks`](DelayPolicy) of jitter
/// headroom. The result is rounded **up** (never under-cover the latency) and clamped to
/// `[min_delay, max_delay]`. Pure: no clock, no IO. A non-finite or negative sample is treated as
/// zero latency (clamps to `min_delay`) rather than panicking.
pub fn target_delay_ticks(smoothed_rtt_secs: f64, cfg: &DelayPolicy) -> u64 {
    let one_way = if smoothed_rtt_secs.is_finite() && smoothed_rtt_secs > 0.0 {
        smoothed_rtt_secs / 2.0
    } else {
        0.0
    };
    // ceil(one_way * tick_hz): never under-cover the measured latency.
    let raw_ticks = (one_way * cfg.tick_hz as f64).ceil();
    // raw_ticks is finite and >= 0 here; cap before the cast so it can't overflow u64.
    let raw = raw_ticks.min(cfg.max_delay as f64) as u64;
    raw.saturating_add(cfg.safety_margin_ticks)
        .clamp(cfg.min_delay, cfg.max_delay)
}

/// The hysteresis gate. Given a smoothed RTT, the session's current delay, and how many ticks have
/// elapsed since the last applied change, decide whether to propose a new delay.
///
/// Returns `Some(new_delay)` **only** when both:
///  1. the dwell has elapsed (`ticks_since_last_change >= min_dwell_ticks`), and
///  2. the projected target is **outside** the dead-band
///     (`|target - current| > dead_band_ticks`).
///
/// Otherwise `None` — jitter inside the band, or too soon since the last change. Pure: no clock, no
/// IO. This is THE seam the tests drive directly (no `Game`, no transport, no timing).
pub fn decide_delay(
    smoothed_rtt_secs: f64,
    current_delay: u64,
    ticks_since_last_change: u64,
    cfg: &DelayPolicy,
) -> Option<u64> {
    if ticks_since_last_change < cfg.min_dwell_ticks {
        return None; // anti-thrash dwell not yet satisfied
    }
    let target = target_delay_ticks(smoothed_rtt_secs, cfg);
    if target.abs_diff(current_delay) <= cfg.dead_band_ticks {
        return None; // inside the dead-band: treat as jitter, hold steady
    }
    Some(target)
}

/// Host-side RTT estimator driving [`Lockstep::propose_delay`]. Folds raw RTT samples into an EWMA
/// ([`observe_rtt`](Self::observe_rtt)) and, when polled, applies the [`decide_delay`] hysteresis
/// to yield an integer delay target. All float state is confined here; the value handed to `core`
/// is always an integer.
///
/// [`Lockstep::propose_delay`]: gonedark_core::lockstep::Lockstep::propose_delay
#[derive(Clone, Debug)]
pub struct RttDelayEstimator {
    cfg: DelayPolicy,
    /// Smoothed RTT in seconds, or `None` until the first sample arrives. `None` ⇒ inert (no
    /// proposal) so the estimator never invents a delay change before it has measured anything.
    smoothed_rtt_secs: Option<f64>,
    /// The frontier tick at which the last change was proposed, or `None` if none has been. `None`
    /// lets the FIRST change bypass the dwell so the session adapts to its measured RTT promptly.
    last_change_tick: Option<u64>,
}

impl RttDelayEstimator {
    /// A fresh estimator with the given policy (use [`DelayPolicy::default`] for the standard one).
    pub fn new(cfg: DelayPolicy) -> Self {
        RttDelayEstimator {
            cfg,
            smoothed_rtt_secs: None,
            last_change_tick: None,
        }
    }

    /// Fold one measured RTT sample (seconds) into the EWMA. The first sample seeds the average
    /// directly; later samples blend with [`ewma_alpha`](DelayPolicy). Non-finite / negative
    /// samples are ignored (a bad clock read must not poison the estimate). Pure: no IO.
    ///
    /// **Sample source (stubbed):** the production caller measures RTT with a transport-level
    /// ping/pong and feeds it here. That requires no change to the `core::lockstep` wire protocol —
    /// it is a host/transport concern. Until such a source is wired, this is simply never called,
    /// leaving the estimator inert (see the module docs).
    pub fn observe_rtt(&mut self, rtt_secs: f64) {
        if !rtt_secs.is_finite() || rtt_secs < 0.0 {
            return;
        }
        self.smoothed_rtt_secs = Some(match self.smoothed_rtt_secs {
            None => rtt_secs,
            Some(prev) => self.cfg.ewma_alpha * rtt_secs + (1.0 - self.cfg.ewma_alpha) * prev,
        });
    }

    /// The current smoothed RTT estimate (seconds), or `None` if no sample has been observed.
    pub fn smoothed_rtt_secs(&self) -> Option<f64> {
        self.smoothed_rtt_secs
    }

    /// Poll for a delay decision at the lockstep frontier tick `now_tick`, given the session's
    /// `current_delay`. Returns `Some(new_delay)` when the hysteresis gate fires (and records
    /// `now_tick` as the last change so the dwell starts counting); `None` otherwise. Returns
    /// `None` while no RTT has been observed. Clock-free: the caller supplies `now_tick` (the
    /// lockstep frontier), so dwell is measured in sim ticks, never wall-clock.
    pub fn poll_decision(&mut self, current_delay: u64, now_tick: u64) -> Option<u64> {
        let smoothed = self.smoothed_rtt_secs?;
        // Dwell measured from the last change; the first-ever change is unconstrained so the
        // session can adapt to its measured RTT without waiting out a dwell it never started.
        let since = match self.last_change_tick {
            Some(t) => now_tick.saturating_sub(t),
            None => self.cfg.min_dwell_ticks,
        };
        let decision = decide_delay(smoothed, current_delay, since, &self.cfg)?;
        self.last_change_tick = Some(now_tick);
        Some(decision)
    }

    /// The `guard` (lead in ticks) to pass alongside the proposed delay to
    /// [`Lockstep::propose_delay`]. Generous enough to cover the worst-case ONE-WAY latency so
    /// every peer receives the `DelayChange` frame before its effective tick: the full modelled RTT
    /// in ticks plus the jitter margin. `propose_delay` clamps the lead to at least
    /// `current_delay + 1` regardless, so this is a floor, not an exact figure.
    ///
    /// [`Lockstep::propose_delay`]: gonedark_core::lockstep::Lockstep::propose_delay
    pub fn guard_ticks(&self) -> u64 {
        let rtt = self.smoothed_rtt_secs.unwrap_or(0.0);
        let rtt = if rtt.is_finite() && rtt > 0.0 { rtt } else { 0.0 };
        let full_rtt_ticks = (rtt * self.cfg.tick_hz as f64).ceil();
        let ticks = full_rtt_ticks.min((2 * self.cfg.max_delay) as f64) as u64;
        ticks.saturating_add(self.cfg.safety_margin_ticks).max(1)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gonedark_core::lockstep::Lockstep;
    use gonedark_core::sim::Command;

    // ----- pure projection: RTT → target ticks -----

    #[test]
    fn target_covers_one_way_latency_rounded_up_plus_margin() {
        let cfg = DelayPolicy::default(); // tick_hz 60, margin 2, min 1, max 12
                                          // 100 ms RTT → 50 ms one-way → 0.05*60 = 3 ticks, +2 margin = 5.
        assert_eq!(target_delay_ticks(0.100, &cfg), 5);
        // A latency that lands mid-tick rounds UP (never under-cover): 0.07/2*60 = 2.1 → 3, +2 = 5.
        assert_eq!(target_delay_ticks(0.070, &cfg), 5);
    }

    #[test]
    fn target_clamps_to_min_and_max() {
        let cfg = DelayPolicy::default();
        // Exactly-zero RTT contributes no latency ticks, so the target is just the safety margin
        // (>= min_delay, never below the floor and never 0).
        assert_eq!(target_delay_ticks(0.0, &cfg), cfg.safety_margin_ticks);
        assert!(target_delay_ticks(0.0, &cfg) >= cfg.min_delay);
        // Any non-zero RTT rounds UP to at least one latency tick (never under-cover), plus margin.
        assert_eq!(target_delay_ticks(0.0001, &cfg), 1 + cfg.safety_margin_ticks);
        // A huge RTT clamps to the ceiling, never unbounded.
        assert_eq!(target_delay_ticks(10.0, &cfg), cfg.max_delay);
    }

    #[test]
    fn target_treats_garbage_samples_as_zero_latency() {
        // A non-finite / negative sample contributes no latency: the target collapses to the same
        // zero-latency value (the safety margin), never a panic or a wild number.
        let cfg = DelayPolicy::default();
        let zero = target_delay_ticks(0.0, &cfg);
        assert_eq!(target_delay_ticks(f64::NAN, &cfg), zero);
        assert_eq!(target_delay_ticks(f64::INFINITY, &cfg), zero);
        assert_eq!(target_delay_ticks(-1.0, &cfg), zero);
    }

    // ----- pure hysteresis gate -----

    #[test]
    fn rising_rtt_raises_the_delay() {
        let cfg = DelayPolicy::default();
        // current delay 2; 100 ms RTT projects to target 5. Outside the dead-band, dwell elapsed.
        let d = decide_delay(0.100, 2, cfg.min_dwell_ticks, &cfg);
        assert_eq!(d, Some(5), "rising RTT must raise the delay");
        assert!(d.unwrap() > 2);
    }

    #[test]
    fn falling_rtt_lowers_the_delay() {
        let cfg = DelayPolicy::default();
        // current delay 8; 20 ms RTT → 10 ms one-way → 0.6 → ceil 1, +2 = 3.
        let d = decide_delay(0.020, 8, cfg.min_dwell_ticks, &cfg);
        assert_eq!(d, Some(3), "falling RTT must lower the delay");
        assert!(d.unwrap() < 8);
    }

    #[test]
    fn small_jitter_inside_the_dead_band_makes_no_change() {
        let cfg = DelayPolicy::default(); // dead_band 1
                                          // current delay 5; 100 ms RTT projects to target 5 → diff 0, inside band.
        assert_eq!(decide_delay(0.100, 5, cfg.min_dwell_ticks, &cfg), None);
        // A one-tick wobble (target 6 vs current 5) is still inside the band (diff == dead_band).
        // 0.1233/2*60 = 3.699 → ceil 4, +2 = 6.
        assert_eq!(decide_delay(0.1233, 5, cfg.min_dwell_ticks, &cfg), None);
        // But a two-tick move (target 8) breaks out: 0.167/2 = 0.0835 *60 = 5.01 → ceil 6, +2 = 8.
        // diff |8-5| = 3 > 1 → change.
        assert_eq!(decide_delay(0.167, 5, cfg.min_dwell_ticks, &cfg), Some(8));
    }

    #[test]
    fn minimum_dwell_between_changes_is_respected() {
        let cfg = DelayPolicy::default();
        // A target far outside the band, but the dwell has NOT elapsed → suppressed.
        assert_eq!(decide_delay(0.200, 2, cfg.min_dwell_ticks - 1, &cfg), None);
        assert_eq!(decide_delay(0.200, 2, 0, &cfg), None);
        // Exactly at the dwell threshold it is allowed.
        assert!(decide_delay(0.200, 2, cfg.min_dwell_ticks, &cfg).is_some());
    }

    // ----- EWMA accumulator -----

    #[test]
    fn ewma_seeds_on_first_sample_then_blends() {
        let mut est = RttDelayEstimator::new(DelayPolicy::default());
        assert_eq!(est.smoothed_rtt_secs(), None, "inert until first sample");
        est.observe_rtt(0.100);
        assert_eq!(
            est.smoothed_rtt_secs(),
            Some(0.100),
            "first sample seeds directly"
        );
        // alpha 0.2: next = 0.2*0.200 + 0.8*0.100 = 0.12.
        est.observe_rtt(0.200);
        let s = est.smoothed_rtt_secs().unwrap();
        assert!(
            (s - 0.12).abs() < 1e-9,
            "blended toward the new sample, got {s}"
        );
    }

    #[test]
    fn ewma_rides_out_a_single_outlier() {
        let mut est = RttDelayEstimator::new(DelayPolicy::default());
        for _ in 0..20 {
            est.observe_rtt(0.050); // steady 50 ms
        }
        let steady = est.smoothed_rtt_secs().unwrap();
        assert!((steady - 0.050).abs() < 1e-3);
        // One huge spike must not swing the smoothed value all the way to it.
        est.observe_rtt(1.000);
        let after = est.smoothed_rtt_secs().unwrap();
        assert!(
            after < 0.300,
            "a single outlier must not dominate the EWMA, got {after}"
        );
    }

    #[test]
    fn observe_ignores_garbage_samples() {
        let mut est = RttDelayEstimator::new(DelayPolicy::default());
        est.observe_rtt(0.080);
        let before = est.smoothed_rtt_secs();
        est.observe_rtt(f64::NAN);
        est.observe_rtt(-5.0);
        est.observe_rtt(f64::INFINITY);
        assert_eq!(
            est.smoothed_rtt_secs(),
            before,
            "a bad clock read must not move the estimate"
        );
    }

    // ----- estimator dwell bookkeeping -----

    #[test]
    fn estimator_is_inert_without_samples() {
        let mut est = RttDelayEstimator::new(DelayPolicy::default());
        assert_eq!(est.poll_decision(0, 10_000), None, "no sample → no proposal");
    }

    #[test]
    fn first_change_bypasses_dwell_then_subsequent_changes_wait() {
        let cfg = DelayPolicy::default();
        let mut est = RttDelayEstimator::new(cfg);
        est.observe_rtt(0.100); // → target 5
                                // Frontier tick 100 (< min_dwell from 0), but the first change is unconstrained.
        assert_eq!(
            est.poll_decision(2, 100),
            Some(5),
            "first change adapts immediately"
        );
        // Immediately after, a further change is blocked until the dwell elapses, even with a
        // wildly different RTT.
        est.observe_rtt(0.000); // pulls the EWMA down toward a lower target over time
        assert_eq!(
            est.poll_decision(5, 100 + cfg.min_dwell_ticks - 1),
            None,
            "second change waits out the dwell"
        );
        // Drive the EWMA down to a clearly-lower target, then cross the dwell boundary.
        for _ in 0..30 {
            est.observe_rtt(0.000);
        }
        let after = est.poll_decision(5, 100 + cfg.min_dwell_ticks);
        assert!(
            matches!(after, Some(d) if d < 5),
            "after the dwell a lower RTT lowers the delay, got {after:?}"
        );
    }

    #[test]
    fn guard_covers_full_rtt_plus_margin_and_is_at_least_one() {
        let cfg = DelayPolicy::default();
        let mut est = RttDelayEstimator::new(cfg);
        // No sample → no modelled latency, so the guard is just the safety margin (and at least 1).
        assert_eq!(est.guard_ticks(), cfg.safety_margin_ticks);
        assert!(est.guard_ticks() >= 1);
        est.observe_rtt(0.100); // 100 ms → 6 ticks full RTT, +2 margin = 8.
        assert_eq!(est.guard_ticks(), 8);
    }

    // ----- end-to-end: estimator decision actually lands on a real Lockstep -----

    /// Step both peers in lockstep up to `until_tick`, exchanging empty command sets (and any
    /// delay-change frames) so the gate clears each tick — mirrors `drive_lockstep`'s pump without a
    /// transport object.
    fn drive_two_peer_to(ls: &mut Lockstep, peer: &mut Lockstep, until_tick: u64) {
        while ls.next_tick() < until_tick {
            if ls.submit_tick() <= ls.next_tick() {
                ls.submit(Vec::<Command>::new());
            }
            if peer.submit_tick() <= peer.next_tick() {
                peer.submit(Vec::<Command>::new());
            }
            for f in ls.drain_outbound() {
                let _ = peer.deliver(&f);
            }
            for f in peer.drain_outbound() {
                let _ = ls.deliver(&f);
            }
            while ls.try_advance().is_some() {}
            while peer.try_advance().is_some() {}
        }
    }

    /// Drive the FULL host path — estimator → `propose_delay` → `try_advance` commit — against a
    /// real two-peer `Lockstep`, proving the decision the pure seam returns becomes a committed
    /// integer delay change on the protocol (no `Game`, no transport, no clock). This is the
    /// integration the wiring in `Game::frame` performs.
    #[test]
    fn estimator_decision_commits_as_a_real_delay_change() {
        let cfg = DelayPolicy::default();
        let initial_delay = 2;
        let mut ls = Lockstep::new(2, 0, initial_delay);
        let mut peer = Lockstep::new(2, 1, initial_delay);
        let mut est = RttDelayEstimator::new(cfg);

        // Warm the session a little so the frontier is past warmup.
        drive_two_peer_to(&mut ls, &mut peer, 5);

        // A rising RTT: 100 ms → target delay 5 (> current 2, outside the band).
        est.observe_rtt(0.100);
        let now = ls.submit_tick().max(ls.next_tick());
        let target = est
            .poll_decision(ls.delay(), now)
            .expect("rising RTT must yield a decision");
        assert_eq!(target, 5);

        let guard = est.guard_ticks();
        let effective = ls
            .propose_delay(target, guard)
            .expect("first proposal is never AlreadyPending");
        assert!(
            effective > ls.next_tick(),
            "the change takes effect strictly in the future"
        );
        assert_eq!(
            ls.delay(),
            initial_delay,
            "delay unchanged until the effective tick"
        );

        // Drive both peers past the effective tick; the agreed change must commit identically on
        // BOTH ends (the proposer applies its own pending change; the peer applies the delivered
        // DelayChange frame).
        drive_two_peer_to(&mut ls, &mut peer, effective + 2);
        assert_eq!(ls.delay(), target, "proposer committed the new delay");
        assert_eq!(peer.delay(), target, "peer committed the identical new delay");
        assert_eq!(ls.pending_delay(), None, "no change left pending after commit");
    }
}
