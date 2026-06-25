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

use gonedark_engine::{Game, DEFAULT_SEED};
use gonedark_pal_desktop::{DesktopAudio, DesktopInput, DesktopRenderSurface};
use std::sync::Arc;
use std::time::Instant;
use winit::application::ApplicationHandler;
use winit::event::{DeviceEvent, DeviceId, WindowEvent};
use winit::event_loop::{ActiveEventLoop, EventLoop};
use winit::window::{Window, WindowAttributes, WindowId};

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
}

impl App {
    fn new() -> Self {
        App {
            surface: None,
            shell: None,
            screen: Screen::Title,
            input: DesktopInput::new(),
            audio: DesktopAudio::new(),
            last_frame: Instant::now(),
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
                let input = self.input.drain_frame();
                let viewport = surface.size();
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

        match transition {
            Some(HostTransition::EnterMatch) => {
                let surface = self.surface.as_ref().expect("surface exists in resumed");
                let game = Game::new(surface.device(), surface.format(), DEFAULT_SEED);
                self.screen = Screen::InMatch(Box::new(game));
                // Don't charge the time spent on the title screen to the first sim tick.
                self.last_frame = Instant::now();
            }
            // Settings surface not built yet (phase-4-plan §2 surface 3) — a no-op placeholder.
            Some(HostTransition::OpenSettings) => {}
            Some(HostTransition::Exit) => event_loop.exit(),
            None => {}
        }
    }
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
        // Route input by screen: the egui shell gets UI events on the title screen; the game input
        // accumulator gets them in a match. (A stray event in the other state is simply ignored, so
        // nothing leaks between the shell and the sim.)
        match self.screen {
            Screen::Title => {
                if let (Some(sh), Some(surface)) = (self.shell.as_mut(), self.surface.as_ref()) {
                    sh.on_window_event(surface.window(), &event);
                }
            }
            Screen::InMatch(_) => self.input.handle_window_event(&event),
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
        if matches!(self.screen, Screen::InMatch(_)) {
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

fn main() {
    let event_loop = EventLoop::new().expect("create winit event loop");
    let mut app = App::new();
    event_loop.run_app(&mut app).expect("run winit app");
}
