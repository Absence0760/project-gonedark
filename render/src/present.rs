//! The full-screen **present grade** — the cinematic tonemap applied once over the whole 3D scene as
//! it is upscaled onto the swapchain ([`crate::scene_target::SceneTarget::present`]), before the
//! crisp native-resolution HUD/text chrome is drawn on top.
//!
//! Two pieces live here:
//!  - [`PresentUniform`] — the tiny uniform the blit shader reads, carrying the **"going dark"**
//!    amount (`0` in command view, `1` while embodied).
//!  - [`going_dark_grade`] — the **off-GPU reference twin** of `present.wgsl`'s `fs_present`: the base
//!    grade ([`crate::theme::present_grade`]) followed by the embodied dark intensification. WGSL
//!    cannot import Rust, so the grade math is duplicated in the shader; this mirror is unit-tested so
//!    a regression (a crushed centre, a runaway tint, an out-of-range output) fails CI rather than
//!    only showing up on screen.
//!
//! ## Why the dark grade is here, and why it is fair (invariant #6)
//!
//! WS-E deepens the visceral "world goes dark" moment **as presentation only** — it never reveals or
//! hides map intel. It is safe because:
//!  - it is edge-weighted (a **tunnel vignette**) and shadow-weighted, so the **lit centre stays
//!    readable** — going dark reads as tunnel vision closing in, not a black screen you can't play;
//!  - while embodied the fog filter already draws **only the avatar** ([`crate::fog::visible_instances`])
//!    — there are no enemy units in the frame to hide in a deepened shadow;
//!  - the amber avatar is bright (high luminance), so the shadow crush leaves it untouched;
//!  - the HUD, hitmarker, and directional-alert cues are drawn AFTER this pass onto the native
//!    swapchain, so the fairness channel is never dimmed.
//!
//! Float side of invariant #1/#4: every value is a render-only `f32`; nothing here is read by
//! `core`/the sim, so the grade can never move the per-tick checksum.

use crate::theme::{self, Rgb};

