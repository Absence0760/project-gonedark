//! The shell's reusable egui component library and shared layout constants: menu/chip/confirm
//! buttons, the framed cards, the banner/section/divider primitives, the fixed-width over-backdrop
//! screen scaffold, keycap chips, and the selectable card row + status chip. Every screen composes
//! these, so a look/behaviour change lands in one place. Mostly `Ui`-bound glue (exempt from unit
//! tests); the one pure decision here, [`confirm_click`], is tested.

use crate::shell::theme::*;

/// The shared "menu button" width — the title screen's action stack is this wide, and it is the
/// minimum width a [`footer_button`] can shrink to, so button columns line up across screens.
pub(crate) const MENU_BUTTON_W: f32 = 256.0;

/// The shared width of every over-backdrop screen card (Settings / Profile / Army-select / About /
/// mode-mission-briefing). One number so the whole shell family reads as one system instead of the
/// old per-screen 420/460/500 drift, and — load-bearing — so [`over_backdrop_screen`] can pin the
/// centred content column to a *fixed* width. Without a fixed width, egui's `vertical_centered`
/// lets full-width widgets (sliders, `horizontal` rows) stretch to the window edge while intrinsic
/// -width labels centre, which is the "controls far-left, headings centred, dead middle" bug.
pub(crate) const SHELL_CARD_W: f32 = 480.0;

/// The wide variant of [`SHELL_CARD_W`] for the one screen whose content is a real table (the
/// gunsmith's six fixed-width slot rows). Same card language, one sanctioned wider size — not a
/// per-screen free-for-all.
pub(crate) const SHELL_CARD_WIDE_W: f32 = 800.0;

/// The label column width shared by the two-column key/value rows on the shell screens (Settings
/// sliders/cyclers, Profile identity), so every value control starts at the same x.
pub(crate) const SETTINGS_LABEL_W: f32 = 172.0;

/// Vertical gap between a screen's content and its footer button(s), shared so the footer rhythm
/// reads as one system across the mode/mission/briefing/settings family.
pub(crate) const FOOTER_GAP: f32 = 20.0;

/// How a [`footer_button`] reads in the visual hierarchy: the one amber call-to-action, a neutral
/// secondary, or a de-emphasised tertiary (e.g. QUIT / BACK).
///
/// Shell-wide emphasis policy: **Primary is reserved for the screen's one forward action** (DEPLOY,
/// CAMPAIGN, DONE). BACK is Secondary when it is the sole exit on a screen, and Tertiary when a
/// Primary CTA is present — a back-out must never be the loudest control on screen.
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
    // All three confirm-gated actions live inside a card, so ride the card-width footer button.
    if footer_button(ui, shown, shown_emphasis) {
        let (new_armed, did_fire) = confirm_click(armed);
        ui.data_mut(|d| d.insert_temp(id, new_armed));
        fired = did_fire;
    }
    fired
}

/// Draw one emphasis-styled action button at an explicit width — the body of [`footer_button`].
/// Only the primary button sets an explicit fill; secondary/tertiary leave the fill to the widget
/// ramp in [`shell_style`](crate::shell::theme::shell_style) so they visibly lift on hover. Glue.
fn emphasis_button(ui: &mut egui::Ui, text: &str, emphasis: Emphasis, width: f32) -> bool {
    use egui::{Button, RichText};
    let fg = match emphasis {
        Emphasis::Primary => INK,
        Emphasis::Secondary => BONE,
        Emphasis::Tertiary => ASH,
    };
    let mut button =
        Button::new(RichText::new(text).color(fg).size(TYPE_BUTTON)).min_size([width, 46.0].into());
    if matches!(emphasis, Emphasis::Primary) {
        button = button.fill(AMBER);
    }
    ui.add(button).clicked()
}

