//! The desktop **app shell** — native out-of-match chrome (D32) for Windows/Linux, drawn with
//! **egui** (D36: the desktop-toolkit call D32 left open). It is host-side, holds no game/sim logic,
//! and drives the shared engine only through the `core::shell` seam — the desktop counterpart of the
//! Android Jetpack-Compose shell (D35).
//!
//! Two layers, kept apart exactly like the Android shell:
//!  - a tiny **pure seam** ([`resolve_title_action`], [`build_stamp`]/[`build_channel`]) — the
//!    testable decision/formatting logic, unit-tested below with no GPU or window;
//!  - the **egui glue** ([`EguiShell`]) — device-gated chrome (an egui context + the winit input
//!    bridge + the wgpu renderer) that draws the title screen and reports the clicked action. The
//!    glue is exempt from unit tests (CLAUDE.md: thin, un-constructible-in-test platform glue), so
//!    the real logic is pushed down into the pure seam where it *is* tested.

use gonedark_engine::loadout_ui::{LoadoutEditor, LoadoutSlot};
use gonedark_pal_desktop::DesktopRenderSurface;
use winit::window::Window;

// ---- The pure seam (unit-tested) ----------------------------------------------------------------

/// A top-level action the player can pick on the title screen.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum TitleAction {
    /// Start a match — opens the pre-match gunsmith / loadout screen (Deploy from there enters the
    /// shared engine).
    Start,
    /// Open settings (a placeholder until the Settings surface lands).
    Settings,
    /// Quit the app.
    Quit,
}

/// What the host does in response to a title action — the decision table the run loop switches on.
/// Separated from [`TitleAction`] so it is unit-testable without a window.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum HostTransition {
    /// Switch the host to the pre-match gunsmith / loadout screen. Start now lands here first; the
    /// screen's **Deploy** is what subsequently creates the `Game` (carrying the chosen loadout).
    OpenLoadout,
    /// Lazily create `engine::Game` and switch the host to the in-match screen.
    EnterMatch,
    /// Open the (not-yet-built) settings surface — a no-op placeholder today.
    OpenSettings,
    /// Tear down and exit the app.
    Exit,
    /// Leave the current match and return to the title screen — the post-match summary's DISMISS,
    /// and any other in-match "give up the match without quitting the app" path. Drops the `Game`.
    ExitToTitle,
}

/// Map a title action to the host transition it triggers (the pure run-loop decision).
pub fn resolve_title_action(action: TitleAction) -> HostTransition {
    match action {
        // Start no longer enters the match directly — it opens the gunsmith so the player picks a
        // loadout first; Deploy from the gunsmith is what creates the `Game`.
        TitleAction::Start => HostTransition::OpenLoadout,
        TitleAction::Settings => HostTransition::OpenSettings,
        TitleAction::Quit => HostTransition::Exit,
    }
}

// ---- The gunsmith / loadout screen — pure seam (unit-tested) -------------------------------------

/// An action the pre-match gunsmith / loadout screen can emit in a frame.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum LoadoutAction {
    /// Cycle the slot at on-screen index `slot_index` forward (`true`) or back (`false`) — an edit.
    Cycle { slot_index: usize, forward: bool },
    /// Reset every slot to the neutral all-`Standard` baseline.
    Reset,
    /// Deploy with the current loadout — leave the gunsmith and enter the match.
    Deploy,
    /// Abandon the gunsmith and return to the title screen (no match started).
    Back,
}

/// The screen-level outcome of a [`LoadoutAction`] once applied to the editor — what the host run
/// loop switches on. Separated from the egui glue so it is unit-testable without a window.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum LoadoutStep {
    /// Stay on the gunsmith (an edit was applied, or nothing happened this frame).
    Stay,
    /// Enter the match, fielding the editor's current loadout.
    Deploy,
    /// Return to the title screen without starting a match.
    Back,
}

