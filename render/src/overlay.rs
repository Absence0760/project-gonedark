//! The in-session shell **overlay** (Phase 4 WS-B, D32 carve-out): the pause / surrender-confirm /
//! reconnect-prompt / post-match-summary surfaces, drawn in-engine on top of the match frame.
//!
//! Like [`hud`](crate::hud), this is a screen-space LOAD pass (it composites over the already-
//! rendered frame, never clears) and a **pure presentation derivation** — it reads only the small,
//! already-presentation-safe overlay description the host hands it ([`Overlay`]) and emits
//! screen-space quads. It is checksum-neutral by construction: it never touches sim state and the
//! host computes it from `core::shell`/`engine::session_shell` views, not from `&World`.
//!
//! ## Fairness (invariant #6) is preserved structurally
//!
//! The overlay draws **opaque dim panels + bars** — chrome, not intel. It carries NO world
//! positions, no fog mask, no off-screen unit state: the post-match summary is integer counts
//! (`core::shell::MatchSummary`, all `i64`/`Fixed`), and a count is not a map reveal. Drawing the
//! overlay never widens the avatar-only fog the unit pass already applied while embodied; the dark
//! frame stays dark underneath. A summary is only ever fed in on the *ended* surface (the match is
//! over, the player is no longer embodied) — the host gates that, but even mid-match the overlay
//! has no spatial data to leak.
//!
//! The testable layout math (which panels appear, their rects, the summary bar lengths) lives in
//! the free [`overlay_quads`] so it is unit-testable without a GPU — exactly the `interpolate_
//! instances` / `marker_for` pattern.

use crate::text::{Anchor, TextRenderer};
use gonedark_core::shell::{FactionStats, MatchOutcome, MatchSummary};
use wgpu::util::DeviceExt;

/// Which in-session overlay surface the host wants drawn this frame. A flat, presentation-only
/// description — the render side never owns the session state machine (that is `engine`); it is
/// handed exactly what to draw. `Summary` carries the integer-only [`MatchSummary`] (no float, no
/// world position — invariant #1/#6).
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Overlay {
    /// No overlay this frame (the match is playing). Draws nothing.
    None,
    /// The pause overlay: a single dim full-screen scrim.
    Paused,
    /// The reconnect prompt: a centered panel. `desynced` picks the (here, color-coded via the
    /// quad's role) copy — stalled vs a confirmed divergence.
    ReconnectPrompt { desynced: bool },
    /// The post-match summary: a centered panel plus a per-faction bar row. Full-info, shown only
    /// after the match ends (not embodied).
    Summary(MatchSummary),
}

/// A semantic role for an overlay quad, so the shader/tint can distinguish chrome from data and a
/// test can assert *what* was drawn without pixel-matching.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum QuadRole {
    /// A full-screen dim scrim behind a panel (so the match reads as "paused/over").
    Scrim,
    /// A panel background.
    Panel,
    /// A neutral prompt accent (stalled reconnect).
    Accent,
    /// A warning accent (a confirmed desync — the more severe reconnect cause).
    Warning,
    /// A victory accent on the summary.
    Win,
    /// A defeat/draw accent on the summary.
    Loss,
    /// A per-faction data bar in the summary (length encodes a kill count — chrome, not a map).
    DataBar,
    /// A per-faction territory bar in the summary (length encodes control points held).
    TerritoryBar,
    /// A per-faction resource bar in the summary (length encodes resources banked).
    ResourceBar,
    /// A faint full-width track behind a data bar, giving every row a shared reference length so
    /// bars read as *relative*, not absolute.
    BarTrack,
    /// A slightly larger, lighter quad behind a [`Panel`](QuadRole::Panel) — a crisp rim/border so
    /// the panel reads over the dim frame.
    PanelRim,
    /// A soft, dark, heavily-feathered quad behind a panel's rim — a drop shadow that lifts the
    /// card off the dim frame. Drawn first of the panel stack so the rim + panel composite over it.
    PanelShadow,
    /// A neutral actionable choice slot (e.g. Resume / Leave / dismiss). Hit-tested by the
    /// native/touch layer at the quad's NDC rect.
    Button,
    /// The primary/affirmative actionable choice slot (e.g. Resume) — visually distinct from a
    /// secondary [`Button`](QuadRole::Button).
    ButtonPrimary,
}

/// One screen-space overlay quad in NDC, ready to upload. `repr(C)` + `Pod` so it streams straight
/// into the instance buffer; the field order MUST match `overlay.wgsl`'s instance attributes and
/// the `vertex_attr_array` in [`OverlayRenderer::new`]. The `role` is CPU-side only (not uploaded).
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct OverlayQuad {
    /// Center in NDC ([-1,1], +y up).
    pub cx: f32,
    pub cy: f32,
    /// Half-width / half-height in NDC.
    pub hw: f32,
    pub hh: f32,
    pub r: f32,
    pub g: f32,
    pub b: f32,
    pub alpha: f32,
    /// Semantic role (CPU-side; drives the color above and lets tests assert structure).
    pub role: QuadRole,
}

/// The GPU-uploadable slice of an [`OverlayQuad`] (drops the CPU-only `role`, adds the derived
/// card-styling params). `repr(C)` + `Pod`.
///
/// **CPU↔GPU lockstep:** the field order here MUST match, one-for-one, the per-instance
/// `@location`s in `overlay.wgsl`'s `vs_main` AND the `vertex_attr_array` in [`OverlayRenderer::new`]
/// — the three move together. The trailing `radius`/`gradient`/`softness`/`aspect` are *derived*
/// from the quad's [`QuadRole`] + size (see [`quad_style`]) and the renderer's aspect, not stored on
/// the public [`OverlayQuad`], so the layout seam + its callers in `lib.rs` stay untouched.
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, bytemuck::Pod, bytemuck::Zeroable)]
struct OverlayInstance {
    cx: f32,
    cy: f32,
    hw: f32,
    hh: f32,
    r: f32,
    g: f32,
    b: f32,
    alpha: f32,
    /// Corner radius (NDC-y units), clamped to the quad's half-extent in [`quad_style`].
    radius: f32,
    /// Vertical-gradient amount in `[0, 1]` (top of the quad reads slightly lighter).
    gradient: f32,
    /// Edge softness (NDC-y): ~0 for a crisp card edge, larger for a feathered drop shadow.
    softness: f32,
    /// Viewport aspect (width / height) so the SDF corners stay round in pixels, not egg-shaped.
    aspect: f32,
}

impl OverlayQuad {
    /// Build the GPU instance, folding in the per-role card styling ([`quad_style`]) and the
    /// renderer's `aspect`. The styling is derived (not stored on the quad) so the pure layout seam
    /// and the `lib.rs` panel callers that build [`OverlayQuad`] literals need no new fields.
    fn instance(&self, aspect: f32) -> OverlayInstance {
        let style = quad_style(self.role, self.hw, self.hh);
        OverlayInstance {
            cx: self.cx,
            cy: self.cy,
            hw: self.hw,
            hh: self.hh,
            r: self.r,
            g: self.g,
            b: self.b,
            alpha: self.alpha,
            radius: style.radius,
            gradient: style.gradient,
            softness: style.softness,
            aspect,
        }
    }
}

/// The derived card-styling params for a quad, computed purely from its [`QuadRole`] + NDC
/// half-extent so the look is data-driven and unit-testable without a GPU. Floats are render-side
/// only (invariant #1/#4).
#[derive(Clone, Copy, PartialEq, Debug)]
struct QuadStyle {
    /// Corner radius in NDC-y units, clamped to `<= min(hw, hh)` so it can never overrun the rect.
    radius: f32,
    /// Vertical-gradient amount in `[0, 1]`.
    gradient: f32,
    /// Edge softness in NDC-y units (`>= 0`); large only for the drop shadow.
    softness: f32,
}

/// Card corner radius (NDC-y) for panels/rim — the rounded-rect "card" look.
const CORNER_PANEL: f32 = 0.045;
/// Corner radius (NDC-y) for buttons — a touch tighter than a panel.
const CORNER_BUTTON: f32 = 0.022;
/// How far past the panel the drop shadow's soft edge feathers (NDC-y).
const SHADOW_SOFTNESS: f32 = 0.055;