/// A card-footer action button spanning the card's full content width (never narrower than
/// [`MENU_BUTTON_W`]), so footer actions align with the rows above them instead of hanging off the
/// left edge at title-menu width. Glue (it needs a live `Ui`), so it's exempt from unit tests — the
/// click→action mapping it feeds is what the pure
/// [`resolve_title_action`](crate::shell::transitions::resolve_title_action) /
/// [`apply_loadout_action`](crate::shell::loadout::apply_loadout_action) seams cover.
pub(crate) fn footer_button(ui: &mut egui::Ui, text: &str, emphasis: Emphasis) -> bool {
    let w = ui.available_width().max(MENU_BUTTON_W);
    emphasis_button(ui, text, emphasis, w)
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
/// (SETTINGS / PROFILE) — smaller than the full-width [`footer_button`] so it reads as utility chrome
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

/// Minimum breathing room between an over-backdrop card and the viewport's top/bottom edges.
pub(crate) const SHELL_CARD_MARGIN: f32 = 40.0;

/// Where an over-backdrop card sits in the leftover vertical space: 0.42 of the slack above it —
/// the optical centre, a touch above geometric centre, where a framed object reads as balanced.
const CARD_OPTICAL_CENTRE: f32 = 0.42;

/// The card's top offset for this frame: optically centred from the height remembered last frame,
/// clamped to [`SHELL_CARD_MARGIN`]; a fixed top band on the very first frame (no height yet). Pure
/// — the one placement decision in the scaffold, extracted from the egui glue so it's unit-tested.
pub(crate) fn over_backdrop_top(viewport_h: f32, last_card_h: Option<f32>) -> f32 {
    match last_card_h {
        Some(card_h) => ((viewport_h - card_h) * CARD_OPTICAL_CENTRE)
            .clamp(SHELL_CARD_MARGIN, (viewport_h - SHELL_CARD_MARGIN).max(SHELL_CARD_MARGIN)),
        None => viewport_h * 0.10,
    }
}

/// The transparent full-screen panel the over-backdrop screens (settings/profile/about) sit in, with
/// their content centred in a translucent [`glass_card_frame`]. The central panel paints **no** fill
/// (`Frame::NONE`) so the live 3D backdrop shows through around the card. `build` lays out the card's
/// interior; the whole screen returns whatever `build` produced. Glue.
///
/// Vertical placement is *optically centred*: immediate mode can't know the card's height before
/// laying it out, so the previous frame's height (remembered per `id_salt`) decides this frame's top
/// offset. The very first frame falls back to a fixed top band and settles one frame later — invisible
/// live; the screenshot harness renders two passes for exactly this reason.
pub(crate) fn over_backdrop_screen<T>(
    ui: &mut egui::Ui,
    id_salt: &str,
    build: impl FnOnce(&mut egui::Ui) -> T,
) -> T {
    over_backdrop_screen_sized(ui, id_salt, SHELL_CARD_W, build)
}

/// [`over_backdrop_screen`] at an explicit card width — only for the sanctioned wide screens
/// ([`SHELL_CARD_WIDE_W`]); everything else takes the default-width wrapper above.
pub(crate) fn over_backdrop_screen_sized<T>(
    ui: &mut egui::Ui,
    id_salt: &str,
    card_w: f32,
    build: impl FnOnce(&mut egui::Ui) -> T,
) -> T {
    let mut out = None;
    egui::CentralPanel::default()
        .frame(egui::Frame::NONE)
        .show(ui, |ui| {
            let h = ui.available_height();
            let height_id = egui::Id::new(("over_backdrop.card_h", id_salt));
            let last_h: Option<f32> = ui.data(|d| d.get_temp(height_id));
            let top = over_backdrop_top(h, last_h);
            // Bound the card to the viewport and let its content scroll if it overflows — so a
            // shrunk window (down to the min inner size) or a growing list (the campaign
            // mission-select) can never push BACK / footer controls off-screen with no way to
            // reach them. A ScrollArea that fits its content shows no scrollbar, so short screens
            // look identical to before. The frame's own margins + stroke are subtracted so the
            // card's painted edge (not just its scroll interior) always lands inside the viewport.
            let frame = glass_card_frame();
            let frame_v = f32::from(frame.inner_margin.top)
                + f32::from(frame.inner_margin.bottom)
                + 2.0 * frame.stroke.width;
            let max_scroll_h = (h - top - SHELL_CARD_MARGIN - frame_v).max(120.0);
            ui.add_space(top);
            // Centre a fixed-width card BY HAND — pad the left by half the leftover width, then
            // allocate a region of exactly `SHELL_CARD_W`. Leaning on `vertical_centered` here does
            // NOT work: combined with the inner `ScrollArea` it painted the glass frame full-width and
            // top-left-anchored instead of wrapping a centred column (the frame's rect desynced from
            // the content's). Pinning the card region's width makes the frame wrap a centred,
            // fixed-width column whose content fills it — deterministic, no layout-interaction guesswork.
            let pad = ((ui.available_width() - card_w) * 0.5).max(0.0);
            ui.horizontal(|ui| {
                ui.add_space(pad);
                // `allocate_ui` inherits the parent layout — and the parent here is the centring
                // `horizontal`, which would lay the card's interior out left-to-right. Force a
                // top-down column so the banner / rows / footer stack vertically inside the card.
                ui.allocate_ui_with_layout(
                    egui::vec2(card_w, max_scroll_h),
                    egui::Layout::top_down(egui::Align::Min),
                    |ui| {
                        let card = frame.show(ui, |ui| {
                            egui::ScrollArea::vertical()
                                .max_height(max_scroll_h)
                                // Fill the card width (don't shrink horizontally), but shrink to content
                                // height so a short screen shows no scrollbar.
                                .auto_shrink([false, true])
                                .show(ui, |ui| {
                                    ui.set_width(ui.available_width());
                                    out = Some(build(ui));
                                });
                        });
                        ui.data_mut(|d| d.insert_temp(height_id, card.response.rect.height()));
                    },
                );
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
