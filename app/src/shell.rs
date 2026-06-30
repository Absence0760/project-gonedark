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

// ---- The egui glue (device-gated chrome; exempt from unit tests) --------------------------------

// The "going-dark" palette — a near-black field, dim chrome, one amber alert accent (the game's
// directional-alert colour). Shared with the Android shell's Material3 theme (D35) by intent.
const INK: egui::Color32 = egui::Color32::from_rgb(0x07, 0x09, 0x0C);
const PANEL: egui::Color32 = egui::Color32::from_rgb(0x12, 0x18, 0x20);
const BONE: egui::Color32 = egui::Color32::from_rgb(0xE7, 0xEC, 0xEF);
const ASH: egui::Color32 = egui::Color32::from_rgb(0x8A, 0x94, 0x9C);
const AMBER: egui::Color32 = egui::Color32::from_rgb(0xE0, 0x79, 0x1F);

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
        let mut visuals = egui::Visuals::dark();
        visuals.panel_fill = INK;
        ctx.set_visuals(visuals);

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

/// The immediate-mode title-screen UI, drawn into the root [`egui::Ui`] for the frame. Returns the
/// action whose button was clicked this frame.
fn title_ui(ui: &mut egui::Ui, stamp: &str) -> Option<TitleAction> {
    use egui::{Button, RichText};
    let mut action = None;

    // Everything lives in one full-screen CentralPanel — a single centered column (title, tagline,
    // the three actions, then the build stamp). One panel keeps the layout robust (no panel-space
    // splitting); the wgpu clear already paints the ink background behind it.
    egui::CentralPanel::default().show(ui, |ui| {
        let h = ui.available_height();
        ui.vertical_centered(|ui| {
            ui.add_space(h * 0.20);
            ui.label(RichText::new("GOING DARK").color(BONE).size(48.0).strong());
            ui.add_space(6.0);
            ui.label(RichText::new("COMMAND · EMBODY").color(ASH).size(15.0));
            ui.add_space(36.0);

            let button = |ui: &mut egui::Ui, text: &str, fill, fg| {
                ui.add_sized(
                    [240.0, 48.0],
                    Button::new(RichText::new(text).color(fg).size(16.0)).fill(fill),
                )
                .clicked()
            };

            if button(ui, "START", AMBER, INK) {
                action = Some(TitleAction::Start);
            }
            ui.add_space(12.0);
            if button(ui, "SETTINGS", PANEL, BONE) {
                action = Some(TitleAction::Settings);
            }
            ui.add_space(12.0);
            if button(ui, "QUIT", PANEL, ASH) {
                action = Some(TitleAction::Quit);
            }
            ui.add_space(28.0);
            ui.label(RichText::new(stamp).color(ASH).size(12.0));
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
            ui.add_space(h * 0.10);
            ui.label(RichText::new("GUNSMITH").color(BONE).size(40.0).strong());
            ui.add_space(6.0);
            ui.label(
                RichText::new(
                    "Every attachment is a sidegrade — it spends one stat to buy another. \
                     No build is strictly better than any other.",
                )
                .color(ASH)
                .size(13.0),
            );
            ui.add_space(28.0);

            // One aligned row per attachment slot. The on-screen index `i` is exactly the index the
            // editor's `apply_input` routes on (`LoadoutSlot::from_index`), so the cycler maps 1:1.
            for (i, &slot) in LoadoutSlot::ALL.iter().enumerate() {
                ui.horizontal(|ui| {
                    ui.add_sized(
                        [108.0, 30.0],
                        Label::new(RichText::new(slot.label()).color(BONE).size(16.0).strong()),
                    );
                    if ui
                        .add_sized([34.0, 30.0], Button::new(RichText::new("<").color(BONE)))
                        .clicked()
                    {
                        action = Some(LoadoutAction::Cycle {
                            slot_index: i,
                            forward: false,
                        });
                    }
                    ui.add_sized(
                        [150.0, 30.0],
                        Label::new(
                            RichText::new(editor.option_label(slot)).color(AMBER).size(15.0),
                        ),
                    );
                    if ui
                        .add_sized([34.0, 30.0], Button::new(RichText::new(">").color(BONE)))
                        .clicked()
                    {
                        action = Some(LoadoutAction::Cycle {
                            slot_index: i,
                            forward: true,
                        });
                    }
                    ui.add_sized(
                        [168.0, 30.0],
                        Label::new(RichText::new(slot_trade_hint(slot)).color(ASH).size(12.0)),
                    );
                });
                ui.add_space(10.0);
            }

            ui.add_space(22.0);
            let button = |ui: &mut egui::Ui, text: &str, fill, fg| {
                ui.add_sized(
                    [240.0, 46.0],
                    Button::new(RichText::new(text).color(fg).size(16.0)).fill(fill),
                )
                .clicked()
            };
            if button(ui, "DEPLOY", AMBER, INK) {
                action = Some(LoadoutAction::Deploy);
            }
            ui.add_space(10.0);
            if button(ui, "RESET", PANEL, BONE) {
                action = Some(LoadoutAction::Reset);
            }
            ui.add_space(10.0);
            if button(ui, "BACK", PANEL, ASH) {
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
