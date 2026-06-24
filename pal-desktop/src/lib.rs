//! Desktop PAL backend (Linux/Windows — dev + CI host). The concrete winit + wgpu backend
//! (Phase-1 build-order step 4, D19). This crate is the *concrete* side of the seam: floats,
//! `winit`, and `wgpu` are all fine here — invariant #2 only forbids them from `core` and the
//! abstract `pal` trait crate. The GPU device/queue are owned HERE and handed to the renderer
//! by the `app` wiring layer through the concrete accessors below (D19); they never cross the
//! abstract `pal::Rhi` trait.
//!
//! Two concrete types make up the backend:
//!  - [`DesktopRenderSurface`] — owns the `wgpu` `Instance`/`Adapter`/`Device`/`Queue`/
//!    `Surface` for a `winit` window the `app` creates, plus swapchain (re)configuration and
//!    per-frame acquire/present.
//!  - [`DesktopInput`] — accumulates `winit` window/device events and drains them into one
//!    engine-neutral [`gonedark_pal::InputFrame`] per frame, latching edge-triggered intents
//!    (embody / surface / click) so each fires for exactly one drained frame.

use std::sync::Arc;

use gonedark_pal::InputFrame;
use winit::event::{DeviceEvent, ElementState, MouseButton, WindowEvent};
use winit::keyboard::{KeyCode, PhysicalKey};
use winit::window::Window;

/// Owns the `wgpu` surface + device/queue for a `winit` window (D19). Built in the `app`'s
/// `ApplicationHandler::resumed` from a window the `app` creates, then queried for the
/// `&Device`/`&Queue`/`TextureFormat` the renderer needs (`render::Renderer::new(device,
/// format)` and `renderer.render(device, queue, view, …)`).
///
/// The surface (and thus the swapchain) is tied to the window's lifetime, so the window is
/// held behind an `Arc` that this struct keeps alive alongside the `wgpu::Surface<'static>`
/// borrowing it.
pub struct DesktopRenderSurface {
    // Field order matters for drop: `surface` borrows from `window` (via the `'static`
    // self-referential handle), so the surface must drop before the window. Rust drops
    // fields in declaration order, so `surface` is declared first.
    surface: wgpu::Surface<'static>,
    window: Arc<Window>,
    _instance: wgpu::Instance,
    _adapter: wgpu::Adapter,
    device: wgpu::Device,
    queue: wgpu::Queue,
    config: wgpu::SurfaceConfiguration,
}

