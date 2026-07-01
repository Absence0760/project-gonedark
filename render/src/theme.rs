//! The canonical **visual theme** — the single source of truth for the renderer's colour language,
//! type scale, and spacing. Before this module the palette lived as ~15 hand-tuned `const [f32; N]`
//! literals scattered across `lib.rs`, `overlay.rs`, the per-panel HUD modules, and the WGSL
//! shaders, with no shared identity and a documented "retune means editing fifteen files" footgun
//! (see the notes in `shader.wgsl` and `mesh.rs`). This module gathers the load-bearing ones into
//! one named, art-directed palette so the game reads as *intentional* rather than greybox
//! (`docs/roadmap.md` visual-design backlog).
//!
//! ## Identity
//!
//! "Going Dark" is a tactical RTS/FPS hybrid about darkness and divided attention. The palette is
//! built on **deep blue-black inks** with a small, disciplined set of signal accents:
//!  - **factions** read instantly — cool blue (you), hostile red (them), muted grey (neutral);
//!  - the **embodied avatar** is warm amber, the one unit you inhabit;
//!  - **status** colour (health, win/loss, alerts) is reserved for things that change.
//!
//! The base ink/panel/text/amber ramp is deliberately **aligned to the desktop title-shell palette**
//! (`app/src/shell.rs`: `INK`/`PANEL`/`BONE`/`ASH`/`AMBER`) so the out-of-match egui chrome and the
//! in-match `wgpu` HUD finally share one identity instead of drifting apart.
//!
//! ## Float side of invariant #1/#4
//!
//! Every value here is a render-only `f32` colour. Nothing in this module is read by `core`/the sim;
//! a palette retune can never change the per-tick checksum. Colours are normalised sRGB components
//! in `[0, 1]` — the same space the existing shaders and egui already treat them in.
//!
//! ## WGSL
//!
//! WGSL cannot import these Rust consts, so the few colours baked into shader source (the health-bar
//! gradient and selection rim in `shader.wgsl`, the greybox lighting tints in `mesh.wgsl`) carry a
//! doc-comment pointing back here as the source of truth. The values that vary per entity (faction
//! body colour, tracer glow) already travel to the GPU as instance data derived from this module, so
//! those are genuinely shared, not duplicated.

/// A linear-list RGB colour, normalised sRGB components in `[0, 1]`. Matches the `[f32; 3]` the
/// renderer already hands to shaders, so call sites need no conversion.
pub type Rgb = [f32; 3];

/// Construct an RGBA from an [`Rgb`] and an alpha — the common "this panel colour, at this opacity"
/// move. `const` so it can seed other consts.
pub const fn rgba(c: Rgb, a: f32) -> [f32; 4] {
    [c[0], c[1], c[2], a]
}

/// Linearly blend two colours (`t = 0` → `a`, `t = 1` → `b`), component-wise. Used for status ramps
/// (health good→critical) so the ramp endpoints live in the palette, not at the call site.
pub fn mix(a: Rgb, b: Rgb, t: f32) -> Rgb {
    let t = t.clamp(0.0, 1.0);
    [
        a[0] + (b[0] - a[0]) * t,
        a[1] + (b[1] - a[1]) * t,
        a[2] + (b[2] - a[2]) * t,
    ]
}

/// Perceptual relative luminance (Rec. 709 weights) of a colour — used by the tests to assert the
/// text/background ramp keeps a readable lightness ordering, and available to callers that want to
/// pick a legible label colour over a fill.
pub fn luminance(c: Rgb) -> f32 {
    0.2126 * c[0] + 0.7152 * c[1] + 0.0722 * c[2]
}

