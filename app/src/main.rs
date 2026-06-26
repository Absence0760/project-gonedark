//! Desktop host: a `winit` 0.30 `ApplicationHandler` that boots into the native **app shell** —
//! the egui title screen ([D36](../docs/decisions.md)) — and, on **Start**, drives the shared
//! [`gonedark_engine::Game`].
//!
//! Two host screens, the desktop counterpart of Android's `MainActivity → NativeActivity` split
//! ([D35](../docs/decisions.md)): the out-of-match **Title** screen (egui, in [`shell`]) and the
//! in-match **Game** (the shared engine loop — deterministic fixed-tick sim, render interpolation
//! (invariant #4), the embodiment input-source swap (invariant #5)). The shell holds no game logic
//! and reaches `core` only through the `core::shell` seam; today **Start** just creates the default
//! `Game` (match-configuration handoff is deferred with match-setup, Q5).
//!
//! This binary owns only the desktop concerns: the window, the wgpu surface, input plumbing, the
//! egui shell, and the wall clock that feeds per-frame `dt` into the engine's fixed-tick accumulator.

use gonedark_engine::{Game, OverlayClick, Scene, DEFAULT_SEED};
use gonedark_pal_desktop::{DesktopAudio, DesktopInput, DesktopRenderSurface};
use std::sync::Arc;
use std::time::Instant;
use winit::application::ApplicationHandler;
use winit::event::{DeviceEvent, DeviceId, ElementState, WindowEvent};
use winit::event_loop::{ActiveEventLoop, EventLoop};
use winit::keyboard::{KeyCode, PhysicalKey};
use winit::window::{CursorGrabMode, Fullscreen, Window, WindowAttributes, WindowId};

mod shell;
use shell::{build_channel, build_stamp, resolve_title_action, EguiShell, HostTransition};

/// Which host screen is up: the out-of-match title shell, or a running match. Entering a match
/// lazily constructs `Game` (it needs the GPU device that only exists after `resumed`).
enum Screen {
    Title,
    // `Game` is large (the renderer + sim state); box it so the idle `Title` variant doesn't carry
    // that footprint around (clippy::large_enum_variant).
    InMatch(Box<Game>),
}

/// The desktop host: the wgpu surface, the egui shell, the current host screen, input/audio, and
/// the wall clock for per-frame dt. The `Game` lives inside `Screen::InMatch` and is created on
/// Start, so a fresh launch sits on the title screen with no sim running.
struct App {
    surface: Option<DesktopRenderSurface>,
    shell: Option<EguiShell>,
    screen: Screen,
    input: DesktopInput,
    /// The desktop audio sink handed into `Game::frame` for the embodied mix (worker 3).
    audio: DesktopAudio,
    last_frame: Instant,

    /// Whether the OS cursor is currently locked+hidden. Tracked so we only call the (relatively
    /// costly) winit grab/visibility setters on a *change*, not every frame. Cursor capture is a
    /// pure desktop-host concern — it never touches the sim — so it lives here, not in the engine.
    cursor_captured: bool,
    /// Momentary "free the cursor" request — true while **Left Alt** is held (released on key-up).
    /// Lets an embodied player hand the pointer back transiently (e.g. to alt-tab) without opening
    /// the pause menu. A shell overlay (pause / reconnect / summary) frees the cursor on its own.
    alt_held: bool,
    /// Whether the window is currently in borderless fullscreen. Toggled by **F11** on any screen.
    /// A pure window concern — it never touches the sim — so it lives on the host like cursor state.
    fullscreen: bool,

    /// Which [`Scene`] a **Start** boots into — `Scene::Default` (the demo skirmish) unless the
    /// `--scene <name>` launch flag selected a debug sandbox (e.g. `--scene duel`). A pure host
    /// launch choice; it only picks which `Game::new_scene` seeding runs.
    scene: Scene,
}

impl App {
    fn new(scene: Scene) -> Self {
        App {
            surface: None,
            shell: None,
            screen: Screen::Title,
            input: DesktopInput::new(),
            audio: DesktopAudio::new(),
            last_frame: Instant::now(),
            cursor_captured: false,
            alt_held: false,
            fullscreen: false,
            scene,
        }
    }

    /// Toggle borderless fullscreen. `Fullscreen::Borderless(None)` takes the window's current
    /// monitor — no mode change, so it's instant and plays nice with the compositor (the right
    /// default on the dev box's Wayland session). Available on any screen via F11; cursor capture is
    /// orthogonal (`sync_cursor` reconciles it each frame regardless of window mode).
    fn toggle_fullscreen(&mut self) {
        let Some(surface) = self.surface.as_ref() else {
            return;
        };
        self.fullscreen = !self.fullscreen;
        let mode = self.fullscreen.then(|| Fullscreen::Borderless(None));
        surface.window().set_fullscreen(mode);
    }

