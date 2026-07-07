//! The desktop shell's visual identity: the colour ramp (derived from the renderer theme), the
//! point-size type scale, and the cohesive dark [`egui::Style`] every screen shares. Pure data — no
//! GPU or window — so [`shell_style`] is unit-tested.

pub(crate) const fn rgb8(c: gonedark_render::theme::Rgb) -> egui::Color32 {
    egui::Color32::from_rgb(
        (c[0] * 255.0 + 0.5) as u8,
        (c[1] * 255.0 + 0.5) as u8,
        (c[2] * 255.0 + 0.5) as u8,
    )
}

// Shared-identity ramp — derived straight from the renderer theme.
pub(crate) const INK: egui::Color32 = rgb8(gonedark_render::theme::INK);
pub(crate) const BONE: egui::Color32 = rgb8(gonedark_render::theme::BONE);
pub(crate) const ASH: egui::Color32 = rgb8(gonedark_render::theme::ASH);
pub(crate) const RIM: egui::Color32 = rgb8(gonedark_render::theme::RIM);
// In-match-tuned variants — deliberately the SHELL values (the renderer nudges these deeper/warmer
// for the in-match HUD); kept explicit and pinned by tests. PANEL is the card fill; PANEL_RAISED the
// raised/hover/active surface; AMBER the lone signal accent; MUTED the dimmest legible text.
pub(crate) const PANEL: egui::Color32 = egui::Color32::from_rgb(0x12, 0x18, 0x20);
pub(crate) const AMBER: egui::Color32 = egui::Color32::from_rgb(0xE0, 0x79, 0x1F);
pub(crate) const PANEL_RAISED: egui::Color32 = egui::Color32::from_rgb(0x1B, 0x25, 0x31);
pub(crate) const MUTED: egui::Color32 = egui::Color32::from_rgb(0x61, 0x68, 0x75);
// A semi-opaque PANEL for chrome floated over the live 3D title backdrop: the PANEL hue at ~88%
// alpha (224/255) so the moving sky reads faintly behind a card without costing text legibility.
// Only the title screen (which has a backdrop behind it) uses it; the loadout screen keeps the
// opaque PANEL. `Color32` stores PREMULTIPLIED alpha and only `from_rgba_premultiplied` is `const`,
// so the channels here are PANEL (0x12/0x18/0x20) already multiplied by 224/255 (→ 16/21/28); this
// is the const-fn equivalent of `from_rgba_unmultiplied(0x12, 0x18, 0x20, 224)`.
pub(crate) const PANEL_GLASS: egui::Color32 =
    egui::Color32::from_rgba_premultiplied(16, 21, 28, 224);

// The desktop-shell type scale (egui point sizes). One small, fixed ramp so every screen shares a
// heading/body/caption hierarchy instead of each call site picking an ad-hoc glyph size — the
// pixel-space analogue of theme.rs's NDC `TYPE_*` scale. `DISPLAY` is the title hero; `HEADING` the
// per-screen banner (GUNSMITH); `BUTTON`/`BODY`/`CAPTION` the rest.
pub(crate) const TYPE_DISPLAY: f32 = 52.0;
pub(crate) const TYPE_HEADING: f32 = 30.0;
/// A large standalone numeral (the Profile record, post-match tallies) — bigger than `SUBHEAD` so a
/// stat reads as a figure with a caption, not body text that happens to be a number.
pub(crate) const TYPE_STAT: f32 = 28.0;
pub(crate) const TYPE_SUBHEAD: f32 = 16.0;
pub(crate) const TYPE_BUTTON: f32 = 16.0;
pub(crate) const TYPE_BODY: f32 = 14.0;
pub(crate) const TYPE_CAPTION: f32 = 12.0;

