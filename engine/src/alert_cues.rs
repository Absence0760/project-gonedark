//! Cross-modal alert cues (WS-D accessibility, invariant #6) — the NON-visual equivalents of the
//! embodied directional **flash**.
//!
//! While the map is dark, the going-dark alert is a directional edge-ring **flash** (`render::hud`,
//! bearing only) plus **positioned audio** (`audio::mix_cues`). A player who cannot read the colour
//! flash — colour-blind, low-vision — needs the *same* directional alert in another modality, or the
//! core mechanic is unfair to them (invariant #6). This module is the pure seam that turns the live
//! [`AlertChannel`] into those equivalents:
//!
//! - a **directional audio ping** — one positioned [`AudioCue`] per fresh alert, panned by bearing,
//!   at a *constant* gain (bearing only — never distance-attenuated), never muffled, so it cuts
//!   through the muffled off-map strategic bleed the way the flash does; and
//! - a **directional haptic pulse** — a coarse [`HapticPulse`] (kind + a left/center/right side) for
//!   backends with a vibration motor.
//!
//! ## Fairness (invariant #6 — "alerts, not intel")
//!
//! Both channels reveal *exactly* what the flash reveals: a **bearing** and the alert **kind**, and
//! no more. The audio ping's gain is a constant ([`ALERT_PING_GAIN`]) so loudness can never leak
//! range, and the haptic side is deliberately low-resolution (a motor can't localize, and fore/aft
//! both collapse to `Center`) — mirroring the edge-ring flash, which shows direction but not
//! distance. This is the behavioural line between these accessibility cues and the primary,
//! distance-attenuated ambient mix (`audio::mix_cues`).
//!
//! PRESENTATION only: everything here is derived from the already-checksummed alert channel + the
//! listener pose. It reads no sim state, mutates nothing, and never enters `core` — floats are fine
//! (presentation, not the sim) and nothing is checksummed (invariants #1/#4/#7). Free functions, so
//! the selection + direction math is unit-testable without a GPU or an audio/haptic device.

use std::f32::consts::PI;

use gonedark_core::alerts::{AlertChannel, AlertKind};
use gonedark_pal::{AudioCue, SoundId};

/// Which non-visual equivalent(s) of the directional alert **flash** the player has turned on
/// (Settings → Accessibility). Stored by stable ordinal ([`Self::index`]/[`Self::from_index`]),
/// mirroring the shell's `QualityChoice`/`PaletteMode`. Presentation only — never a sim input.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum AlertCueMode {
    /// No cross-modal cue — the base flash + positioned audio only (the shipped default).
    #[default]
    Off,
    /// A bearing-panned audio ping per fresh alert (for players who cannot read the colour flash).
    Audio,
    /// A directional haptic pulse per fresh alert (for backends with a vibration motor).
    Haptic,
    /// Both the audio ping and the haptic pulse.
    AudioHaptic,
}

impl AlertCueMode {
    /// Every mode, in the stable cycle + persisted-ordinal order.
    pub const ALL: [AlertCueMode; 4] = [
        AlertCueMode::Off,
        AlertCueMode::Audio,
        AlertCueMode::Haptic,
        AlertCueMode::AudioHaptic,
    ];

    /// The on-screen label for the Settings cycler.
    pub fn label(self) -> &'static str {
        match self {
            AlertCueMode::Off => "Off",
            AlertCueMode::Audio => "Audio ping",
            AlertCueMode::Haptic => "Haptic",
            AlertCueMode::AudioHaptic => "Audio + haptic",
        }
    }

    /// The next mode, wrapping — what the Settings cycler advances to.
    pub fn next(self) -> AlertCueMode {
        let i = self.index();
        Self::ALL[(i + 1) % Self::ALL.len()]
    }

    /// This mode's stable index in [`Self::ALL`] — the persisted ordinal.
    pub fn index(self) -> usize {
        Self::ALL.iter().position(|&m| m == self).unwrap_or(0)
    }

    /// The mode at persisted index `i`, or [`AlertCueMode::Off`] for an out-of-range ordinal — the
    /// tolerant decode side of [`Self::index`].
    pub fn from_index(i: usize) -> AlertCueMode {
        Self::ALL.get(i).copied().unwrap_or(AlertCueMode::Off)
    }

    /// Whether the directional **audio ping** channel is active.
    #[inline]
    pub fn audio(self) -> bool {
        matches!(self, AlertCueMode::Audio | AlertCueMode::AudioHaptic)
    }

    /// Whether the directional **haptic pulse** channel is active.
    #[inline]
    pub fn haptic(self) -> bool {
        matches!(self, AlertCueMode::Haptic | AlertCueMode::AudioHaptic)
    }
}

