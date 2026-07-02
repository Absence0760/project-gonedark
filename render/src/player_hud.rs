//! Embodied **player vitals** HUD (H4) — the avatar's OWN health bar + ammo readout, drawn over the
//! dark first-person frame while the local player is embodied in an infantry unit.
//!
//! Until now the embodied view had no health or ammo readout at all — only the tank's reload ring —
//! so an infantry player fought blind to their own state. This module adds the two vitals every
//! shooter needs: a bottom-corner HP bar and a magazine count.
//!
//! ## Fairness (invariant #6) is structural
//!
//! Health and ammo are the **avatar's own state**, not intel: they are exactly what the soldier you
//! are controlling already knows about their own body and weapon. This surface carries NO world
//! position, no fog mask, no enemy/off-screen unit information — it is a screen-space NDC readout of
//! two scalars the player owns. So it is safe to draw over the dark embodied frame: it can leak no
//! map intel. (Contrast the command-view [`objective_hud`](crate::objective_hud) / glanceability
//! icons, which ARE strategic intel and are gated OUT of the dark frame.)
//!
//! ## The pure seam
//!
//! Like [`tank_hud`](crate::tank_hud) and [`objective_hud`](crate::objective_hud), all the geometry /
//! formatting math lives in pure free fns — [`health_fraction`], [`ammo_label`], [`player_hud_quads`],
//! [`player_hud_labels`] — so it is unit-testable without a GPU (exactly the `reload_ring_fill` /
//! `objective_hud_quads` pattern). The bar draws through the shared [`overlay`](crate::overlay) quad
//! pipeline and the count through the shared [`text`](crate::text) pass (no new pipeline/shader),
//! so the `lib.rs` wiring mirrors [`render_prompt`](crate::Renderer::render_prompt) exactly.

use crate::overlay::{OverlayQuad, QuadRole};
use crate::text::Anchor;

// --- layout constants (NDC, bottom-LEFT anchor — a conventionally-empty corner clear of the
//     screen-center reticle / scope overlay) ------------------------------------------------------
/// Left edge of the HP bar (a small margin in from the screen edge).
const LEFT: f32 = -0.94;
/// Bottom edge of the HP bar.
const BOTTOM: f32 = -0.90;
/// Full HP-bar width in NDC (the track); the fill spans `frac` of this.
const BAR_W: f32 = 0.42;
/// HP-bar half-height in NDC.
const BAR_HH: f32 = 0.020;
/// The rim quad extends this far past the track on each side to draw a thin border.
const RIM_PAD: f32 = 0.008;

/// HP fraction at/below which the bar reads as critical (red).
const LOW_HP: f32 = 0.30;
/// HP fraction at/below which the bar reads as wounded (amber); above it reads healthy (green).
const MID_HP: f32 = 0.60;

/// Glyph cell height (NDC) of the ammo count — small readable chrome.
const AMMO_SIZE: f32 = 0.050;
/// Gap between the top of the HP bar and the bottom of the ammo count above it.
const LABEL_GAP: f32 = 0.018;
/// Ammo count tint (the bone off-white the rest of the HUD reads in).
const AMMO_COLOR: [f32; 3] = crate::theme::BONE;
/// A warm tint when the magazine is nearly empty — a glanceable "reload soon" nudge on the player's
/// own weapon (not intel).
const AMMO_LOW_COLOR: [f32; 3] = crate::theme::AMBER;
/// Ammo count fraction (rounds left / mag size) at/below which the count warms to [`AMMO_LOW_COLOR`].
const AMMO_LOW_FRAC: f32 = 0.25;

/// Track/fill opacity — solid enough to read at a glance over the dark frame.
const BAR_ALPHA: f32 = 0.90;
/// Ammo count opacity.
const AMMO_ALPHA: f32 = 0.95;