    /// Desktop-host-only keys that apply on **every** screen (title or match): **F11** toggles
    /// borderless fullscreen. Like the cursor keys, these are not in the sim keymap, so handling them
    /// on the host leaves the deterministic input frame untouched.
    fn handle_global_keys(&mut self, event: &WindowEvent) {
        if let WindowEvent::KeyboardInput { event: key, .. } = event {
            if key.state == ElementState::Pressed
                && !key.repeat
                && key.physical_key == PhysicalKey::Code(KeyCode::F11)
            {
                self.toggle_fullscreen();
            }
        }
    }

    /// Lock+hide the OS cursor while embodied so mouse motion drives the FPS look (raw device
    /// deltas) instead of the pointer drifting across on-screen items — and hand it back the moment
    /// the player surfaces, opens a shell overlay (pause / reconnect / summary), or holds Left-Alt.
    /// Idempotent: it only calls the winit setters when the desired state differs from the last one.
    ///
    /// `CursorGrabMode::Locked` (pointer pinned in place) is the Wayland/macOS path; X11 only
    /// supports `Confined`, so we fall back to it there. Either way the cursor is hidden and look
    /// reads from raw `DeviceEvent::MouseMotion`, so both behave the same for the player.
    fn sync_cursor(&mut self) {
        let Some(surface) = self.surface.as_ref() else {
            return;
        };
        let embodied = matches!(&self.screen, Screen::InMatch(game) if game.is_embodied());
        // A shell overlay (pause / reconnect / post-match summary) must free the cursor so the
        // player can click its buttons — even though they may have opened it while embodied.
        let overlay_up = matches!(&self.screen, Screen::InMatch(game) if game.shell_overlay_active());
        let cursor_free = self.alt_held || overlay_up;
        let want = want_cursor_capture(embodied, cursor_free);
        if want == self.cursor_captured {
            return;
        }
        let window = surface.window();
        if want {
            let _ = window
                .set_cursor_grab(CursorGrabMode::Locked)
                .or_else(|_| window.set_cursor_grab(CursorGrabMode::Confined));
            window.set_cursor_visible(false);
        } else {
            let _ = window.set_cursor_grab(CursorGrabMode::None);
            window.set_cursor_visible(true);
        }
        self.cursor_captured = want;
    }

    /// Desktop-host-only key handling for a running match: the pause/surrender entry + cursor
    /// controls. **Esc** toggles the in-session pause overlay (`Game::toggle_pause` — open the menu
    /// while playing, close it while paused; the menu's own **Surrender** button ends the match);
    /// **Left Alt** transiently frees the cursor while held. Neither reaches the sim (they're not in
    /// the keymap `DesktopInput` decodes) — pause is a host-side `SessionAction` that never enters
    /// the deterministic input frame, so the checksum stream is untouched.
    fn handle_host_keys(&mut self, event: &WindowEvent) {
        if let WindowEvent::KeyboardInput { event: key, .. } = event {
            let pressed = key.state == ElementState::Pressed;
            match key.physical_key {
                PhysicalKey::Code(KeyCode::Escape) => {
                    if pressed && !key.repeat {
                        if let Screen::InMatch(game) = &mut self.screen {
                            game.toggle_pause();
                        }
                    }
                }
                PhysicalKey::Code(KeyCode::AltLeft) => self.alt_held = pressed,
                _ => {}
            }
        }
    }

