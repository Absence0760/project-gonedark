//! The shell's reusable egui component library and shared layout constants: menu/chip/confirm
//! buttons, the framed cards, the banner/section/divider primitives, the fixed-width over-backdrop
//! screen scaffold, keycap chips, and the selectable card row + status chip. Every screen composes
//! these, so a look/behaviour change lands in one place. Mostly `Ui`-bound glue (exempt from unit
//! tests); the one pure decision here, [`confirm_click`], is tested.

use crate::shell::theme::*;

/// The shared "menu button" width — every primary/secondary action button is this wide so the action
/// stacks line up into a clean column.
pub(crate) const MENU_BUTTON_W: f32 = 256.0;

/// The shared width of every over-backdrop screen card (Settings / Profile / Army-select / About /
/// mode-mission-briefing). One number so the whole shell family reads as one system instead of the
/// old per-screen 420/460/500 drift, and — load-bearing — so [`over_backdrop_screen`] can pin the
/// centred content column to a *fixed* width. Without a fixed width, egui's `vertical_centered`
/// lets full-width widgets (sliders, `horizontal` rows) stretch to the window edge while intrinsic
/// -width labels centre, which is the "controls far-left, headings centred, dead middle" bug.
pub(crate) const SHELL_CARD_W: f32 = 480.0;

/// The label column width shared by the two-column key/value rows on the shell screens (Settings
/// sliders/cyclers, Profile identity), so every value control starts at the same x.
pub(crate) const SETTINGS_LABEL_W: f32 = 172.0;

/// Vertical gap between a screen's content and its footer button(s), shared so the footer rhythm
/// reads as one system across the mode/mission/briefing/settings family.
pub(crate) const FOOTER_GAP: f32 = 20.0;

/// How a [`menu_button`] reads in the visual hierarchy: the one amber call-to-action, a neutral
/// secondary, or a de-emphasised tertiary (e.g. QUIT / BACK).
#[derive(Clone, Copy)]
pub(crate) enum Emphasis {
    /// Filled amber, ink text — the single primary action on a screen.
    Primary,
    /// Panel-filled, bone text — a normal secondary action (rides the hover/active fill ramp).
    Secondary,
    /// Panel-filled, ash text — a quieter, lower-stakes action.
    Tertiary,
}

/// The pure state transition for a two-click confirm gate on a destructive button. Given whether
/// the button is currently *armed* (already clicked once), a click returns `(new_armed, fired)`:
/// the first click arms it (`(true, false)`) and relabels to a confirm prompt; a click while armed
/// fires the action and disarms (`(false, true)`). Pure → unit-tested; the egui glue
/// [`confirm_menu_button`] carries the transient armed bit.
pub(crate) fn confirm_click(armed: bool) -> (bool, bool) {
    if armed {
        (false, true)
    } else {
        (true, false)
    }
}

/// A destructive-action button that requires two clicks: the first arms it (relabeling to
/// `confirm_label` in the primary/amber emphasis), the second fires. The armed bit lives in egui's
/// transient memory keyed by `id_salt`, so no host state threading is needed and it clears itself
/// when the screen stops drawing the button. Returns `true` only on the confirming click. Guards the
/// three one-click-wipe actions (gunsmith RESET, Settings RESET DEFAULTS, Profile RESET RECORD) that
/// previously destroyed state with no undo. Glue (needs a `Ui`); the decision is [`confirm_click`].
pub(crate) fn confirm_menu_button(
    ui: &mut egui::Ui,
    id_salt: &str,
    label: &str,
    confirm_label: &str,
    emphasis: Emphasis,
) -> bool {
    let id = ui.make_persistent_id(id_salt);
    let armed = ui.data(|d| d.get_temp::<bool>(id).unwrap_or(false));
    // Armed → show the confirm prompt in amber so the escalated state is unmistakable.
    let shown = if armed { confirm_label } else { label };
    let shown_emphasis = if armed { Emphasis::Primary } else { emphasis };
    let mut fired = false;
    if menu_button(ui, shown, shown_emphasis) {
        let (new_armed, did_fire) = confirm_click(armed);
        ui.data_mut(|d| d.insert_temp(id, new_armed));
        fired = did_fire;
    }
    fired
}