/// Apply a [`LoadoutAction`] to the player's [`LoadoutEditor`] and report the resulting screen step.
/// Edits (`Cycle`/`Reset`) mutate the editor and keep us on the gunsmith; `Deploy`/`Back` are screen
/// transitions the run loop acts on. Pure (no egui/window) — the gunsmith's testable decision seam,
/// mirroring [`resolve_title_action`]. The actual loadout *model* (validation + the sidegrade-fairness
/// proof) lives in `core::gunsmith` and is consumed through the editor read-only; this never touches
/// the sim.
pub fn apply_loadout_action(action: LoadoutAction, editor: &mut LoadoutEditor) -> LoadoutStep {
    match action {
        LoadoutAction::Cycle {
            slot_index,
            forward,
        } => {
            // An out-of-range index is a harmless no-op (the editor tolerates stray slot values).
            editor.apply_input(slot_index, forward);
            LoadoutStep::Stay
        }
        LoadoutAction::Reset => {
            editor.reset();
            LoadoutStep::Stay
        }
        LoadoutAction::Deploy => LoadoutStep::Deploy,
        LoadoutAction::Back => LoadoutStep::Back,
    }
}

/// A short, one-line description of the *axis pair* a gunsmith slot trades — the readout that makes
/// the sidegrade nature legible (every option spends one of these axes to buy the other). Pure and
/// static, so it is unit-tested; the numeric per-axis deltas live in `core::gunsmith` and are not
/// surfaced here (they need fixed-point formatting and add nothing to "which way does this trade").
/// ASCII only so it can never tofu in egui's default font.
pub fn slot_trade_hint(slot: LoadoutSlot) -> &'static str {
    match slot {
        LoadoutSlot::Optic => "range <-> fire-rate",
        LoadoutSlot::Barrel => "damage <-> reserve",
        LoadoutSlot::Magazine => "capacity <-> handling",
    }
}

/// Format the title screen's build stamp — e.g. `build dev · v0.0.0`.
pub fn build_stamp(channel: &str, version: &str) -> String {
    format!(
        "build {} · v{}",
        channel.trim().to_ascii_lowercase(),
        version.trim()
    )
}

/// The build channel from cargo's debug-assertions flag: a debug build is "dev", a release "release".
pub fn build_channel(debug_assertions: bool) -> &'static str {
    if debug_assertions {
        "dev"
    } else {
        "release"
    }
}

// ---- The "going-dark" palette + theme -----------------------------------------------------------

// A near-black field, dim chrome, one amber alert accent (the game's directional-alert colour).
// These five base values (INK/PANEL/BONE/ASH/AMBER) are kept **bit-identical to the canonical
// renderer palette** documented in `render/src/theme.rs` (gonedark_render::theme) so the out-of-match
// egui chrome and the in-match wgpu HUD read as one art-directed identity. The `app` crate does not
// depend on `gonedark-render`, so we mirror the hex here rather than take a crate dep just for
// colours — but the two must move together (see the doc-hex annotations in theme.rs).
const INK: egui::Color32 = egui::Color32::from_rgb(0x07, 0x09, 0x0C);
const PANEL: egui::Color32 = egui::Color32::from_rgb(0x12, 0x18, 0x20);
const BONE: egui::Color32 = egui::Color32::from_rgb(0xE7, 0xEC, 0xEF);
const ASH: egui::Color32 = egui::Color32::from_rgb(0x8A, 0x94, 0x9C);
const AMBER: egui::Color32 = egui::Color32::from_rgb(0xE0, 0x79, 0x1F);
// One step lighter than PANEL for raised/hovered/active surfaces; a hairline RIM lifts a card off
// the ink; MUTED is the dimmest legible text (mirrors theme.rs PANEL_RAISED/RIM/MUTED).
const PANEL_RAISED: egui::Color32 = egui::Color32::from_rgb(0x1B, 0x25, 0x31);
const RIM: egui::Color32 = egui::Color32::from_rgb(0x29, 0x30, 0x42);
const MUTED: egui::Color32 = egui::Color32::from_rgb(0x61, 0x68, 0x75);

