//! The egui glue: the [`EguiShell`] — an egui context, the winit→egui input bridge, the egui-wgpu
//! renderer, and the live 3D title backdrop — plus the per-screen `draw_*` dispatch and the title
//! screen's immediate-mode layout ([`title_ui`]). Device-gated chrome, exempt from unit tests; the
//! per-screen *logic* it drives lives in the pure `*_ui` builders and the pure action seams in the
//! sibling modules.

use crate::shell::about::about_ui;
use crate::shell::army::{army_select_ui, ArmySelectAction, ArmySelectState};
use crate::shell::briefing::{briefing_ui, BriefingAction};
use crate::shell::loadout::{loadout_ui, LoadoutAction};
use crate::shell::mission_select::{mission_select_ui, MissionSelectAction};
use crate::shell::mode_select::{mode_select_ui, ModeSelectAction};
use crate::shell::profile::{profile_ui, ProfileAction, ProfileState};
use crate::shell::settings::{settings_ui, SettingsAction, SettingsState};
use crate::shell::theme::*;
use crate::shell::transitions::TitleAction;
use crate::shell::util::pointer_to_ndc;
use crate::shell::widgets::*;

use gonedark_core::campaign::{Campaign, Difficulty, NodeId};
use gonedark_engine::keybind::GameAction;
use gonedark_engine::loadout_ui::LoadoutEditor;
use gonedark_pal_desktop::DesktopRenderSurface;
use gonedark_render::title_backdrop::TitleBackdrop;
use winit::window::Window;

/// The egui-backed title screen: an egui context, the winit→egui input bridge, and the egui-wgpu
/// renderer that paints into the same surface the engine uses. Owns no game state.
pub(crate) struct EguiShell {
    ctx: egui::Context,
    state: egui_winit::State,
    renderer: egui_wgpu::Renderer,
    stamp: String,
    /// The live 3D title backdrop (`render` crate). Painted behind the title-screen egui pass (which
    /// then composites with `LoadOp::Load`). `Option` so a future fallible build could degrade to a
    /// flat-clear title without panicking the shell — the pinned `new` is infallible today, so it is
    /// always `Some`. Only the title screen uses it; the loadout screen clears its own ink panel.
    backdrop: Option<TitleBackdrop>,
    /// Transient Settings **rebind-editor** state (D75 follow-up): the action currently capturing a
    /// key (`Some` while the row shows "press a key…"), and the last conflict `(action, owner)` to
    /// surface as feedback. Ephemeral UI interaction state — not a persisted pref (the map itself
    /// lives on `SettingsState::keybinds`), so it rides the device-gated glue, not the pure model.
    rebinding: Option<GameAction>,
    rebind_conflict: Option<(GameAction, GameAction)>,
}

impl EguiShell {
    /// Build the shell against the desktop surface's device/format and the window (for input/DPI).
    /// `stamp` is the already-formatted build/version line (see
    /// [`build_stamp`](crate::shell::util::build_stamp)).
    /// `scene_format` is the swapchain's (sRGB) format — the target for the 3D title backdrop, which
    /// renders into the sRGB view. `egui_format` is its linear twin (`shell_format`) — the target for
    /// the egui renderer, which draws into the linear view. They differ so egui gets its preferred
    /// gamma-space path while the backdrop stays gamma-correct (see `run_and_paint`).
    pub(crate) fn new(
        device: &wgpu::Device,
        scene_format: wgpu::TextureFormat,
        egui_format: wgpu::TextureFormat,
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
        let renderer =
            egui_wgpu::Renderer::new(device, egui_format, egui_wgpu::RendererOptions::default());

        // Build the live 3D title backdrop against the swapchain's sRGB scene format (it renders into
        // the sRGB view). Infallible per the pinned API, so always `Some` today.
        let backdrop = Some(TitleBackdrop::new(device, scene_format));

        EguiShell {
            ctx,
            state,
            renderer,
            stamp,
            backdrop,
            rebinding: None,
            rebind_conflict: None,
        }
    }

    /// Feed one winit window event to egui (pointer/keys). Returns whether egui consumed it.
    pub(crate) fn on_window_event(
        &mut self,
        window: &Window,
        event: &winit::event::WindowEvent,
    ) -> bool {
        self.state.on_window_event(window, event).consumed
    }