    /// One presented frame. On `Title` the egui shell draws and may return a host transition
    /// (start a match / open settings / quit); on `InMatch` the shared engine loop runs as before.
    fn render_frame(&mut self, event_loop: &ActiveEventLoop) {
        let now = Instant::now();
        let dt = now.duration_since(self.last_frame).as_secs_f32();
        self.last_frame = now;

        let Some(surface) = self.surface.as_mut() else {
            return;
        };

        // Run the current screen; defer any state transition until the screen borrow is released.
        let mut transition = None;
        match &mut self.screen {
            Screen::Title => {
                if let Some(sh) = self.shell.as_mut() {
                    if let Some(action) = sh.draw_title(surface) {
                        transition = Some(resolve_title_action(action));
                    }
                }
            }
            Screen::InMatch(game) => {
                let mut input = self.input.drain_frame();
                let viewport = surface.size();
                // Shell overlay buttons (pause / reconnect / post-match summary). A click while an
                // overlay is up belongs to that overlay, not the match world: hit-test it in NDC and
                // either feed the resolved session action back to the shell or — for the terminal
                // summary's DISMISS — leave the match. When the click is consumed here we strip it
                // from `input` so the same release doesn't also drive a world selection underneath.
                if input.pointer_up {
                    if let Some((px, py)) = input.pointer {
                        let (w, h) = viewport;
                        let ndc = (2.0 * px / w as f32 - 1.0, 1.0 - 2.0 * py / h as f32);
                        match game.overlay_click(ndc) {
                            Some(OverlayClick::Session(action)) => {
                                game.apply_session_action(action);
                                input.pointer_up = false;
                                input.pointer_down = false;
                            }
                            Some(OverlayClick::Dismiss) => {
                                transition = Some(HostTransition::ExitToTitle);
                            }
                            None => {}
                        }
                    }
                }
                // While a shell overlay is up the match is frozen underneath it: drop the rest of
                // this frame's input so a click that missed an overlay button (or a held key) can't
                // drive selection / fire the weapon / pan the camera behind the menu. The overlay's
                // own buttons were already resolved above, before this blanking. (Single-player also
                // halts the tick via `halts_local_tick`; this stops *world input*, not the clock.)
                if game.shell_overlay_active() {
                    input = Default::default();
                }
                // Skip the match frame entirely when we're leaving it this turn; the title screen
                // draws next frame.
                if transition.is_none() {
                    if let Some((frame, view)) = surface.acquire() {
                        game.frame(
                            &input,
                            dt,
                            viewport,
                            surface.device(),
                            surface.queue(),
                            &view,
                            &mut self.audio,
                        );
                        surface.present(frame);
                    }
                }
            }
        }

        match transition {
            Some(HostTransition::EnterMatch) => {
                let surface = self.surface.as_ref().expect("surface exists in resumed");
                let game = Game::new_scene(
                    surface.device(),
                    surface.format(),
                    DEFAULT_SEED,
                    self.scene,
                );
                self.screen = Screen::InMatch(Box::new(game));
                // Don't charge the time spent on the title screen to the first sim tick.
                self.last_frame = Instant::now();
            }
            // Settings surface not built yet (phase-4-plan §2 surface 3) — a no-op placeholder.
            Some(HostTransition::OpenSettings) => {}
            Some(HostTransition::Exit) => event_loop.exit(),
            // Drop the `Game` and return to the title screen (the post-match DISMISS path).
            Some(HostTransition::ExitToTitle) => {
                self.screen = Screen::Title;
                self.last_frame = Instant::now();
            }
            None => {}
        }

        // Reconcile cursor capture against the (possibly just-changed) embodiment/screen state, so a
        // surface, death-eject, or match-exit hands the pointer back within the same frame.
        self.sync_cursor();
    }
}

/// Whether the desktop host should lock+hide the OS cursor this frame. True only while **embodied**
/// and the player has NOT requested a free cursor (Esc toggle or held Left-Alt). In the command
/// (RTS) view the cursor is always free — it *is* the pointer; capture is purely the embodied-FPS
/// concern. Pure (no winit types) so it is unit-tested without a real `Window`.
fn want_cursor_capture(embodied: bool, cursor_free: bool) -> bool {
    embodied && !cursor_free
}

impl ApplicationHandler for App {
    /// Create the window + GPU surface + the egui shell once the event loop is ready (`Game` is
    /// created later, on Start). On desktop `resumed` fires once at startup; guard a redundant
    /// second create.
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.surface.is_some() {
            return;
        }
        let window = event_loop
            .create_window(WindowAttributes::default().with_title("Going Dark"))
            .expect("create winit window");
        let window: Arc<Window> = Arc::new(window);

        let surface = DesktopRenderSurface::new(window);
        let stamp = build_stamp(build_channel(cfg!(debug_assertions)), env!("CARGO_PKG_VERSION"));
        let shell = EguiShell::new(surface.device(), surface.format(), surface.window(), stamp);