// The desktop-shell type scale (egui point sizes). One small, fixed ramp so every screen shares a
// heading/body/caption hierarchy instead of each call site picking an ad-hoc glyph size — the
// pixel-space analogue of theme.rs's NDC `TYPE_*` scale. `DISPLAY` is the title hero; `HEADING` the
// per-screen banner (GUNSMITH); `BUTTON`/`BODY`/`CAPTION` the rest.
const TYPE_DISPLAY: f32 = 52.0;
const TYPE_HEADING: f32 = 30.0;
const TYPE_SUBHEAD: f32 = 16.0;
const TYPE_BUTTON: f32 = 16.0;
const TYPE_BODY: f32 = 14.0;
const TYPE_CAPTION: f32 = 12.0;

/// Build the shell's cohesive dark [`egui::Style`] — the single source of truth for the title /
/// gunsmith / settings chrome's look (fills, widget ramp, corner radii, spacing, and the
/// heading->caption type scale). Pure data: `egui::Style`/`Visuals` are plain structs with no GPU or
/// window, so this is unit-tested below (unlike the [`EguiShell`] glue that *applies* it). Keeping it
/// pure also means a retune is one function, asserted by tests, rather than scattered `set_*` calls.
fn shell_style() -> egui::Style {
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
    v.selection.bg_fill = egui::Color32::from_rgba_unmultiplied(0xE0, 0x79, 0x1F, 96);
    v.selection.stroke = Stroke::new(1.0, AMBER);

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

// ---- The egui glue (device-gated chrome; exempt from unit tests) --------------------------------

/// The egui-backed title screen: an egui context, the winit→egui input bridge, and the egui-wgpu
/// renderer that paints into the same surface the engine uses. Owns no game state.
pub struct EguiShell {
    ctx: egui::Context,
    state: egui_winit::State,
    renderer: egui_wgpu::Renderer,
    stamp: String,
}

impl EguiShell {
    /// Build the shell against the desktop surface's device/format and the window (for input/DPI).
    /// `stamp` is the already-formatted build/version line (see [`build_stamp`]).
    pub fn new(
        device: &wgpu::Device,
        format: wgpu::TextureFormat,
        window: &Window,
        stamp: String,
    ) -> Self {
        let ctx = egui::Context::default();
        // Install the art-directed shell theme once on the context (the pure [`shell_style`] is the
        // single source of truth; this is the one place the glue applies it). egui 0.35 keeps a style
        // per theme, so pin the preference to Dark and write our style into every theme slot — the
        // shell is dark-only, never tracking the system light/dark setting.
        ctx.set_theme(egui::ThemePreference::Dark);
        ctx.all_styles_mut(|style| *style = shell_style());

        let state = egui_winit::State::new(
            ctx.clone(),
            egui::ViewportId::ROOT,
            window,
            None, // native pixels-per-point — let egui read the window scale factor
            None, // system theme
            None, // max texture side
        );
        let renderer = egui_wgpu::Renderer::new(device, format, egui_wgpu::RendererOptions::default());

        EguiShell {
            ctx,
            state,
            renderer,
            stamp,
        }
    }

    /// Feed one winit window event to egui (pointer/keys). Returns whether egui consumed it.
    pub fn on_window_event(&mut self, window: &Window, event: &winit::event::WindowEvent) -> bool {
        self.state.on_window_event(window, event).consumed
    }

    /// Draw the title screen for one frame and return a clicked [`TitleAction`], if any. Pure
    /// presentation — it never touches sim state.
    pub fn draw_title(&mut self, surface: &mut DesktopRenderSurface) -> Option<TitleAction> {
        // Clone the stamp so the immediate-mode closure doesn't alias the `&mut self` borrow
        // `run_and_paint` takes.
        let stamp = self.stamp.clone();
        self.run_and_paint(surface, |ui| title_ui(ui, &stamp))
    }

    /// Draw the pre-match gunsmith / loadout screen for one frame and return the [`LoadoutAction`]
    /// whose control was used, if any. `editor` is the host-side pre-match selection state (read-only
    /// here — it never reaches the sim). Pure presentation, same paint path as the title screen.
    pub fn draw_loadout(
        &mut self,
        surface: &mut DesktopRenderSurface,
        editor: &LoadoutEditor,
    ) -> Option<LoadoutAction> {
        self.run_and_paint(surface, |ui| loadout_ui(ui, editor))
    }

    /// Run one egui frame (`build` lays out the UI and returns this frame's action) and paint the
    /// tessellated output into a freshly-acquired surface frame. The shared paint path behind both
    /// [`draw_title`](Self::draw_title) and [`draw_loadout`](Self::draw_loadout) — device-gated glue,
    /// exempt from unit tests; the per-screen *logic* it drives lives in the pure `*_ui` builders and
    /// the pure action seams above.
    fn run_and_paint<T>(
        &mut self,
        surface: &mut DesktopRenderSurface,
        // `egui::Context::run_ui` takes an `FnMut` (it may run the UI more than once for a sizing
        // pass), so the per-screen builder is `FnMut` too.
        mut build: impl FnMut(&mut egui::Ui) -> Option<T>,
    ) -> Option<T> {
        let ctx = self.ctx.clone();

        // Run egui (needs the window for input gather + platform output).
        let raw_input = self.state.take_egui_input(surface.window());
        let mut action = None;
        let full_output = ctx.run_ui(raw_input, |ui| {
            action = build(ui);
        });
        self.state
            .handle_platform_output(surface.window(), full_output.platform_output);

        let ppp = full_output.pixels_per_point;
        let paint_jobs = ctx.tessellate(full_output.shapes, ppp);
        let (w, h) = surface.size();

        // Acquire the frame (owned — the `&mut` surface borrow ends as this returns).
        let Some((frame, view)) = surface.acquire() else {
            return action;
        };

        let device = surface.device();
        let queue = surface.queue();
        for (id, delta) in &full_output.textures_delta.set {
            self.renderer.update_texture(device, queue, *id, delta);
        }
        let screen = egui_wgpu::ScreenDescriptor {
            size_in_pixels: [w, h],
            pixels_per_point: ppp,
        };
        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("gonedark.shell.egui"),
        });
        let user_cmds =
            self.renderer
                .update_buffers(device, queue, &mut encoder, &paint_jobs, &screen);
        {
            let pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("gonedark.shell.egui_pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color {
                            r: 0.007,
                            g: 0.009,
                            b: 0.013,
                            a: 1.0,
                        }),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                multiview_mask: None,
                timestamp_writes: None,
                occlusion_query_set: None,
            });
            // egui-wgpu's `render` wants a `RenderPass<'static>`; the pass borrows only owned locals
            // here, so forgetting the lifetime is sound for the duration of the call.
            let mut pass = pass.forget_lifetime();
            self.renderer.render(&mut pass, &paint_jobs, &screen);
        }
        queue.submit(user_cmds.into_iter().chain(std::iter::once(encoder.finish())));
        surface.present(frame);
        for id in &full_output.textures_delta.free {
            self.renderer.free_texture(id);
        }
        action
    }
}

