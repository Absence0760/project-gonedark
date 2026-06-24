//! Desktop host: a thin `winit` 0.30 `ApplicationHandler` that drives the shared
//! [`gonedark_engine::Game`] (Phase-1 build-order step 5, docs/phase-1-plan.md §5).
//!
//! The platform-agnostic game loop — the deterministic fixed-tick sim, render interpolation
//! (invariant #4), the embodiment input-source swap (invariant #5), and the avatar-local
//! prediction seam (D15) — now lives in the `engine` crate so the Android entry
//! (`pal-android::android_main`) runs the *same* loop. This binary owns only the desktop
//! concerns: the window, the wgpu surface, input plumbing, and the wall clock that feeds the
//! per-frame `dt` into the engine's fixed-tick accumulator.

use gonedark_engine::{Game, DEFAULT_SEED};
use gonedark_pal_desktop::{DesktopInput, DesktopRenderSurface};
use std::sync::Arc;
use std::time::Instant;
use winit::application::ApplicationHandler;
use winit::event::{DeviceEvent, DeviceId, WindowEvent};
use winit::event_loop::{ActiveEventLoop, EventLoop};
use winit::window::{Window, WindowAttributes, WindowId};

/// The desktop host: the shared game (created lazily in `resumed`, once a GPU device
/// exists), the wgpu surface, the input accumulator, and the wall clock for the per-frame dt.
struct App {
    game: Option<Game>,
    surface: Option<DesktopRenderSurface>,
    input: DesktopInput,
    last_frame: Instant,
}

impl App {
    fn new() -> Self {
        App {
            game: None,
            surface: None,
            input: DesktopInput::new(),
            last_frame: Instant::now(),
        }
    }

    /// One presented frame: drain input + dt, acquire a surface frame, drive the shared
    /// engine loop, present. All the sim/render/camera logic is inside `Game::frame`.
    fn frame(&mut self) {
        let input = self.input.drain_frame();
        let now = Instant::now();
        let dt = now.duration_since(self.last_frame).as_secs_f32();
        self.last_frame = now;

        // `surface` and `game` are disjoint fields, so both mutable borrows are fine.
        let (Some(surface), Some(game)) = (self.surface.as_mut(), self.game.as_mut()) else {
            return;
        };
        let viewport = surface.size();

        if let Some((frame, view)) = surface.acquire() {
            game.frame(
                &input,
                dt,
                viewport,
                surface.device(),
                surface.queue(),
                &view,
            );
            surface.present(frame);
        }
    }
}

impl ApplicationHandler for App {
    /// Create the window + GPU surface + the shared game once the event loop is ready. On
    /// desktop `resumed` fires once at startup; guard against a redundant second create.
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.surface.is_some() {
            return;
        }
        let window = event_loop
            .create_window(WindowAttributes::default().with_title("Going Dark"))
            .expect("create winit window");
        let window: Arc<Window> = Arc::new(window);

        let surface = DesktopRenderSurface::new(window);
        let game = Game::new(surface.device(), surface.format(), DEFAULT_SEED);

        self.surface = Some(surface);
        self.game = Some(game);
        // Reset the clock so window-creation latency isn't charged to the first tick.
        self.last_frame = Instant::now();
    }

    fn window_event(
        &mut self,
        event_loop: &ActiveEventLoop,
        _window_id: WindowId,
        event: WindowEvent,
    ) {
        // Feed every window event to the input accumulator first (pointer / keys / clicks).
        self.input.handle_window_event(&event);

        match event {
            WindowEvent::CloseRequested => event_loop.exit(),
            WindowEvent::Resized(size) => {
                if let Some(surface) = self.surface.as_mut() {
                    surface.resize(size.width, size.height);
                }
            }
            WindowEvent::RedrawRequested => self.frame(),
            _ => {}
        }
    }

    /// Raw mouse-look (the FPS look axis) arrives as device events.
    fn device_event(
        &mut self,
        _event_loop: &ActiveEventLoop,
        _device_id: DeviceId,
        event: DeviceEvent,
    ) {
        self.input.handle_device_event(&event);
    }

    /// Keep a continuous render loop: request another redraw as soon as the queue drains.
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
