//! Embodied-safe **teach prompt** banner (CP-7 onboarding) — a centered lower-third card the host
//! draws over the embodied (dark) frame to telegraph the *cost* and the *controls* of going dark.
//!
//! Like [`overlay`](crate::overlay) and [`objective_hud`](crate::objective_hud) this is a screen-
//! space LOAD pass + a **pure presentation derivation**: the host (`engine::onboarding`) owns the
//! teach state machine and hands in exactly which [`Prompt`] to draw; this module turns it into NDC
//! quads + labels through the shared overlay-quad + [`text`](crate::text) passes.
//!
//! ## Fairness (invariant #6) is structural
//!
//! The prompt is a **lower-third card carrying static teaching copy** — it has NO world position, no
//! fog mask, no off-screen unit state. It telegraphs *time-cost* and *which button surfaces you*,
//! never the enemy's location, so it is safe to draw over the dark embodied frame: it can leak no
//! intel the avatar's own eyes don't already show. The card never spans the screen (it is not a
//! scrim) and never widens the avatar-only fog beneath it.
//!
//! The testable layout math (card rect, line placement, fade) lives in the free [`prompt_quads`] /
//! [`prompt_labels`] so it is unit-testable without a GPU — exactly the `overlay_quads` /
//! `objective_hud_quads` pattern. [`Renderer::render_prompt`](crate::Renderer::render_prompt) is the
//! only GPU-touching glue.

use crate::overlay::{OverlayQuad, QuadRole};
use crate::text::{measure, Anchor};

/// The emotional tone of a teach prompt — it picks the title/accent color so a *caution* nudge, a
/// *danger* warning, and a *reflective* post-death framing each read distinctly. Presentation only.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum PromptTone {
    /// A calm heads-up (amber): the first going-dark telegraph.
    Caution,
    /// A pressing warning (red): you have lingered in the dark too long.
    Danger,
    /// A cool, reflective framing (blue): the post-death "you stayed too long" payoff.
    Reflect,
}

/// One teach prompt the host wants drawn this frame: a short `title`, up to a couple of `body` lines,
/// a [`PromptTone`], and a fade `alpha` in `[0,1]` the host ramps so the card plays in and out. Pure
/// presentation data — it carries NO world position (invariant #6: teaching chrome, not intel).
#[derive(Clone, PartialEq, Debug)]
pub struct Prompt {
    pub title: String,
    pub body: Vec<String>,
    pub tone: PromptTone,
    pub alpha: f32,
}

impl Prompt {
    /// Nothing to draw — no title and no body.
    pub fn is_empty(&self) -> bool {
        self.title.is_empty() && self.body.is_empty()
    }
}

/// One laid-out label for the text pass: text + NDC placement + tint + fade. Mirrors
/// [`objective_hud::ObjectiveLabel`](crate::objective_hud::ObjectiveLabel) so the module stays
/// self-contained and its layout is unit-testable without touching `TextRenderer`.
#[derive(Clone, Debug, PartialEq)]
pub struct PromptLabel {
    pub text: String,
    pub pos: [f32; 2],
    pub size: f32,
    pub anchor: Anchor,
    pub color: [f32; 3],
    pub alpha: f32,
}

// --- layout constants (NDC) --------------------------------------------------------------------
/// Vertical center of the card — the lower third, clear of the screen center (the FPS reticle / the
/// hitmarker) and the bottom touch-control band.
const CENTER_Y: f32 = -0.58;
/// Title glyph cell height. **M6:** bumped from `0.050` — at the old size the highest-urgency teach
/// copy rendered ~7-9px cap-height on a ~390-430pt landscape phone, barely legible over the dark
/// frame. These are module-local (independent of the shared `theme` type scale, which the command-
/// view chrome uses) so the teach card can be sized for its own worst-case reading distance.
const TITLE_SIZE: f32 = 0.072;
/// Body-line glyph cell height (smaller than the title). **M6:** bumped from `0.036` for the same
/// phone-legibility reason as [`TITLE_SIZE`].
const BODY_SIZE: f32 = 0.050;
/// Gap below the title, before the first body line (a touch more than between body lines).
const TITLE_BODY_GAP: f32 = 0.024;
/// Gap between consecutive body lines.
const LINE_GAP: f32 = 0.016;
/// Horizontal padding between the widest line and the card edge — the shared screen-edge/margin step.
const PAD_X: f32 = crate::theme::SPACE_MARGIN;
/// Vertical padding between the line stack and the card edge — the shared standard-inset step.
const PAD_Y: f32 = crate::theme::SPACE_STD;
/// The rim quad extends this far past the card on each side to draw a thin border.
const RIM_PAD: f32 = 0.010;
/// Half-height of the tone accent strip across the very top of the card.
const ACCENT_HH: f32 = 0.006;