/// Derive a quad's [`QuadStyle`] from its role + half-extent. Pure: the corner radius is always
/// clamped to `<= min(hw, hh)` (so it cannot exceed the rect), the gradient to `[0, 1]`, and the
/// softness to `>= 0`. This is the testable seam for the card styling.
fn quad_style(role: QuadRole, hw: f32, hh: f32) -> QuadStyle {
    let min_half = hw.min(hh).max(0.0);
    // (raw corner radius, gradient amount, edge softness) per role, before clamping.
    let (raw_radius, gradient, softness): (f32, f32, f32) = match role {
        // A flat full-screen darkening — square, no gradient, crisp.
        QuadRole::Scrim => (0.0, 0.0, 0.0),
        // Cards: rounded corners + a faint top-lighter gradient. The rim rounds a hair tighter than
        // its pad so the border tracks the panel's curve; the panel itself reads as a lit card.
        QuadRole::Panel => (CORNER_PANEL, 0.55, 0.0),
        QuadRole::PanelRim => (CORNER_PANEL + PANEL_RIM_PAD, 0.0, 0.0),
        // The drop shadow: very rounded + heavily feathered, no gradient.
        QuadRole::PanelShadow => (CORNER_PANEL + 0.04, 0.0, SHADOW_SOFTNESS),
        // Buttons: rounded with a clear lit gradient (the primary slot a touch stronger).
        QuadRole::Button => (CORNER_BUTTON, 0.5, 0.0),
        QuadRole::ButtonPrimary => (CORNER_BUTTON, 0.65, 0.0),
        // Accent strips: pill ends (radius = the strip's half-height) + a gentle gradient.
        QuadRole::Accent | QuadRole::Warning | QuadRole::Win | QuadRole::Loss => {
            (min_half, 0.4, 0.0)
        }
        // Data bars get rounded (pill) ends so they read as deliberate gauges; the track is flatter.
        QuadRole::DataBar | QuadRole::TerritoryBar | QuadRole::ResourceBar => (min_half, 0.35, 0.0),
        QuadRole::BarTrack => (min_half, 0.0, 0.0),
    };
    QuadStyle {
        radius: raw_radius.clamp(0.0, min_half),
        gradient: gradient.clamp(0.0, 1.0),
        softness: softness.max(0.0),
    }
}

// Layout constants (NDC). Panels are centered; the scrim spans the screen.
const SCRIM_ALPHA: f32 = 0.55;
const PANEL_HW: f32 = 0.5;
const PANEL_HH: f32 = 0.32;
/// Half-height of the accent strip across the top of a panel (reconnect cause / match outcome).
/// Named so tuning the strip moves the summary rows that sit below it (see `SUMMARY_ROWS_TOP`).
const ACCENT_STRIP_HH: f32 = 0.04;
/// Gap between the bottom of the accent strip and the first summary row.
const ROW_TOP_GAP: f32 = 0.06;
/// A faint track behind each bar reads at low alpha so it never competes with the data bar.
const BAR_TRACK_ALPHA: f32 = 0.35;
/// The rim quad extends this far past the panel half-extent to draw a thin border.
const PANEL_RIM_PAD: f32 = 0.012;
/// Per-faction summary bar geometry.
const BAR_MAX_HW: f32 = 0.42; // a full bar spans most of the panel width
const BAR_HH: f32 = 0.035;
const BAR_GAP: f32 = 0.1; // vertical spacing between faction rows
/// Top of the first summary bar row, derived from the accent strip so the rows always clear it.
/// The accent strip occupies `[PANEL_HH - 2*ACCENT_STRIP_HH, PANEL_HH]`; rows start a gap below.
const SUMMARY_ROWS_TOP: f32 = PANEL_HH - 2.0 * ACCENT_STRIP_HH - ROW_TOP_GAP;
/// Within a faction row, the three bars (kills / territory / resources) stack at these sub-offsets
/// so a row shows all three facts at once without overlapping the next row.
const BAR_SUB_GAP: f32 = 0.025;
/// Actionable-choice (button) slot geometry, laid out below the panel body.
const BUTTON_HW: f32 = 0.18;
const BUTTON_HH: f32 = 0.045;
const BUTTON_GAP: f32 = 0.04; // horizontal spacing between adjacent slots
const BUTTON_ROW_CY: f32 = -PANEL_HH + 0.09; // near the panel's lower edge

fn color(role: QuadRole) -> [f32; 3] {
    use crate::theme;
    match role {
        QuadRole::Scrim => [0.0, 0.0, 0.0],
        QuadRole::Panel => theme::PANEL,
        QuadRole::Accent => [0.30, 0.55, 0.90], // calm blue: "waiting on a peer"
        QuadRole::Warning => theme::STATUS_CRIT, // red: a confirmed desync
        QuadRole::Win => theme::STATUS_GOOD,    // green: victory
        QuadRole::Loss => [0.70, 0.70, 0.75],   // grey: defeat/draw
        QuadRole::DataBar => theme::DATA_KILLS, // blue: kills
        QuadRole::TerritoryBar => theme::DATA_TERRITORY, // green: territory held
        QuadRole::ResourceBar => theme::DATA_RESOURCE, // amber: resources banked
        QuadRole::BarTrack => theme::HAIRLINE,  // faint track behind a data bar
        QuadRole::PanelRim => theme::RIM,       // a lighter border behind the panel
        QuadRole::PanelShadow => [0.0, 0.0, 0.0], // a black, soft drop shadow lifting the card
        QuadRole::Button => theme::PANEL_RAISED, // a neutral choice slot
        QuadRole::ButtonPrimary => theme::AMBER, // the warm, affirmative call-to-action slot
    }
}

fn quad(cx: f32, cy: f32, hw: f32, hh: f32, alpha: f32, role: QuadRole) -> OverlayQuad {
    let [r, g, b] = color(role);
    OverlayQuad {
        cx,
        cy,
        hw,
        hh,
        r,
        g,
        b,
        alpha,
        role,
    }
}

/// The normalized "score" a faction's summary bar encodes — a presentation ratio computed ABOVE
/// the seam from the integer summary (invariant #1 keeps floats out of `core`; this is render-side
/// float math). Here: units killed relative to the largest kill count in the match, so the
/// best-performing side reads as a full bar. A zero-kill match yields zero-length bars (no NaN).
fn bar_fraction(stats: &FactionStats, max_kills: u32) -> f32 {
    frac_of(stats.units_killed as i64, max_kills as i64)
}

/// Generic presentation ratio: an integer fact over the match-max of that fact, clamped to [0,1].
/// Guarded against a zero (or negative) max so a stat that nobody scored yields a zero-length bar
/// and never a NaN (mirrors [`bar_fraction`]). Render-side float math only (invariant #1).
fn frac_of(value: i64, max: i64) -> f32 {
    if max <= 0 {
        0.0
    } else {
        (value as f32 / max as f32).clamp(0.0, 1.0)
    }
}

/// The actionable choices a surface offers, in vocabulary order, as the [`QuadRole`] each slot
/// draws with ([`ButtonPrimary`](QuadRole::ButtonPrimary) is the affirmative slot). These are a
/// fixed, deterministic per-surface vocabulary — `engine::session_shell` owns *which* actions are
/// live; the renderer only lays out the slots. (Surfacing them as a derivation keeps the `Overlay`
/// enum the host already constructs untouched.)
fn surface_choices(overlay: &Overlay) -> &'static [QuadRole] {
    match overlay {
        Overlay::None => &[],
        // Resume (primary) + Surrender.
        Overlay::Paused => &[QuadRole::ButtonPrimary, QuadRole::Button],
        // Resume (primary) + Leave.
        Overlay::ReconnectPrompt { .. } => &[QuadRole::ButtonPrimary, QuadRole::Button],
        // A single dismiss.
        Overlay::Summary(_) => &[QuadRole::ButtonPrimary],
    }
}

/// How far the drop shadow extends past the panel's half-extent (NDC) before it feathers out.
const PANEL_SHADOW_PAD: f32 = 0.045;
/// The drop shadow is nudged slightly downward so the card reads as lit from above.
const PANEL_SHADOW_DROP: f32 = 0.02;
/// Opacity of the (black) drop shadow over the scrim — subtle; the soft feather does the work.
const PANEL_SHADOW_ALPHA: f32 = 0.5;