/// Everything the player-vitals HUD needs this frame — the avatar's own HP and magazine, filled by
/// the host from the embodied unit's (read-only) sim state at the float boundary (invariant #4: the
/// `Fixed` → `f32` hop happens host-side, never in `core`). Pure presentation data with no world
/// position (invariant #6).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct PlayerHudState {
    /// Current hit points (world units — already `f32` at the render boundary).
    pub current_hp: f32,
    /// Maximum hit points. `<= 0` ⇒ nothing to draw (no embodied body).
    pub max_hp: f32,
    /// Rounds left in the current magazine.
    pub ammo: u32,
    /// Magazine capacity. `0` ⇒ the weapon has no magazine model → no count is drawn.
    pub mag_size: u32,
}

impl PlayerHudState {
    /// Nothing to draw — no embodied body (no positive max HP).
    pub fn is_empty(&self) -> bool {
        self.max_hp <= 0.0
    }
}

/// One laid-out label for the text pass: text + NDC placement + tint + fade. Mirrors
/// [`objective_hud::ObjectiveLabel`](crate::objective_hud::ObjectiveLabel) /
/// [`prompt::PromptLabel`](crate::prompt::PromptLabel) so the module stays self-contained and its
/// layout is unit-testable without a `TextRenderer`.
#[derive(Clone, Debug, PartialEq)]
pub struct PlayerHudLabel {
    pub text: String,
    pub pos: [f32; 2],
    pub size: f32,
    pub anchor: Anchor,
    pub color: [f32; 3],
    pub alpha: f32,
}

/// HP fill fraction in `[0, 1]`: `1` = full health, `0` = none (or no body). Clamped — an over-max
/// `current_hp` still reads full, a negative one reads empty. A non-positive `max_hp` (no embodied
/// body) reads `0`. Pure, host-testable (the testable seam for the HP bar — the [`reload_ring_fill`]
/// analogue).
///
/// [`reload_ring_fill`]: crate::tank_hud::reload_ring_fill
pub fn health_fraction(current_hp: f32, max_hp: f32) -> f32 {
    if max_hp <= 0.0 {
        return 0.0;
    }
    (current_hp / max_hp).clamp(0.0, 1.0)
}

/// The HP-bar fill colour for a given health `frac`: healthy (green) → wounded (amber) → critical
/// (red), a glanceable read of the avatar's own condition. Pure, host-testable.
fn health_color(frac: f32) -> [f32; 3] {
    if frac <= LOW_HP {
        crate::theme::STATUS_CRIT
    } else if frac <= MID_HP {
        crate::theme::AMBER
    } else {
        crate::theme::STATUS_GOOD
    }
}

/// The magazine readout string — `"<ammo> / <mag_size>"` (e.g. `"12 / 30"`). Pure, host-testable
/// (the ammo-count formatting seam).
pub fn ammo_label(ammo: u32, mag_size: u32) -> String {
    format!("{ammo} / {mag_size}")
}

/// The HP bar's quads (drawn through the [`overlay`](crate::overlay) quad pipeline): a rim, the full
/// track, and a left-anchored fill sized to the current HP fraction. An empty state (no body) or a
/// zero fill emits no fill quad. Pure + GPU-free → unit-tested.
pub fn player_hud_quads(state: &PlayerHudState) -> Vec<OverlayQuad> {
    player_hud_quads_scaled(state, 1.0)
}