/// Reference implementation of the full-screen **present-pass grade** in `present.wgsl`'s
/// `fs_present`. The shader applies this once over the whole world scene (before the crisp
/// native-resolution HUD/text chrome is drawn on top). WGSL cannot import Rust, so the grade math is
/// duplicated in the shader — this is the off-GPU twin, kept in lockstep and unit-tested so a grade
/// regression (out-of-range output, a lost vignette, a runaway tint) fails in CI rather than only on
/// screen. `rgb` is the scene colour, `uv` the fullscreen `[0,1]` coordinate (v=0 at the top).
pub fn present_grade(rgb: Rgb, uv: [f32; 2]) -> Rgb {
    fn smoothstep(e0: f32, e1: f32, x: f32) -> f32 {
        let t = ((x - e0) / (e1 - e0)).clamp(0.0, 1.0);
        t * t * (3.0 - 2.0 * t)
    }

    let mut c = rgb;

    // 1. Contrast S-curve (smoothstep about the mid-point), mixed in at a restrained weight.
    let s = [
        c[0] * c[0] * (3.0 - 2.0 * c[0]),
        c[1] * c[1] * (3.0 - 2.0 * c[1]),
        c[2] * c[2] * (3.0 - 2.0 * c[2]),
    ];
    c = mix(c, s, 0.22);

    // 2. Split-tone: cool the shadows (SUBTRACTIVE — pull the warm channels down, never raise blue,
    //    so the grade can't fabricate a "player-blue" pixel the fairness harness would read as intel),
    //    warm the highlights toward amber.
    let l = luminance(c);
    let shadow_w = 1.0 - smoothstep(0.0, 0.55, l);
    let highlight_w = smoothstep(0.5, 1.0, l);
    let shadow_tint = [-0.018, -0.006, 0.0];
    let highlight_tint = [0.028, 0.012, -0.014];
    for i in 0..3 {
        c[i] += shadow_tint[i] * shadow_w + highlight_tint[i] * highlight_w;
    }

    // 3. Desaturate the deepest shadows partially toward their own luminance ("going dark" mood).
    let grey = luminance(c);
    c = mix(c, [grey, grey, grey], shadow_w * 0.12);

    // 4. Smooth radial vignette — darkens only toward the corners.
    let d = [uv[0] - 0.5, uv[1] - 0.5];
    let r = (d[0] * d[0] + d[1] * d[1]).sqrt() * 1.414_213_5;
    let vignette = 1.0 - smoothstep(0.55, 1.15, r) * 0.34;
    for ch in &mut c {
        *ch = (*ch * vignette).clamp(0.0, 1.0);
    }
    c
}

// ---- Base ink / surface ramp (aligned to the title-shell palette) -------------------------------

/// The deepest background — the clear colour behind the world and the darkest scrim. (`INK` #07090C.)
pub const INK: Rgb = [0.027, 0.035, 0.047];
/// A resting panel/card fill — the contextual command panel, the objective card, the post-match
/// summary backing. (`PANEL` #121820, a touch deeper for the in-match HUD.)
pub const PANEL: Rgb = [0.043, 0.055, 0.078];
/// A raised/hover/selected surface — one step lighter than [`PANEL`] for buttons and active rows.
pub const PANEL_RAISED: Rgb = [0.082, 0.098, 0.130];
/// A panel's border/rim — a lighter hairline that lifts a card off the background.
pub const RIM: Rgb = [0.16, 0.19, 0.26];
/// A faint structural hairline — minor grid lines, bar tracks, dividers.
pub const HAIRLINE: Rgb = [0.10, 0.13, 0.18];

// ---- Type / foreground --------------------------------------------------------------------------

/// Primary text — the high-contrast "bone" used for labels and headings. (`BONE` #E7ECEF.)
pub const BONE: Rgb = [0.906, 0.925, 0.937];
/// Secondary text — the muted "ash" for sub-labels, captions, and disabled rows. (`ASH` #8A949C.)
pub const ASH: Rgb = [0.541, 0.580, 0.612];
/// Dimmed/unavailable text — below [`ASH`], for unaffordable build options and inactive entries.
pub const MUTED: Rgb = [0.38, 0.41, 0.46];

// ---- Signal accent ------------------------------------------------------------------------------

/// The warm signal accent — primary calls to action, the build stamp, focus highlights, and the
/// embodied avatar family. (`AMBER` #E0791F, nudged warmer for the in-match HUD.)
pub const AMBER: Rgb = [0.92, 0.55, 0.16];

// ---- Factions -----------------------------------------------------------------------------------

/// You — a cool, confident blue.
pub const PLAYER: Rgb = [0.27, 0.62, 0.96];
/// Them — a hostile red, kept distinct from the health-critical red by being a touch cooler/darker.
pub const ENEMY: Rgb = [0.93, 0.30, 0.24];
/// Unowned — a desaturated grey that recedes against both faction colours.
pub const NEUTRAL: Rgb = [0.55, 0.56, 0.62];
/// The one unit you inhabit — warm amber, the embodiment signature. Brighter than [`AMBER`] so the
/// possessed token pops out of the command view.
pub const AVATAR: Rgb = [1.0, 0.82, 0.24];