const PANEL_COLOR: [f32; 3] = crate::theme::PANEL;
const RIM_COLOR: [f32; 3] = crate::theme::RIM;
/// Off-white the body lines draw in, so they read over the dim card.
const BODY_COLOR: [f32; 3] = [0.90, 0.92, 0.96];

/// The accent / title color for a [`PromptTone`]. These are a **deliberate distinct sub-palette**,
/// NOT the raw `theme` signal accents: the teach card draws over the *dark embodied frame*, so each
/// tone is intentionally lightened/desaturated relative to `theme::AMBER` / `STATUS_CRIT` / `PLAYER`
/// for legibility against black (the same phone-legibility reasoning as the M6 title/body sizes).
/// Repointing them at the raw accents would darken the copy against the dark frame, so they are kept
/// local by design — they share the theme's *hue families* (warm caution, red danger, cool reflect)
/// without inheriting its command-view luminance.
fn tone_color(tone: PromptTone) -> [f32; 3] {
    match tone {
        PromptTone::Caution => [0.96, 0.80, 0.34], // amber family, lightened for the dark frame
        PromptTone::Danger => [0.92, 0.46, 0.40],  // red family, lightened for the dark frame
        PromptTone::Reflect => [0.74, 0.83, 0.96], // cool-blue family, lightened for the dark frame
    }
}

/// Per-line glyph cell heights, top-down: the title then each body line. Drives the card height.
fn line_heights(p: &Prompt) -> Vec<f32> {
    let mut h = vec![BODY_SIZE; 1 + p.body.len()];
    h[0] = TITLE_SIZE; // the first line is the title
    h
}

/// The gap BELOW each line (between line `i` and `i+1`): `TITLE_BODY_GAP` after the title, `LINE_GAP`
/// between body lines. One shorter than the line count (the last line has no gap below).
fn line_gaps(p: &Prompt) -> Vec<f32> {
    let n_lines = 1 + p.body.len();
    (0..n_lines.saturating_sub(1))
        .map(|i| if i == 0 { TITLE_BODY_GAP } else { LINE_GAP })
        .collect()
}

/// Each line's NDC width at the given viewport `aspect`, top-down (title then body). Aspect-corrected
/// through [`measure`] so the card hugs the text on a wide window (the raw-NDC chrome footgun).
fn line_widths(p: &Prompt, aspect: f32) -> Vec<f32> {
    let mut w = vec![measure(&p.title, TITLE_SIZE, aspect).0];
    for line in &p.body {
        w.push(measure(line, BODY_SIZE, aspect).0);
    }
    w
}

/// The card half-extent `(hw, hh)` and the NDC `top` y of the first line, for a prompt at the given
/// `aspect`. Pure — shared by [`prompt_quads`] and [`prompt_labels`] so the card and its text always
/// agree (the `box_geom` pattern).
fn card_geom(p: &Prompt, aspect: f32) -> (f32, f32, f32) {
    let total_h: f32 = line_heights(p).iter().sum::<f32>() + line_gaps(p).iter().sum::<f32>();
    let max_w = line_widths(p, aspect).into_iter().fold(0.0_f32, f32::max);
    let hw = max_w * 0.5 + PAD_X;
    let hh = total_h * 0.5 + PAD_Y;
    let top = CENTER_Y + total_h * 0.5; // top edge of the first (TopCenter-anchored) line box
    (hw, hh, top)
}

/// The card's background + rim + tone-accent quads (drawn through the overlay quad pipeline),
/// auto-sized to the prompt's line stack. Empty/invisible prompt ⇒ no quads. Back-to-front (rim,
/// panel, accent) so each composites over the last. Pure + GPU-free → unit-tested.
pub fn prompt_quads(p: &Prompt, aspect: f32) -> Vec<OverlayQuad> {
    if p.is_empty() || p.alpha <= 0.0 {
        return Vec::new();
    }
    let (hw, hh, _) = card_geom(p, aspect);
    let a = p.alpha.clamp(0.0, 1.0);
    let [tr, tg, tb] = tone_color(p.tone);
    vec![
        // Rim (behind), then the panel fill, then a thin tone strip across the top inner edge.
        OverlayQuad {
            cx: 0.0,
            cy: CENTER_Y,
            hw: hw + RIM_PAD,
            hh: hh + RIM_PAD,
            r: RIM_COLOR[0],
            g: RIM_COLOR[1],
            b: RIM_COLOR[2],
            alpha: 0.92 * a,
            role: QuadRole::PanelRim,
        },
        OverlayQuad {
            cx: 0.0,
            cy: CENTER_Y,
            hw,
            hh,
            r: PANEL_COLOR[0],
            g: PANEL_COLOR[1],
            b: PANEL_COLOR[2],
            alpha: 0.86 * a,
            role: QuadRole::Panel,
        },
        OverlayQuad {
            cx: 0.0,
            cy: CENTER_Y + hh - ACCENT_HH,
            hw: (hw - PAD_X * 0.5).max(0.0),
            hh: ACCENT_HH,
            r: tr,
            g: tg,
            b: tb,
            alpha: 0.95 * a,
            role: QuadRole::Accent,
        },
    ]
}