/// Emit a panel as a card: a soft drop shadow, then its rim, then the panel body — back-to-front so
/// each composites over the last ("border/shadow = outer quad first", the same pattern
/// [`push_tracked_bar`] uses). The shadow + rim are opaque-chrome anchors; `alpha` applies to the
/// panel fill. The shadow is slightly larger than the rim and dropped a touch so the card lifts off
/// the dim frame; the rim leaves a thin readable border; the shader rounds + gradient-shades each.
fn push_panel_with_rim(out: &mut Vec<OverlayQuad>, alpha: f32) {
    out.push(quad(
        0.0,
        -PANEL_SHADOW_DROP,
        PANEL_HW + PANEL_SHADOW_PAD,
        PANEL_HH + PANEL_SHADOW_PAD,
        PANEL_SHADOW_ALPHA,
        QuadRole::PanelShadow,
    ));
    out.push(quad(
        0.0,
        0.0,
        PANEL_HW + PANEL_RIM_PAD,
        PANEL_HH + PANEL_RIM_PAD,
        1.0,
        QuadRole::PanelRim,
    ));
    out.push(quad(0.0, 0.0, PANEL_HW, PANEL_HH, alpha, QuadRole::Panel));
}

/// Emit a faint full-width track at row `cy`, then a left-anchored data bar of fraction `frac` over
/// it (track first, so the bar reads over a shared reference length). A zero-length bar is skipped
/// but the track is always drawn so every row shows the same baseline.
fn push_tracked_bar(out: &mut Vec<OverlayQuad>, cy: f32, frac: f32, role: QuadRole) {
    out.push(quad(
        0.0,
        cy,
        BAR_MAX_HW,
        BAR_HH,
        BAR_TRACK_ALPHA,
        QuadRole::BarTrack,
    ));
    let hw = (BAR_MAX_HW * frac).max(0.0);
    if hw > 0.0 {
        out.push(quad(-BAR_MAX_HW + hw, cy, hw, BAR_HH, 1.0, role));
    }
}

/// Lay out the surface's choice button slots in a centered row near the panel's lower edge, at
/// deterministic NDC rects (left-to-right, in vocabulary order) the native/touch layer hit-tests.
fn push_button_row(out: &mut Vec<OverlayQuad>, choices: &[QuadRole]) {
    if choices.is_empty() {
        return;
    }
    let n = choices.len() as f32;
    // Total row half-width = n slots + (n-1) gaps; center it on x=0.
    let total_hw = n * BUTTON_HW + (n - 1.0) * BUTTON_GAP * 0.5;
    let mut cx = -total_hw + BUTTON_HW;
    for role in choices {
        out.push(quad(cx, BUTTON_ROW_CY, BUTTON_HW, BUTTON_HH, 1.0, *role));
        cx += 2.0 * BUTTON_HW + BUTTON_GAP;
    }
}

/// Hit-test a point in NDC (`x` rightward, `y` upward — the same screen space [`overlay_quads`]
/// lays the chrome out in) against the overlay's choice-button row, returning the 0-based slot
/// index of the button under the point, or `None` if it misses every button (or the overlay has
/// none). The geometry mirrors [`push_button_row`] exactly, so a hit here corresponds 1:1 to a
/// drawn button — this is the seam the native/touch layer calls to turn a tap into a slot.
pub fn button_slot_at(overlay: &Overlay, ndc_x: f32, ndc_y: f32) -> Option<usize> {
    // Reject anything outside the button row's vertical band before walking the slots.
    if (ndc_y - BUTTON_ROW_CY).abs() > BUTTON_HH {
        return None;
    }
    let choices = surface_choices(overlay);
    let n = choices.len();
    if n == 0 {
        return None;
    }
    let total_hw = n as f32 * BUTTON_HW + (n as f32 - 1.0) * BUTTON_GAP * 0.5;
    let mut cx = -total_hw + BUTTON_HW;
    for slot in 0..n {
        if (ndc_x - cx).abs() <= BUTTON_HW {
            return Some(slot);
        }
        cx += 2.0 * BUTTON_HW + BUTTON_GAP;
    }
    None
}

/// Build the screen-space overlay quads for `overlay`. Pure (no GPU, no sim) — the testable layout
/// seam. Returns an empty vec for [`Overlay::None`]. Quads are returned back-to-front (scrim first,
/// then panel, then accents/bars) so an alpha-blended LOAD pass composites correctly.
pub fn overlay_quads(overlay: &Overlay) -> Vec<OverlayQuad> {
    match overlay {
        Overlay::None => Vec::new(),
        Overlay::Paused => {
            // A single dim scrim across the whole screen + a small "paused" panel (rim first).
            let mut out = vec![quad(0.0, 0.0, 1.0, 1.0, SCRIM_ALPHA, QuadRole::Scrim)];
            push_panel_with_rim(&mut out, 0.92);
            push_button_row(&mut out, surface_choices(overlay));
            out
        }
        Overlay::ReconnectPrompt { desynced } => {
            let accent = if *desynced {
                QuadRole::Warning
            } else {
                QuadRole::Accent
            };
            let mut out = vec![quad(0.0, 0.0, 1.0, 1.0, SCRIM_ALPHA, QuadRole::Scrim)];
            push_panel_with_rim(&mut out, 0.92);
            // An accent strip across the top of the panel signals the cause (blue/red).
            out.push(quad(
                0.0,
                PANEL_HH - ACCENT_STRIP_HH,
                PANEL_HW,
                ACCENT_STRIP_HH,
                1.0,
                accent,
            ));
            push_button_row(&mut out, surface_choices(overlay));
            out
        }
        Overlay::Summary(summary) => {
            let mut out = vec![quad(0.0, 0.0, 1.0, 1.0, SCRIM_ALPHA, QuadRole::Scrim)];
            push_panel_with_rim(&mut out, 0.95);
            // Outcome accent strip across the top of the panel.
            let outcome_role = match summary.outcome {
                MatchOutcome::Victory(_) => QuadRole::Win,
                MatchOutcome::Draw => QuadRole::Loss,
            };
            out.push(quad(
                0.0,
                PANEL_HH - ACCENT_STRIP_HH,
                PANEL_HW,
                ACCENT_STRIP_HH,
                1.0,
                outcome_role,
            ));

            // Per-faction bars, top-down inside the panel. Each row shows three facts — kills,
            // territory, resources — each normalized by its own match-max (a presentation ratio,
            // never a spatial reveal; territory/resources are integer counts, not positions, so
            // invariant #6 holds). Each bar sits over a faint full-width track so the lengths read
            // as relative against a shared reference.
            let max_kills = summary
                .per_faction
                .iter()
                .map(|s| s.units_killed)
                .max()
                .unwrap_or(0);
            let max_territory = summary
                .per_faction
                .iter()
                .map(|s| s.territory_held)
                .max()
                .unwrap_or(0);
            let max_resources = summary
                .per_faction
                .iter()
                .map(|s| s.resources_total)
                .max()
                .unwrap_or(0);
            // Start the rows below the accent strip (derived from it) and lay them out downward.
            let top = SUMMARY_ROWS_TOP;
            for (row, stats) in summary.per_faction.iter().enumerate() {
                let row_cy = top - row as f32 * BAR_GAP;
                push_tracked_bar(
                    &mut out,
                    row_cy,
                    bar_fraction(stats, max_kills),
                    QuadRole::DataBar,
                );
                push_tracked_bar(
                    &mut out,
                    row_cy - BAR_SUB_GAP,
                    frac_of(stats.territory_held as i64, max_territory as i64),
                    QuadRole::TerritoryBar,
                );
                push_tracked_bar(
                    &mut out,
                    row_cy - 2.0 * BAR_SUB_GAP,
                    frac_of(stats.resources_total, max_resources),
                    QuadRole::ResourceBar,
                );
            }
            push_button_row(&mut out, surface_choices(overlay));
            out
        }
    }
}