// ---- Status -------------------------------------------------------------------------------------

/// Full health / victory / "good" — a clear green.
pub const STATUS_GOOD: Rgb = [0.30, 0.86, 0.36];
/// Drained health / confirmed-desync / "bad" — a hot red. The health bar ramps [`STATUS_CRIT`] →
/// [`STATUS_GOOD`] so a near-dead unit glows red even at a sliver of fill.
pub const STATUS_CRIT: Rgb = [0.92, 0.22, 0.16];
/// Spent/empty segment of a bar — a desaturated charcoal, deliberately off pure red so an empty
/// health segment can't be misread as an enemy-faction body at small command-view sizes.
pub const STATUS_LOST: Rgb = [0.17, 0.18, 0.22];

// ---- Data-bar accents (post-match summary) ------------------------------------------------------

/// Kills / damage dealt.
pub const DATA_KILLS: Rgb = [0.42, 0.66, 0.92];
/// Territory held.
pub const DATA_TERRITORY: Rgb = [0.52, 0.82, 0.46];
/// Resources banked.
pub const DATA_RESOURCE: Rgb = [0.95, 0.74, 0.32];

// ---- Type scale (NDC heights) -------------------------------------------------------------------
//
// The in-match HUD lays out in NDC ([-1,1], so the full screen height is 2.0). A small, fixed type
// scale keeps label sizes consistent across panels instead of each module picking an ad-hoc glyph
// height. These are *glyph cell heights* in NDC for the `text` pass.

/// Heading / banner text (post-match result, mission banner).
pub const TYPE_HEADING: f32 = 0.085;
/// Section title (panel headers, the objective title).
pub const TYPE_TITLE: f32 = 0.055;
/// Body / row label (the default HUD label size).
pub const TYPE_BODY: f32 = 0.040;
/// Caption / secondary detail (costs, sub-labels).
pub const TYPE_CAPTION: f32 = 0.030;

// ---- Spacing scale (NDC) ------------------------------------------------------------------------

/// Tight inset — padding inside a row, gap between a glyph run and its icon.
pub const SPACE_TIGHT: f32 = 0.015;
/// Standard inset — a panel's inner padding, the step between stacked rows.
pub const SPACE_STD: f32 = 0.030;
/// Loose inset — the gap between distinct panel groups, the screen-edge margin.
pub const SPACE_LOOSE: f32 = 0.060;

#[cfg(test)]
mod tests {
    use super::*;

    /// Every palette colour must be a valid normalised component in `[0, 1]` — a value outside that
    /// range would clip unpredictably on the way to the swapchain.
    #[test]
    fn all_palette_colours_are_in_unit_range() {
        let all: &[Rgb] = &[
            INK, PANEL, PANEL_RAISED, RIM, HAIRLINE, BONE, ASH, MUTED, AMBER, PLAYER, ENEMY,
            NEUTRAL, AVATAR, STATUS_GOOD, STATUS_CRIT, STATUS_LOST, DATA_KILLS, DATA_TERRITORY,
            DATA_RESOURCE,
        ];
        for c in all {
            for &ch in c {
                assert!((0.0..=1.0).contains(&ch), "colour channel {ch} out of [0,1] in {c:?}");
            }
        }
    }

    /// The surface→text ramp must keep a strictly increasing lightness so labels stay readable over
    /// their backings: ink (darkest) < panel < raised < ash (secondary text) < bone (primary text).
    #[test]
    fn surface_to_text_ramp_increases_in_luminance() {
        let ramp = [INK, PANEL, PANEL_RAISED, ASH, BONE];
        for w in ramp.windows(2) {
            assert!(
                luminance(w[0]) < luminance(w[1]),
                "luminance ramp not strictly increasing: {:?} !< {:?}",
                w[0],
                w[1]
            );
        }
        // Muted body text sits below secondary (ash) but is still lighter than the panel it labels.
        assert!(luminance(PANEL) < luminance(MUTED));
        assert!(luminance(MUTED) < luminance(ASH));
    }

