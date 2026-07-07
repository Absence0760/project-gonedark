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
//! formatting math lives in pure free fns — `health_fraction`, `ammo_label`, `player_hud_quads`,
//! `player_hud_labels` — so it is unit-testable without a GPU (exactly the `reload_ring_fill` /
//! `objective_hud_quads` pattern). The bar draws through the shared [`overlay`](crate::overlay) quad
//! pipeline and the count through the shared [`text`](crate::text) pass (no new pipeline/shader),
//! so the `lib.rs` wiring mirrors [`render_prompt`](crate::Renderer::render_prompt) exactly.

use crate::overlay::{OverlayQuad, QuadRole};
use crate::text::Anchor;

// --- layout constants (NDC, bottom-LEFT anchor — a conventionally-empty corner clear of the
//     screen-center reticle / scope overlay) ------------------------------------------------------
/// Left edge of the HP bar — the shared screen-edge inset (`theme`), so the vitals bar hangs the
/// same distance off the edge as the objective card and corner readout (was an ad-hoc 0.06).
const LEFT: f32 = -1.0 + crate::theme::EDGE_INSET;
/// Bottom edge of the HP bar.
const BOTTOM: f32 = -0.90;
/// Full HP-bar width in NDC (the track); the fill spans `frac` of this.
const BAR_W: f32 = 0.42;
/// HP-bar half-height in NDC.
const BAR_HH: f32 = 0.020;
/// The rim quad extends this far past the track on each side — the shared panel spec (`theme`).
const RIM_PAD: f32 = crate::theme::PANEL_RIM_PAD;

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

// --- low/empty magazine cue (invariant #6: never colour alone) ----------------------------------
/// Glyph cell height (NDC) of the low/empty-magazine cue line — slightly smaller than the count it
/// annotates, still combat-glanceable.
const CUE_SIZE: f32 = 0.042;
/// Low-magazine cue text. A literal word, not just a warm tint, so the state survives colour-blind
/// play and the dark frame (invariant #6: cross-modal, never colour alone). All-caps per the HUD
/// convention (the atlas has lowercase, the HUD voice doesn't).
const AMMO_LOW_TEXT: &str = "LOW";
/// Empty-magazine cue text — the truthful recovery action. The magazine reloads **manually** from
/// the unit's carried reserve (`Command::Reload`, D67) — so the honest prompt for an empty mag is
/// RELOAD, not a resupply call. (This surface only knows the magazine; a reserve-dry "RESUPPLY AT
/// CAMP" cue needs the host to feed `Weapon::reserve` into [`PlayerHudState`] — D67's deferred
/// out-of-ammo cue.) ASCII-only (`--`, not an em dash): the atlas bakes 0x20..0x7E.
const AMMO_OUT_TEXT: &str = "MAG EMPTY -- RELOAD";
/// Low-magazine cue tint — the shared caution orange (a *warning* about own state, not yet a stop).
const AMMO_LOW_CUE_COLOR: [f32; 3] = crate::theme::ALERT_WARN;
/// Empty-magazine tint (cue AND count) — [`STATUS_CRIT`](crate::theme::STATUS_CRIT), the theme's
/// "drained own-state" red (an empty mag is your own weapon run dry, not a world alert like
/// `ALERT_DANGER`; the two alias to the one HUD danger red anyway).
const AMMO_OUT_COLOR: [f32; 3] = crate::theme::STATUS_CRIT;

/// Track/fill opacity — deliberately denser than the shared `theme::PANEL_BG_ALPHA` card fill: this
/// is a data bar over the DARK embodied frame, and it must read solid at a combat glance.
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

/// Is the magazine **low** (some rounds left, at/below `AMMO_LOW_FRAC` of capacity)? Exclusive of
/// empty — a dry mag reads as [`ammo_out`], never as merely low. A magazine-less weapon
/// (`mag_size == 0`) is never low. Pure, host-testable (the cue-state seam).
pub fn ammo_low(ammo: u32, mag_size: u32) -> bool {
    ammo > 0 && mag_size > 0 && (ammo as f32) <= AMMO_LOW_FRAC * mag_size as f32
}

