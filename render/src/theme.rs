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

/// Rec. 709 weighted sum of the **gamma-encoded** sRGB channels — a cheap lightness proxy for
/// **ordering only** (which of two colours reads lighter), used by the surface→text ramp guard and
/// to pick the vignette/split-tone direction in [`present_grade`]. This is **gamma-space ordering
/// only, NOT a WCAG contrast metric** — it skips the mandatory sRGB linearisation, so it reads far
/// too bright for dim colours and will silently pass unreadable text pairs. For any legibility /
/// accessibility decision use [`contrast_ratio`], which linearises first per the WCAG spec.
pub fn luminance(c: Rgb) -> f32 {
    0.2126 * c[0] + 0.7152 * c[1] + 0.0722 * c[2]
}

/// WCAG relative luminance of an sRGB colour: linearise each channel (undo the sRGB transfer
/// function) before applying the Rec. 709 weights. This is the step [`luminance`] omits — and the
/// reason gamma-space weights read too bright for dim colours and silently pass unreadable pairs.
/// No external crate: the transfer function is implemented inline with `powf` (floats are fine here
/// — this is the render crate; invariant #1 forbids floats only in `core`/the sim, not in render).
fn relative_luminance(c: Rgb) -> f32 {
    fn lin(ch: f32) -> f32 {
        if ch <= 0.040_45 {
            ch / 12.92
        } else {
            ((ch + 0.055) / 1.055).powf(2.4)
        }
    }
    0.2126 * lin(c[0]) + 0.7152 * lin(c[1]) + 0.0722 * lin(c[2])
}

/// The **WCAG 2.x contrast ratio** between two colours, `(L_lighter + 0.05) / (L_darker + 0.05)`
/// over their [`relative_luminance`]. Ranges from `1.0` (identical) to `21.0` (black on white), and
/// is symmetric in its arguments. Use this — not [`luminance`] — for every text-legibility check:
/// WCAG AA wants **≥ 4.5:1** for normal text and **≥ 3:1** for large text / UI components.
pub fn contrast_ratio(a: Rgb, b: Rgb) -> f32 {
    let (la, lb) = (relative_luminance(a), relative_luminance(b));
    let (lo, hi) = if la < lb { (la, lb) } else { (lb, la) };
    (hi + 0.05) / (lo + 0.05)
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
/// Sits on [`PANEL`]/[`INK`] (the command-panel fill and the darkest scrim); lightened from the
/// original `#616875` so it actually clears WCAG AA (≥ 4.5:1) against those surfaces — the old value
/// managed only ~3.5:1 by the linearised metric (see `wcag_contrast_meets_aa_for_used_text_pairs`).
pub const MUTED: Rgb = [0.46, 0.49, 0.54];

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

// ---- Colorblind-safe palette ramps (WS-D accessibility, invariant #6) ---------------------------
//
// Faction identity ("mine / theirs / neutral / the one I possess") rests on hue in the default
// palette: cool blue vs hostile red vs grey vs amber. For a colour-vision-deficient (CVD) player
// that hue split can collapse — red↔green under protanopia/deuteranopia, blue↔yellow under
// tritanopia — which is unfair on a game whose whole read is "whose unit is that". The going-dark
// alert HUD already carries redundant SHAPE + a luminance-spread palette + optional CVD text labels
// (`hud.rs`); this is the *other* half — an opt-in alternate faction ramp, chosen so the four
// identity colours stay mutually separable *after* the relevant dichromacy is simulated.
//
// The ramps are picked from the Okabe-Ito CVD-safe qualitative set (blue / orange / yellow for the
// red-green modes; a red-green-axis set for tritanopia, whose deficiency spares that axis). This is
// **presentation only** — the swap changes only which `f32` colour the renderer bakes into an
// instance; it never reaches `core`/the sim or the per-tick checksum (invariant #1/#4).

/// The hue-carrying identity colours the renderer bakes per unit — the subset a colourblind ramp
/// swaps. Everything else in the palette (ink/panel/text ramp) is neutral/luminance-ordered and does
/// not rely on hue, so it is shared across modes. A plain `Copy` value so a caller can hold the
/// active palette and hand its fields to `faction_color_in`.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct Palette {
    /// You — the local commander's units.
    pub player: Rgb,
    /// Them — the hostile faction.
    pub enemy: Rgb,
    /// Unowned / neutral.
    pub neutral: Rgb,
    /// The one unit you inhabit (embodiment signature).
    pub avatar: Rgb,
}