/// Build the shell's cohesive dark [`egui::Style`] — the single source of truth for the title /
/// gunsmith / settings chrome's look (fills, widget ramp, corner radii, spacing, and the
/// heading->caption type scale). Pure data: `egui::Style`/`Visuals` are plain structs with no GPU or
/// window, so this is unit-tested below (unlike the [`EguiShell`] glue that *applies* it). Keeping it
/// pure also means a retune is one function, asserted by tests, rather than scattered `set_*` calls.
pub(crate) fn shell_style() -> egui::Style {
    use egui::{CornerRadius, FontFamily, FontId, Stroke, TextStyle};

    let mut style = egui::Style::default();
    let mut v = egui::Visuals::dark();

    // Surfaces: ink behind everything, panel for cards, amber as the lone signal accent.
    v.panel_fill = INK;
    v.window_fill = PANEL;
    v.window_stroke = Stroke::new(1.0, RIM);
    v.window_corner_radius = CornerRadius::same(10);
    v.faint_bg_color = PANEL;
    v.extreme_bg_color = INK;
    v.hyperlink_color = AMBER;
    // The selection fill is AMBER at ~38% alpha — derived from the const, not a re-typed hex, so it
    // tracks any AMBER retune (was a duplicated 0xE0/0x79/0x1F literal).
    v.selection.bg_fill =
        egui::Color32::from_rgba_unmultiplied(AMBER.r(), AMBER.g(), AMBER.b(), 96);
    v.selection.stroke = Stroke::new(1.0, AMBER);
    // Fill the slider track up to the handle (in the selection amber) so a slider reads its value
    // at a glance instead of as a bare hairline with a floating knob.
    v.slider_trailing_fill = true;

    // The widget interaction ramp: a button at rest sits on PANEL with a RIM hairline; hover/active
    // lift it to PANEL_RAISED, ring it in amber, and nudge it out by a pixel for tactile feedback.
    // Secondary buttons (no explicit fill) ride this ramp directly, so their fill changes on hover;
    // the primary (amber-filled) button keeps its fill but still gains the amber rim + expansion.
    let radius = CornerRadius::same(6);
    let w = &mut v.widgets;

    w.noninteractive.bg_fill = PANEL;
    w.noninteractive.weak_bg_fill = PANEL;
    w.noninteractive.bg_stroke = Stroke::new(1.0, RIM);
    w.noninteractive.fg_stroke = Stroke::new(1.0, BONE);
    w.noninteractive.corner_radius = radius;

    w.inactive.bg_fill = PANEL;
    w.inactive.weak_bg_fill = PANEL;
    w.inactive.bg_stroke = Stroke::new(1.0, RIM);
    w.inactive.fg_stroke = Stroke::new(1.0, BONE);
    w.inactive.corner_radius = radius;
    w.inactive.expansion = 0.0;

    w.hovered.bg_fill = PANEL_RAISED;
    w.hovered.weak_bg_fill = PANEL_RAISED;
    w.hovered.bg_stroke = Stroke::new(1.0, AMBER);
    w.hovered.fg_stroke = Stroke::new(1.5, BONE);
    w.hovered.corner_radius = radius;
    w.hovered.expansion = 1.0;

    w.active.bg_fill = PANEL_RAISED;
    w.active.weak_bg_fill = PANEL_RAISED;
    w.active.bg_stroke = Stroke::new(1.5, AMBER);
    w.active.fg_stroke = Stroke::new(1.5, BONE);
    w.active.corner_radius = radius;
    w.active.expansion = 1.0;

    // Open menus/combos mirror the pressed look (WidgetVisuals is Copy).
    w.open = w.active;

    style.visuals = v;

    // Generous, even spacing so rows and buttons breathe.
    style.spacing.item_spacing = egui::vec2(10.0, 8.0);
    style.spacing.button_padding = egui::vec2(16.0, 9.0);
    // A real slider track length. egui's default (~100px) collapses the track to a stub next to its
    // value box — the "tiny sliders on the far left" in the Settings screenshot. A fixed, generous
    // width lets the audio/sensitivity/FOV sliders read as proper controls inside the shell card.
    style.spacing.slider_width = 200.0;
    // Checkbox / radio glyphs at egui's 14px default all but vanish on the dark card (a 1px RIM
    // outline on PANEL). Larger icons keep the toggle targets legible and finger-friendly.
    style.spacing.icon_width = 20.0;
    style.spacing.icon_width_inner = 12.0;
    style.spacing.icon_spacing = 8.0;
    // A solid, always-drawn scrollbar: when a card's content overflows (Settings on a short
    // window), the default hover-only floating bar leaves no visible cue that more rows exist
    // below the fold.
    style.spacing.scroll = egui::style::ScrollStyle::solid();

    // The default text styles follow the scale. Per-widget `RichText::size`/`color` still override
    // these where a screen wants the title hero or an amber readout, but unstyled text is consistent.
    style.text_styles = [
        (
            TextStyle::Heading,
            FontId::new(TYPE_HEADING, FontFamily::Proportional),
        ),
        (
            TextStyle::Body,
            FontId::new(TYPE_BODY, FontFamily::Proportional),
        ),
        (
            TextStyle::Button,
            FontId::new(TYPE_BUTTON, FontFamily::Proportional),
        ),
        (
            TextStyle::Small,
            FontId::new(TYPE_CAPTION, FontFamily::Proportional),
        ),
        (
            TextStyle::Monospace,
            FontId::new(TYPE_BODY, FontFamily::Monospace),
        ),
    ]
    .into();

    style
}