/// A screen-space text label the overlay wants drawn, computed alongside [`overlay_quads`] so the
/// summary panels carry real numeric/word labels (W4). Pure data — `pos`/`size` are NDC, mirroring
/// the quads; the `text` is uppercase ASCII the [`text`](crate::text) pass renders. Kept as its own
/// type (rather than reaching into `TextRenderer`) so [`overlay_labels`] stays a GPU-free, testable
/// seam — the same pattern as [`overlay_quads`].
#[derive(Clone, PartialEq, Debug)]
pub struct TextLabel {
    pub text: String,
    pub pos: [f32; 2],
    pub size: f32,
    pub anchor: Anchor,
    pub color: [f32; 3],
}

/// Glyph cell height (NDC) of the summary's bar-row numeric labels — sized to sit within a bar row.
const LABEL_SIZE: f32 = 0.030;
/// Glyph cell height (NDC) of the summary outcome title (VICTORY / DEFEAT / DRAW) — the largest.
const TITLE_SIZE: f32 = 0.055;
/// Glyph cell height (NDC) of a button's caption.
const BUTTON_LABEL_SIZE: f32 = 0.034;
/// Light off-white the labels draw in, so they read over the dim panels.
const LABEL_COLOR: [f32; 3] = [0.92, 0.94, 0.98];

/// A short human label for a faction, used as the per-row tag in the summary (uppercase: the font
/// is all-caps). Neutral rows are tagged too so a three-row summary stays unambiguous.
fn faction_label(faction: gonedark_core::shell::FactionTag) -> &'static str {
    use gonedark_core::shell::FactionTag;
    match faction {
        FactionTag::Player => "YOU",
        FactionTag::Enemy => "FOE",
        FactionTag::Neutral => "NEU",
    }
}

/// The outcome title for a summary, from the local player's perspective. Drawn large across the top.
fn outcome_title(outcome: MatchOutcome) -> &'static str {
    use gonedark_core::components::Faction;
    match outcome {
        MatchOutcome::Victory(Faction::Player) => "VICTORY",
        MatchOutcome::Victory(_) => "DEFEAT",
        MatchOutcome::Draw => "DRAW",
    }
}

/// The caption a button slot draws, by surface position. The renderer owns the slot *layout*;
/// `engine::session_shell` owns *which* actions are live, but the per-surface vocabulary is fixed
/// and deterministic (mirrors [`surface_choices`]), so the captions are a safe render-side
/// derivation. A later host worker can override these by queueing its own strings if it wants
/// localized copy — the seam is the public [`TextRenderer::queue`].
fn button_label(overlay: &Overlay, slot: usize) -> &'static str {
    match (overlay, slot) {
        (Overlay::Paused, 0) => "RESUME",
        (Overlay::Paused, 1) => "QUIT",
        (Overlay::ReconnectPrompt { .. }, 0) => "RESUME",
        (Overlay::ReconnectPrompt { .. }, 1) => "LEAVE",
        (Overlay::Summary(_), 0) => "DISMISS",
        _ => "",
    }
}

/// Build the screen-space text labels for `overlay`. Pure (no GPU, no sim) — the testable label
/// seam (mirrors [`overlay_quads`]). Returns an empty vec for surfaces with no text. For the summary
/// this is the W4 payload: the outcome title, a per-row faction tag, and the three NUMERIC readouts
/// (kills / territory / resources) so the bars finally carry their actual counts. Button captions
/// are emitted for every surface that has a button row.
pub fn overlay_labels(overlay: &Overlay) -> Vec<TextLabel> {
    let mut out: Vec<TextLabel> = Vec::new();

    // Button captions, centered on each slot rect (shared by every surface with a button row).
    let choices = surface_choices(overlay);
    if !choices.is_empty() {
        let n = choices.len() as f32;
        let mut cx = -(n * BUTTON_HW + (n - 1.0) * BUTTON_GAP * 0.5) + BUTTON_HW;
        for (slot, _role) in choices.iter().enumerate() {
            let caption = button_label(overlay, slot);
            if !caption.is_empty() {
                out.push(TextLabel {
                    text: caption.to_string(),
                    pos: [cx, BUTTON_ROW_CY],
                    size: BUTTON_LABEL_SIZE,
                    anchor: Anchor::Center,
                    color: LABEL_COLOR,
                });
            }
            cx += 2.0 * BUTTON_HW + BUTTON_GAP;
        }
    }

    if let Overlay::Summary(summary) = overlay {
        // The outcome title, large, centered just above the panel body.
        out.push(TextLabel {
            text: outcome_title(summary.outcome).to_string(),
            pos: [0.0, PANEL_HH + 0.06],
            size: TITLE_SIZE,
            anchor: Anchor::BottomCenter,
            color: LABEL_COLOR,
        });

        // Per-faction numeric readouts, one row per faction, aligned with the bar rows in
        // `overlay_quads` (same `SUMMARY_ROWS_TOP` / `BAR_GAP` geometry). The number is drawn at the
        // RIGHT end of the bar track so it never overlaps the bar fill, and the faction tag at the
        // left. These are the literal integer counts — chrome, not intel (invariant #6).
        let top = SUMMARY_ROWS_TOP;
        for (row, stats) in summary.per_faction.iter().enumerate() {
            let row_cy = top - row as f32 * BAR_GAP;
            // Faction tag at the far left of the row, vertically centered on the kill bar.
            out.push(TextLabel {
                text: faction_label(stats.faction).to_string(),
                pos: [-BAR_MAX_HW - 0.02, row_cy],
                size: LABEL_SIZE,
                anchor: Anchor::Center,
                color: LABEL_COLOR,
            });
            // Three numbers, each right-anchored past its bar track (kills / territory / resources),
            // stacked on the same sub-offsets the bars use.
            let nums = [
                (row_cy, stats.units_killed as i64),
                (row_cy - BAR_SUB_GAP, stats.territory_held as i64),
                (row_cy - 2.0 * BAR_SUB_GAP, stats.resources_total),
            ];
            for (cy, value) in nums {
                out.push(TextLabel {
                    text: value.to_string(),
                    pos: [BAR_MAX_HW + 0.03, cy],
                    size: LABEL_SIZE,
                    anchor: Anchor::Center,
                    color: LABEL_COLOR,
                });
            }
        }
    }

    out
}

/// A unit-quad corner in [-1, 1]^2 (the shader scales it by the per-quad half-size).
#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct QuadVertex {
    corner: [f32; 2],
}

const QUAD_VERTS: [QuadVertex; 6] = [
    QuadVertex {
        corner: [-1.0, -1.0],
    },
    QuadVertex {
        corner: [1.0, -1.0],
    },
    QuadVertex { corner: [1.0, 1.0] },
    QuadVertex {
        corner: [-1.0, -1.0],
    },
    QuadVertex { corner: [1.0, 1.0] },
    QuadVertex {
        corner: [-1.0, 1.0],
    },
];

const INITIAL_CAP: usize = 16;

/// Screen-space in-session shell overlay renderer. Owns its own pipeline + buffers (a separate
/// pipeline from the unit + HUD passes so the three never contend for a shader). Alpha-blended LOAD
/// pass: composites over the already-rendered (possibly dark) frame.
pub struct OverlayRenderer {
    pipeline: wgpu::RenderPipeline,
    quad_buf: wgpu::Buffer,
    instance_buf: wgpu::Buffer,
    instance_cap: usize,
    /// The shared text pass (W4) — the overlay owns one so its panels carry real labels/numbers
    /// without the host wiring a separate call. Flushed at the end of [`OverlayRenderer::render`].
    text: TextRenderer,
    /// Viewport aspect (width / height), set via [`set_aspect`](OverlayRenderer::set_aspect) once per
    /// frame and folded into every uploaded instance so the SDF rounded corners stay round in pixels
    /// (not egg-shaped) on a wide window. Defaults to 1.0 (square — the viz viewport).
    aspect: f32,
}