/// The shared "menu button" width — every primary/secondary action button is this wide so the action
/// stacks line up into a clean column.
const MENU_BUTTON_W: f32 = 256.0;

/// How a [`menu_button`] reads in the visual hierarchy: the one amber call-to-action, a neutral
/// secondary, or a de-emphasised tertiary (e.g. QUIT / BACK).
#[derive(Clone, Copy)]
enum Emphasis {
    /// Filled amber, ink text — the single primary action on a screen.
    Primary,
    /// Panel-filled, bone text — a normal secondary action (rides the hover/active fill ramp).
    Secondary,
    /// Panel-filled, ash text — a quieter, lower-stakes action.
    Tertiary,
}

/// Draw one full-width menu button in the shell style and report whether it was clicked. Glue (it
/// needs a live `Ui`), so it's exempt from unit tests — the click→action mapping it feeds is what the
/// pure [`resolve_title_action`] / [`apply_loadout_action`] seams cover. Only the primary button sets
/// an explicit fill; secondary/tertiary leave the fill to the widget ramp in [`shell_style`] so they
/// visibly lift on hover.
fn menu_button(ui: &mut egui::Ui, text: &str, emphasis: Emphasis) -> bool {
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
fn accent_rule(ui: &mut egui::Ui, width: f32) {
    let (rect, _) = ui.allocate_exact_size(egui::vec2(width, 2.0), egui::Sense::hover());
    ui.painter().rect_filled(rect, egui::CornerRadius::same(1), AMBER);
}

/// The framed "card" the menu/content column sits in — a PANEL fill with a RIM hairline, rounded,
/// with comfortable inner padding. It shrinks to its content, so inside a `vertical_centered` column
/// it renders as a centred panel rather than a full-bleed band. Glue (returns an egui builder).
fn card_frame() -> egui::Frame {
    egui::Frame::default()
        .fill(PANEL)
        .stroke(egui::Stroke::new(1.0, RIM))
        .corner_radius(egui::CornerRadius::same(12))
        .inner_margin(egui::Margin::same(22))
}

/// The immediate-mode title-screen UI, drawn into the root [`egui::Ui`] for the frame. Returns the
/// action whose button was clicked this frame.
fn title_ui(ui: &mut egui::Ui, stamp: &str) -> Option<TitleAction> {
    use egui::RichText;
    let mut action = None;

    // Everything lives in one full-screen CentralPanel — a single centered column (the title hero +
    // amber rule + tagline, a framed card holding the three actions, then the build stamp). One panel
    // keeps the layout robust (no panel-space splitting); the panel fill paints the ink background.
    egui::CentralPanel::default().show(ui, |ui| {
        let h = ui.available_height();
        ui.vertical_centered(|ui| {
            ui.add_space(h * 0.18);
            // Title hero: tracked-out caps in bone, the amber rule, then the muted tagline.
            ui.label(
                RichText::new("GOING DARK")
                    .color(BONE)
                    .size(TYPE_DISPLAY)
                    .strong(),
            );
            ui.add_space(10.0);
            accent_rule(ui, 132.0);
            ui.add_space(10.0);
            ui.label(
                // U+00B7 middle dot (the same glyph the build stamp uses) — proven to render in
                // egui's default font, so the tagline can never tofu.
                RichText::new("COMMAND \u{00B7} EMBODY")
                    .color(ASH)
                    .size(TYPE_SUBHEAD),
            );
            ui.add_space(34.0);

            // The action stack, framed as a card so it reads as a deliberate panel.
            card_frame().show(ui, |ui| {
                ui.vertical_centered(|ui| {
                    if menu_button(ui, "START", Emphasis::Primary) {
                        action = Some(TitleAction::Start);
                    }
                    ui.add_space(10.0);
                    if menu_button(ui, "SETTINGS", Emphasis::Secondary) {
                        action = Some(TitleAction::Settings);
                    }
                    ui.add_space(10.0);
                    if menu_button(ui, "QUIT", Emphasis::Tertiary) {
                        action = Some(TitleAction::Quit);
                    }
                });
            });

            ui.add_space(26.0);
            ui.label(RichText::new(stamp).color(MUTED).size(TYPE_CAPTION));
        });
    });

    action
}

/// The immediate-mode gunsmith / loadout screen, drawn into the root [`egui::Ui`] for the frame.
/// Reads the current selection from `editor` (host-side pre-match state — never the sim) and returns
/// the action whose control was used this frame. Layout: a centered column of the three attachment
/// slots — each a `<` / `>` cycler over its current option plus the slot's trade-axis hint — the
/// sidegrade explainer, then DEPLOY / RESET / BACK. All the decision logic is in the pure seam
/// ([`apply_loadout_action`], [`slot_trade_hint`], and the `core::gunsmith`-backed editor); this fn
/// is just the egui glue.
fn loadout_ui(ui: &mut egui::Ui, editor: &LoadoutEditor) -> Option<LoadoutAction> {
    use egui::{Button, Label, RichText};
    let mut action = None;

    egui::CentralPanel::default().show(ui, |ui| {
        let h = ui.available_height();
        ui.vertical_centered(|ui| {
            ui.add_space(h * 0.09);
            // Screen banner + amber rule, mirroring the title hero treatment.
            ui.label(
                RichText::new("GUNSMITH")
                    .color(BONE)
                    .size(TYPE_HEADING)
                    .strong(),
            );
            ui.add_space(8.0);
            accent_rule(ui, 100.0);
            ui.add_space(10.0);
            ui.label(
                RichText::new(
                    "Every attachment is a sidegrade -- it spends one stat to buy another. \
                     No build is strictly better than any other.",
                )
                .color(ASH)
                .size(TYPE_BODY),
            );
            ui.add_space(24.0);

            // The three attachment slots live in a framed card so the cyclers read as a panel.
            card_frame().show(ui, |ui| {
                // One aligned row per attachment slot. The on-screen index `i` is exactly the index
                // the editor's `apply_input` routes on (`LoadoutSlot::from_index`), so the cycler maps
                // 1:1.
                for (i, &slot) in LoadoutSlot::ALL.iter().enumerate() {
                    ui.horizontal(|ui| {
                        ui.add_sized(
                            [104.0, 32.0],
                            Label::new(
                                RichText::new(slot.label()).color(BONE).size(TYPE_SUBHEAD).strong(),
                            ),
                        );
                        if ui
                            .add_sized([34.0, 32.0], Button::new(RichText::new("<").color(BONE)))
                            .clicked()
                        {
                            action = Some(LoadoutAction::Cycle {
                                slot_index: i,
                                forward: false,
                            });
                        }
                        ui.add_sized(
                            [150.0, 32.0],
                            Label::new(
                                RichText::new(editor.option_label(slot))
                                    .color(AMBER)
                                    .size(TYPE_BODY)
                                    .strong(),
                            ),
                        );
                        if ui
                            .add_sized([34.0, 32.0], Button::new(RichText::new(">").color(BONE)))
                            .clicked()
                        {
                            action = Some(LoadoutAction::Cycle {
                                slot_index: i,
                                forward: true,
                            });
                        }
                        ui.add_sized(
                            [172.0, 32.0],
                            Label::new(
                                RichText::new(slot_trade_hint(slot)).color(MUTED).size(TYPE_CAPTION),
                            ),
                        );
                    });
                    if i + 1 < LoadoutSlot::ALL.len() {
                        ui.add_space(8.0);
                    }
                }
            });

            ui.add_space(22.0);
            if menu_button(ui, "DEPLOY", Emphasis::Primary) {
                action = Some(LoadoutAction::Deploy);
            }
            ui.add_space(10.0);
            if menu_button(ui, "RESET", Emphasis::Secondary) {
                action = Some(LoadoutAction::Reset);
            }
            ui.add_space(10.0);
            if menu_button(ui, "BACK", Emphasis::Tertiary) {
                action = Some(LoadoutAction::Back);
            }
        });
    });

    action
}

#[cfg(test)]
mod tests {
    //! The pure seam only — the egui glue (`EguiShell`/`title_ui`/`loadout_ui`/`run_and_paint`) needs
    //! a GPU + window and is the exempt device-gated chrome (D32 / CLAUDE.md testing rule).
    use super::*;
    // Re-imported explicitly: the parent's `use` of these is private, so it isn't pulled in by the
    // `super::*` glob above.
    use gonedark_engine::loadout_ui::{LoadoutEditor, LoadoutSlot};

    #[test]
    fn debug_build_is_the_dev_channel() {
        assert_eq!(build_channel(true), "dev");
    }

    #[test]
    fn release_build_is_the_release_channel() {
        assert_eq!(build_channel(false), "release");
    }

    #[test]
    fn stamp_formats_channel_and_version() {
        assert_eq!(build_stamp("dev", "0.0.0"), "build dev · v0.0.0");
    }

    #[test]
    fn stamp_normalises_case_and_trims_whitespace() {
        assert_eq!(build_stamp("  RELEASE ", " 1.2.3 "), "build release · v1.2.3");
    }

    #[test]
    fn start_opens_the_gunsmith() {
        // Start no longer enters the match directly — it routes through the loadout screen first.
        assert_eq!(
            resolve_title_action(TitleAction::Start),
            HostTransition::OpenLoadout
        );
    }

    #[test]
    fn settings_opens_settings() {
        assert_eq!(
            resolve_title_action(TitleAction::Settings),
            HostTransition::OpenSettings
        );
    }

    #[test]
    fn quit_exits() {
        assert_eq!(resolve_title_action(TitleAction::Quit), HostTransition::Exit);
    }

    #[test]
    fn stamp_matches_the_path_main_uses() {
        // The exact composition `resumed()` performs: channel from debug flag, then format.
        assert_eq!(build_stamp(build_channel(true), "0.0.0"), "build dev · v0.0.0");
        assert_eq!(
            build_stamp(build_channel(false), "0.0.0"),
            "build release · v0.0.0"
        );
    }

    // ---- The gunsmith / loadout pure seam --------------------------------------------------------

    #[test]
    fn cycle_action_edits_the_routed_slot_and_stays() {
        let mut ed = LoadoutEditor::new();
        assert_eq!(ed.option_label(LoadoutSlot::Optic), "Standard");
        // Index 0 is the Optic slot (LoadoutSlot::from_index order); cycling forward advances it.
        let step = apply_loadout_action(
            LoadoutAction::Cycle {
                slot_index: 0,
                forward: true,
            },
            &mut ed,
        );
        assert_eq!(step, LoadoutStep::Stay);
        assert_eq!(ed.option_label(LoadoutSlot::Optic), "Marksman");
        // The other slots are untouched by an Optic cycle.
        assert_eq!(ed.option_label(LoadoutSlot::Barrel), "Standard");
        assert_eq!(ed.option_label(LoadoutSlot::Magazine), "Standard");
    }

    #[test]
    fn cycle_forward_then_back_round_trips() {
        let mut ed = LoadoutEditor::new();
        apply_loadout_action(
            LoadoutAction::Cycle {
                slot_index: 1,
                forward: true,
            },
            &mut ed,
        );
        apply_loadout_action(
            LoadoutAction::Cycle {
                slot_index: 1,
                forward: false,
            },
            &mut ed,
        );
        assert_eq!(ed.current(), LoadoutEditor::new().current());
    }

    #[test]
    fn out_of_range_cycle_is_a_harmless_stay_noop() {
        let mut ed = LoadoutEditor::new();
        let before = ed.current();
        let step = apply_loadout_action(
            LoadoutAction::Cycle {
                slot_index: 99,
                forward: true,
            },
            &mut ed,
        );
        assert_eq!(step, LoadoutStep::Stay);
        assert_eq!(ed.current(), before, "a stray slot index changes nothing");
    }

    #[test]
    fn reset_action_returns_to_baseline_and_stays() {
        let mut ed = LoadoutEditor::new();
        apply_loadout_action(
            LoadoutAction::Cycle {
                slot_index: 0,
                forward: true,
            },
            &mut ed,
        );
        assert_ne!(ed.current(), LoadoutEditor::new().current());
        let step = apply_loadout_action(LoadoutAction::Reset, &mut ed);
        assert_eq!(step, LoadoutStep::Stay);
        assert_eq!(ed.current(), LoadoutEditor::new().current());
    }

    #[test]
    fn deploy_and_back_are_screen_transitions_that_leave_the_editor_alone() {
        let mut ed = LoadoutEditor::new();
        apply_loadout_action(
            LoadoutAction::Cycle {
                slot_index: 2,
                forward: true,
            },
            &mut ed,
        );
        let chosen = ed.current();
        // Deploy/Back report a screen step but never mutate the chosen loadout.
        assert_eq!(apply_loadout_action(LoadoutAction::Deploy, &mut ed), LoadoutStep::Deploy);
        assert_eq!(ed.current(), chosen, "Deploy carries the chosen loadout unchanged");
        assert_eq!(apply_loadout_action(LoadoutAction::Back, &mut ed), LoadoutStep::Back);
        assert_eq!(ed.current(), chosen, "Back doesn't alter the editor either");
    }

    // ---- The shell theme (pure egui::Style data — no GPU/window, so it IS testable) --------------

    #[test]
    fn shell_style_paints_the_going_dark_surfaces() {
        let style = shell_style();
        let v = &style.visuals;
        // The base surfaces are the palette: ink behind everything, panel for windows/cards.
        assert_eq!(v.panel_fill, INK);
        assert_eq!(v.window_fill, PANEL);
        assert_eq!(v.extreme_bg_color, INK);
        // Amber is the lone accent.
        assert_eq!(v.hyperlink_color, AMBER);
        assert_eq!(v.selection.stroke.color, AMBER);
    }

    #[test]
    fn shell_style_widget_ramp_lifts_on_hover_and_rings_in_amber() {
        let w = &shell_style().visuals.widgets;
        // A button at rest sits on PANEL; hover/active lift it to the raised surface.
        assert_eq!(w.inactive.weak_bg_fill, PANEL);
        assert_eq!(w.hovered.weak_bg_fill, PANEL_RAISED);
        assert_eq!(w.active.weak_bg_fill, PANEL_RAISED);
        assert_ne!(
            w.inactive.weak_bg_fill, w.hovered.weak_bg_fill,
            "secondary buttons must visibly change fill on hover"
        );
        // The focus ring is amber, and hover nudges the widget outward for tactile feedback.
        assert_eq!(w.hovered.bg_stroke.color, AMBER);
        assert!(w.hovered.expansion > w.inactive.expansion);
        // Open menus mirror the pressed look.
        assert_eq!(w.open.weak_bg_fill, w.active.weak_bg_fill);
    }

    #[test]
    fn shell_style_type_scale_matches_the_named_ramp_and_descends() {
        use egui::TextStyle;
        let style = shell_style();
        let size = |s: TextStyle| style.text_styles.get(&s).map(|f| f.size).unwrap();
        assert_eq!(size(TextStyle::Heading), TYPE_HEADING);
        assert_eq!(size(TextStyle::Button), TYPE_BUTTON);
        assert_eq!(size(TextStyle::Body), TYPE_BODY);
        assert_eq!(size(TextStyle::Small), TYPE_CAPTION);
        // The hierarchy is strictly descending (a guard against a future edit inverting two sizes).
        assert!(TYPE_DISPLAY > TYPE_HEADING);
        assert!(TYPE_HEADING > TYPE_SUBHEAD);
        assert!(TYPE_SUBHEAD >= TYPE_BUTTON);
        assert!(TYPE_BUTTON > TYPE_BODY);
        assert!(TYPE_BODY > TYPE_CAPTION);
    }

    #[test]
    fn each_slot_advertises_its_own_trade_axis_pair() {
        // Every slot trades a distinct, disjoint axis pair (the source of the no-strict-domination
        // proof in core::gunsmith); the hints must reflect that and stay ASCII (no tofu).
        assert_eq!(slot_trade_hint(LoadoutSlot::Optic), "range <-> fire-rate");
        assert_eq!(slot_trade_hint(LoadoutSlot::Barrel), "damage <-> reserve");
        assert_eq!(slot_trade_hint(LoadoutSlot::Magazine), "capacity <-> handling");
        // All three are distinct — no slot duplicates another's pitch.
        let hints = [
            slot_trade_hint(LoadoutSlot::Optic),
            slot_trade_hint(LoadoutSlot::Barrel),
            slot_trade_hint(LoadoutSlot::Magazine),
        ];
        assert!(hints[0] != hints[1] && hints[1] != hints[2] && hints[0] != hints[2]);
        assert!(
            hints.iter().all(|h| h.is_ascii()),
            "trade hints must be ASCII to render in egui's default font"
        );
    }
}