impl DesktopRenderSurface {
    /// Create instance/surface/adapter/device/queue (`pollster`-blocked) and configure the
    /// swapchain for the window's current inner size. Picks the surface's preferred sRGB
    /// format and an `Fifo` present mode (universally supported; vsync — the variable-rate
    /// renderer interpolates, so we do not need a low-latency present mode here).
    pub fn new(window: Arc<Window>) -> Self {
        let size = window.inner_size();

        let instance = wgpu::Instance::default();

        // winit 0.30 `Window` implements the rwh 0.6 handle traits; an `Arc<Window>` is
        // accepted directly and yields a `Surface<'static>` that keeps the handle alive.
        let surface = instance
            .create_surface(window.clone())
            .expect("create wgpu surface for winit window");

        let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
            power_preference: wgpu::PowerPreference::HighPerformance,
            force_fallback_adapter: false,
            compatible_surface: Some(&surface),
        }))
        .expect("request a wgpu adapter compatible with the surface");

        let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
            label: Some("gonedark-desktop-device"),
            required_features: wgpu::Features::empty(),
            required_limits: wgpu::Limits::default(),
            memory_hints: wgpu::MemoryHints::Performance,
            experimental_features: wgpu::ExperimentalFeatures::default(),
            trace: wgpu::Trace::Off,
        }))
        .expect("request a wgpu device + queue");

        let caps = surface.get_capabilities(&adapter);
        // Prefer an sRGB format so the renderer's colours land gamma-correct; fall back to
        // the surface's first reported format if none is sRGB.
        let format = caps
            .formats
            .iter()
            .copied()
            .find(wgpu::TextureFormat::is_srgb)
            .unwrap_or_else(|| caps.formats[0]);

        let config = wgpu::SurfaceConfiguration {
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            format,
            // Clamp to >=1 so an initially-zero-area window still yields a valid config; a
            // real size arrives via `resize` before the first frame.
            width: size.width.max(1),
            height: size.height.max(1),
            present_mode: wgpu::PresentMode::Fifo,
            desired_maximum_frame_latency: 2,
            alpha_mode: caps.alpha_modes[0],
            view_formats: vec![],
        };
        surface.configure(&device, &config);

        DesktopRenderSurface {
            surface,
            window,
            _instance: instance,
            _adapter: adapter,
            device,
            queue,
            config,
        }
    }

    /// The owned `wgpu::Device` — handed to `render::Renderer::new` and `render` (D19).
    pub fn device(&self) -> &wgpu::Device {
        &self.device
    }

    /// The owned `wgpu::Queue` — handed to `render::Renderer::render` for uploads/submit.
    pub fn queue(&self) -> &wgpu::Queue {
        &self.queue
    }

    /// The configured swapchain texture format — passed to `render::Renderer::new` so its
    /// pipeline's colour target matches the surface.
    pub fn format(&self) -> wgpu::TextureFormat {
        self.config.format
    }

    /// Current swapchain size in pixels.
    pub fn size(&self) -> (u32, u32) {
        (self.config.width, self.config.height)
    }

    /// The `winit` window this surface renders into (e.g. to request a redraw).
    pub fn window(&self) -> &Window {
        &self.window
    }

    /// Reconfigure the swapchain on resize. Zero-area resizes are ignored (minimising a
    /// window reports a 0x0 size on some platforms; reconfiguring to it is invalid).
    pub fn resize(&mut self, width: u32, height: u32) {
        if width == 0 || height == 0 {
            return;
        }
        self.config.width = width;
        self.config.height = height;
        self.surface.configure(&self.device, &self.config);
    }

    /// Acquire the next swapchain image plus a default [`wgpu::TextureView`] to render into.
    /// Returns `None` when the surface is lost/outdated/occluded — we reconfigure here and
    /// skip the frame; the caller simply tries again next frame.
    ///
    /// wgpu 29 returns a [`wgpu::CurrentSurfaceTexture`] enum (not a `Result`): a `Suboptimal`
    /// frame is still usable, so we render it and let the next `resize`/`acquire`
    /// reconfigure; `Timeout`/`Occluded`/`Outdated`/`Lost`/`Validation` all skip the frame.
    pub fn acquire(&mut self) -> Option<(wgpu::SurfaceTexture, wgpu::TextureView)> {
        match self.surface.get_current_texture() {
            // Usable frame: `Success`, plus `Suboptimal` (mismatched props but renderable —
            // configure() will catch up; rendering it this frame avoids a visible stall).
            wgpu::CurrentSurfaceTexture::Success(frame)
            | wgpu::CurrentSurfaceTexture::Suboptimal(frame) => {
                let view = frame
                    .texture
                    .create_view(&wgpu::TextureViewDescriptor::default());
                Some((frame, view))
            }
            // Surface needs recreating (resize / device change). Reconfigure and skip; the
            // caller recreates the frame on the next tick.
            wgpu::CurrentSurfaceTexture::Outdated | wgpu::CurrentSurfaceTexture::Lost => {
                self.surface.configure(&self.device, &self.config);
                None
            }
            // Transient: skip this frame and try again next tick.
            wgpu::CurrentSurfaceTexture::Timeout
            | wgpu::CurrentSurfaceTexture::Occluded
            | wgpu::CurrentSurfaceTexture::Validation => None,
        }
    }

    /// Present a previously [`acquire`](Self::acquire)d frame to the screen.
    pub fn present(&self, frame: wgpu::SurfaceTexture) {
        frame.present();
    }
}

/// Translates `winit` events into the engine-neutral [`gonedark_pal::InputFrame`]
/// (platforms.md §5). State is accumulated across events between frames; edge-triggered
/// intents (embody / surface / click) are *latched* so they read `true` for exactly one
/// [`drain_frame`](Self::drain_frame) and are then cleared.
///
/// Default keymap (desktop): `E` embody, `Q` surface, `WASD` move, raw mouse delta looks,
/// left-click is the command-layer pointer-down, right-click or `Space` fires.
#[derive(Debug)]
pub struct DesktopInput {
    // Accumulated (held / latest) state.
    pointer: Option<(f32, f32)>,
    pointer_down: bool,
    fire: bool,
    move_up: bool,
    move_down: bool,
    move_left: bool,
    move_right: bool,
    // Accumulated raw mouse-look delta since the last drain (cleared each frame).
    look_dx: f32,
    look_dy: f32,
    // Edge-triggered latches — set on key/button press, cleared on drain.
    embody_latch: bool,
    surface_latch: bool,
    click_latch: bool,
}