impl OverlayRenderer {
    /// Build the overlay pipeline against the swapchain `surface_format`. The `device` is borrowed
    /// (D19). Alpha blending so panels dim the frame beneath.
    pub fn new(device: &wgpu::Device, surface_format: wgpu::TextureFormat) -> Self {
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("gonedark.overlay_shader"),
            source: wgpu::ShaderSource::Wgsl(include_str!("overlay.wgsl").into()),
        });

        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("gonedark.overlay_pipeline_layout"),
            bind_group_layouts: &[],
            immediate_size: 0,
        });

        let quad_layout = wgpu::VertexBufferLayout {
            array_stride: std::mem::size_of::<QuadVertex>() as u64,
            step_mode: wgpu::VertexStepMode::Vertex,
            attributes: &wgpu::vertex_attr_array![0 => Float32x2],
        };
        let instance_layout = wgpu::VertexBufferLayout {
            array_stride: std::mem::size_of::<OverlayInstance>() as u64,
            step_mode: wgpu::VertexStepMode::Instance,
            // 1=center(vec2), 2=half(vec2), 3=color(vec3), 4=alpha, 5=radius, 6=gradient,
            // 7=softness, 8=aspect. MUST stay in lockstep with `OverlayInstance` + `overlay.wgsl`.
            attributes: &wgpu::vertex_attr_array![
                1 => Float32x2,
                2 => Float32x2,
                3 => Float32x3,
                4 => Float32,
                5 => Float32,
                6 => Float32,
                7 => Float32,
                8 => Float32
            ],
        };

        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("gonedark.overlay_pipeline"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_main"),
                buffers: &[quad_layout, instance_layout],
                compilation_options: wgpu::PipelineCompilationOptions::default(),
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs_main"),
                targets: &[Some(wgpu::ColorTargetState {
                    format: surface_format,
                    blend: Some(wgpu::BlendState::ALPHA_BLENDING),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
                compilation_options: wgpu::PipelineCompilationOptions::default(),
            }),
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::TriangleList,
                ..Default::default()
            },
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            multiview_mask: None,
            cache: None,
        });

        let quad_buf = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("gonedark.overlay_quad_vbo"),
            contents: bytemuck::cast_slice(&QUAD_VERTS),
            usage: wgpu::BufferUsages::VERTEX,
        });

        let instance_cap = INITIAL_CAP;
        let instance_buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("gonedark.overlay_instance_vbo"),
            size: (instance_cap * std::mem::size_of::<OverlayInstance>()) as u64,
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        let text = TextRenderer::new(device, surface_format);

        OverlayRenderer {
            pipeline,
            quad_buf,
            instance_buf,
            instance_cap,
            text,
            aspect: 1.0,
        }
    }

    /// Set the viewport aspect (width / height) so the overlay's captions/numbers stay square in
    /// pixels instead of stretching on a wide window. Forwarded to the owned text pass; the host calls
    /// it once per frame before [`render`](OverlayRenderer::render). The centered panel quads stay raw
    /// NDC (a modal centered on screen) — only the glyphs are corrected.
    pub fn set_aspect(&mut self, aspect: f32) {
        self.aspect = aspect;
        self.text.set_aspect(aspect);
    }

    /// Draw the in-session overlay on top of `view` (a LOAD pass — never clears). Builds the quad
    /// set via [`overlay_quads`], uploads it, and records one LOAD render pass so the overlay
    /// composites over the (possibly dark) match frame. No-op for [`Overlay::None`].
    pub fn render(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        view: &wgpu::TextureView,
        overlay: &Overlay,
    ) {
        let quads = overlay_quads(overlay);
        if quads.is_empty() {
            return;
        }

        // Queue this surface's text labels (W4): summary numbers/title + button captions. Flushed
        // after the panel quads below so the glyphs composite on top of the chrome.
        for label in overlay_labels(overlay) {
            self.text.queue(
                label.text,
                label.pos,
                label.size,
                label.anchor,
                label.color,
                1.0,
            );
        }

        self.draw_quads(device, queue, view, &quads);

        // Flush the queued labels in their own LOAD pass, on top of the panels just drawn.
        self.text.render(device, queue, view);
    }

    /// Upload + draw a set of NDC [`OverlayQuad`]s in one LOAD pass (no text). The shared quad
    /// primitive behind both the in-session overlay above and the command-view panel box
    /// ([`crate::Renderer::render_command_panel`]); the caller owns any text labels and draws them in
    /// a following pass so glyphs composite on top. A no-op on an empty slice. The instance buffer
    /// grows as needed and is reused across both callers.
    pub fn draw_quads(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        view: &wgpu::TextureView,
        quads: &[OverlayQuad],
    ) {
        if quads.is_empty() {
            return;
        }
        let instances: Vec<OverlayInstance> =
            quads.iter().map(|q| q.instance(self.aspect)).collect();

        if instances.len() > self.instance_cap {
            let new_cap = instances.len().next_power_of_two();
            self.instance_buf = device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("gonedark.overlay_instance_vbo"),
                size: (new_cap * std::mem::size_of::<OverlayInstance>()) as u64,
                usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });
            self.instance_cap = new_cap;
        }
        queue.write_buffer(&self.instance_buf, 0, bytemuck::cast_slice(&instances));

        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("gonedark.overlay_encoder"),
        });
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("gonedark.overlay_pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Load,
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                multiview_mask: None,
                timestamp_writes: None,
                occlusion_query_set: None,
            });
            pass.set_pipeline(&self.pipeline);
            pass.set_vertex_buffer(0, self.quad_buf.slice(..));
            pass.set_vertex_buffer(1, self.instance_buf.slice(..));
            pass.draw(0..QUAD_VERTS.len() as u32, 0..instances.len() as u32);
        }
        queue.submit(std::iter::once(encoder.finish()));
    }
}

#[cfg(test)]
mod tests {
    //! `render` is the float boundary, so f32 layout math is fair game. `OverlayRenderer::new`
    //! needs a real `wgpu::Device` (no display in CI), so the pipeline path is untested; the
    //! testable layout math is factored into [`overlay_quads`].

    use super::*;
    use gonedark_core::components::{Faction, FACTION_COUNT};
    use gonedark_core::shell::{FactionStats, MatchOutcome, MatchSummary};

    fn roles(quads: &[OverlayQuad]) -> Vec<QuadRole> {
        quads.iter().map(|q| q.role).collect()
    }

    fn summary_with_kills(player: u32, enemy: u32, outcome: MatchOutcome) -> MatchSummary {
        let mut per_faction: [FactionStats; FACTION_COUNT] = Default::default();
        for f in Faction::ALL {
            per_faction[f.index()].faction = f.into();
        }
        per_faction[Faction::Player.index()].units_killed = player;
        per_faction[Faction::Enemy.index()].units_killed = enemy;
        MatchSummary {
            outcome,
            end_tick: 3600,
            per_faction,
        }
    }

    /// Full builder so per-stat bar tests can set territory/resources independently of kills.
    #[allow(clippy::too_many_arguments)]
    fn summary_full(
        outcome: MatchOutcome,
        p_kills: u32,
        e_kills: u32,
        p_terr: u32,
        e_terr: u32,
        p_res: i64,
        e_res: i64,
    ) -> MatchSummary {
        let mut per_faction: [FactionStats; FACTION_COUNT] = Default::default();
        for f in Faction::ALL {
            per_faction[f.index()].faction = f.into();
        }
        per_faction[Faction::Player.index()].units_killed = p_kills;
        per_faction[Faction::Enemy.index()].units_killed = e_kills;
        per_faction[Faction::Player.index()].territory_held = p_terr;
        per_faction[Faction::Enemy.index()].territory_held = e_terr;
        per_faction[Faction::Player.index()].resources_total = p_res;
        per_faction[Faction::Enemy.index()].resources_total = e_res;
        MatchSummary {
            outcome,
            end_tick: 3600,
            per_faction,
        }
    }

    #[test]
    fn none_draws_nothing() {
        assert!(overlay_quads(&Overlay::None).is_empty());
    }

    #[test]
    fn paused_is_scrim_shadow_rim_panel() {
        let q = overlay_quads(&Overlay::Paused);
        // Scrim, then the soft drop shadow, then the panel rim, then the panel (back-to-front so
        // each composites over the last — shadow/border = outer quad first).
        assert_eq!(q[0].role, QuadRole::Scrim);
        assert_eq!(q[1].role, QuadRole::PanelShadow);
        assert_eq!(q[2].role, QuadRole::PanelRim);
        assert_eq!(q[3].role, QuadRole::Panel);
        // The scrim spans the whole screen; the panel is centered and smaller, rim + shadow larger.
        assert_eq!((q[0].hw, q[0].hh), (1.0, 1.0));
        assert!(q[3].hw < 1.0 && q[3].hh < 1.0);
        assert_eq!((q[3].cx, q[3].cy), (0.0, 0.0));
        assert!(
            q[2].hw > q[3].hw && q[2].hh > q[3].hh,
            "rim is larger than the panel"
        );
        assert!(
            q[1].hw > q[2].hw && q[1].hh > q[2].hh,
            "shadow is larger than the rim"
        );
    }