    /// The three faction colours (and the avatar) must be mutually distinguishable — a minimum
    /// squared-distance floor so "mine / theirs / neutral / possessed" never collide on screen.
    #[test]
    fn faction_colours_are_mutually_distinct() {
        fn dist2(a: Rgb, b: Rgb) -> f32 {
            (0..3).map(|i| (a[i] - b[i]).powi(2)).sum()
        }
        let fams = [PLAYER, ENEMY, NEUTRAL, AVATAR];
        for i in 0..fams.len() {
            for j in (i + 1)..fams.len() {
                assert!(
                    dist2(fams[i], fams[j]) > 0.05,
                    "faction colours too close: {:?} vs {:?}",
                    fams[i],
                    fams[j]
                );
            }
        }
    }

    /// `mix` clamps `t` and hits its endpoints exactly.
    #[test]
    fn mix_clamps_and_hits_endpoints() {
        assert_eq!(mix(STATUS_CRIT, STATUS_GOOD, 0.0), STATUS_CRIT);
        assert_eq!(mix(STATUS_CRIT, STATUS_GOOD, 1.0), STATUS_GOOD);
        assert_eq!(mix(STATUS_CRIT, STATUS_GOOD, -1.0), STATUS_CRIT);
        assert_eq!(mix(STATUS_CRIT, STATUS_GOOD, 2.0), STATUS_GOOD);
        let mid = mix([0.0, 0.0, 0.0], [1.0, 0.5, 0.25], 0.5);
        assert_eq!(mid, [0.5, 0.25, 0.125]);
    }

    /// `rgba` carries the rgb through and appends alpha.
    #[test]
    fn rgba_appends_alpha() {
        assert_eq!(rgba(PANEL, 0.8), [PANEL[0], PANEL[1], PANEL[2], 0.8]);
    }

    /// The present grade must keep every output channel in `[0,1]` for any in-range input — a value
    /// outside that range would clip unpredictably on the way to the swapchain. Sweep a grid of
    /// colours across the whole frame (centre + corners).
    #[test]
    fn present_grade_stays_in_unit_range() {
        let uvs = [[0.5, 0.5], [0.0, 0.0], [1.0, 1.0], [0.0, 1.0], [1.0, 0.0]];
        for r in 0..=4 {
            for g in 0..=4 {
                for b in 0..=4 {
                    let c = [r as f32 / 4.0, g as f32 / 4.0, b as f32 / 4.0];
                    for uv in uvs {
                        let out = present_grade(c, uv);
                        for &ch in &out {
                            assert!(
                                (0.0..=1.0).contains(&ch),
                                "graded channel {ch} out of [0,1] for {c:?} @ {uv:?}"
                            );
                        }
                    }
                }
            }
        }
    }

    /// The vignette must darken the corners relative to the centre for the same colour — the whole
    /// point of the pass is a focused frame. A neutral mid-grey at the extreme corner reads dimmer
    /// than at screen centre.
    #[test]
    fn present_grade_vignette_darkens_corners() {
        let mid = [0.5, 0.5, 0.5];
        let centre = luminance(present_grade(mid, [0.5, 0.5]));
        let corner = luminance(present_grade(mid, [1.0, 1.0]));
        assert!(corner < centre, "corner {corner} not darker than centre {centre}");
    }

    /// The split-tone must push the identity mid-grey warm-ward at the bright end and cool-ward at the
    /// dark end — the cohesion lever. At screen centre (no vignette) a light grey ends up with more
    /// red than blue; a dark grey ends up with more blue than red.
    #[test]
    fn present_grade_split_tone_direction() {
        let light = present_grade([0.8, 0.8, 0.8], [0.5, 0.5]);
        assert!(light[0] > light[2], "highlights not warmed: {light:?}");
        let dark = present_grade([0.12, 0.12, 0.12], [0.5, 0.5]);
        assert!(dark[2] > dark[0], "shadows not cooled: {dark:?}");
    }

    /// The type scale is strictly descending (heading > title > body > caption) — a guard against a
    /// future edit accidentally inverting two sizes.
    #[test]
    fn type_scale_is_descending() {
        assert!(TYPE_HEADING > TYPE_TITLE);
        assert!(TYPE_TITLE > TYPE_BODY);
        assert!(TYPE_BODY > TYPE_CAPTION);
    }

    /// Spacing scale is strictly ascending.
    #[test]
    fn spacing_scale_is_ascending() {
        assert!(SPACE_TIGHT < SPACE_STD);
        assert!(SPACE_STD < SPACE_LOOSE);
    }
}