/// Draw one full-width menu button in the shell style and report whether it was clicked. Glue (it
/// needs a live `Ui`), so it's exempt from unit tests — the click→action mapping it feeds is what the
/// pure [`resolve_title_action`](crate::shell::transitions::resolve_title_action) /
/// [`apply_loadout_action`](crate::shell::loadout::apply_loadout_action) seams cover. Only the primary
/// button sets an explicit fill; secondary/tertiary leave the fill to the widget ramp in
/// [`shell_style`](crate::shell::theme::shell_style) so they visibly lift on hover.
pub(crate) fn menu_button(ui: &mut egui::Ui, text: &str, emphasis: Emphasis) -> bool {
    use egui::{Button, RichText};
    let fg = match emphasis {
        Emphasis::Primary => INK,
        Emphasis::Secondary => BONE,
        Emphasis::Tertiary => ASH,
    };
    let mut button =
        Button::new(RichText::new(text).color(fg).size(TYPE_BUTTON)).min_size([MENU_BUTTON_W, 46.0].into());
    if matches!(emphasis, Emphasis::Primary) {
        button = button.fill(AMBER);
    }
    ui.add(button).clicked()
}

/// A short amber accent rule, centred under a heading — the one bit of "brand" line work that ties
/// the title and gunsmith screens together. Pure presentation glue (needs a `Ui`/painter).
pub(crate) fn accent_rule(ui: &mut egui::Ui, width: f32) {
    let (rect, _) = ui.allocate_exact_size(egui::vec2(width, 2.0), egui::Sense::hover());
    ui.painter().rect_filled(rect, egui::CornerRadius::same(1), AMBER);
}

/// The framed "card" the menu/content column sits in — a PANEL fill with a RIM hairline, rounded,
/// with comfortable inner padding. It shrinks to its content, so inside a `vertical_centered` column
/// it renders as a centred panel rather than a full-bleed band. Glue (returns an egui builder).
pub(crate) fn card_frame() -> egui::Frame {
    egui::Frame::default()
        .fill(PANEL)
        .stroke(egui::Stroke::new(1.0, RIM))
        .corner_radius(egui::CornerRadius::same(12))
        .inner_margin(egui::Margin::same(22))
}

/// The title screen's framed card — [`card_frame`] refilled with the translucent [`PANEL_GLASS`] so
/// the live 3D backdrop bleeds faintly through it. Glue (returns an egui builder).
pub(crate) fn glass_card_frame() -> egui::Frame {
    card_frame().fill(PANEL_GLASS)
}

/// A compact secondary "chip" button for the title screen's top-right utility cluster
/// (SETTINGS / PROFILE) — smaller than the full-width [`menu_button`] so it reads as utility chrome
/// rather than a primary action. Rides the [`shell_style`](crate::shell::theme::shell_style) widget
/// ramp (lifts to PANEL_RAISED + an amber rim on hover). Glue (needs a live `Ui`); the click→action
/// mapping it feeds is what the pure
/// [`resolve_title_action`](crate::shell::transitions::resolve_title_action) seam covers. Text-only,
/// uppercase ASCII — never a risky glyph (the file's tofu caution: only default-font glyphs like
/// U+00B7 are trusted).
pub(crate) fn chip_button(ui: &mut egui::Ui, text: &str, width: f32) -> bool {
    use egui::{Button, RichText};
    ui.add(
        Button::new(RichText::new(text).color(BONE).size(TYPE_BODY))
            .min_size([width, 32.0].into()),
    )
    .clicked()
}

/// A centred screen banner — the heading + amber rule treatment the gunsmith/settings/profile/about
/// screens share, so they read as one family. Glue (needs a `Ui`).
pub(crate) fn screen_banner(ui: &mut egui::Ui, title: &str, rule_w: f32) {
    use egui::RichText;
    ui.label(
        RichText::new(title)
            .color(BONE)
            .size(TYPE_HEADING)
            .strong(),
    );
    ui.add_space(8.0);
    accent_rule(ui, rule_w);
    ui.add_space(16.0);
}

/// A left-aligned section sub-heading inside a screen card (e.g. "AUDIO", "CONTROLS"). Glue.
pub(crate) fn section_label(ui: &mut egui::Ui, text: &str) {
    use egui::RichText;
    ui.add_space(6.0);
    ui.label(RichText::new(text).color(ASH).size(TYPE_CAPTION).strong());
    ui.add_space(6.0);
}

/// A hairline `RIM` divider spanning the current row width — the low-emphasis section break for
/// long single-card screens (Settings, About group boundaries). Deliberately `RIM`, not `AMBER`:
/// amber is the lone signal accent (reserved for `accent_rule`/active state), so a generic divider
/// stays quiet chrome. Glue.
pub(crate) fn section_divider(ui: &mut egui::Ui) {
    ui.add_space(4.0);
    let w = ui.available_width();
    let (rect, _) = ui.allocate_exact_size(egui::vec2(w, 1.0), egui::Sense::hover());
    ui.painter().rect_filled(rect, egui::CornerRadius::ZERO, RIM);
    ui.add_space(10.0);
}