    #[test]
    fn reconnect_stalled_uses_accent_desynced_uses_warning() {
        let stalled = overlay_quads(&Overlay::ReconnectPrompt { desynced: false });
        assert!(roles(&stalled).contains(&QuadRole::Accent));
        assert!(!roles(&stalled).contains(&QuadRole::Warning));

        let desync = overlay_quads(&Overlay::ReconnectPrompt { desynced: true });
        assert!(roles(&desync).contains(&QuadRole::Warning));
        assert!(!roles(&desync).contains(&QuadRole::Accent));
    }

    #[test]
    fn summary_victory_uses_win_accent_draw_uses_loss() {
        let win = overlay_quads(&Overlay::Summary(summary_with_kills(
            5,
            2,
            MatchOutcome::Victory(Faction::Player),
        )));
        assert!(roles(&win).contains(&QuadRole::Win));
        assert!(!roles(&win).contains(&QuadRole::Loss));

        let draw = overlay_quads(&Overlay::Summary(summary_with_kills(
            0,
            0,
            MatchOutcome::Draw,
        )));
        assert!(roles(&draw).contains(&QuadRole::Loss));
        assert!(!roles(&draw).contains(&QuadRole::Win));
    }

    #[test]
    fn summary_bar_length_tracks_relative_kills() {
        // Player 4 kills, enemy 2 kills → player bar is full width, enemy bar half.
        let q = overlay_quads(&Overlay::Summary(summary_with_kills(
            4,
            2,
            MatchOutcome::Victory(Faction::Player),
        )));
        let bars: Vec<&OverlayQuad> = q.iter().filter(|q| q.role == QuadRole::DataBar).collect();
        assert_eq!(bars.len(), 2, "two non-zero faction bars (neutral has 0)");
        // Player row is first (rows are in Faction::ALL order). Its half-width is the max.
        assert!(
            (bars[0].hw - BAR_MAX_HW).abs() < 1e-5,
            "leader is a full bar"
        );
        assert!(
            (bars[1].hw - BAR_MAX_HW * 0.5).abs() < 1e-5,
            "half the kills → half the bar"
        );
    }

    #[test]
    fn summary_zero_kills_draws_no_bars_no_nan() {
        let q = overlay_quads(&Overlay::Summary(summary_with_kills(
            0,
            0,
            MatchOutcome::Draw,
        )));
        let bars = q.iter().filter(|q| q.role == QuadRole::DataBar).count();
        assert_eq!(bars, 0, "no kills → no bars (and no division-by-zero NaN)");
        for q in &q {
            assert!(q.hw.is_finite() && q.hh.is_finite());
        }
    }

    #[test]
    fn bar_fraction_is_safe_at_zero_max() {
        let stats = FactionStats {
            units_killed: 0,
            ..Default::default()
        };
        assert_eq!(bar_fraction(&stats, 0), 0.0);
    }

    /// Every overlay surface that draws starts with a full-screen scrim — the match reads as
    /// interrupted/over beneath it, and nothing of the live frame peeks through as "intel".
    #[test]
    fn every_drawn_surface_dims_the_frame_first() {
        for ov in [
            Overlay::Paused,
            Overlay::ReconnectPrompt { desynced: false },
            Overlay::ReconnectPrompt { desynced: true },
            Overlay::Summary(summary_with_kills(
                1,
                0,
                MatchOutcome::Victory(Faction::Player),
            )),
        ] {
            let q = overlay_quads(&ov);
            assert_eq!(
                q[0].role,
                QuadRole::Scrim,
                "first quad is the scrim for {ov:?}"
            );
            assert_eq!((q[0].hw, q[0].hh), (1.0, 1.0));
        }
    }

    /// Fairness guard (invariant #6): no overlay quad carries a world position — every quad is in
    /// NDC and bounded to the screen. The overlay has no spatial sim data to leak.
    #[test]
    fn overlay_quads_are_screen_space_only() {
        let q = overlay_quads(&Overlay::Summary(summary_with_kills(
            3,
            1,
            MatchOutcome::Victory(Faction::Player),
        )));
        for quad in &q {
            assert!(quad.cx >= -1.5 && quad.cx <= 1.5, "cx in NDC range");
            assert!(quad.cy >= -1.5 && quad.cy <= 1.5, "cy in NDC range");
        }
    }

    #[test]
    fn summary_territory_leader_gets_full_bar() {
        // Player holds 3 territory, enemy 1 → player TerritoryBar is full, enemy's a third.
        let q = overlay_quads(&Overlay::Summary(summary_full(
            MatchOutcome::Victory(Faction::Player),
            0,
            0,
            3,
            1,
            0,
            0,
        )));
        let terr: Vec<&OverlayQuad> = q
            .iter()
            .filter(|q| q.role == QuadRole::TerritoryBar)
            .collect();
        assert_eq!(terr.len(), 2, "two non-zero territory bars (neutral has 0)");
        assert!(
            (terr[0].hw - BAR_MAX_HW).abs() < 1e-5,
            "territory leader is a full bar"
        );
        assert!(
            (terr[1].hw - BAR_MAX_HW / 3.0).abs() < 1e-5,
            "a third the territory → a third the bar"
        );
    }

    #[test]
    fn summary_zero_resource_match_draws_no_resource_bar_no_nan() {
        // Nobody banked resources → no ResourceBar (no NaN from a zero match-max).
        let q = overlay_quads(&Overlay::Summary(summary_full(
            MatchOutcome::Draw,
            2,
            1,
            0,
            0,
            0,
            0,
        )));
        let res = q.iter().filter(|q| q.role == QuadRole::ResourceBar).count();
        assert_eq!(
            res, 0,
            "no resources → no resource bars (and no division-by-zero NaN)"
        );
        for q in &q {
            assert!(q.hw.is_finite() && q.hh.is_finite());
        }
    }

    #[test]
    fn resource_bars_render_when_nonzero() {
        let q = overlay_quads(&Overlay::Summary(summary_full(
            MatchOutcome::Victory(Faction::Player),
            0,
            0,
            0,
            0,
            1000,
            500,
        )));
        let res: Vec<&OverlayQuad> = q
            .iter()
            .filter(|q| q.role == QuadRole::ResourceBar)
            .collect();
        assert_eq!(res.len(), 2, "two non-zero resource bars");
        assert!(
            (res[0].hw - BAR_MAX_HW).abs() < 1e-5,
            "resource leader is a full bar"
        );
        assert!(
            (res[1].hw - BAR_MAX_HW * 0.5).abs() < 1e-5,
            "half the resources → half the bar"
        );
    }

    /// Every data bar (kills / territory / resources) is immediately preceded by its faint track,
    /// so each row shows a shared reference length and bars read as relative.
    #[test]
    fn each_data_bar_is_preceded_by_a_track() {
        let q = overlay_quads(&Overlay::Summary(summary_full(
            MatchOutcome::Victory(Faction::Player),
            4,
            2,
            3,
            1,
            1000,
            500,
        )));
        let is_data_bar = |r: QuadRole| {
            matches!(
                r,
                QuadRole::DataBar | QuadRole::TerritoryBar | QuadRole::ResourceBar
            )
        };
        for (i, quad) in q.iter().enumerate() {
            if is_data_bar(quad.role) {
                assert!(i > 0, "a data bar is never the first quad");
                assert_eq!(
                    q[i - 1].role,
                    QuadRole::BarTrack,
                    "data bar at {i} must be preceded by its track"
                );
                // The track spans the full bar width as a shared reference.
                assert_eq!(q[i - 1].hw, BAR_MAX_HW);
            }
        }
        // A track is drawn for every faction row's three facts even when nothing scored.
        let tracks = q.iter().filter(|q| q.role == QuadRole::BarTrack).count();
        assert_eq!(
            tracks,
            3 * FACTION_COUNT,
            "one track per stat per faction row"
        );
    }