/// Hermite smoothstep in `[0,1]` (the GLSL/WGSL `smoothstep`) — matches `present.wgsl`.
#[inline]
fn smoothstep(e0: f32, e1: f32, x: f32) -> f32 {
    let t = ((x - e0) / (e1 - e0)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// The "going dark" amount the present pass should apply this frame: `1.0` while the local player is
/// embodied (the strategic map is dark), `0.0` in command view. A trivial but named seam so the
/// Renderer's choice is explicit and unit-tested, and so `dark = 0` is provably the identity (command
/// view stays byte-identical to the pre-WS-E grade).
#[inline]
pub fn dark_amount(world_dark: bool) -> f32 {
    if world_dark {
        1.0
    } else {
        0.0
    }
}

/// The embodied "world goes dark" intensification layered on top of the base [`theme::present_grade`]
/// — the **reference twin** of the `dark` branch in `present.wgsl`'s `fs_present`. Keep every constant
/// in lockstep with the shader.
///
/// `dark` in `[0,1]` scales the whole effect (so `dark = 0` returns `c` untouched — the command-view
/// identity). It applies, in order: a **tunnel vignette** that darkens only toward the edges (the
/// centre stays lit — fairness #6); a partial **desaturation of the shadows** toward their own
/// luminance; a **subtractive ink-cool** of the shadows (warm channels pulled DOWN, never blue raised,
/// so the grade can't fabricate a "player-blue" pixel the fairness harness would misread); and a
/// **multiplicative deepening** of the shadows. All shadow terms are weighted by `shadow_w`, so a
/// brightly-lit pixel is left essentially untouched.
fn apply_dark(mut c: Rgb, uv: [f32; 2], dark: f32) -> Rgb {
    let dark = dark.clamp(0.0, 1.0);

    // Tunnel vignette: darken only toward the edges (r ~0 at centre, ~1 at the corner). Leaves the
    // inner ~30% fully bright, so the embodied view reads as vision closing in, never a black frame.
    let d = [uv[0] - 0.5, uv[1] - 0.5];
    let r = (d[0] * d[0] + d[1] * d[1]).sqrt() * 1.414_213_5;
    let tunnel = 1.0 - smoothstep(0.30, 1.05, r) * 0.55 * dark;

    // Shadow weight: strongest in the darks, ~0 by mid-grey — so lit surfaces (and the amber avatar)
    // are left alone and only the gloom deepens.
    let l = theme::luminance(c);
    let shadow_w = 1.0 - smoothstep(0.0, 0.5, l);

    // Desaturate the shadows toward their own luminance (the darkness reads as ink, not muddy colour).
    let grey = theme::luminance(c);
    c = theme::mix(c, [grey, grey, grey], shadow_w * 0.35 * dark);

    // Subtractive ink-cool of the shadows (never raise blue → #6-safe), then a multiplicative deepen.
    let ink_tint = [-0.020, -0.012, 0.0];
    let deepen = 1.0 - shadow_w * 0.22 * dark;
    for i in 0..3 {
        c[i] = ((c[i] + ink_tint[i] * shadow_w * dark) * deepen * tunnel).clamp(0.0, 1.0);
    }
    c
}

/// The full present grade for the scene: the base [`theme::present_grade`] then, when `dark > 0`, the
/// embodied [`apply_dark`] intensification. `rgb` is the scene colour, `uv` the fullscreen `[0,1]`
/// coordinate (v=0 at the top), `dark` the [`dark_amount`]. At `dark = 0` this is exactly
/// `theme::present_grade` (command view unchanged). Unit-tested; keep in lockstep with `present.wgsl`.
pub fn going_dark_grade(rgb: Rgb, uv: [f32; 2], dark: f32) -> Rgb {
    let c = theme::present_grade(rgb, uv);
    if dark <= 0.0 {
        return c;
    }
    apply_dark(c, uv, dark)
}

/// The present-pass uniform — `params = (dark, 0, 0, 0)` matching `present.wgsl`'s `Present` struct.
/// `repr(C)` + `Pod` so it uploads straight into the uniform buffer; the field order/offsets MUST
/// match the shader.
#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
pub struct PresentUniform {
    /// x = "going dark" amount `[0,1]`; y/z/w reserved padding (kept 0).
    pub params: [f32; 4],
}

impl PresentUniform {
    /// Build the uniform from the "going dark" amount (clamped to `[0,1]`).
    pub fn new(dark: f32) -> Self {
        PresentUniform {
            params: [dark.clamp(0.0, 1.0), 0.0, 0.0, 0.0],
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::theme;

    const EPS: f32 = 1e-6;

    #[test]
    fn dark_amount_is_binary() {
        assert_eq!(dark_amount(false), 0.0);
        assert_eq!(dark_amount(true), 1.0);
    }

    #[test]
    fn uniform_clamps_dark() {
        assert_eq!(PresentUniform::new(0.4).params[0], 0.4);
        assert_eq!(PresentUniform::new(5.0).params[0], 1.0, "over-range clamps to 1");
        assert_eq!(PresentUniform::new(-2.0).params[0], 0.0, "under-range clamps to 0");
        assert_eq!(
            [
                PresentUniform::new(0.4).params[1],
                PresentUniform::new(0.4).params[2],
                PresentUniform::new(0.4).params[3]
            ],
            [0.0, 0.0, 0.0]
        );
    }

    /// `dark = 0` is exactly the base present grade — command view is byte-identical to pre-WS-E.
    #[test]
    fn command_view_is_the_untouched_base_grade() {
        let uvs = [[0.5, 0.5], [0.0, 0.0], [1.0, 1.0], [0.2, 0.8]];
        for c in [[0.1, 0.1, 0.1], [0.5, 0.4, 0.3], [0.9, 0.9, 0.9], [0.3, 0.6, 0.2]] {
            for uv in uvs {
                let base = theme::present_grade(c, uv);
                let got = going_dark_grade(c, uv, 0.0);
                for i in 0..3 {
                    assert!((base[i] - got[i]).abs() < EPS, "dark=0 must equal present_grade");
                }
            }
        }
    }

    /// Every graded channel stays in `[0,1]` for any in-range input at full dark — a value outside
    /// that range would clip unpredictably on the way to the swapchain.
    #[test]
    fn dark_grade_stays_in_unit_range() {
        let uvs = [[0.5, 0.5], [0.0, 0.0], [1.0, 1.0], [0.0, 1.0], [1.0, 0.0]];
        for r in 0..=4 {
            for g in 0..=4 {
                for b in 0..=4 {
                    let c = [r as f32 / 4.0, g as f32 / 4.0, b as f32 / 4.0];
                    for uv in uvs {
                        for &dark in &[0.5, 1.0] {
                            for &ch in &going_dark_grade(c, uv, dark) {
                                assert!(
                                    (0.0..=1.0).contains(&ch),
                                    "channel {ch} out of [0,1] for {c:?} @ {uv:?} dark={dark}"
                                );
                            }
                        }
                    }
                }
            }
        }
    }

    /// The dark grade deepens the corners MORE than command view does (the tunnel closing in), while
    /// the lit CENTRE stays readable — the fairness contract (invariant #6): visceral, not blinding.
    #[test]
    fn dark_deepens_edges_but_keeps_the_centre_readable() {
        let mid = [0.5, 0.5, 0.5];
        // Corner: full-dark must be dimmer than command view at the same corner.
        let cmd_corner = theme::luminance(going_dark_grade(mid, [1.0, 1.0], 0.0));
        let dark_corner = theme::luminance(going_dark_grade(mid, [1.0, 1.0], 1.0));
        assert!(dark_corner < cmd_corner, "dark corner {dark_corner} !< command {cmd_corner}");

        // Centre: a lit mid-grey at screen centre stays clearly visible under full dark (the tunnel
        // leaves the centre alone and the shadow crush barely touches a mid pixel).
        let centre = theme::luminance(going_dark_grade([0.6, 0.6, 0.6], [0.5, 0.5], 1.0));
        assert!(centre > 0.3, "lit centre must stay readable under dark, got {centre}");
    }

    /// The going-dark effect is monotone in `dark`: more dark, dimmer corner. Guards against a sign
    /// flip in any of the stacked terms.
    #[test]
    fn dark_is_monotone_at_the_edge() {
        let mid = [0.5, 0.5, 0.5];
        let l0 = theme::luminance(going_dark_grade(mid, [0.95, 0.95], 0.0));
        let l5 = theme::luminance(going_dark_grade(mid, [0.95, 0.95], 0.5));
        let l1 = theme::luminance(going_dark_grade(mid, [0.95, 0.95], 1.0));
        assert!(l0 > l5 && l5 > l1, "edge must darken monotonically with dark ({l0},{l5},{l1})");
    }

    /// The dark intensification is **subtractive on the warm channels only** — it never RAISES blue
    /// above what the base grade already produced (the fairness harness reads a bright player-blue
    /// pixel as intel; the ink-cool must only pull warm channels down + deepen, never manufacture
    /// blue). So the dark grade's blue channel is always ≤ the base grade's blue for the same pixel.
    #[test]
    fn dark_term_never_raises_blue_over_the_base_grade() {
        let uvs = [[0.5, 0.5], [0.1, 0.1], [0.9, 0.9], [0.3, 0.7]];
        for c in [[0.12, 0.12, 0.12], [0.2, 0.15, 0.1], [0.4, 0.4, 0.5], [0.05, 0.05, 0.05]] {
            for uv in uvs {
                let base = theme::present_grade(c, uv);
                let dark = going_dark_grade(c, uv, 1.0);
                assert!(
                    dark[2] <= base[2] + EPS,
                    "dark blue {} exceeded base blue {} for {c:?} @ {uv:?}",
                    dark[2],
                    base[2]
                );
            }
        }
    }
}