impl Palette {
    /// The shipped default palette — the named [`PLAYER`]/[`ENEMY`]/[`NEUTRAL`]/[`AVATAR`] consts.
    /// `palette(PaletteMode::Off)` returns exactly this, so the default look is byte-identical.
    pub const DEFAULT: Palette = Palette {
        player: PLAYER,
        enemy: ENEMY,
        neutral: NEUTRAL,
        avatar: AVATAR,
    };

    /// Red-green (protanopia / deuteranopia) safe ramp. Blue + orange + yellow are the classic
    /// Okabe-Ito pairing that survives both red-green dichromacies; neutral stays a mid grey. Avatar
    /// (yellow) separates from enemy (orange) chiefly by luminance, which red-green CVD preserves.
    pub const CVD_REDGREEN: Palette = Palette {
        player: [0.00, 0.45, 0.70],  // Okabe-Ito blue
        enemy: [0.90, 0.52, 0.00],   // orange (warm "warning", CVD-distinct from blue)
        neutral: [0.60, 0.60, 0.62], // mid grey
        avatar: [0.95, 0.90, 0.25],  // bright yellow — highest luminance, the possessed unit
    };

    /// Tritanopia (blue-yellow deficiency) safe ramp. Tritanopia SPARES the red-green axis, so the
    /// ramp leans on it: green player vs red enemy vs grey neutral, with a near-white bright avatar
    /// that separates from all three by luminance (white is stable under tritanopia).
    pub const CVD_TRITAN: Palette = Palette {
        player: [0.15, 0.60, 0.30],  // green (red-green axis, spared by tritanopia)
        enemy: [0.90, 0.15, 0.20],   // red
        neutral: [0.60, 0.60, 0.62], // mid grey
        avatar: [0.96, 0.92, 0.90],  // bright near-white — luminance-separated from the rest
    };
}

/// The player's colourblind-palette choice (Settings → Accessibility). `Off` is the shipped hue
/// palette; the three CVD modes swap in an alternate faction ramp tuned for that deficiency. Stored
/// by stable ordinal for persistence (the [`Self::index`]/[`Self::from_index`] pair), mirroring the
/// shell's `QualityChoice`/`FactionPref`. Presentation only — never a sim input.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum PaletteMode {
    /// The shipped hue palette (blue / red / grey / amber).
    #[default]
    Off,
    /// Red-green deficiency (the most common CVD) — the blue/orange ramp.
    Deuteranopia,
    /// The other red-green deficiency — same blue/orange ramp (both spare the blue-yellow axis).
    Protanopia,
    /// Blue-yellow deficiency — the red-green-axis ramp.
    Tritanopia,
}

impl PaletteMode {
    /// Every mode, in the stable cycle + persisted-ordinal order.
    pub const ALL: [PaletteMode; 4] = [
        PaletteMode::Off,
        PaletteMode::Deuteranopia,
        PaletteMode::Protanopia,
        PaletteMode::Tritanopia,
    ];

    /// The on-screen label for the Settings cycler.
    pub fn label(self) -> &'static str {
        match self {
            PaletteMode::Off => "Off",
            PaletteMode::Deuteranopia => "Deuteranopia (red-green)",
            PaletteMode::Protanopia => "Protanopia (red-green)",
            PaletteMode::Tritanopia => "Tritanopia (blue-yellow)",
        }
    }

    /// The next mode, wrapping — what the Settings cycler advances to.
    pub fn next(self) -> PaletteMode {
        let i = self.index();
        Self::ALL[(i + 1) % Self::ALL.len()]
    }

    /// This mode's stable index in [`Self::ALL`] — the persisted ordinal.
    pub fn index(self) -> usize {
        Self::ALL.iter().position(|&m| m == self).unwrap_or(0)
    }

    /// The mode at persisted index `i`, or [`PaletteMode::Off`] for an out-of-range ordinal — the
    /// tolerant decode side of [`Self::index`].
    pub fn from_index(i: usize) -> PaletteMode {
        Self::ALL.get(i).copied().unwrap_or(PaletteMode::Off)
    }
}