/// The transparent full-screen panel the over-backdrop screens (settings/profile/about) sit in, with
/// their content centred in a translucent [`glass_card_frame`]. The central panel paints **no** fill
/// (`Frame::NONE`) so the live 3D backdrop shows through around the card. `build` lays out the card's
/// interior; the whole screen returns whatever `build` produced. Glue.
pub(crate) fn over_backdrop_screen<T>(
    ui: &mut egui::Ui,
    top_frac: f32,
    build: impl FnOnce(&mut egui::Ui) -> T,
) -> T {
    let mut out = None;
    egui::CentralPanel::default()
        .frame(egui::Frame::NONE)
        .show(ui, |ui| {
            let h = ui.available_height();
            // Bound the card to the viewport and let its content scroll if it overflows — so a
            // shrunk window (down to the min inner size) or a growing list (the campaign
            // mission-select) can never push BACK / footer controls off-screen with no way to
            // reach them. A ScrollArea that fits its content shows no scrollbar, so short screens
            // look identical to before.
            let max_card_h = (h * (1.0 - top_frac) - 24.0).max(120.0);
            ui.vertical_centered(|ui| {
                ui.add_space(h * top_frac);
                glass_card_frame().show(ui, |ui| {
                    egui::ScrollArea::vertical()
                        .max_height(max_card_h)
                        .show(ui, |ui| {
                            // Pin the content column to a fixed width. This is the load-bearing half
                            // of the shared layout fix: without it, `vertical_centered` above lets a
                            // full-width widget (a slider, a `horizontal` row) stretch to the window
                            // edge while intrinsic-width labels centre — the "controls far-left,
                            // headings centred, dead middle" bug across every over-backdrop screen.
                            ui.set_width(SHELL_CARD_W);
                            out = Some(build(ui));
                        });
                });
            });
        });
    out.expect("over_backdrop_screen build ran")
}

/// One keybinding rendered as a small rounded "keycap" — [`PANEL_RAISED`] fill, [`RIM`] hairline,
/// bold [`AMBER`] text — so it reads as a physical key. Used only for the literal
/// COMMAND/EMBODIED/GLOBAL rows in [`about_ui`](crate::shell::about::about_ui); the GOING DARK concept
/// block (whose `keys` cell holds a concept name, not a binding) renders as plain text via `about_ui`'s
/// own branch, never a chip.
pub(crate) fn keycap_chip(ui: &mut egui::Ui, text: &str) {
    use egui::{Frame, RichText, Stroke};
    Frame::default()
        .fill(PANEL_RAISED)
        .stroke(Stroke::new(1.0, RIM))
        .corner_radius(egui::CornerRadius::same(4))
        .inner_margin(egui::Margin::symmetric(8, 3))
        .show(ui, |ui| {
            ui.label(RichText::new(text).color(AMBER).size(TYPE_BODY).strong());
        });
}

/// A full-width selectable "card row": fills the list's available width (never shrink-wraps to its
/// content, unlike [`card_frame`]), rides the amber hover ring, and reports whether it was clicked
/// this frame. `enabled == false` disables the click and the ring (a locked row). `content` draws
/// the row interior against the already-full-width `Ui` it's handed. Glue. Used by the mode/mission
/// tiles so both read as one row language instead of a bare button and a double-framed mini-card.
pub(crate) fn selectable_row(
    ui: &mut egui::Ui,
    id_salt: impl std::hash::Hash + std::fmt::Debug,
    enabled: bool,
    content: impl FnOnce(&mut egui::Ui),
) -> bool {
    let id = ui.id().with(id_salt);
    let inner = egui::Frame::default()
        .fill(PANEL)
        .stroke(egui::Stroke::new(1.0, RIM))
        .corner_radius(egui::CornerRadius::same(8))
        .inner_margin(egui::Margin::symmetric(16, 12))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            content(ui);
        });
    let sense = if enabled {
        egui::Sense::click()
    } else {
        egui::Sense::hover()
    };
    let response = ui.interact(inner.response.rect, id, sense);
    if enabled && response.hovered() {
        ui.painter().rect_stroke(
            inner.response.rect,
            egui::CornerRadius::same(8),
            egui::Stroke::new(1.5, AMBER),
            egui::StrokeKind::Outside,
        );
    }
    enabled && response.clicked()
}

/// A small rounded status badge (LOCKED / AVAILABLE / CLEARED · TIER) — [`PANEL_RAISED`] fill, the
/// caller's status colour for both border and text. Sits at the right of a mission row. Glue.
pub(crate) fn status_chip(ui: &mut egui::Ui, text: &str, color: egui::Color32) {
    egui::Frame::default()
        .fill(PANEL_RAISED)
        .stroke(egui::Stroke::new(1.0, color))
        .corner_radius(egui::CornerRadius::same(4))
        .inner_margin(egui::Margin::symmetric(8, 3))
        .show(ui, |ui| {
            ui.label(egui::RichText::new(text).color(color).size(TYPE_CAPTION).strong());
        });
}