/// The card's text labels — the tone-tinted title, then each body line in off-white, stacked
/// top-down and horizontally centered. Empty/invisible prompt ⇒ no labels. Pure + GPU-free →
/// unit-tested. All carry the prompt's fade `alpha`.
pub fn prompt_labels(p: &Prompt, aspect: f32) -> Vec<PromptLabel> {
    if p.is_empty() || p.alpha <= 0.0 {
        return Vec::new();
    }
    let (_, _, top) = card_geom(p, aspect);
    let a = p.alpha.clamp(0.0, 1.0);
    let mut out = Vec::with_capacity(1 + p.body.len());
    let mut y = top;
    out.push(PromptLabel {
        text: p.title.clone(),
        pos: [0.0, y],
        size: TITLE_SIZE,
        anchor: Anchor::TopCenter,
        color: tone_color(p.tone),
        alpha: a,
    });
    y -= TITLE_SIZE + TITLE_BODY_GAP;
    for (i, line) in p.body.iter().enumerate() {
        out.push(PromptLabel {
            text: line.clone(),
            pos: [0.0, y],
            size: BODY_SIZE,
            anchor: Anchor::TopCenter,
            color: BODY_COLOR,
            alpha: a,
        });
        y -= BODY_SIZE;
        if i + 1 < p.body.len() {
            y -= LINE_GAP;
        }
    }
    out
}

#[cfg(test)]
mod tests {
    //! `render` is the float boundary, so f32 layout math is fair game. The card pipeline needs a
    //! real `wgpu::Device` (no display in CI), so it is untested here; the testable layout math is
    //! factored into [`prompt_quads`] / [`prompt_labels`].

    use super::*;

    fn prompt() -> Prompt {
        Prompt {
            title: "GOING DARK".into(),
            body: vec![
                "The map is blind now.".into(),
                "SURFACE to take command.".into(),
            ],
            tone: PromptTone::Caution,
            alpha: 1.0,
        }
    }

    #[test]
    fn empty_or_invisible_prompt_draws_nothing() {
        let empty = Prompt {
            title: String::new(),
            body: vec![],
            tone: PromptTone::Caution,
            alpha: 1.0,
        };
        assert!(prompt_quads(&empty, 1.0).is_empty());
        assert!(prompt_labels(&empty, 1.0).is_empty());

        let faded = Prompt {
            alpha: 0.0,
            ..prompt()
        };
        assert!(prompt_quads(&faded, 1.0).is_empty(), "alpha 0 → nothing");
        assert!(prompt_labels(&faded, 1.0).is_empty());
    }

    #[test]
    fn quads_are_rim_then_panel_then_accent() {
        let q = prompt_quads(&prompt(), 1.0);
        assert_eq!(q.len(), 3);
        assert_eq!(q[0].role, QuadRole::PanelRim);
        assert_eq!(q[1].role, QuadRole::Panel);
        assert_eq!(q[2].role, QuadRole::Accent);
        // Rim is larger than the panel (a crisp border).
        assert!(q[0].hw > q[1].hw && q[0].hh > q[1].hh);
    }

    #[test]
    fn labels_are_title_then_body_top_down_and_centered() {
        let ls = prompt_labels(&prompt(), 1.0);
        assert_eq!(ls.len(), 3, "title + two body lines");
        assert_eq!(ls[0].text, "GOING DARK");
        assert_eq!(ls[1].text, "The map is blind now.");
        assert_eq!(ls[2].text, "SURFACE to take command.");
        // Stack strictly downward.
        assert!(ls[1].pos[1] < ls[0].pos[1]);
        assert!(ls[2].pos[1] < ls[1].pos[1]);
        // All horizontally centered (TopCenter at x=0).
        assert!(ls.iter().all(|l| l.pos[0] == 0.0));
        assert!(ls.iter().all(|l| l.anchor == Anchor::TopCenter));
    }