impl Default for DesktopInput {
    fn default() -> Self {
        DesktopInput {
            pointer: None,
            pointer_down: false,
            fire: false,
            move_up: false,
            move_down: false,
            move_left: false,
            move_right: false,
            look_dx: 0.0,
            look_dy: 0.0,
            embody_latch: false,
            surface_latch: false,
            click_latch: false,
        }
    }
}

impl DesktopInput {
    pub fn new() -> Self {
        Self::default()
    }

    /// Feed one `winit` [`WindowEvent`]. Pointer position and held key/button state are
    /// accumulated; press *edges* latch the one-shot intents.
    pub fn handle_window_event(&mut self, event: &WindowEvent) {
        match event {
            WindowEvent::CursorMoved { position, .. } => {
                self.pointer = Some((position.x as f32, position.y as f32));
            }
            WindowEvent::CursorLeft { .. } => {
                self.pointer = None;
            }
            WindowEvent::MouseInput { state, button, .. } => {
                let pressed = *state == ElementState::Pressed;
                match button {
                    MouseButton::Left => {
                        self.pointer_down = pressed;
                        if pressed {
                            self.click_latch = true; // edge: command-layer click
                        }
                    }
                    MouseButton::Right => {
                        self.fire = pressed;
                    }
                    _ => {}
                }
            }
            WindowEvent::KeyboardInput { event, .. } => {
                let pressed = event.state == ElementState::Pressed;
                if let PhysicalKey::Code(code) = event.physical_key {
                    match code {
                        // Edge-triggered intents: latch only on the press edge, and only on
                        // the first press (ignore key-repeat) so they fire exactly once.
                        KeyCode::KeyE => {
                            if pressed && !event.repeat {
                                self.embody_latch = true;
                            }
                        }
                        KeyCode::KeyQ => {
                            if pressed && !event.repeat {
                                self.surface_latch = true;
                            }
                        }
                        // Held locomotion axes (WASD).
                        KeyCode::KeyW => self.move_up = pressed,
                        KeyCode::KeyS => self.move_down = pressed,
                        KeyCode::KeyA => self.move_left = pressed,
                        KeyCode::KeyD => self.move_right = pressed,
                        // Fire (alternative to right-click), held.
                        KeyCode::Space => self.fire = pressed,
                        _ => {}
                    }
                }
            }
            _ => {}
        }
    }

    /// Feed one `winit` [`DeviceEvent`] for raw, unaccelerated mouse-look deltas (the FPS
    /// look axis). Deltas accumulate until the next drain.
    pub fn handle_device_event(&mut self, event: &DeviceEvent) {
        if let DeviceEvent::MouseMotion { delta } = event {
            self.look_dx += delta.0 as f32;
            self.look_dy += delta.1 as f32;
        }
    }

    /// Produce one frame's [`InputFrame`] and clear the edge-triggered fields (embody /
    /// surface / click latches) plus the accumulated look delta. Held state (pointer
    /// position, WASD, fire) persists into the next frame, mirroring real key state.
    pub fn drain_frame(&mut self) -> InputFrame {
        // WASD → a normalised-ish move axis. Screen convention: +Y is down, so W (up) is
        // -Y. Opposing keys cancel. No diagonal normalisation here — the consumer clamps.
        let mx = (self.move_right as i32 - self.move_left as i32) as f32;
        let my = (self.move_down as i32 - self.move_up as i32) as f32;

        // A left-click latched this frame counts as a command-layer pointer-down even if the
        // button was already released by drain time, so a fast click is never dropped.
        let pointer_down = self.pointer_down || self.click_latch;

        let frame = InputFrame {
            pointer: self.pointer,
            pointer_down,
            embody_pressed: self.embody_latch,
            surface_pressed: self.surface_latch,
            move_axis: (mx, my),
            look_axis: (self.look_dx, self.look_dy),
            fire: self.fire,
        };

        // Clear one-shot state for the next accumulation window.
        self.embody_latch = false;
        self.surface_latch = false;
        self.click_latch = false;
        self.look_dx = 0.0;
        self.look_dy = 0.0;

        frame
    }
}

/// Also satisfy the abstract `pal::Input` seam so `DesktopInput` can be used wherever the
/// engine-neutral trait is expected. `poll` is the trait's per-frame drain.
impl gonedark_pal::Input for DesktopInput {
    fn poll(&mut self) -> InputFrame {
        self.drain_frame()
    }
}
