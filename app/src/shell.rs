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
use gonedark_render::title_backdrop::TitleBackdrop;
use winit::window::Window;

// ---- The pure seam (unit-tested) ----------------------------------------------------------------

/// A top-level action the player can pick on the title screen. The three play modes all open the
/// gunsmith→match flow today; their divergence is future work (see [`resolve_title_action`]).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum TitleAction {
    /// The PvE story campaign — the first shippable pillar (`docs/pve-campaign.md`, D58).
    Campaign,
    /// A standalone PvE skirmish against the scripted enemy commander.
    Pve,
    /// Player-vs-player — the lockstep-netcode match.
    Pvp,
    /// Open settings (a placeholder until the Settings surface lands).
    Settings,
    /// Open the player profile / progression surface (a no-op placeholder until it lands).
    Profile,
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
    /// Open the (not-yet-built) player profile / progression surface — a no-op placeholder today.
    OpenProfile,
    /// Tear down and exit the app.
    Exit,
    /// Leave the current match and return to the title screen — the post-match summary's DISMISS,
    /// and any other in-match "give up the match without quitting the app" path. Drops the `Game`.
    ExitToTitle,
}

/// Map a title action to the host transition it triggers (the pure run-loop decision).
pub fn resolve_title_action(action: TitleAction) -> HostTransition {
    match action {
        // All three play modes currently share the ONE gunsmith→match flow: each opens the gunsmith
        // so the player picks a loadout first, and Deploy from the gunsmith is what creates the
        // `Game`. PvP/PvE/Campaign mode divergence (netcode lobby vs scripted-AI scenario vs the
        // Operations-hub campaign, `docs/pve-campaign.md`) is future work — there are no separate
        // mode systems today, so they deliberately fold to the same transition here.
        TitleAction::Campaign | TitleAction::Pve | TitleAction::Pvp => HostTransition::OpenLoadout,
        TitleAction::Settings => HostTransition::OpenSettings,
        TitleAction::Profile => HostTransition::OpenProfile,
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

/// Convert an egui pointer position (logical points, origin top-left, y down) into the title
/// backdrop's NDC ([-1, 1] on both axes, origin centre, **y up**) given the surface size in the same
/// logical points. Pure arithmetic — extracted from the [`EguiShell`] glue exactly so the cursor
/// mapping the 3D backdrop reacts to is unit-tested (the wgpu compositing around it is exempt). This
/// is host presentation math, not sim — the f32s here never touch `core` (invariant #1 is about the
/// sim, not the renderer's float boundary).
pub fn pointer_to_ndc(pos: [f32; 2], size_points: [f32; 2]) -> [f32; 2] {
    // Guard a zero/negative extent (a not-yet-sized surface) so we never divide by zero.
    let w = if size_points[0] > 0.0 { size_points[0] } else { 1.0 };
    let h = if size_points[1] > 0.0 { size_points[1] } else { 1.0 };
    [(pos[0] / w) * 2.0 - 1.0, 1.0 - (pos[1] / h) * 2.0]
}

// ---- The "going-dark" palette + theme -----------------------------------------------------------

// A near-black field, dim chrome, one amber alert accent (the game's directional-alert colour).
// These five base values (INK/PANEL/BONE/ASH/AMBER) are kept **bit-identical to the canonical
// renderer palette** documented in `render/src/theme.rs` (gonedark_render::theme) so the out-of-match
// egui chrome and the in-match wgpu HUD read as one art-directed identity. (`app` now *does* depend
// on `gonedark-render` — for the 3D title backdrop, see [`EguiShell`] — but we still mirror the hex
// here rather than pull the colour table through that dep: egui wants `Color32`, render wants linear
// `[f32; 4]`, and this egui chrome predates the dep. The two palettes must still move together; see
// the doc-hex annotations in theme.rs.)
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
// A semi-opaque PANEL for chrome floated over the live 3D title backdrop: the PANEL hue at ~88%
// alpha (224/255) so the moving sky reads faintly behind a card without costing text legibility.
// Only the title screen (which has a backdrop behind it) uses it; the loadout screen keeps the
// opaque PANEL. `Color32` stores PREMULTIPLIED alpha and only `from_rgba_premultiplied` is `const`,
// so the channels here are PANEL (0x12/0x18/0x20) already multiplied by 224/255 (→ 16/21/28); this
// is the const-fn equivalent of `from_rgba_unmultiplied(0x12, 0x18, 0x20, 224)`.
const PANEL_GLASS: egui::Color32 = egui::Color32::from_rgba_premultiplied(16, 21, 28, 224);

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
    /// The live 3D title backdrop (`render` crate). Painted behind the title-screen egui pass (which
    /// then composites with `LoadOp::Load`). `Option` so a future fallible build could degrade to a
    /// flat-clear title without panicking the shell — the pinned `new` is infallible today, so it is
    /// always `Some`. Only the title screen uses it; the loadout screen clears its own ink panel.
    backdrop: Option<TitleBackdrop>,
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

        // Build the live 3D title backdrop against the same device/format the egui pass and the
        // engine share. Infallible per the pinned API, so always `Some` today.
        let backdrop = Some(TitleBackdrop::new(device, format));

        EguiShell {
            ctx,
            state,
            renderer,
            stamp,
            backdrop,
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
        // `with_backdrop = true`: paint the live 3D backdrop into the frame first, then composite the
        // title HUD over it (`LoadOp::Load`).
        self.run_and_paint(surface, true, |ui| title_ui(ui, &stamp))
    }

    /// Draw the pre-match gunsmith / loadout screen for one frame and return the [`LoadoutAction`]
    /// whose control was used, if any. `editor` is the host-side pre-match selection state (read-only
    /// here — it never reaches the sim). Pure presentation, same paint path as the title screen.
    pub fn draw_loadout(
        &mut self,
        surface: &mut DesktopRenderSurface,
        editor: &LoadoutEditor,
    ) -> Option<LoadoutAction> {
        // `with_backdrop = false`: the gunsmith keeps its opaque ink panel (it has no 3D backdrop),
        // so the egui pass clears as before — no regression to `draw_loadout`.
        self.run_and_paint(surface, false, |ui| loadout_ui(ui, editor))
    }

    /// Run one egui frame (`build` lays out the UI and returns this frame's action) and paint the
    /// tessellated output into a freshly-acquired surface frame. The shared paint path behind both
    /// [`draw_title`](Self::draw_title) and [`draw_loadout`](Self::draw_loadout) — device-gated glue,
    /// exempt from unit tests; the per-screen *logic* it drives lives in the pure `*_ui` builders and
    /// the pure action seams above.
    ///
    /// When `with_backdrop` is set (the title screen), the live 3D
    /// [`gonedark_render::title_backdrop::TitleBackdrop`] is painted into the acquired view FIRST
    /// (it clears the view to its sky and submits its own encoder), and the egui pass then composites
    /// over it with `LoadOp::Load`. Otherwise (the gunsmith) the egui pass clears the view itself —
    /// the original opaque behaviour, unchanged. The animation clock + cursor handed to the backdrop
    /// come from this just-run frame's egui input (a one-frame lag is fine), with the pixel→NDC
    /// conversion living in the pure [`pointer_to_ndc`] seam.
    fn run_and_paint<T>(
        &mut self,
        surface: &mut DesktopRenderSurface,
        with_backdrop: bool,
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

        // Pull the backdrop's animation clock + cursor from this frame's egui input. `i.time` is a
        // monotonic seconds clock; the latest pointer is in egui logical points (origin top-left),
        // mapped to NDC against the surface size in the same logical points (physical / ppp).
        let time = ctx.input(|i| i.time) as f32;
        let cursor = ctx.input(|i| i.pointer.latest_pos()).map(|p| {
            let size_points = [w as f32 / ppp, h as f32 / ppp];
            pointer_to_ndc([p.x, p.y], size_points)
        });

        // Acquire the frame (owned — the `&mut` surface borrow ends as this returns).
        let Some((frame, view)) = surface.acquire() else {
            return action;
        };

        let device = surface.device();
        let queue = surface.queue();

        // Paint the 3D backdrop into the view BEFORE egui (it clears + submits its own encoder), so
        // the egui pass below loads over it. `self.backdrop`/`self.renderer` are disjoint fields, so
        // this split borrow is fine.
        if with_backdrop {
            if let Some(bd) = self.backdrop.as_mut() {
                bd.render(device, queue, &view, (w, h), time, cursor);
            }
        }

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
            // Title: LOAD over the backdrop the pass above painted. Gunsmith: CLEAR to ink (no
            // backdrop), preserving the original opaque look.
            let load = if with_backdrop {
                wgpu::LoadOp::Load
            } else {
                wgpu::LoadOp::Clear(wgpu::Color {
                    r: 0.007,
                    g: 0.009,
                    b: 0.013,
                    a: 1.0,
                })
            };
            let pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("gonedark.shell.egui_pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load,
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

/// The title screen's framed card — [`card_frame`] refilled with the translucent [`PANEL_GLASS`] so
/// the live 3D backdrop bleeds faintly through it. Glue (returns an egui builder).
fn glass_card_frame() -> egui::Frame {
    card_frame().fill(PANEL_GLASS)
}

/// A compact secondary "chip" button for the title screen's top-right utility cluster
/// (SETTINGS / PROFILE) — smaller than the full-width [`menu_button`] so it reads as utility chrome
/// rather than a primary action. Rides the [`shell_style`] widget ramp (lifts to PANEL_RAISED + an
/// amber rim on hover). Glue (needs a live `Ui`); the click→action mapping it feeds is what the pure
/// [`resolve_title_action`] seam covers. Text-only, uppercase ASCII — never a risky glyph (the
/// file's tofu caution: only default-font glyphs like U+00B7 are trusted).
fn chip_button(ui: &mut egui::Ui, text: &str) -> bool {
    use egui::{Button, RichText};
    ui.add(
        Button::new(RichText::new(text).color(BONE).size(TYPE_BODY))
            .min_size([104.0, 32.0].into()),
    )
    .clicked()
}

/// The immediate-mode title-screen UI — a real HUD-anchored landing screen drawn over the live 3D
/// [`gonedark_render::title_backdrop::TitleBackdrop`]. Returns the action whose control was clicked
/// this frame.
///
/// Layout (four floating [`egui::Area`]s anchored to the corners over the backdrop, so the central
/// field stays transparent and the 3D shows through — there is deliberately **no** opaque
/// CentralPanel fill here):
///  - **top-left**  — the brand: GOING DARK hero + amber rule + the COMMAND · EMBODY tagline;
///  - **top-right** — a compact SETTINGS / PROFILE utility chip row;
///  - **bottom-left** — the DEPLOY cluster: CAMPAIGN (the lone amber CTA), PvE, PvP, then QUIT,
///    in a translucent [`glass_card_frame`] so it reads as a deliberate panel;
///  - **bottom-right** — the muted build stamp, the quiet corner opposite the play cluster.
fn title_ui(ui: &mut egui::Ui, stamp: &str) -> Option<TitleAction> {
    use egui::{Align2, Area, Id, RichText};
    let mut action = None;
    // Areas attach to the context, not the parent `Ui`, so they float over the (transparent) root
    // and composite over the backdrop. Clone the ctx so each `.show` is independent.
    let ctx = ui.ctx().clone();

    // ---- Brand, top-left -------------------------------------------------------------------------
    Area::new(Id::new("title.brand"))
        .anchor(Align2::LEFT_TOP, egui::vec2(40.0, 44.0))
        .show(&ctx, |ui| {
            ui.label(
                RichText::new("GOING DARK")
                    .color(BONE)
                    .size(TYPE_DISPLAY)
                    .strong(),
            );
            ui.add_space(10.0);
            accent_rule(ui, 150.0);
            ui.add_space(10.0);
            ui.label(
                // U+00B7 middle dot (the same glyph the build stamp uses) — proven to render in
                // egui's default font, so the tagline can never tofu.
                RichText::new("COMMAND \u{00B7} EMBODY")
                    .color(ASH)
                    .size(TYPE_SUBHEAD),
            );
        });

    // ---- Utility chips, top-right ----------------------------------------------------------------
    Area::new(Id::new("title.utility"))
        .anchor(Align2::RIGHT_TOP, egui::vec2(-32.0, 32.0))
        .show(&ctx, |ui| {
            ui.horizontal(|ui| {
                if chip_button(ui, "SETTINGS") {
                    action = Some(TitleAction::Settings);
                }
                if chip_button(ui, "PROFILE") {
                    action = Some(TitleAction::Profile);
                }
            });
        });

    // ---- Deploy cluster, bottom-left -------------------------------------------------------------
    Area::new(Id::new("title.deploy"))
        .anchor(Align2::LEFT_BOTTOM, egui::vec2(40.0, -40.0))
        .show(&ctx, |ui| {
            glass_card_frame().show(ui, |ui| {
                ui.label(
                    RichText::new("DEPLOY")
                        .color(ASH)
                        .size(TYPE_SUBHEAD)
                        .strong(),
                );
                ui.add_space(6.0);
                accent_rule(ui, 72.0);
                ui.add_space(14.0);
                // One amber call-to-action (CAMPAIGN); the other modes are neutral secondaries; QUIT
                // is the quiet tertiary at the foot.
                if menu_button(ui, "CAMPAIGN", Emphasis::Primary) {
                    action = Some(TitleAction::Campaign);
                }
                ui.add_space(10.0);
                if menu_button(ui, "PvE", Emphasis::Secondary) {
                    action = Some(TitleAction::Pve);
                }
                ui.add_space(10.0);
                if menu_button(ui, "PvP", Emphasis::Secondary) {
                    action = Some(TitleAction::Pvp);
                }
                ui.add_space(14.0);
                if menu_button(ui, "QUIT", Emphasis::Tertiary) {
                    action = Some(TitleAction::Quit);
                }
            });
        });

    // ---- Build stamp, bottom-right (the quiet corner opposite the play cluster) -------------------
    Area::new(Id::new("title.stamp"))
        .anchor(Align2::RIGHT_BOTTOM, egui::vec2(-28.0, -24.0))
        .show(&ctx, |ui| {
            ui.label(RichText::new(stamp).color(MUTED).size(TYPE_CAPTION));
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
    fn every_play_mode_opens_the_gunsmith() {
        // All three play modes currently share the one gunsmith→match flow (mode divergence is
        // future work), so each must route through the loadout screen first — Deploy from there is
        // what creates the `Game`.
        for mode in [TitleAction::Campaign, TitleAction::Pve, TitleAction::Pvp] {
            assert_eq!(
                resolve_title_action(mode),
                HostTransition::OpenLoadout,
                "{mode:?} must open the gunsmith"
            );
        }
    }

    #[test]
    fn settings_opens_settings() {
        assert_eq!(
            resolve_title_action(TitleAction::Settings),
            HostTransition::OpenSettings
        );
    }

    #[test]
    fn profile_opens_profile() {
        assert_eq!(
            resolve_title_action(TitleAction::Profile),
            HostTransition::OpenProfile
        );
    }

    #[test]
    fn quit_exits() {
        assert_eq!(resolve_title_action(TitleAction::Quit), HostTransition::Exit);
    }

    #[test]
    fn pointer_maps_to_centre_corners_and_flips_y() {
        // A 800x600-point surface. The centre is the NDC origin; corners map to ±1 with y up.
        let size = [800.0, 600.0];
        let approx = |a: [f32; 2], b: [f32; 2]| {
            (a[0] - b[0]).abs() < 1e-5 && (a[1] - b[1]).abs() < 1e-5
        };
        assert!(approx(pointer_to_ndc([400.0, 300.0], size), [0.0, 0.0]));
        // Top-left pixel (0,0) → NDC (-1, +1): y is flipped (egui y-down → NDC y-up).
        assert!(approx(pointer_to_ndc([0.0, 0.0], size), [-1.0, 1.0]));
        // Bottom-right pixel → NDC (+1, -1).
        assert!(approx(pointer_to_ndc([800.0, 600.0], size), [1.0, -1.0]));
    }

    #[test]
    fn pointer_to_ndc_guards_a_zero_size_surface() {
        // A not-yet-sized surface (0x0) must not divide by zero — it degrades to a finite result.
        let ndc = pointer_to_ndc([10.0, 10.0], [0.0, 0.0]);
        assert!(ndc[0].is_finite() && ndc[1].is_finite());
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
