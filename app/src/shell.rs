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

use gonedark_pal_desktop::DesktopRenderSurface;
use winit::window::Window;

// ---- The pure seam (unit-tested) ----------------------------------------------------------------

/// A top-level action the player can pick on the title screen.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum TitleAction {
    /// Start a match — enter the shared engine.
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
    /// Lazily create `engine::Game` and switch the host to the in-match screen.
    EnterMatch,
    /// Open the (not-yet-built) settings surface — a no-op placeholder today.
    OpenSettings,
    /// Tear down and exit the app.
    Exit,
}

/// Map a title action to the host transition it triggers (the pure run-loop decision).
pub fn resolve_title_action(action: TitleAction) -> HostTransition {
    match action {
        TitleAction::Start => HostTransition::EnterMatch,
        TitleAction::Settings => HostTransition::OpenSettings,
        TitleAction::Quit => HostTransition::Exit,
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

    /// Draw the title screen for one frame and return a clicked [`TitleAction`], if any. Renders the
    /// egui output into a freshly-acquired surface frame and presents it. Pure presentation — it
    /// never touches sim state.
    pub fn draw_title(&mut self, surface: &mut DesktopRenderSurface) -> Option<TitleAction> {
        let ctx = self.ctx.clone();

        // Run egui (needs the window for input gather + platform output).
        let raw_input = self.state.take_egui_input(surface.window());
        let mut action = None;
        let full_output = ctx.run_ui(raw_input, |ui| {
            action = title_ui(ui, &self.stamp);
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
            label: Some("gonedark.shell.title"),
        });
        let user_cmds =
            self.renderer
                .update_buffers(device, queue, &mut encoder, &paint_jobs, &screen);
        {
            let pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("gonedark.shell.title_pass"),
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
    egui::CentralPanel::default().show_inside(ui, |ui| {
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

#[cfg(test)]
mod tests {
    //! The pure seam only — the egui glue (`EguiShell`/`title_ui`) needs a GPU + window and is the
    //! exempt device-gated chrome (D32 / CLAUDE.md testing rule).
    use super::*;

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
    fn start_enters_a_match() {
        assert_eq!(resolve_title_action(TitleAction::Start), HostTransition::EnterMatch);
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
}