/// Select the active faction [`Palette`] for a [`PaletteMode`] — the pure, unit-tested selection
/// seam. `Off` returns [`Palette::DEFAULT`] (byte-identical to the shipped look); the two red-green
/// modes share [`Palette::CVD_REDGREEN`]; tritanopia gets [`Palette::CVD_TRITAN`].
pub fn palette(mode: PaletteMode) -> Palette {
    match mode {
        PaletteMode::Off => Palette::DEFAULT,
        PaletteMode::Deuteranopia | PaletteMode::Protanopia => Palette::CVD_REDGREEN,
        PaletteMode::Tritanopia => Palette::CVD_TRITAN,
    }
}

/// A dichromacy to simulate — the three single-cone-missing forms of colour blindness.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum CvdSim {
    /// Missing L-cones (red-deficient).
    Protanopia,
    /// Missing M-cones (green-deficient).
    Deuteranopia,
    /// Missing S-cones (blue-deficient).
    Tritanopia,
}

/// Simulate how an sRGB colour `c` appears to a dichromat of type `sim`. Uses the standard
/// full-severity sRGB-space dichromacy matrices (the Viénot/Brettel-derived approximations used by
/// the common colourblindness simulators). Presentation-side `f32` math (invariant #1: floats live
/// only in rendering); its only consumer is the accessibility ramp tests, which assert the CVD ramps
/// stay mutually separable *after* this projection — i.e. that a CVD player can still tell the
/// factions apart. Output is clamped back into `[0, 1]`.
pub fn simulate_cvd(c: Rgb, sim: CvdSim) -> Rgb {
    let m: [[f32; 3]; 3] = match sim {
        CvdSim::Protanopia => [
            [0.567, 0.433, 0.000],
            [0.558, 0.442, 0.000],
            [0.000, 0.242, 0.758],
        ],
        CvdSim::Deuteranopia => [
            [0.625, 0.375, 0.000],
            [0.700, 0.300, 0.000],
            [0.000, 0.300, 0.700],
        ],
        CvdSim::Tritanopia => [
            [0.950, 0.050, 0.000],
            [0.000, 0.433, 0.567],
            [0.000, 0.475, 0.525],
        ],
    };
    let mut out = [0.0f32; 3];
    for (i, row) in m.iter().enumerate() {
        out[i] = (row[0] * c[0] + row[1] * c[1] + row[2] * c[2]).clamp(0.0, 1.0);
    }
    out
}

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
        // Muted body text sits below secondary (ash) but is still lighter than the panel it labels,
        // with a comfortable gap either side so the dim/secondary tiers stay visually distinct.
        assert!(luminance(PANEL) < luminance(MUTED));
        assert!(luminance(MUTED) < luminance(ASH));
        assert!(
            luminance(ASH) - luminance(MUTED) > 0.05,
            "muted collapsed into ash (gap {})",
            luminance(ASH) - luminance(MUTED)
        );
    }

    /// The real accessibility guard: every text/surface pair the HUD actually draws must clear WCAG
    /// AA, measured with the *linearised* [`contrast_ratio`] (the metric [`luminance`] can't give).
    /// [`BONE`] (primary) and [`ASH`] (secondary) clear 4.5:1 against all three surfaces; [`MUTED`]
    /// (dimmed) clears 4.5:1 against its real backings — the [`PANEL`] command-panel fill and the
    /// [`INK`] scrim. `MUTED` is *not* placed on [`PANEL_RAISED`] (raised rows carry active/affordable
    /// `BONE` text), but we still assert it clears the 3:1 large-text / UI-component floor there as a
    /// defensive check. Without the linearisation fix `MUTED` failed this at ~3.5:1.
    #[test]
    fn wcag_contrast_meets_aa_for_used_text_pairs() {
        const AA_TEXT: f32 = 4.5; // normal-text floor
        const AA_UI: f32 = 3.0; // large-text / UI-component floor
        let surfaces = [("INK", INK), ("PANEL", PANEL), ("PANEL_RAISED", PANEL_RAISED)];

        for (sn, s) in surfaces {
            for (tn, t) in [("BONE", BONE), ("ASH", ASH)] {
                let cr = contrast_ratio(t, s);
                assert!(cr >= AA_TEXT, "{tn} on {sn}: {cr:.3}:1 < {AA_TEXT}:1 (WCAG AA)");
            }
        }

        // MUTED on its actual surfaces — normal-text AA.
        for (sn, s) in [("PANEL", PANEL), ("INK", INK)] {
            let cr = contrast_ratio(MUTED, s);
            assert!(cr >= AA_TEXT, "MUTED on {sn}: {cr:.3}:1 < {AA_TEXT}:1 (WCAG AA)");
        }
        // MUTED is not drawn on the raised surface; hold the 3:1 UI-component floor there anyway.
        let cr_raised = contrast_ratio(MUTED, PANEL_RAISED);
        assert!(
            cr_raised >= AA_UI,
            "MUTED on PANEL_RAISED: {cr_raised:.3}:1 < {AA_UI}:1 (UI-component floor)"
        );
    }

    /// [`contrast_ratio`] sanity: black on white is the WCAG maximum ~21:1, identical colours are
    /// 1:1, and the ratio is symmetric in its two arguments (it must not matter which is background).
    #[test]
    fn contrast_ratio_extremes_and_symmetry() {
        const WHITE: Rgb = [1.0, 1.0, 1.0];
        const BLACK: Rgb = [0.0, 0.0, 0.0];
        let bw = contrast_ratio(WHITE, BLACK);
        assert!((bw - 21.0).abs() < 0.05, "white/black not ~21:1: {bw}");
        assert!((contrast_ratio(BONE, BONE) - 1.0).abs() < 1e-6, "identical not 1:1");
        // Symmetric: swapping the pair leaves the ratio unchanged.
        assert!((contrast_ratio(WHITE, BLACK) - contrast_ratio(BLACK, WHITE)).abs() < 1e-6);
        assert!((contrast_ratio(MUTED, PANEL) - contrast_ratio(PANEL, MUTED)).abs() < 1e-6);
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

    // ---- colourblind-safe palette ramps (WS-D) ----

    fn dist2(a: Rgb, b: Rgb) -> f32 {
        (0..3).map(|i| (a[i] - b[i]).powi(2)).sum()
    }

    /// `palette(Off)` must be byte-identical to the shipped named consts, so enabling then disabling
    /// the accessibility ramp returns the default look exactly (no drift).
    #[test]
    fn off_palette_matches_the_shipped_consts() {
        let p = palette(PaletteMode::Off);
        assert_eq!(p, Palette::DEFAULT);
        assert_eq!(p.player, PLAYER);
        assert_eq!(p.enemy, ENEMY);
        assert_eq!(p.neutral, NEUTRAL);
        assert_eq!(p.avatar, AVATAR);
    }

    /// Every colour in every mode's palette stays a valid normalised component in `[0,1]` — an
    /// out-of-range channel would clip unpredictably on the way to the swapchain.
    #[test]
    fn every_palette_mode_is_in_unit_range() {
        for &mode in &PaletteMode::ALL {
            let p = palette(mode);
            for c in [p.player, p.enemy, p.neutral, p.avatar] {
                for ch in c {
                    assert!((0.0..=1.0).contains(&ch), "{mode:?} colour {c:?} out of [0,1]");
                }
            }
        }
    }

    /// The four identity colours must be mutually distinct in EVERY mode (raw, before any CVD
    /// projection) — the same floor the default palette holds, so a CVD ramp never accidentally
    /// collapses two factions for a *non*-CVD player either.
    #[test]
    fn every_palette_modes_factions_are_mutually_distinct() {
        for &mode in &PaletteMode::ALL {
            let p = palette(mode);
            let fams = [p.player, p.enemy, p.neutral, p.avatar];
            for i in 0..fams.len() {
                for j in (i + 1)..fams.len() {
                    assert!(
                        dist2(fams[i], fams[j]) > 0.05,
                        "{mode:?}: faction colours too close: {:?} vs {:?}",
                        fams[i],
                        fams[j]
                    );
                }
            }
        }
    }

    /// The accessibility guarantee, made testable: for each CVD mode, SIMULATE the very deficiency it
    /// targets on its own ramp, and assert the four factions are STILL mutually separable. This is
    /// the property that makes the ramp fair (invariant #6) — a CVD player can still tell "mine /
    /// theirs / neutral / possessed" apart. The floor is lower than the raw one because a dichromat
    /// projection compresses the gamut, but it must stay well clear of a collision.
    #[test]
    fn cvd_ramps_stay_separable_under_their_own_deficiency() {
        const FLOOR: f32 = 0.02; // squared-distance ≈ 0.14 Euclidean, in the compressed dichromat gamut
        for (mode, sim) in [
            (PaletteMode::Deuteranopia, CvdSim::Deuteranopia),
            (PaletteMode::Protanopia, CvdSim::Protanopia),
            (PaletteMode::Tritanopia, CvdSim::Tritanopia),
        ] {
            let p = palette(mode);
            let fams = [p.player, p.enemy, p.neutral, p.avatar]
                .map(|c| simulate_cvd(c, sim));
            for i in 0..fams.len() {
                for j in (i + 1)..fams.len() {
                    assert!(
                        dist2(fams[i], fams[j]) > FLOOR,
                        "{mode:?}: under {sim:?} factions collide: {:?} vs {:?} (d²={})",
                        fams[i],
                        fams[j],
                        dist2(fams[i], fams[j])
                    );
                }
            }
        }
    }

    /// The DEFAULT hue palette FAILS a red-green player (this is *why* the alternate ramp exists):
    /// blue player vs red enemy is fine, but the default enemy-red and status-good-green collapse.
    /// We prove the ramp earns its keep by showing the default player/enemy pair, while distinct,
    /// loses a chunk of its separation under deuteranopia yet the CVD ramp keeps more — a guard that
    /// the alternate ramp is genuinely better, not cosmetic.
    #[test]
    fn cvd_ramp_beats_the_default_under_red_green() {
        let d = palette(PaletteMode::Off);
        let c = palette(PaletteMode::Deuteranopia);
        let sim = CvdSim::Deuteranopia;
        // Player↔enemy separation under simulated deuteranopia, default vs CVD ramp.
        let def_sep = dist2(simulate_cvd(d.player, sim), simulate_cvd(d.enemy, sim));
        let cvd_sep = dist2(simulate_cvd(c.player, sim), simulate_cvd(c.enemy, sim));
        assert!(
            cvd_sep > def_sep,
            "CVD ramp should separate player/enemy better under deuteranopia (cvd={cvd_sep}, default={def_sep})"
        );
    }

    /// `PaletteMode` ordinals round-trip and `next` cycles through every mode back to the start —
    /// the persistence + Settings-cycler contract (mirrors `QualityChoice`).
    #[test]
    fn palette_mode_index_round_trips_and_next_cycles() {
        for (i, &mode) in PaletteMode::ALL.iter().enumerate() {
            assert_eq!(mode.index(), i);
            assert_eq!(PaletteMode::from_index(i), mode);
        }
        // Out-of-range ordinal falls back to Off (tolerant decode).
        assert_eq!(PaletteMode::from_index(999), PaletteMode::Off);
        // `next` walks the whole cycle and returns to the start after ALL.len() steps.
        let mut m = PaletteMode::Off;
        for _ in 0..PaletteMode::ALL.len() {
            m = m.next();
        }
        assert_eq!(m, PaletteMode::Off);
    }

    /// `simulate_cvd` keeps its output in `[0,1]` and leaves a pure grey unchanged (a dichromat still
    /// sees achromatic values), which anchors the projection as sane.
    #[test]
    fn simulate_cvd_is_bounded_and_greys_pass_through() {
        for sim in [CvdSim::Protanopia, CvdSim::Deuteranopia, CvdSim::Tritanopia] {
            for v in [0.0, 0.25, 0.5, 0.75, 1.0] {
                let out = simulate_cvd([v, v, v], sim);
                for ch in out {
                    assert!((0.0..=1.0).contains(&ch));
                    assert!((ch - v).abs() < 1e-3, "grey {v} shifted under {sim:?}: {out:?}");
                }
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