    #[test]
    fn title_takes_the_tone_color_body_is_off_white() {
        let ls = prompt_labels(&prompt(), 1.0);
        assert_eq!(ls[0].color, tone_color(PromptTone::Caution));
        assert_eq!(ls[1].color, BODY_COLOR);
        assert_eq!(ls[2].color, BODY_COLOR);
        // Distinct tones give distinct title/accent colors.
        assert_ne!(tone_color(PromptTone::Caution), tone_color(PromptTone::Danger));
        assert_ne!(tone_color(PromptTone::Danger), tone_color(PromptTone::Reflect));
    }

    #[test]
    fn fade_alpha_propagates_to_every_quad_and_label() {
        let faded = Prompt {
            alpha: 0.5,
            ..prompt()
        };
        for q in prompt_quads(&faded, 1.0) {
            // Each quad's alpha is its base opacity scaled by the prompt fade (<= base, > 0).
            assert!(q.alpha > 0.0 && q.alpha <= 0.95 * 0.5 + 1e-6);
        }
        for l in prompt_labels(&faded, 1.0) {
            assert!((l.alpha - 0.5).abs() < 1e-6);
        }
    }

    #[test]
    fn card_sits_in_the_lower_third_and_off_the_center() {
        let q = prompt_quads(&prompt(), 1.0);
        let panel = &q[1];
        assert!(panel.cy < 0.0, "card is in the lower half");
        // Its top edge stays well below the screen center (clear of the reticle/hitmarker).
        assert!(panel.cy + panel.hh < 0.0, "whole card is in the bottom half");
    }

    #[test]
    fn fairness_quads_and_labels_are_screen_space_only() {
        // Invariant #6: a teach prompt carries no world position — every quad/label is bounded NDC.
        let q = prompt_quads(&prompt(), 16.0 / 9.0);
        for quad in &q {
            assert!(quad.cx >= -1.5 && quad.cx <= 1.5);
            assert!(quad.cy >= -1.5 && quad.cy <= 1.5);
        }
        for l in prompt_labels(&prompt(), 16.0 / 9.0) {
            assert!(l.pos[0] >= -1.5 && l.pos[0] <= 1.5);
            assert!(l.pos[1] >= -1.5 && l.pos[1] <= 1.5);
        }
    }

    #[test]
    fn card_hugs_text_narrower_on_a_wide_window() {
        // Aspect correction: the same prompt yields a narrower card on a wide viewport (the glyphs
        // are narrower in NDC), proving the geometry runs through the aspect-aware `measure`.
        let square = prompt_quads(&prompt(), 1.0)[1].hw;
        let wide = prompt_quads(&prompt(), 16.0 / 9.0)[1].hw;
        assert!(wide < square, "card is narrower on a wide screen");
    }

    #[test]
    fn prompt_type_is_phone_legible_and_card_still_clears_center_and_bottom() {
        // M6: the teach copy must read on a phone — a real cap-height, not the old ~7px. The title is
        // the larger step; both clear a legibility floor.
        assert!(TITLE_SIZE >= 0.06, "title is phone-legible, got {TITLE_SIZE}");
        assert!(BODY_SIZE >= 0.045, "body is phone-legible, got {BODY_SIZE}");
        assert!(TITLE_SIZE > BODY_SIZE, "title is the larger step");
        // Even at the bumped size the card still hugs the lower third: its top edge stays clear of the
        // screen center (the FPS reticle / hitmarker) and its bottom stays on-screen — checked on a
        // wide phone-landscape aspect where the geometry runs through the aspect-aware `measure`.
        let q = prompt_quads(&prompt(), 20.0 / 9.0);
        let panel = &q[1];
        let top = panel.cy + panel.hh;
        let bottom = panel.cy - panel.hh;
        assert!(top < -0.05, "card top clears the screen center, got {top}");
        assert!(bottom > -1.0, "card bottom stays on-screen, got {bottom}");
    }

    #[test]
    fn single_line_prompt_has_a_shorter_card() {
        let one = Prompt {
            title: "STILL DARK".into(),
            body: vec!["Your squad fights without you.".into()],
            tone: PromptTone::Danger,
            alpha: 1.0,
        };
        let two = prompt();
        assert!(
            prompt_quads(&one, 1.0)[1].hh < prompt_quads(&two, 1.0)[1].hh,
            "fewer lines → shorter card"
        );
        // One title + one body line.
        assert_eq!(prompt_labels(&one, 1.0).len(), 2);
    }
}