/// A coarse haptic direction. A vibration motor can't localize precisely, so bearing collapses to a
/// side — this is DELIBERATELY low-resolution (an alert, not intel — the same fairness bound as the
/// edge-ring flash, which shows bearing but not distance). Fore and aft both read as [`Center`].
///
/// [`Center`]: HapticSide::Center
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum HapticSide {
    /// The alert bears to the player's left.
    Left,
    /// Dead ahead or behind (a motor can't tell fore from aft).
    Center,
    /// The alert bears to the player's right.
    Right,
}

/// One directional haptic pulse mirroring a fresh alert flash, for players who rely on neither the
/// visual nor the audio channel. PRESENTATION only — derived from the already-checksummed alert
/// channel, never sim state, never checksummed (invariants #1/#6/#7). The desktop PAL has no
/// vibration sink today; this pure descriptor + the selection seam are the deliverable, and the
/// Android vibrator wiring is a later `pal-android` task (the same "desktop drives it, Android parity
/// deferred" split the colourblind palette took).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct HapticPulse {
    /// Which of the four alert kinds fired (a backend may vary the buzz pattern by kind).
    pub kind: AlertKind,
    /// The coarse bearing side.
    pub side: HapticSide,
    /// The sim tick the alert was raised on.
    pub tick: u64,
}

/// Constant gain for an alert ping: BEARING-ONLY, never distance-attenuated, so the cue reveals the
/// same thing the edge-ring flash does (a direction) and no more — loudness must not leak range
/// (invariant #6 "alerts, not intel"). Near full so the ping cuts through the muffled off-map bleed.
const ALERT_PING_GAIN: f32 = 0.9;

/// Half-width (radians) of the fore-aft arc that reads as [`HapticSide::Center`] — outside it the
/// pulse biases left/right. ~30° each side of the fore-aft axis.
const HAPTIC_CENTER_ARC: f32 = PI / 6.0;

/// The sound identity for each alert kind's ping — reusing the shared [`SoundId`] vocabulary (no new
/// backend samples). Each kind gets a distinct identity so the four alerts stay separable by ear.
#[inline]
fn alert_ping_sound(kind: AlertKind) -> SoundId {
    match kind {
        AlertKind::TakingFire => SoundId::Gunfire,
        AlertKind::UnitLost => SoundId::UnitDown,
        AlertKind::BaseUnderAttack => SoundId::BaseHit,
        AlertKind::TerritoryLost => SoundId::Capture,
    }
}

/// Bearing (radians, relative to `yaw`, normalized to `(-PI, PI]`; `0` = dead ahead, positive =
/// right) from the listener at `avatar_world` to `pos_world`. The SAME frame convention as
/// [`crate::audio::mix_cues`] and the alert HUD, so every cross-modal cue agrees on "to the right."
/// Pure.
#[inline]
fn bearing(pos_world: (f32, f32), avatar_world: (f32, f32), yaw: f32) -> f32 {
    let dx = pos_world.0 - avatar_world.0;
    let dy = pos_world.1 - avatar_world.1;
    normalize_angle(yaw - dy.atan2(dx))
}

/// Wrap an angle (radians) into `(-PI, PI]` — the [`crate::audio`] convention.
#[inline]
fn normalize_angle(mut a: f32) -> f32 {
    while a > PI {
        a -= 2.0 * PI;
    }
    while a <= -PI {
        a += 2.0 * PI;
    }
    a
}