/// [`player_hud_quads`] with an explicit physical `ui_scale` (DPI/point-per-NDC correction). The bar
/// width/height + rim scale so the vitals bar grows in lockstep with the ammo count's scaled glyphs;
/// `ui_scale == 1.0` is byte-identical to [`player_hud_quads`]. LEFT/BOTTOM (screen-edge anchors)
/// stay put so the bar keeps hugging the corner. The renderer threads its live scale in here.
pub fn player_hud_quads_scaled(state: &PlayerHudState, ui_scale: f32) -> Vec<OverlayQuad> {
    if state.is_empty() {
        return Vec::new();
    }
    let frac = health_fraction(state.current_hp, state.max_hp);
    let bar_w = BAR_W * ui_scale;
    let bar_hh = BAR_HH * ui_scale;
    let hw = bar_w * 0.5;
    let cx = LEFT + hw;
    let cy = BOTTOM + bar_hh; // bottom edge sits at BOTTOM
    let mut out = Vec::with_capacity(3);

    // Rim (behind) — a crisp border, like the objective/prompt cards.
    out.push(OverlayQuad {
        cx,
        cy,
        hw: hw + RIM_PAD * ui_scale,
        hh: bar_hh + RIM_PAD * ui_scale,
        r: crate::theme::RIM[0],
        g: crate::theme::RIM[1],
        b: crate::theme::RIM[2],
        alpha: BAR_ALPHA,
        role: QuadRole::PanelRim,
    });
    // Track (the faint full-width reference behind the fill).
    out.push(OverlayQuad {
        cx,
        cy,
        hw,
        hh: bar_hh,
        r: crate::theme::HAIRLINE[0],
        g: crate::theme::HAIRLINE[1],
        b: crate::theme::HAIRLINE[2],
        alpha: BAR_ALPHA,
        role: QuadRole::BarTrack,
    });
    // Fill — left-anchored, width = frac · BAR_W, coloured by condition. Skip a zero-width fill.
    if frac > 0.0 {
        let fill_w = bar_w * frac;
        let fhw = fill_w * 0.5;
        let [r, g, b] = health_color(frac);
        out.push(OverlayQuad {
            cx: LEFT + fhw, // pinned to the left edge of the track
            cy,
            hw: fhw,
            hh: bar_hh,
            r,
            g,
            b,
            alpha: BAR_ALPHA,
            role: QuadRole::DataBar,
        });
    }
    out
}

/// The HP bar's text labels — the magazine count above the bar (when the weapon has a magazine).
/// Left-aligned with the bar. An empty state (no body) or a magazine-less weapon (`mag_size == 0`)
/// emits nothing. Pure + GPU-free → unit-tested.
pub fn player_hud_labels(state: &PlayerHudState) -> Vec<PlayerHudLabel> {
    player_hud_labels_scaled(state, 1.0)
}

/// [`player_hud_labels`] with an explicit physical `ui_scale`. The count POSITION rides above the
/// SCALED bar with a scaled gap; the emitted `size` stays UNSCALED — the text pass multiplies it by
/// `ui_scale` at draw time (no double-scaling). `ui_scale == 1.0` is byte-identical.
pub fn player_hud_labels_scaled(state: &PlayerHudState, ui_scale: f32) -> Vec<PlayerHudLabel> {
    if state.is_empty() || state.mag_size == 0 {
        return Vec::new();
    }
    // The bar's top edge; the count sits a gap above it, growing down from a TopLeft anchor.
    let bar_top = BOTTOM + 2.0 * BAR_HH * ui_scale;
    let label_top = bar_top + LABEL_GAP * ui_scale + AMMO_SIZE * ui_scale;
    // Warm the count when the magazine is nearly spent — a nudge on the player's own weapon.
    let low = (state.ammo as f32) <= AMMO_LOW_FRAC * state.mag_size as f32;
    let color = if low { AMMO_LOW_COLOR } else { AMMO_COLOR };
    vec![PlayerHudLabel {
        text: ammo_label(state.ammo, state.mag_size),
        pos: [LEFT, label_top],
        size: AMMO_SIZE,
        anchor: Anchor::TopLeft,
        color,
        alpha: AMMO_ALPHA,
    }]
}

#[cfg(test)]
mod tests {
    //! `render` is the float boundary (invariant #1: floats live only in rendering), so f32 layout
    //! math is fair game. There is no GPU pipeline here (the bar draws through the shared overlay +
    //! text passes), so every fn below is directly unit-testable.

    use super::*;

    fn state() -> PlayerHudState {
        PlayerHudState {
            current_hp: 75.0,
            max_hp: 100.0,
            ammo: 12,
            mag_size: 30,
        }
    }

    // ---- health fraction (the reload_ring_fill analogue) ----

    #[test]
    fn full_hp_is_full_bar_zero_is_empty() {
        assert!((health_fraction(100.0, 100.0) - 1.0).abs() < 1e-6, "full HP → full bar");
        assert!((health_fraction(0.0, 100.0) - 0.0).abs() < 1e-6, "zero HP → empty bar");
    }