    /// Whether the Settings rebind editor is mid-capture (a row is armed, waiting for a key). The
    /// host reads this to suppress the global F11 fullscreen hotkey during capture — otherwise
    /// pressing F11 to *bind* it to an action would also silently flip the window into fullscreen.
    pub(crate) fn is_capturing_rebind(&self) -> bool {
        self.rebinding.is_some()
    }

    /// Draw the title screen for one frame and return a clicked [`TitleAction`], if any. Pure
    /// presentation — it never touches sim state.
    pub(crate) fn draw_title(&mut self, surface: &mut DesktopRenderSurface) -> Option<TitleAction> {
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
    pub(crate) fn draw_loadout(
        &mut self,
        surface: &mut DesktopRenderSurface,
        editor: &LoadoutEditor,
    ) -> Option<LoadoutAction> {
        // `with_backdrop = true`: the gunsmith sits in the same translucent card over the live 3D
        // backdrop as every other out-of-match screen, so the whole shell reads as one family (it
        // previously kept an opaque ink panel — the lone odd-one-out surface).
        self.run_and_paint(surface, true, |ui| loadout_ui(ui, editor))
    }

    /// Draw the Settings screen for one frame and return the [`SettingsAction`] whose control was
    /// used, if any. `state` is the host-side preference model (edited in place by the sliders);
    /// `fullscreen` is the host's current window mode (reflected by the video checkbox). Drawn over the
    /// live 3D backdrop so the out-of-match shell stays cohesive. Pure presentation — never the sim.
    pub(crate) fn draw_settings(
        &mut self,
        surface: &mut DesktopRenderSurface,
        state: &mut SettingsState,
        fullscreen: bool,
    ) -> Option<SettingsAction> {
        // Copy the transient rebind-editor state into locals so the paint closure doesn't alias the
        // `&mut self` borrow `run_and_paint` takes (the `stamp` clone pattern), then write it back.
        let mut rebinding = self.rebinding;
        let mut conflict = self.rebind_conflict;
        let action = self.run_and_paint(surface, true, |ui| {
            settings_ui(ui, state, fullscreen, &mut rebinding, &mut conflict)
        });
        self.rebinding = rebinding;
        self.rebind_conflict = conflict;
        action
    }

    /// Draw the player Profile screen for one frame and return the [`ProfileAction`] used, if any.
    /// `profile` is the host-side identity/record (the callsign field edits it in place). Over the
    /// backdrop, same as Settings. Pure presentation.
    pub(crate) fn draw_profile(
        &mut self,
        surface: &mut DesktopRenderSurface,
        profile: &mut ProfileState,
    ) -> Option<ProfileAction> {
        self.run_and_paint(surface, true, |ui| profile_ui(ui, profile))
    }

    /// Draw the **army-select** screen for one frame and return the [`ArmySelectAction`] used, if any.
    /// `state` is the host-side army pick (read here to highlight the current card). Over the live 3D
    /// backdrop, same as the other out-of-match screens. Pure presentation — the decision logic is the
    /// pure [`apply_army_select_action`](crate::shell::army::apply_army_select_action) seam and the sim
    /// routing is the `core::shell` SelectArmy seam.
    pub(crate) fn draw_army_select(
        &mut self,
        surface: &mut DesktopRenderSurface,
        state: &ArmySelectState,
    ) -> Option<ArmySelectAction> {
        self.run_and_paint(surface, true, |ui| army_select_ui(ui, state))
    }

    /// Draw the About / controls-reference screen for one frame. Returns `true` on BACK (the only
    /// control), so the run loop returns to Settings. Static content over the backdrop. Pure.
    pub(crate) fn draw_about(&mut self, surface: &mut DesktopRenderSurface) -> bool {
        let stamp = self.stamp.clone();
        self.run_and_paint(surface, true, |ui| about_ui(ui, &stamp).then_some(()))
            .is_some()
    }

    /// Draw the Pve/Pvp **mode / map select** screen for one frame and return the
    /// [`ModeSelectAction`] used, if any (D81). The mode table is the static
    /// [`SHELL_GAME_MODES`](gonedark_engine::shell_modes::SHELL_GAME_MODES); this holds no host state.
    /// Over the live 3D backdrop, same as the other out-of-match screens. Pure presentation — the
    /// picked mode's scene resolution is the `engine`-tested `GameMode::scene` seam, this is the
    /// device-gated glue.
    pub(crate) fn draw_mode_select(
        &mut self,
        surface: &mut DesktopRenderSurface,
    ) -> Option<ModeSelectAction> {
        self.run_and_paint(surface, true, mode_select_ui)
    }

    /// Draw the Operations-hub **mission-select** screen for one frame and return the
    /// [`MissionSelectAction`] used, if any. `campaign` is the host-side campaign model (read-only
    /// here — it is never sim state, never checksummed). Over the live 3D backdrop, same as the
    /// other out-of-match screens. Pure presentation — the tile-launchable gate lives in the pure
    /// [`playable_node`](crate::shell::mission_select::playable_node) seam, this is the device-gated
    /// glue.
    pub(crate) fn draw_mission_select(
        &mut self,
        surface: &mut DesktopRenderSurface,
        campaign: &Campaign,
    ) -> Option<MissionSelectAction> {
        self.run_and_paint(surface, true, |ui| mission_select_ui(ui, campaign))
    }

    /// Draw the **briefing** screen for `node` for one frame and return the [`BriefingAction`] used,
    /// if any. Reads the node's briefing through [`Campaign::briefing`]; `selected` is the host-side
    /// replay-tier selector the difficulty cycler edits in place. Over the backdrop. Pure
    /// presentation — the decision logic is the pure
    /// [`apply_briefing_action`](crate::shell::briefing::apply_briefing_action) seam.
    pub(crate) fn draw_briefing(
        &mut self,
        surface: &mut DesktopRenderSurface,
        campaign: &Campaign,
        node: NodeId,
        selected: Difficulty,
    ) -> Option<BriefingAction> {
        self.run_and_paint(surface, true, |ui| briefing_ui(ui, campaign, node, selected))
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

        // Apply egui's texture deltas BEFORE acquiring the frame. egui emits the font-atlas upload
        // exactly once (in `textures_delta.set`) and then clears it; the upload only needs the
        // device/queue, not the frame. If we deferred it past `acquire()` we'd drop that one-time
        // delta on any frame `acquire()` returns `None` (as it does while the surface settles on
        // startup), and every later `render()` would report the atlas "Missing" — an invisible shell.
        // Scoped so the device/queue borrows release before the `&mut` `acquire()` below.
        {
            let device = surface.device();
            let queue = surface.queue();
            for (id, delta) in &full_output.textures_delta.set {
                self.renderer.update_texture(device, queue, *id, delta);
            }
        }

        // Acquire the frame (owned — the `&mut` surface borrow ends as this returns). On a miss, still
        // release the textures egui flagged for freeing so they can't leak across skipped frames.
        let Some((frame, view)) = surface.acquire() else {
            for id in &full_output.textures_delta.free {
                self.renderer.free_texture(id);
            }
            return action;
        };

        // A linear (non-sRGB) view of the same swapchain texture for the egui pass — egui blends in
        // gamma space and renders invisibly into the sRGB `view` the backdrop/scene use. See
        // `DesktopRenderSurface::shell_view`. Reads the same pixels the backdrop wrote, so `LoadOp::Load`
        // over the backdrop still composites correctly.
        let egui_view = surface.shell_view(&frame);
        let device = surface.device();
        let queue = surface.queue();

        // Paint the 3D backdrop into the (sRGB) view BEFORE egui (it clears + submits its own encoder),
        // so the egui pass below loads over it. `self.backdrop`/`self.renderer` are disjoint fields, so
        // this split borrow is fine.
        if with_backdrop {
            if let Some(bd) = self.backdrop.as_mut() {
                bd.render(device, queue, &view, (w, h), time, cursor);
            }
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
            // backdrop), preserving the original opaque look. The clear targets the linear
            // (non-sRGB) egui view, so these are gamma-space (raw-byte) values matching the INK
            // panel colour (0x07,0x09,0x0C) — not linear values.
            let load = if with_backdrop {
                wgpu::LoadOp::Load
            } else {
                wgpu::LoadOp::Clear(wgpu::Color {
                    r: 0x07 as f64 / 255.0,
                    g: 0x09 as f64 / 255.0,
                    b: 0x0C as f64 / 255.0,
                    a: 1.0,
                })
            };
            let pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("gonedark.shell.egui_pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &egui_view,
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

/// The immediate-mode title-screen UI — a real HUD-anchored landing screen drawn over the live 3D
/// [`gonedark_render::title_backdrop::TitleBackdrop`]. Returns the action whose control was clicked
/// this frame.
///
/// Layout (four floating [`egui::Area`]s anchored to the corners over the backdrop, so the central
/// field stays transparent and the 3D shows through — there is deliberately **no** opaque
/// CentralPanel fill here):
///  - **top-left**  — the brand: GOING DARK hero + amber rule + the COMMAND · EMBODY tagline;
///  - **top-right** — a compact SETTINGS / PROFILE / ARMY / FIELD MANUAL utility chip row;
///  - **bottom-centre** — the DEPLOY cluster: CAMPAIGN (the lone amber CTA), PvE / PvP, then QUIT,
///    in a translucent [`glass_card_frame`] so it reads as a deliberate panel;
///  - **bottom-right** — the muted build stamp, the quiet corner opposite the play cluster.
pub(crate) fn title_ui(ui: &mut egui::Ui, stamp: &str) -> Option<TitleAction> {
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
                // Uniform width for all chips (fits "FIELD MANUAL", the longest) so they read as a
                // clean pill row, not three short + one wide. A gap separates account utility
                // (SETTINGS/PROFILE) from the pre-match/reference pair (ARMY/FIELD MANUAL).
                const CHIP_W: f32 = 132.0;
                if chip_button(ui, "SETTINGS", CHIP_W) {
                    action = Some(TitleAction::Settings);
                }
                if chip_button(ui, "PROFILE", CHIP_W) {
                    action = Some(TitleAction::Profile);
                }
                ui.add_space(16.0);
                // The army-select entry (US vs FR) — a pre-deploy pick fielded at every match start.
                if chip_button(ui, "ARMY", CHIP_W) {
                    action = Some(TitleAction::Army);
                }
                // The field manual (About) — reachable straight from the title, mirroring Android's
                // title About entry (it is also reachable from Settings).
                if chip_button(ui, "FIELD MANUAL", CHIP_W) {
                    action = Some(TitleAction::About);
                }
            });
        });

    // ---- Deploy cluster, bottom-centre -----------------------------------------------------------
    // Anchored bottom-centre so the play cluster reads as the focal point of the screen rather than
    // a lonely stack in one corner, balancing the brand (top-left) / chips (top-right) / stamp
    // (bottom-right) around it.
    Area::new(Id::new("title.deploy"))
        .anchor(Align2::CENTER_BOTTOM, egui::vec2(0.0, -48.0))
        .show(&ctx, |ui| {
            use egui::Button;
            glass_card_frame().show(ui, |ui| {
                ui.vertical_centered(|ui| {
                    ui.label(
                        RichText::new("DEPLOY")
                            .color(ASH)
                            .size(TYPE_SUBHEAD)
                            .strong(),
                    );
                    ui.add_space(6.0);
                    accent_rule(ui, 72.0);
                    ui.add_space(14.0);
                    // CAMPAIGN is the lone CTA — taller as well as amber, so it outranks the peers by
                    // shape, not just colour.
                    if ui
                        .add(
                            Button::new(RichText::new("CAMPAIGN").color(INK).size(TYPE_BUTTON))
                                .min_size([MENU_BUTTON_W, 58.0].into())
                                .fill(AMBER),
                        )
                        .clicked()
                    {
                        action = Some(TitleAction::Campaign);
                    }
                    ui.add_space(10.0);
                    // PvE / PvP are peers — side by side, so the card gains width instead of a third
                    // stacked full-width row.
                    ui.horizontal(|ui| {
                        let half = (MENU_BUTTON_W - 10.0) / 2.0;
                        if ui
                            .add(
                                Button::new(RichText::new("PvE").color(BONE).size(TYPE_BUTTON))
                                    .min_size([half, 46.0].into()),
                            )
                            .clicked()
                        {
                            action = Some(TitleAction::Pve);
                        }
                        if ui
                            .add(
                                Button::new(RichText::new("PvP").color(BONE).size(TYPE_BUTTON))
                                    .min_size([half, 46.0].into()),
                            )
                            .clicked()
                        {
                            action = Some(TitleAction::Pvp);
                        }
                    });
                    ui.add_space(16.0);
                    // QUIT is the odd one out (exit) — a quiet frameless text control, not a peer of
                    // the three play actions.
                    if ui
                        .add(
                            Button::new(RichText::new("QUIT").color(MUTED).size(TYPE_CAPTION))
                                .frame(false)
                                .min_size([80.0, 28.0].into()),
                        )
                        .clicked()
                    {
                        action = Some(TitleAction::Quit);
                    }
                });
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