    /// The panel rim is drawn directly before the panel for every surface that has a panel — a
    /// crisp border over the dim frame. The scrim still comes first (fairness guard intact).
    #[test]
    fn panel_rim_precedes_each_panel() {
        for ov in [
            Overlay::Paused,
            Overlay::ReconnectPrompt { desynced: false },
            Overlay::Summary(summary_with_kills(
                1,
                0,
                MatchOutcome::Victory(Faction::Player),
            )),
        ] {
            let q = overlay_quads(&ov);
            let panel_i = q
                .iter()
                .position(|q| q.role == QuadRole::Panel)
                .expect("surface has a panel");
            assert!(panel_i > 0, "panel is not the first quad");
            assert_eq!(
                q[panel_i - 1].role,
                QuadRole::PanelRim,
                "the panel is preceded by its rim for {ov:?}"
            );
            assert_eq!(
                q[0].role,
                QuadRole::Scrim,
                "scrim is still first for {ov:?}"
            );
        }
    }

    #[test]
    fn surfaces_lay_out_their_choice_buttons() {
        // Paused: Resume (primary) + Surrender.
        let paused = overlay_quads(&Overlay::Paused);
        assert_eq!(
            paused
                .iter()
                .filter(|q| q.role == QuadRole::ButtonPrimary)
                .count(),
            1,
            "paused has one primary (Resume)"
        );
        assert_eq!(
            paused.iter().filter(|q| q.role == QuadRole::Button).count(),
            1,
            "paused has one secondary (Surrender)"
        );

        // ReconnectPrompt: Resume (primary) + Leave.
        let recon = overlay_quads(&Overlay::ReconnectPrompt { desynced: true });
        assert_eq!(
            recon
                .iter()
                .filter(|q| q.role == QuadRole::ButtonPrimary)
                .count(),
            1
        );
        assert_eq!(
            recon.iter().filter(|q| q.role == QuadRole::Button).count(),
            1
        );

        // Summary: a single dismiss (primary), no secondary.
        let summary = overlay_quads(&Overlay::Summary(summary_with_kills(
            1,
            0,
            MatchOutcome::Victory(Faction::Player),
        )));
        assert_eq!(
            summary
                .iter()
                .filter(|q| q.role == QuadRole::ButtonPrimary)
                .count(),
            1,
            "summary has one dismiss"
        );
        assert_eq!(
            summary
                .iter()
                .filter(|q| q.role == QuadRole::Button)
                .count(),
            0,
            "summary has no secondary button"
        );
    }

    /// Button slots are laid out at deterministic, non-overlapping, in-bounds NDC rects so the
    /// native/touch layer can hit-test them.
    #[test]
    fn button_slots_are_deterministic_and_disjoint() {
        let q = overlay_quads(&Overlay::Paused);
        let mut buttons: Vec<&OverlayQuad> = q
            .iter()
            .filter(|q| matches!(q.role, QuadRole::Button | QuadRole::ButtonPrimary))
            .collect();
        assert_eq!(buttons.len(), 2);
        buttons.sort_by(|a, b| a.cx.partial_cmp(&b.cx).unwrap());
        // Adjacent slots do not overlap (left edge of the right >= right edge of the left).
        let left_right = buttons[0].cx + buttons[0].hw;
        let right_left = buttons[1].cx - buttons[1].hw;
        assert!(right_left >= left_right - 1e-6, "button slots are disjoint");
        // The row is centered on x=0.
        assert!(
            (buttons[0].cx + buttons[1].cx).abs() < 1e-5,
            "button row is centered"
        );
        for b in &buttons {
            assert!(
                b.cx - b.hw >= -PANEL_HW && b.cx + b.hw <= PANEL_HW,
                "in the panel"
            );
        }
    }

    /// `button_slot_at` agrees 1:1 with the drawn button quads: each drawn slot center hit-tests to
    /// its own index, the gap between two slots misses, a point above the row misses, and an overlay
    /// with no buttons always misses.
    #[test]
    fn button_slot_at_matches_drawn_quads() {
        // Two-button surface (Resume / Surrender): every drawn slot center maps to its own index.
        let overlay = Overlay::Paused;
        let mut buttons: Vec<OverlayQuad> = overlay_quads(&overlay)
            .into_iter()
            .filter(|q| matches!(q.role, QuadRole::Button | QuadRole::ButtonPrimary))
            .collect();
        buttons.sort_by(|a, b| a.cx.partial_cmp(&b.cx).unwrap());
        for (i, b) in buttons.iter().enumerate() {
            assert_eq!(
                button_slot_at(&overlay, b.cx, b.cy),
                Some(i),
                "slot {i} center hits its own button"
            );
        }
        // The gap between the two slots misses both.
        let gap_x = (buttons[0].cx + buttons[1].cx) / 2.0;
        assert_eq!(
            button_slot_at(&overlay, gap_x, BUTTON_ROW_CY),
            None,
            "the inter-button gap misses"
        );
        // A point on a button's x but well above the row band misses.
        assert_eq!(
            button_slot_at(&overlay, buttons[0].cx, BUTTON_ROW_CY + 2.0 * BUTTON_HH),
            None,
            "above the row band misses"
        );

        // Single-button surface (the post-match DISMISS): its center hits slot 0.
        let summary = Overlay::Summary(summary_with_kills(
            1,
            0,
            MatchOutcome::Victory(Faction::Player),
        ));
        assert_eq!(
            button_slot_at(&summary, 0.0, BUTTON_ROW_CY),
            Some(0),
            "the lone DISMISS button hit-tests at the row center"
        );

        // An overlay with no choices (None) never reports a hit.
        assert_eq!(button_slot_at(&Overlay::None, 0.0, BUTTON_ROW_CY), None);
    }

    /// The summary bar rows start strictly below the accent strip, derived from `ACCENT_STRIP_HH`
    /// so tuning the strip moves the rows automatically (no magic 0.14 to hand-re-derive).
    #[test]
    fn summary_rows_start_below_accent_strip() {
        let q = overlay_quads(&Overlay::Summary(summary_full(
            MatchOutcome::Victory(Faction::Player),
            4,
            2,
            3,
            1,
            1000,
            500,
        )));
        let accent_bottom = PANEL_HH - 2.0 * ACCENT_STRIP_HH;
        for bar in q.iter().filter(|q| {
            matches!(
                q.role,
                QuadRole::DataBar | QuadRole::TerritoryBar | QuadRole::ResourceBar
            )
        }) {
            assert!(
                bar.cy < accent_bottom,
                "row cy {} must sit below the accent strip bottom {accent_bottom}",
                bar.cy
            );
        }
    }

    /// Regression guard for the "longer = more, top-down" read: every DataBar shares the same left
    /// anchor (cx - hw == -BAR_MAX_HW) and successive non-zero rows stack strictly downward.
    #[test]
    fn summary_bars_share_left_anchor_and_stack_downward() {
        let q = overlay_quads(&Overlay::Summary(summary_full(
            MatchOutcome::Victory(Faction::Player),
            4,
            2,
            0,
            0,
            0,
            0,
        )));
        let bars: Vec<&OverlayQuad> = q.iter().filter(|q| q.role == QuadRole::DataBar).collect();
        assert!(bars.len() >= 2, "need at least two non-zero kill bars");
        for bar in &bars {
            assert!(
                (bar.cx - bar.hw - (-BAR_MAX_HW)).abs() < 1e-5,
                "every data bar shares the left anchor -BAR_MAX_HW"
            );
        }
        // Rows stack downward: each subsequent bar's cy is strictly less than the previous.
        for pair in bars.windows(2) {
            assert!(
                pair[1].cy < pair[0].cy,
                "successive rows stack downward (cy decreasing)"
            );
        }
    }

    // ---- text labels (W4) ----

    #[test]
    fn none_has_no_labels() {
        assert!(overlay_labels(&Overlay::None).is_empty());
    }

    #[test]
    fn paused_labels_its_buttons() {
        let labels = overlay_labels(&Overlay::Paused);
        let texts: Vec<&str> = labels.iter().map(|l| l.text.as_str()).collect();
        assert!(texts.contains(&"RESUME"), "paused labels Resume");
        assert!(texts.contains(&"QUIT"), "paused labels its secondary");
    }