    #[test]
    fn health_fraction_clamps_and_handles_no_body() {
        assert_eq!(health_fraction(150.0, 100.0), 1.0, "over-max clamps to full");
        assert_eq!(health_fraction(-5.0, 100.0), 0.0, "negative clamps to empty");
        assert_eq!(health_fraction(50.0, 0.0), 0.0, "no body (max 0) reads empty");
        assert_eq!(health_fraction(50.0, -1.0), 0.0, "no body (max < 0) reads empty");
        for hp in 0..=200 {
            let f = health_fraction(hp as f32, 100.0);
            assert!((0.0..=1.0).contains(&f), "fraction {f} out of [0,1] at hp={hp}");
        }
    }

    #[test]
    fn health_fraction_is_monotonic() {
        let low = health_fraction(20.0, 100.0);
        let mid = health_fraction(50.0, 100.0);
        let high = health_fraction(90.0, 100.0);
        assert!(low < mid && mid < high, "{low} < {mid} < {high}");
        assert!((mid - 0.5).abs() < 1e-6, "half HP is half the bar");
    }

    // ---- ammo formatting ----

    #[test]
    fn ammo_label_formats_current_over_mag() {
        assert_eq!(ammo_label(12, 30), "12 / 30");
        assert_eq!(ammo_label(0, 30), "0 / 30");
        assert_eq!(ammo_label(30, 30), "30 / 30");
        assert_eq!(ammo_label(150, 200), "150 / 200");
    }

    // ---- HP bar geometry ----

    #[test]
    fn no_body_draws_nothing() {
        let empty = PlayerHudState::default();
        assert!(empty.is_empty());
        assert!(player_hud_quads(&empty).is_empty(), "no body → no bar");
        assert!(player_hud_labels(&empty).is_empty(), "no body → no ammo");
    }

    #[test]
    fn bar_is_rim_track_then_fill_when_healthy() {
        let q = player_hud_quads(&state());
        assert_eq!(q.len(), 3, "rim + track + fill");
        assert_eq!(q[0].role, QuadRole::PanelRim);
        assert_eq!(q[1].role, QuadRole::BarTrack);
        assert_eq!(q[2].role, QuadRole::DataBar);
        // Rim is larger than the track (a crisp border).
        assert!(q[0].hw > q[1].hw && q[0].hh > q[1].hh);
    }

    #[test]
    fn zero_hp_draws_the_track_but_no_fill() {
        let mut s = state();
        s.current_hp = 0.0;
        let q = player_hud_quads(&s);
        assert_eq!(q.len(), 2, "rim + track, no fill quad at 0 HP");
        assert!(q.iter().all(|quad| quad.role != QuadRole::DataBar));
    }

    #[test]
    fn fill_width_tracks_health_and_pins_to_the_left_edge() {
        let full = {
            let mut s = state();
            s.current_hp = 100.0;
            player_hud_quads(&s)
        };
        let half = {
            let mut s = state();
            s.current_hp = 50.0;
            player_hud_quads(&s)
        };
        let full_fill = full.iter().find(|q| q.role == QuadRole::DataBar).unwrap();
        let half_fill = half.iter().find(|q| q.role == QuadRole::DataBar).unwrap();
        // More HP → a wider fill.
        assert!(full_fill.hw > half_fill.hw, "fill widens with health");
        // Full fill spans the whole track width.
        assert!((full_fill.hw - BAR_W * 0.5).abs() < 1e-6, "full HP fills the track");
        // Both fills start pinned to the track's left edge (LEFT), never re-centering.
        assert!((full_fill.cx - full_fill.hw - LEFT).abs() < 1e-6, "full fill left edge at LEFT");
        assert!((half_fill.cx - half_fill.hw - LEFT).abs() < 1e-6, "half fill left edge at LEFT");
    }