/// Is the magazine **empty** (`ammo == 0` on a weapon that has one)? A magazine-less weapon
/// (`mag_size == 0`, e.g. the Medic) is never "out" — it has no mag to run dry. Pure, host-testable.
pub fn ammo_out(ammo: u32, mag_size: u32) -> bool {
    mag_size > 0 && ammo == 0
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

/// The HP bar's text labels — the magazine count above the bar (when the weapon has a magazine),
/// plus a **cue line** stacked above the count when the mag runs low (`"LOW"`) or dry
/// (`"MAG EMPTY -- RELOAD"`). The cue is text, not just tint, so the state reads without colour
/// (invariant #6: alerts must be cross-modal — colour alone excludes colour-blind play and washes
/// out over the dark frame). Left-aligned with the bar. An empty state (no body) or a magazine-less
/// weapon (`mag_size == 0`) emits nothing. Pure + GPU-free → unit-tested. Deliberately **stateless
/// per frame** — no wall-clock pulsing — so the derivation stays a pure function of `state`.
pub fn player_hud_labels(state: &PlayerHudState) -> Vec<PlayerHudLabel> {
    player_hud_labels_scaled(state, 1.0)
}

/// [`player_hud_labels`] with an explicit physical `ui_scale`. The count/cue POSITIONS ride above
/// the SCALED bar with scaled gaps; the emitted `size`s stay UNSCALED — the text pass multiplies by
/// `ui_scale` at draw time (no double-scaling). `ui_scale == 1.0` is byte-identical.
pub fn player_hud_labels_scaled(state: &PlayerHudState, ui_scale: f32) -> Vec<PlayerHudLabel> {
    if state.is_empty() || state.mag_size == 0 {
        return Vec::new();
    }
    // The bar's top edge; the count sits a gap above it, growing down from a TopLeft anchor.
    let bar_top = BOTTOM + 2.0 * BAR_HH * ui_scale;
    let label_top = bar_top + LABEL_GAP * ui_scale + AMMO_SIZE * ui_scale;
    // Magazine cue state: dry outranks low (a dry mag is never "merely low" — `ammo_low` excludes
    // zero, so the two are mutually exclusive by construction).
    let out = ammo_out(state.ammo, state.mag_size);
    let low = ammo_low(state.ammo, state.mag_size);
    // Count tint: neutral bone → warm (low) → crit red (dry). The colour still carries the glance
    // read; the cue line below carries the same state as TEXT (invariant #6: never colour alone).
    let color = if out {
        AMMO_OUT_COLOR
    } else if low {
        AMMO_LOW_COLOR
    } else {
        AMMO_COLOR
    };
    let mut labels = vec![PlayerHudLabel {
        text: ammo_label(state.ammo, state.mag_size),
        pos: [LEFT, label_top],
        size: AMMO_SIZE,
        anchor: Anchor::TopLeft,
        color,
        alpha: AMMO_ALPHA,
    }];
    // The cue line — one gap above the count, sharing its left edge, growing down from TopLeft.
    if out || low {
        let cue_top = label_top + LABEL_GAP * ui_scale + CUE_SIZE * ui_scale;
        let (text, cue_color) = if out {
            // Truthful recovery copy: the mag refills via manual reload from carried reserve (D67).
            (AMMO_OUT_TEXT, AMMO_OUT_COLOR)
        } else {
            (AMMO_LOW_TEXT, AMMO_LOW_CUE_COLOR)
        };
        labels.push(PlayerHudLabel {
            text: text.to_string(),
            pos: [LEFT, cue_top],
            size: CUE_SIZE,
            anchor: Anchor::TopLeft,
            color: cue_color,
            alpha: AMMO_ALPHA,
        });
    }
    labels
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
        assert!(
            (health_fraction(100.0, 100.0) - 1.0).abs() < 1e-6,
            "full HP → full bar"
        );
        assert!(
            (health_fraction(0.0, 100.0) - 0.0).abs() < 1e-6,
            "zero HP → empty bar"
        );
    }

    #[test]
    fn health_fraction_clamps_and_handles_no_body() {
        assert_eq!(
            health_fraction(150.0, 100.0),
            1.0,
            "over-max clamps to full"
        );
        assert_eq!(
            health_fraction(-5.0, 100.0),
            0.0,
            "negative clamps to empty"
        );
        assert_eq!(
            health_fraction(50.0, 0.0),
            0.0,
            "no body (max 0) reads empty"
        );
        assert_eq!(
            health_fraction(50.0, -1.0),
            0.0,
            "no body (max < 0) reads empty"
        );
        for hp in 0..=200 {
            let f = health_fraction(hp as f32, 100.0);
            assert!(
                (0.0..=1.0).contains(&f),
                "fraction {f} out of [0,1] at hp={hp}"
            );
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
        // Rim is larger than the track (a crisp border) — by exactly the shared panel rim
        // thickness (converged from a module-local 0.008; pinned so it can't drift back).
        assert!(q[0].hw > q[1].hw && q[0].hh > q[1].hh);
        assert!(
            (q[0].hw - q[1].hw - crate::theme::PANEL_RIM_PAD).abs() < 1e-6,
            "rim thickness is the shared PANEL_RIM_PAD"
        );
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
        assert!(
            (full_fill.hw - BAR_W * 0.5).abs() < 1e-6,
            "full HP fills the track"
        );
        // Both fills start pinned to the track's left edge (LEFT), never re-centering.
        assert!(
            (full_fill.cx - full_fill.hw - LEFT).abs() < 1e-6,
            "full fill left edge at LEFT"
        );
        assert!(
            (half_fill.cx - half_fill.hw - LEFT).abs() < 1e-6,
            "half fill left edge at LEFT"
        );
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
        assert_ne!(
            wounded, healthy,
            "wounded must not read the same as healthy"
        );
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
        assert!(
            (l.pos[0] - LEFT).abs() < 1e-6,
            "ammo count left-aligned with the bar"
        );
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
        assert!(
            !player_hud_quads(&s).is_empty(),
            "HP bar is independent of the ammo count"
        );
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
        assert_eq!(
            full, AMMO_COLOR,
            "a full mag reads in the neutral bone tint"
        );
        assert_eq!(
            low, AMMO_LOW_COLOR,
            "a near-empty mag warms to the low-ammo tint"
        );
        assert_ne!(full, low);
    }

    // ---- low/empty magazine cue (invariant #6: never colour alone) ----

    #[test]
    fn cue_predicates_split_low_out_and_healthy() {
        // Boundary: 25% of a 30-round mag is 7.5, so 7 is low and 8 is not.
        assert!(ammo_low(7, 30), "7/30 is at/below the low fraction");
        assert!(!ammo_low(8, 30), "8/30 is above the low fraction");
        // Dry outranks low: zero is OUT, never merely LOW (mutually exclusive by construction).
        assert!(ammo_out(0, 30));
        assert!(!ammo_low(0, 30), "an empty mag is out, not low");
        assert!(!ammo_out(1, 30), "one round left is low, not out");
        assert!(ammo_low(1, 30));
        // A magazine-less weapon (Medic / melee) has no mag to run low OR dry.
        assert!(!ammo_low(0, 0));
        assert!(!ammo_out(0, 0));
    }

    #[test]
    fn healthy_mag_emits_no_cue() {
        let ls = player_hud_labels(&state()); // 12/30 = 0.4, above the low fraction
        assert_eq!(ls.len(), 1, "healthy mag → the count only, no cue line");
        assert_eq!(ls[0].color, AMMO_COLOR);
    }

    #[test]
    fn low_mag_emits_a_text_low_cue_not_colour_alone() {
        let mut s = state();
        s.ammo = 3; // 3/30 = 0.1 <= AMMO_LOW_FRAC
        let ls = player_hud_labels(&s);
        assert_eq!(ls.len(), 2, "count + cue line");
        assert_eq!(ls[0].color, AMMO_LOW_COLOR, "count warms (colour channel)");
        assert_eq!(
            ls[1].text, "LOW",
            "…and the state is also spelled out (text channel)"
        );
        assert_eq!(
            ls[1].color,
            crate::theme::ALERT_WARN,
            "cue in the shared caution tint"
        );
    }

    #[test]
    fn empty_mag_cue_names_the_truthful_recovery_action() {
        let mut s = state();
        s.ammo = 0;
        let ls = player_hud_labels(&s);
        assert_eq!(ls.len(), 2, "count + cue line");
        // The mag refills via MANUAL reload from carried reserve (D67, `Command::Reload`) — so the
        // honest empty-mag prompt is RELOAD (resupply-at-camp is the reserve's recovery, which this
        // surface cannot see). Both the count and the cue read in the drained-own-state red.
        assert_eq!(ls[1].text, "MAG EMPTY -- RELOAD");
        assert_eq!(
            ls[1].color,
            crate::theme::STATUS_CRIT,
            "cannot fight → crit red"
        );
        assert_eq!(
            ls[0].color,
            crate::theme::STATUS_CRIT,
            "the 0-count reads crit too"
        );
        assert_ne!(ls[1].text, "LOW", "dry outranks low — never both");
    }

    #[test]
    fn cue_text_is_uppercase_ascii_for_the_hud_atlas() {
        // The atlas bakes full printable ASCII (0x20..0x7E) — lowercase would render, but the HUD
        // voice is all-caps; and anything outside ASCII (an em dash) would drop glyphs entirely.
        for cue in [AMMO_LOW_TEXT, AMMO_OUT_TEXT] {
            assert!(
                cue.chars().all(|c| (' '..='~').contains(&c)),
                "{cue:?} not printable ASCII"
            );
            assert!(
                !cue.chars().any(|c| c.is_ascii_lowercase()),
                "{cue:?} breaks all-caps"
            );
        }
    }

    #[test]
    fn cue_stacks_above_the_count_left_aligned_and_on_screen() {
        for ammo in [0, 3] {
            let mut s = state();
            s.ammo = ammo;
            let ls = player_hud_labels(&s);
            let (count, cue) = (&ls[0], &ls[1]);
            assert!(
                (cue.pos[0] - LEFT).abs() < 1e-6,
                "cue shares the bar's left edge"
            );
            // TopLeft anchors grow down: the cue's bottom edge sits at/above the count's top.
            assert!(
                cue.pos[1] - cue.size >= count.pos[1],
                "cue rides above the count"
            );
            // Still inside the bottom-left quadrant (clear of the centre reticle, on-screen).
            assert!(cue.pos[0] < 0.0 && cue.pos[1] < 0.0 && cue.pos[1] > -1.0);
        }
    }

    #[test]
    fn cue_positions_scale_with_ui_scale() {
        let mut s = state();
        s.ammo = 0;
        // Identity at 1× (the golden-test contract) …
        assert_eq!(player_hud_labels(&s), player_hud_labels_scaled(&s, 1.0));
        // … and at 2× the cue rides higher (scaled gaps/sizes) while its emitted size stays
        // unscaled — the text pass applies ui_scale at draw time (no double-scaling).
        let base = &player_hud_labels_scaled(&s, 1.0)[1];
        let scaled = &player_hud_labels_scaled(&s, 2.0)[1];
        assert!(
            scaled.pos[1] > base.pos[1],
            "cue rises with the scaled stack"
        );
        assert_eq!(scaled.size, base.size, "emitted size stays unscaled");
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
        assert!(
            (scaled_track - 2.0 * base_track).abs() < 1e-6,
            "track half-width doubles at 2×"
        );
    }

    // ---- fairness (invariant #6): screen-space only, bottom-left, clear of centre ----

    #[test]
    fn hud_is_screen_space_bottom_left_and_off_the_center() {
        // Every quad/label is bounded NDC with no world position, sits in the bottom-left quadrant,
        // and stays clear of the screen center (the reticle / scope overlay).
        for q in player_hud_quads(&state()) {
            assert!(
                q.cx >= -1.0 && q.cx <= 1.0 && q.cy >= -1.0 && q.cy <= 1.0,
                "quad on-screen"
            );
            assert!(q.cx < 0.0, "quad in the left half");
            assert!(q.cy < 0.0, "quad in the bottom half");
            // Its right edge stays well left of screen center (clear of the centered reticle/scope).
            assert!(
                q.cx + q.hw < 0.0,
                "quad stays out of the screen-center column"
            );
        }
        for l in player_hud_labels(&state()) {
            assert!(l.pos[0] >= -1.0 && l.pos[0] <= 1.0 && l.pos[1] >= -1.0 && l.pos[1] <= 1.0);
            assert!(
                l.pos[0] < 0.0 && l.pos[1] < 0.0,
                "label in the bottom-left quadrant"
            );
        }
    }
}