/// Collapse a bearing to a coarse [`HapticSide`]. Dead-ahead and behind both read as `Center` (a
/// single motor can't disambiguate fore/aft); otherwise the sign picks the side (positive = right).
#[inline]
fn haptic_side(azimuth: f32) -> HapticSide {
    let a = azimuth.abs();
    if a <= HAPTIC_CENTER_ARC || a >= PI - HAPTIC_CENTER_ARC {
        HapticSide::Center
    } else if azimuth > 0.0 {
        HapticSide::Right
    } else {
        HapticSide::Left
    }
}

/// Directional **audio pings** for this frame's FRESH alerts (those raised on `tick`), one positioned
/// [`AudioCue`] each: a per-kind [`SoundId`], a bearing-panned azimuth, a CONSTANT gain (bearing only
/// — no range leak) and never muffled (an alert must cut through the muffled off-map bleed). Empty
/// unless `mode.audio()`. The host appends these to the frame mix.
///
/// PURE: reads the already-checksummed alert channel + the listener pose, mutates nothing, never a
/// sim read (invariants #1/#4/#6). Positions hop Q16.16 → f32 via the one sanctioned converter.
pub fn alert_audio_cues(
    alerts: &AlertChannel,
    avatar_world: (f32, f32),
    yaw: f32,
    tick: u64,
    mode: AlertCueMode,
) -> Vec<AudioCue> {
    if !mode.audio() {
        return Vec::new();
    }
    alerts
        .recent
        .iter()
        .filter(|a| a.tick == tick)
        .map(|a| {
            let pos = (
                gonedark_render::fixed_to_f32(a.pos.x),
                gonedark_render::fixed_to_f32(a.pos.y),
            );
            AudioCue {
                sound: alert_ping_sound(a.kind),
                azimuth: bearing(pos, avatar_world, yaw),
                gain: ALERT_PING_GAIN,
                muffled: false,
            }
        })
        .collect()
}