    #[test]
    fn low_health_reads_critical_high_reads_healthy() {
        // The condition colour changes with HP so a wounded avatar reads at a glance (own state).
        let crit = health_color(0.10);
        let wounded = health_color(0.45);
        let healthy = health_color(0.90);
        assert_eq!(crit, crate::theme::STATUS_CRIT);
        assert_eq!(healthy, crate::theme::STATUS_GOOD);
        assert_ne!(crit, healthy, "critical must not read the same as healthy");
        assert_ne!(wounded, healthy, "wounded must not read the same as healthy");
        assert_ne!(wounded, crit, "wounded must not read the same as critical");
    }

    // ---- ammo label placement ----

    #[test]
    fn ammo_label_sits_at_the_bottom_left_above_the_bar() {
        let ls = player_hud_labels(&state());
        assert_eq!(ls.len(), 1, "one ammo count");
        let l = &ls[0];
        assert_eq!(l.text, "12 / 30");
        assert_eq!(l.anchor, Anchor::TopLeft);
        // Shares the bar's left edge.
        assert!((l.pos[0] - LEFT).abs() < 1e-6, "ammo count left-aligned with the bar");
        // Sits above the bar's top edge (its bottom = top - size is above the bar top).
        let bar_top = BOTTOM + 2.0 * BAR_HH;
        assert!(l.pos[1] - l.size >= bar_top, "count rides above the bar");
    }

    #[test]
    fn magazine_less_weapon_draws_no_count() {
        let mut s = state();
        s.mag_size = 0; // e.g. a melee/no-mag weapon
        assert!(player_hud_labels(&s).is_empty(), "no magazine → no count");
        // ...but the HP bar still draws.
        assert!(!player_hud_quads(&s).is_empty(), "HP bar is independent of the ammo count");
    }

    #[test]
    fn ammo_count_warms_when_the_magazine_runs_low() {
        let full = {
            let mut s = state();
            s.ammo = 30;
            player_hud_labels(&s)[0].color
        };
        let low = {
            let mut s = state();
            s.ammo = 3; // 3/30 = 0.1 <= AMMO_LOW_FRAC
            player_hud_labels(&s)[0].color
        };
        assert_eq!(full, AMMO_COLOR, "a full mag reads in the neutral bone tint");
        assert_eq!(low, AMMO_LOW_COLOR, "a near-empty mag warms to the low-ammo tint");
        assert_ne!(full, low);
    }

    #[test]
    fn ui_scale_one_is_byte_identical_and_scales_the_bar() {
        // The identity contract the golden tests rely on, plus a check that the bar actually scales.
        let s = state();
        assert_eq!(player_hud_quads(&s), player_hud_quads_scaled(&s, 1.0));
        assert_eq!(player_hud_labels(&s), player_hud_labels_scaled(&s, 1.0));
        // At 2× the track's half-width doubles (the bar grows with the scaled ammo glyphs).
        let base_track = player_hud_quads_scaled(&s, 1.0)[1].hw;
        let scaled_track = player_hud_quads_scaled(&s, 2.0)[1].hw;
        assert!((scaled_track - 2.0 * base_track).abs() < 1e-6, "track half-width doubles at 2×");
    }

    // ---- fairness (invariant #6): screen-space only, bottom-left, clear of centre ----

    #[test]
    fn hud_is_screen_space_bottom_left_and_off_the_center() {
        // Every quad/label is bounded NDC with no world position, sits in the bottom-left quadrant,
        // and stays clear of the screen center (the reticle / scope overlay).
        for q in player_hud_quads(&state()) {
            assert!(q.cx >= -1.0 && q.cx <= 1.0 && q.cy >= -1.0 && q.cy <= 1.0, "quad on-screen");
            assert!(q.cx < 0.0, "quad in the left half");
            assert!(q.cy < 0.0, "quad in the bottom half");
            // Its right edge stays well left of screen center (clear of the centered reticle/scope).
            assert!(q.cx + q.hw < 0.0, "quad stays out of the screen-center column");
        }
        for l in player_hud_labels(&state()) {
            assert!(l.pos[0] >= -1.0 && l.pos[0] <= 1.0 && l.pos[1] >= -1.0 && l.pos[1] <= 1.0);
            assert!(l.pos[0] < 0.0 && l.pos[1] < 0.0, "label in the bottom-left quadrant");
        }
    }
}