    #[test]
    fn summary_labels_outcome_title() {
        let win = overlay_labels(&Overlay::Summary(summary_with_kills(
            5,
            2,
            MatchOutcome::Victory(Faction::Player),
        )));
        assert!(
            win.iter().any(|l| l.text == "VICTORY"),
            "player victory titles VICTORY"
        );

        let loss = overlay_labels(&Overlay::Summary(summary_with_kills(
            2,
            5,
            MatchOutcome::Victory(Faction::Enemy),
        )));
        assert!(
            loss.iter().any(|l| l.text == "DEFEAT"),
            "enemy victory titles DEFEAT for the local player"
        );

        let draw = overlay_labels(&Overlay::Summary(summary_with_kills(
            0,
            0,
            MatchOutcome::Draw,
        )));
        assert!(draw.iter().any(|l| l.text == "DRAW"), "a draw titles DRAW");
    }

    #[test]
    fn summary_labels_the_numeric_counts() {
        // The W4 payload: the bars get their actual numbers. Player 7 kills, 3 territory, 1234
        // resources must all appear as label strings.
        let q = overlay_labels(&Overlay::Summary(summary_full(
            MatchOutcome::Victory(Faction::Player),
            7,
            2,
            3,
            1,
            1234,
            500,
        )));
        let texts: Vec<&str> = q.iter().map(|l| l.text.as_str()).collect();
        assert!(texts.contains(&"7"), "kills count labelled");
        assert!(texts.contains(&"3"), "territory count labelled");
        assert!(texts.contains(&"1234"), "resources count labelled");
        assert!(texts.contains(&"DISMISS"), "summary dismiss button labelled");
    }

    #[test]
    fn summary_labels_a_number_per_faction_stat() {
        // Three stats (kills/territory/resources) per faction row, plus a faction tag per row, plus
        // the title and the dismiss button. The numeric labels alone must be 3 * FACTION_COUNT.
        let q = overlay_labels(&Overlay::Summary(summary_full(
            MatchOutcome::Draw,
            1,
            2,
            3,
            4,
            5,
            6,
        )));
        let numeric = q
            .iter()
            .filter(|l| l.text.chars().all(|c| c.is_ascii_digit()))
            .count();
        assert_eq!(
            numeric,
            3 * FACTION_COUNT,
            "one number per stat per faction row"
        );
    }

    /// Fairness guard (invariant #6): every label is NDC chrome, never a world position.
    #[test]
    fn labels_are_screen_space_only() {
        let q = overlay_labels(&Overlay::Summary(summary_full(
            MatchOutcome::Victory(Faction::Player),
            3,
            1,
            2,
            0,
            900,
            100,
        )));
        for l in &q {
            assert!(l.pos[0] >= -1.5 && l.pos[0] <= 1.5, "label x in NDC range");
            assert!(l.pos[1] >= -1.5 && l.pos[1] <= 1.5, "label y in NDC range");
            assert!(l.size > 0.0, "label has a positive size");
        }
    }

    // ---- card styling (quad_style) ----

    /// Every role the overlay draws, so the styling invariants below can sweep the whole vocabulary.
    const ALL_ROLES: &[QuadRole] = &[
        QuadRole::Scrim,
        QuadRole::Panel,
        QuadRole::Accent,
        QuadRole::Warning,
        QuadRole::Win,
        QuadRole::Loss,
        QuadRole::DataBar,
        QuadRole::TerritoryBar,
        QuadRole::ResourceBar,
        QuadRole::BarTrack,
        QuadRole::PanelRim,
        QuadRole::PanelShadow,
        QuadRole::Button,
        QuadRole::ButtonPrimary,
    ];

    /// The corner radius can never overrun the rect: for *every* role and a range of half-extents
    /// (including a degenerate sliver), `radius <= min(hw, hh)`. This is the load-bearing clamp that
    /// keeps the SDF rounded-rect well-formed.
    #[test]
    fn quad_style_radius_never_exceeds_half_size() {
        let sizes = [(0.001, 0.001), (0.01, 0.035), (0.18, 0.045), (0.5, 0.32)];
        for &role in ALL_ROLES {
            for &(hw, hh) in &sizes {
                let s = quad_style(role, hw, hh);
                let min_half = hw.min(hh);
                assert!(
                    s.radius <= min_half + 1e-6 && s.radius >= 0.0,
                    "{role:?} radius {} must be in [0, {min_half}] for ({hw},{hh})",
                    s.radius
                );
            }
        }
    }

    /// The gradient amount stays in `[0, 1]` and the softness stays non-negative for every role —
    /// the shader treats both as such, so out-of-range values would render wrong.
    #[test]
    fn quad_style_gradient_and_softness_are_bounded() {
        for &role in ALL_ROLES {
            let s = quad_style(role, 0.5, 0.32);
            assert!(
                (0.0..=1.0).contains(&s.gradient),
                "{role:?} gradient {} out of [0,1]",
                s.gradient
            );
            assert!(s.softness >= 0.0, "{role:?} softness {} < 0", s.softness);
        }
    }

    /// The scrim is a flat full-screen darkening — no rounding, no gradient, no feather; everything
    /// else stays crisp except the drop shadow, which is the only role that feathers its edge.
    #[test]
    fn scrim_is_flat_and_only_shadow_is_soft() {
        let scrim = quad_style(QuadRole::Scrim, 1.0, 1.0);
        assert_eq!(scrim, QuadStyle { radius: 0.0, gradient: 0.0, softness: 0.0 });
        for &role in ALL_ROLES {
            let s = quad_style(role, 0.5, 0.32);
            if role == QuadRole::PanelShadow {
                assert!(s.softness > 0.0, "the drop shadow feathers its edge");
            } else {
                assert_eq!(s.softness, 0.0, "{role:?} has a crisp (un-feathered) edge");
            }
        }
    }

    /// Panels and buttons get a real corner radius (a rounded card), and the data bars + their
    /// tracks get pill ends (radius == the bar's half-height), so they read as deliberate gauges.
    #[test]
    fn cards_round_and_bars_get_pill_ends() {
        assert!(quad_style(QuadRole::Panel, PANEL_HW, PANEL_HH).radius > 0.0);
        assert!(quad_style(QuadRole::Button, BUTTON_HW, BUTTON_HH).radius > 0.0);
        // A wide, short bar: min(hw, hh) == hh, so the radius is the half-height (pill ends).
        for role in [
            QuadRole::DataBar,
            QuadRole::TerritoryBar,
            QuadRole::ResourceBar,
            QuadRole::BarTrack,
        ] {
            let s = quad_style(role, BAR_MAX_HW, BAR_HH);
            assert!(
                (s.radius - BAR_HH).abs() < 1e-6,
                "{role:?} pill radius {} should equal the bar half-height {BAR_HH}",
                s.radius
            );
        }
    }

    /// The GPU instance carries the derived styling: the same quad styled differently by role, and
    /// the renderer's aspect is folded in verbatim so corners stay round on a wide window.
    #[test]
    fn instance_folds_in_style_and_aspect() {
        let panel = quad(0.0, 0.0, PANEL_HW, PANEL_HH, 0.9, QuadRole::Panel);
        let inst = panel.instance(1.6);
        let style = quad_style(QuadRole::Panel, PANEL_HW, PANEL_HH);
        assert_eq!(inst.radius, style.radius);
        assert_eq!(inst.gradient, style.gradient);
        assert_eq!(inst.softness, style.softness);
        assert_eq!(inst.aspect, 1.6);
        // The geometry/color still passes straight through (layout/anchors unchanged).
        assert_eq!((inst.cx, inst.cy, inst.hw, inst.hh), (0.0, 0.0, PANEL_HW, PANEL_HH));
        assert_eq!(inst.alpha, 0.9);
    }

    #[test]
    fn overlay_wgsl_parses_and_validates() {
        let src = include_str!("overlay.wgsl");
        let module = naga::front::wgsl::parse_str(src).expect("overlay.wgsl must parse");
        let mut validator = naga::valid::Validator::new(
            naga::valid::ValidationFlags::all(),
            naga::valid::Capabilities::all(),
        );
        validator
            .validate(&module)
            .expect("overlay.wgsl must validate");
    }
}