/// Directional **haptic pulses** for this frame's FRESH alerts (those raised on `tick`), one
/// [`HapticPulse`] each (kind + coarse side). Empty unless `mode.haptic()`. PURE — the same fairness
/// bound and derivation as [`alert_audio_cues`]; a backend with a vibration motor consumes the
/// result, desktop currently drops it.
pub fn alert_haptic_pulses(
    alerts: &AlertChannel,
    avatar_world: (f32, f32),
    yaw: f32,
    tick: u64,
    mode: AlertCueMode,
) -> Vec<HapticPulse> {
    if !mode.haptic() {
        return Vec::new();
    }
    alerts
        .recent
        .iter()
        .filter(|a| a.tick == tick)
        .map(|a| {
            let pos = (
                gonedark_render::fixed_to_f32(a.pos.x),
                gonedark_render::fixed_to_f32(a.pos.y),
            );
            HapticPulse {
                kind: a.kind,
                side: haptic_side(bearing(pos, avatar_world, yaw)),
                tick: a.tick,
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use gonedark_core::components::Vec2;
    use gonedark_core::fixed::Fixed;

    const EPS: f32 = 1e-4;

    fn pos(x: i32, y: i32) -> Vec2 {
        Vec2::new(Fixed::from_int(x), Fixed::from_int(y))
    }

    fn channel(alerts: &[(AlertKind, Vec2, u64)]) -> AlertChannel {
        let mut ch = AlertChannel::new();
        for &(kind, pos, tick) in alerts {
            ch.recent
                .push(gonedark_core::alerts::Alert { kind, pos, tick });
        }
        ch
    }

    // ---- AlertCueMode ordinal / cycle / predicates ---------------------------------------------

    #[test]
    fn mode_defaults_off() {
        assert_eq!(AlertCueMode::default(), AlertCueMode::Off);
    }

    #[test]
    fn mode_ordinal_round_trips_every_variant() {
        for &m in &AlertCueMode::ALL {
            assert_eq!(AlertCueMode::from_index(m.index()), m);
        }
        // An out-of-range ordinal falls back to Off (tolerant decode).
        assert_eq!(AlertCueMode::from_index(999), AlertCueMode::Off);
    }

    #[test]
    fn mode_cycle_visits_all_and_wraps() {
        let mut seen = Vec::new();
        let mut m = AlertCueMode::Off;
        for _ in 0..AlertCueMode::ALL.len() {
            seen.push(m);
            m = m.next();
        }
        assert_eq!(m, AlertCueMode::Off, "cycle wraps back to the start");
        for &v in &AlertCueMode::ALL {
            assert!(seen.contains(&v), "cycle visits {:?}", v);
        }
    }

    #[test]
    fn mode_predicates_gate_the_two_channels() {
        assert!(!AlertCueMode::Off.audio() && !AlertCueMode::Off.haptic());
        assert!(AlertCueMode::Audio.audio() && !AlertCueMode::Audio.haptic());
        assert!(!AlertCueMode::Haptic.audio() && AlertCueMode::Haptic.haptic());
        assert!(AlertCueMode::AudioHaptic.audio() && AlertCueMode::AudioHaptic.haptic());
    }

    // ---- gating -------------------------------------------------------------------------------

    #[test]
    fn off_mode_emits_nothing() {
        let ch = channel(&[(AlertKind::TakingFire, pos(10, 0), 5)]);
        assert!(alert_audio_cues(&ch, (0.0, 0.0), 0.0, 5, AlertCueMode::Off).is_empty());
        assert!(alert_haptic_pulses(&ch, (0.0, 0.0), 0.0, 5, AlertCueMode::Off).is_empty());
    }

    #[test]
    fn haptic_only_mode_emits_no_audio_and_vice_versa() {
        let ch = channel(&[(AlertKind::TakingFire, pos(10, 0), 5)]);
        assert!(alert_audio_cues(&ch, (0.0, 0.0), 0.0, 5, AlertCueMode::Haptic).is_empty());
        assert_eq!(
            alert_haptic_pulses(&ch, (0.0, 0.0), 0.0, 5, AlertCueMode::Haptic).len(),
            1
        );
        assert!(alert_haptic_pulses(&ch, (0.0, 0.0), 0.0, 5, AlertCueMode::Audio).is_empty());
        assert_eq!(
            alert_audio_cues(&ch, (0.0, 0.0), 0.0, 5, AlertCueMode::Audio).len(),
            1
        );
    }

    // ---- freshness: only alerts raised on `tick` are cued (no per-frame re-ping) ---------------

    #[test]
    fn only_fresh_alerts_are_cued() {
        // Two alerts: one raised this tick (7), one older (3). Only the fresh one produces a cue.
        let ch = channel(&[
            (AlertKind::TakingFire, pos(10, 0), 3),
            (AlertKind::UnitLost, pos(0, 10), 7),
        ]);
        let cues = alert_audio_cues(&ch, (0.0, 0.0), 0.0, 7, AlertCueMode::Audio);
        assert_eq!(cues.len(), 1, "only the tick-7 alert is fresh");
        assert_eq!(cues[0].sound, SoundId::UnitDown);
        let pulses = alert_haptic_pulses(&ch, (0.0, 0.0), 0.0, 7, AlertCueMode::Haptic);
        assert_eq!(pulses.len(), 1);
        assert_eq!(pulses[0].kind, AlertKind::UnitLost);
    }

    // ---- per-kind sound identity --------------------------------------------------------------

    #[test]
    fn each_alert_kind_maps_to_a_distinct_ping_sound() {
        let kinds = [
            AlertKind::TakingFire,
            AlertKind::UnitLost,
            AlertKind::BaseUnderAttack,
            AlertKind::TerritoryLost,
        ];
        let sounds: Vec<SoundId> = kinds.iter().map(|&k| alert_ping_sound(k)).collect();
        for i in 0..sounds.len() {
            for j in (i + 1)..sounds.len() {
                assert_ne!(
                    sounds[i], sounds[j],
                    "{:?} and {:?} must sound different",
                    kinds[i], kinds[j]
                );
            }
        }
    }

    // ---- fairness: bearing only, never distance (constant gain), never muffled -----------------

    #[test]
    fn ping_gain_is_constant_regardless_of_distance() {
        let near = channel(&[(AlertKind::TakingFire, pos(1, 0), 1)]);
        let far = channel(&[(AlertKind::TakingFire, pos(900, 0), 1)]);
        let n = alert_audio_cues(&near, (0.0, 0.0), 0.0, 1, AlertCueMode::Audio);
        let f = alert_audio_cues(&far, (0.0, 0.0), 0.0, 1, AlertCueMode::Audio);
        assert_eq!(
            n[0].gain, f[0].gain,
            "loudness must not leak range (invariant #6)"
        );
        assert_eq!(n[0].gain, ALERT_PING_GAIN);
    }

    #[test]
    fn ping_is_never_muffled_so_it_cuts_through_the_offmap_bleed() {
        // A strategic alert (TerritoryLost) — the ambient mix muffles the underlying Capture, but the
        // accessibility ping must stay clear so a colour-blind player can localize it.
        let ch = channel(&[(AlertKind::TerritoryLost, pos(0, 20), 2)]);
        let cues = alert_audio_cues(&ch, (0.0, 0.0), 0.0, 2, AlertCueMode::Audio);
        assert!(!cues[0].muffled);
    }

    // ---- bearing / haptic side sign convention (matches audio::mix_cues + the HUD) -------------

    #[test]
    fn alert_straight_ahead_is_azimuth_zero_and_center() {
        // Listener at origin facing +x (yaw 0); alert at (+x, 0) is dead ahead.
        let ch = channel(&[(AlertKind::TakingFire, pos(10, 0), 0)]);
        let cues = alert_audio_cues(&ch, (0.0, 0.0), 0.0, 0, AlertCueMode::Audio);
        assert!(cues[0].azimuth.abs() < EPS, "azimuth {}", cues[0].azimuth);
        let pulses = alert_haptic_pulses(&ch, (0.0, 0.0), 0.0, 0, AlertCueMode::Haptic);
        assert_eq!(pulses[0].side, HapticSide::Center);
    }

    #[test]
    fn alert_to_the_sides_has_consistent_signs() {
        // Facing +x with up +Z (right-handed), the player's right is -y: a source at (0,+y) is LEFT
        // (azimuth < 0) and one at (0,-y) is RIGHT (azimuth > 0) — the mix_cues convention.
        let left = channel(&[(AlertKind::TakingFire, pos(0, 10), 0)]);
        let right = channel(&[(AlertKind::TakingFire, pos(0, -10), 0)]);
        let lc = alert_audio_cues(&left, (0.0, 0.0), 0.0, 0, AlertCueMode::Audio);
        let rc = alert_audio_cues(&right, (0.0, 0.0), 0.0, 0, AlertCueMode::Audio);
        assert!(lc[0].azimuth < 0.0, "left azimuth {}", lc[0].azimuth);
        assert!(rc[0].azimuth > 0.0, "right azimuth {}", rc[0].azimuth);

        let lp = alert_haptic_pulses(&left, (0.0, 0.0), 0.0, 0, AlertCueMode::Haptic);
        let rp = alert_haptic_pulses(&right, (0.0, 0.0), 0.0, 0, AlertCueMode::Haptic);
        assert_eq!(lp[0].side, HapticSide::Left);
        assert_eq!(rp[0].side, HapticSide::Right);
    }

    #[test]
    fn alert_behind_reads_as_center_side() {
        // A source directly behind (yaw 0, at -x) → azimuth ~PI → Center (a motor can't tell fore/aft).
        let ch = channel(&[(AlertKind::UnitLost, pos(-10, 0), 0)]);
        let pulses = alert_haptic_pulses(&ch, (0.0, 0.0), 0.0, 0, AlertCueMode::Haptic);
        assert_eq!(pulses[0].side, HapticSide::Center);
    }

    #[test]
    fn haptic_side_thresholds_are_exact_at_the_center_arc() {
        // Just inside the fore arc → Center; just outside → a side.
        assert_eq!(haptic_side(HAPTIC_CENTER_ARC - 0.01), HapticSide::Center);
        assert_eq!(haptic_side(HAPTIC_CENTER_ARC + 0.01), HapticSide::Right);
        assert_eq!(haptic_side(-(HAPTIC_CENTER_ARC + 0.01)), HapticSide::Left);
        // Near the aft axis.
        assert_eq!(
            haptic_side(PI - HAPTIC_CENTER_ARC + 0.01),
            HapticSide::Center
        );
        assert_eq!(
            haptic_side(PI - HAPTIC_CENTER_ARC - 0.01),
            HapticSide::Right
        );
    }
}