        self.surface = Some(surface);
        self.shell = Some(shell);
        // Reset the clock so window-creation latency isn't charged to the first frame.
        self.last_frame = Instant::now();
    }

    fn window_event(
        &mut self,
        event_loop: &ActiveEventLoop,
        _window_id: WindowId,
        event: WindowEvent,
    ) {
        // Global host keys (F11 fullscreen) apply on every screen, before screen-specific routing.
        self.handle_global_keys(&event);

        // Route input by screen: the egui shell gets UI events on the title screen; the game input
        // accumulator gets them in a match. (A stray event in the other state is simply ignored, so
        // nothing leaks between the shell and the sim.)
        match self.screen {
            Screen::Title => {
                if let (Some(sh), Some(surface)) = (self.shell.as_mut(), self.surface.as_ref()) {
                    sh.on_window_event(surface.window(), &event);
                }
            }
            Screen::InMatch(_) => {
                // Host-level keys (Esc → pause toggle, Left-Alt → free cursor) are consumed here,
                // then the event also feeds the sim input accumulator (they're not in its keymap, so
                // no double-use).
                self.handle_host_keys(&event);
                self.input.handle_window_event(&event);
            }
        }

        match event {
            WindowEvent::CloseRequested => event_loop.exit(),
            WindowEvent::Resized(size) => {
                if let Some(surface) = self.surface.as_mut() {
                    surface.resize(size.width, size.height);
                }
            }
            WindowEvent::RedrawRequested => self.render_frame(event_loop),
            _ => {}
        }
    }

    /// Raw mouse-look (the FPS look axis) arrives as device events — only meaningful in a match.
    fn device_event(
        &mut self,
        _event_loop: &ActiveEventLoop,
        _device_id: DeviceId,
        event: DeviceEvent,
    ) {
        // Only feed raw mouse-look while the cursor is actually captured: when the player has freed
        // the pointer for UI (Esc / Alt) or is in the command view, moving the mouse must NOT turn
        // the camera.
        if self.cursor_captured {
            self.input.handle_device_event(&event);
        }
    }

    /// Keep a continuous render loop: request another redraw as soon as the queue drains. (The egui
    /// title screen also needs steady repaints for hover/click feedback.)
    fn about_to_wait(&mut self, _event_loop: &ActiveEventLoop) {
        if let Some(surface) = self.surface.as_ref() {
            surface.window().request_redraw();
        }
    }
}

/// Extract the `--scene <name>` / `--scene=<name>` launch token from CLI args, if present. Pure
/// (no env / no `Scene::parse`), so it's host-tested without a window; `main` resolves the token to
/// a [`Scene`] and warns on an unknown name.
fn scene_token(args: &[String]) -> Option<String> {
    let mut it = args.iter();
    while let Some(a) = it.next() {
        if let Some(name) = a.strip_prefix("--scene=") {
            return Some(name.to_string());
        }
        if a == "--scene" {
            return it.next().cloned();
        }
    }
    None
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let scene = match scene_token(&args) {
        Some(tok) => Scene::parse(&tok).unwrap_or_else(|| {
            eprintln!("unknown --scene {tok:?}; using default (known scenes: default, duel)");
            Scene::Default
        }),
        None => Scene::Default,
    };

    let event_loop = EventLoop::new().expect("create winit event loop");
    let mut app = App::new(scene);
    event_loop.run_app(&mut app).expect("run winit app");
}

#[cfg(test)]
mod cursor_tests {
    //! The cursor-capture *decision* is the only logic worth testing here; the winit grab/visibility
    //! calls and the key/screen plumbing around it are thin, un-constructible glue (`Window`,
    //! `KeyEvent`, `ActiveEventLoop` have no public test constructors), so they're exercised by
    //! running the app, not unit tests — matching this crate's existing testable-seam convention.
    use super::want_cursor_capture;

    #[test]
    fn captures_only_while_embodied_and_not_freed() {
        // Embodied with the cursor not freed → lock+hide it (the FPS look path).
        assert!(want_cursor_capture(true, false));
        // Embodied but the player asked for the cursor (Esc toggle or held Alt) → hand it back.
        assert!(!want_cursor_capture(true, true));
    }

    #[test]
    fn never_captures_in_command_view() {
        // Not embodied (RTS command view, or title) → the cursor is always the free pointer,
        // regardless of the free-cursor request.
        assert!(!want_cursor_capture(false, false));
        assert!(!want_cursor_capture(false, true));
    }
}

#[cfg(test)]
mod scene_arg_tests {
    use super::scene_token;

    fn args(xs: &[&str]) -> Vec<String> {
        xs.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn extracts_scene_token_in_both_forms() {
        assert_eq!(scene_token(&args(&["--scene", "duel"])).as_deref(), Some("duel"));
        assert_eq!(scene_token(&args(&["--scene=duel"])).as_deref(), Some("duel"));
        // Other flags around it don't interfere.
        assert_eq!(
            scene_token(&args(&["--foo", "--scene", "default", "--bar"])).as_deref(),
            Some("default"),
        );
    }

    #[test]
    fn absent_or_dangling_scene_flag_is_none() {
        assert_eq!(scene_token(&args(&[])), None);
        assert_eq!(scene_token(&args(&["--fullscreen"])), None);
        // `--scene` with no following value: nothing to take.
        assert_eq!(scene_token(&args(&["--scene"])), None);
    }
}
