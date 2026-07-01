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

use gonedark_pal::{InputFrame, PowerState, ThermalSensor, ThermalState};
use winit::event::{DeviceEvent, ElementState, MouseButton, MouseScrollDelta, WindowEvent};
use winit::keyboard::{KeyCode, PhysicalKey};
use winit::window::Window;

/// Desktop audio backend (worker 3). Owns `DesktopAudio`, the [`gonedark_pal::Audio`] sink the
/// `app` host constructs and hands to `Game::frame`.
mod audio;
pub use audio::DesktopAudio;

/// Transport backends (worker B, D27): the [`gonedark_pal::Transport`] seam that moves opaque
/// lockstep frames between two endpoints. Two concrete impls live here:
///  - [`LoopbackTransport`] — the dev-only in-process double (no socket); a connected pair shares
///    queues, for single-process two-instance verification.
///  - [`UdpTransport`] — the real-socket sibling over `std::net::UdpSocket`: one opaque frame ↔ one
///    UDP datagram, non-blocking drain, no reliability of its own (the lockstep retransmit/dedup
///    window already tolerates UDP loss/reorder/dup). **UDP now**; a QUIC transport (Wi-Fi↔cellular
///    path migration) is the documented future option behind the same trait, per D27.
mod transport;
pub use transport::{LoopbackTransport, UdpTransport};

/// Transport-level RTT ping/pong (Phase 3 WS-B): the live sample source for `engine`'s
/// adaptive-input-delay estimator. [`PingPongTransport`] wraps any [`gonedark_pal::Transport`],
/// multiplexes its own tagged ping/pong datagrams over it, and surfaces measured round-trip times
/// via a cloneable [`RttSamples`] handle the host drains into `Game::observe_rtt`. Deliberately NOT a
/// `core::lockstep` wire frame — RTT is a host/transport wall-clock concern, kept out of the
/// clock-free `core` (invariants #1/#2). The pure measurement logic ([`RttMeter`] + the frame codec)
/// is unit-tested directly; [`SystemClock`] is the production wall clock.
mod pingpong;
pub use pingpong::{
    decode, encode_ping, encode_pong, wrap_lockstep, Decoded, PingPongTransport, RttMeter,
    RttSamples, SystemClock,
};

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
/// Default keymap (desktop, classic-RTS split — D42): `E` embody, `Q` surface, `WASD` move, raw
/// mouse delta looks. **Left-click selects** (the command-layer pointer-down / band-select) and, when
/// embodied, **fires**; **right-click commands** the current selection (move, or attack on an enemy);
/// `Space` is the alternate fire. Number keys `1`–`0` pick the advanced order/stance vocabulary slots.
/// Command-view production keys (Phase 2): `B` places a Camp at the cursor, `R`/`H` queue a Rifleman/
/// Heavy at the active camp, `U` upgrades it.
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
    // Accumulated mouse-wheel delta since the last drain (cleared each frame). Positive = zoom in.
    scroll: f32,
    // Edge-triggered latches — set on key/button press, cleared on drain.
    embody_latch: bool,
    surface_latch: bool,
    click_latch: bool,
    // Right-click "command the selection" edge (D42), cleared on drain.
    command_latch: bool,
    // Touch-UI desktop bindings (worker 4/5 consume these via InputFrame): the left-button release
    // edge and the order/stance vocabulary slot keys (1–6).
    release_latch: bool,
    // The "open the order/stance context" input (F): a HELD/level signal — true for as long as the
    // key is down, NOT a one-shot edge. The radial command menu stays open while it is held and
    // closes on release (an edge here would flash the menu for a single frame).
    long_press: bool,
    command_slot: Option<u8>,
    // Command-view production edges (Phase 2 "command and grow your camps"), latched on press and
    // cleared on drain like `command_slot`: B places a Camp at the cursor, R/H queue a Rifleman/
    // Heavy at the active camp, U upgrades it. See `on_key` for the keymap rationale.
    building_slot: Option<u8>,
    train_slot: Option<u8>,
    upgrade_latch: bool,
    // Embodied FPS edges (the on-screen GUI is Android-only, but desktop keys exercise the same
    // mechanics): C toggles crouch, R/V start a reload, Space starts a (cosmetic) jump, X toggles
    // the fire mode (semi ⇄ auto). One-shot, cleared on drain.
    crouch_latch: bool,
    reload_latch: bool,
    jump_latch: bool,
    select_fire_latch: bool,
    // Embodied aim-down-sight / zoom: a HELD/level signal driven by the RIGHT mouse button while
    // embodied (the genre-standard ADS button). The command view consumes the right button as an
    // edge (`command_latch`) and ignores `aim`; the embodied view consumes `aim` (held) and ignores
    // `command_click` — so the two never collide on one button. Tracks the button level, not an edge.
    aim_held: bool,
    // Player look prefs from the Settings screen, applied to the drained `look_axis` (NOT to the raw
    // accumulation — these never feed back into the sim's deterministic state; they shape the host
    // input *before* it crosses the engine boundary, exactly like every other host-side input
    // mapping here). `look_sensitivity` is a multiplier (1.0 = stock); `invert_look_y` flips pitch.
    look_sensitivity: f32,
    invert_look_y: bool,
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
            scroll: 0.0,
            embody_latch: false,
            surface_latch: false,
            click_latch: false,
            command_latch: false,
            release_latch: false,
            long_press: false,
            command_slot: None,
            building_slot: None,
            train_slot: None,
            upgrade_latch: false,
            crouch_latch: false,
            reload_latch: false,
            jump_latch: false,
            select_fire_latch: false,
            aim_held: false,
            look_sensitivity: 1.0,
            invert_look_y: false,
        }
    }
}

/// Apply the player's look prefs to a raw mouse-look delta: scale both axes by `sensitivity` and flip
/// pitch when `invert_y`. Pure host-side input shaping — it runs at drain time, *before* the delta
/// crosses into the engine, so it never affects the deterministic sim (invariant #1: the sim
/// quantises whatever look value it receives; this only changes the host value handed across, exactly
/// like the WASD→axis mapping above). `sensitivity` is range-validated at the Settings boundary
/// (`SettingsState::clamp` keeps it in `[0.1, 3.0]`), so this trusts it and just multiplies.
pub fn scale_look(raw: (f32, f32), sensitivity: f32, invert_y: bool) -> (f32, f32) {
    let y = if invert_y { -raw.1 } else { raw.1 };
    (raw.0 * sensitivity, y * sensitivity)
}

impl DesktopInput {
    pub fn new() -> Self {
        Self::default()
    }

    /// Set the player's look prefs (Settings screen). `sensitivity` multiplies the mouse-look delta
    /// (clamped sane in [`scale_look`]); `invert_y` flips the pitch axis. The host calls this each
    /// match frame from its Settings model. Host-side only — it shapes input before the engine
    /// boundary and never touches the deterministic sim.
    pub fn set_look_prefs(&mut self, sensitivity: f32, invert_y: bool) {
        self.look_sensitivity = sensitivity;
        self.invert_look_y = invert_y;
    }

    /// Feed one `winit` [`WindowEvent`]. This is a **thin decoder**: it unpacks the platform event
    /// into the engine-neutral primitives (a moved/left pointer, a button edge, a key edge with its
    /// held/repeat flags) and forwards them to the pure `on_*` mappers below. All the *meaning*
    /// (which intents latch, which keys are held, the keymap) lives in those mappers so it is
    /// unit-testable without constructing a winit `KeyEvent` (which has private, non-exhaustive
    /// fields). The decode itself is the only part that needs a real winit event.
    pub fn handle_window_event(&mut self, event: &WindowEvent) {
        match event {
            WindowEvent::CursorMoved { position, .. } => {
                self.on_cursor_moved(position.x as f32, position.y as f32);
            }
            WindowEvent::CursorLeft { .. } => self.on_cursor_left(),
            WindowEvent::MouseInput { state, button, .. } => {
                self.on_mouse_button(*button, *state == ElementState::Pressed);
            }
            WindowEvent::MouseWheel { delta, .. } => {
                // Normalise both wheel encodings to "notches": a line-delta is already ~±1 per
                // detent; a pixel-delta (trackpads) is divided by a typical detent height. Wheel-up
                // (positive) = zoom in. Only the Y axis drives zoom.
                let dy = match delta {
                    MouseScrollDelta::LineDelta(_, y) => *y,
                    MouseScrollDelta::PixelDelta(p) => p.y as f32 / 120.0,
                };
                self.on_mouse_wheel(dy);
            }
            WindowEvent::KeyboardInput { event, .. } => {
                if let PhysicalKey::Code(code) = event.physical_key {
                    self.on_key(code, event.state == ElementState::Pressed, event.repeat);
                }
            }
            _ => {}
        }
    }

    /// Feed one `winit` [`DeviceEvent`] for raw, unaccelerated mouse-look deltas (the FPS look
    /// axis). A thin decoder over [`on_mouse_motion`](Self::on_mouse_motion).
    pub fn handle_device_event(&mut self, event: &DeviceEvent) {
        if let DeviceEvent::MouseMotion { delta } = event {
            self.on_mouse_motion(delta.0 as f32, delta.1 as f32);
        }
    }

    // --- Pure input mappers (the testable seam) -------------------------------------------------
    // These take engine-neutral primitives, not winit types, so the keymap + latch/held rules are
    // unit-tested directly (see `input_tests`). `KeyCode`/`MouseButton` are plain constructible
    // enums; the un-constructible `KeyEvent` is decoded away in `handle_window_event` above.

    /// The command-layer pointer moved to `(x, y)` window pixels.
    fn on_cursor_moved(&mut self, x: f32, y: f32) {
        self.pointer = Some((x, y));
    }

    /// The pointer left the window — drop the position so no stale point lingers.
    fn on_cursor_left(&mut self) {
        self.pointer = None;
    }

    /// A mouse button changed state (classic-RTS split, D42). **Left** is the command-layer pointer
    /// (held, with press/release edges latched so a fast click is never dropped) — it drives unit
    /// SELECTION in the command view, and doubles as FIRE while embodied (the two consumers are
    /// mode-exclusive, so one button is unambiguous). **Right** is the edge-triggered "command the
    /// selection here" intent (move / attack); it is not held.
    fn on_mouse_button(&mut self, button: MouseButton, pressed: bool) {
        match button {
            MouseButton::Left => {
                self.pointer_down = pressed;
                // Embodied fire rides the left button (FPS convention); the command view ignores
                // `fire`, so this never collides with left-click selection.
                self.fire = pressed;
                if pressed {
                    self.click_latch = true; // edge: command-layer click (select)
                } else {
                    self.release_latch = true; // edge: drag/tap completed
                }
            }
            // Right button: in the command view its PRESS edge commands the current selection (D42,
            // latched with the cursor already tracked in `pointer`); while embodied its HELD level is
            // aim-down-sight / zoom. Track both — the consumers are mode-exclusive (the engine reads
            // `command_click` only in the command view and `aim` only while embodied), so one button
            // is unambiguous.
            MouseButton::Right => {
                if pressed {
                    self.command_latch = true;
                }
                self.aim_held = pressed;
            }
            _ => {}
        }
    }

    /// A key changed state. `pressed` is the up/down edge; `repeat` is the OS key-repeat flag.
    /// Edge intents (embody/surface, vocabulary slots) latch only on the first press (`pressed &&
    /// !repeat`); held inputs (WASD, fire, the F context key) track the level directly.
    fn on_key(&mut self, code: KeyCode, pressed: bool, repeat: bool) {
        match code {
            // Edge-triggered intents: latch only on the press edge, and only on the first press
            // (ignore key-repeat) so they fire exactly once.
            KeyCode::KeyE => {
                if pressed && !repeat {
                    self.embody_latch = true;
                }
            }
            KeyCode::KeyQ => {
                if pressed && !repeat {
                    self.surface_latch = true;
                }
            }
            // Held locomotion axes (WASD).
            KeyCode::KeyW => self.move_up = pressed,
            KeyCode::KeyS => self.move_down = pressed,
            KeyCode::KeyA => self.move_left = pressed,
            KeyCode::KeyD => self.move_right = pressed,
            // Jump (standard FPS Space binding) — a one-shot edge that starts a cosmetic embodied
            // hop. Space no longer fires: fire is the left mouse button, matching the genre.
            KeyCode::Space => {
                if pressed && !repeat {
                    self.jump_latch = true;
                }
            }
            // Touch-UI desktop bindings: F opens the order/stance context; number keys pick a
            // vocabulary slot (0-based on the wire) — 1–9 → slots 0–8, 0 → slot 9 (see
            // engine::command_ui for the slot table).
            // F is HELD (a level signal, like WASD), NOT an edge: the radial command menu stays open
            // while F is down and closes on release. An edge latch here would open the menu for a
            // single frame and then immediately drop it.
            KeyCode::KeyF => self.long_press = pressed,
            KeyCode::Digit1 if pressed && !repeat => self.command_slot = Some(0),
            KeyCode::Digit2 if pressed && !repeat => self.command_slot = Some(1),
            KeyCode::Digit3 if pressed && !repeat => self.command_slot = Some(2),
            KeyCode::Digit4 if pressed && !repeat => self.command_slot = Some(3),
            KeyCode::Digit5 if pressed && !repeat => self.command_slot = Some(4),
            KeyCode::Digit6 if pressed && !repeat => self.command_slot = Some(5),
            KeyCode::Digit7 if pressed && !repeat => self.command_slot = Some(6),
            KeyCode::Digit8 if pressed && !repeat => self.command_slot = Some(7),
            KeyCode::Digit9 if pressed && !repeat => self.command_slot = Some(8),
            KeyCode::Digit0 if pressed && !repeat => self.command_slot = Some(9),
            // Command-view "command and grow your camps" production keys (Phase 2). Edge-latched
            // (press, ignore key-repeat), command view only — the engine ignores them while embodied.
            // Mnemonic letters, distinct from the 1–0 order/stance vocabulary: B(uild) places a Camp
            // at the cursor's ground point; R(ifleman)/H(eavy) queue that unit at the active camp;
            // U(pgrade) levels the active camp. (These bindings are recorded in docs/decisions.md.)
            KeyCode::KeyB if pressed && !repeat => self.building_slot = Some(0),
            // R queues a Rifleman in the command view AND reloads while embodied (the standard FPS
            // reload key). The two consumers are mode-exclusive — the engine reads `train_slot` only
            // in the command view and `reload_pressed` only while embodied — so one key is unambiguous.
            KeyCode::KeyR if pressed && !repeat => {
                self.train_slot = Some(0);
                self.reload_latch = true;
            }
            KeyCode::KeyH if pressed && !repeat => self.train_slot = Some(1),
            KeyCode::KeyU if pressed && !repeat => self.upgrade_latch = true,
            // Embodied FPS keys (mirror the Android Crouch/Reload buttons so the mechanics are
            // testable on desktop). Edge-latched; the engine ignores them while not embodied.
            // C=crouch, R/V=reload (V kept as a secondary), X=select-fire (semi ⇄ auto).
            KeyCode::KeyC if pressed && !repeat => self.crouch_latch = true,
            KeyCode::KeyV if pressed && !repeat => self.reload_latch = true,
            KeyCode::KeyX if pressed && !repeat => self.select_fire_latch = true,
            _ => {}
        }
    }

    /// Accumulate raw, unaccelerated mouse-look delta (cleared each drain).
    fn on_mouse_motion(&mut self, dx: f32, dy: f32) {
        self.look_dx += dx;
        self.look_dy += dy;
    }

    /// Accumulate mouse-wheel notches for command-view zoom (positive = zoom in; cleared each drain).
    fn on_mouse_wheel(&mut self, dy: f32) {
        self.scroll += dy;
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
            pointer_up: self.release_latch,
            embody_pressed: self.embody_latch,
            surface_pressed: self.surface_latch,
            long_press: self.long_press,
            command_slot: self.command_slot,
            command_click: self.command_latch,
            // Desktop uses the dedicated right-click command (above), not the single-pointer
            // contextual tap — so the "tap commands" touch mode (D43) stays off here.
            command_tap: false,
            building_slot: self.building_slot,
            train_slot: self.train_slot,
            upgrade_pressed: self.upgrade_latch,
            move_axis: (mx, my),
            // Apply the player's look prefs (sensitivity + invert-Y) as the raw delta leaves for the
            // engine — host-side shaping, never a sim input.
            look_axis: scale_look(
                (self.look_dx, self.look_dy),
                self.look_sensitivity,
                self.invert_look_y,
            ),
            fire: self.fire,
            // Aim-down-sight / zoom rides the right button (held); the command view ignores `aim`,
            // so it never collides with the right-click command edge above.
            aim: self.aim_held,
            crouch_pressed: self.crouch_latch,
            reload_pressed: self.reload_latch,
            jump_pressed: self.jump_latch,
            select_fire_pressed: self.select_fire_latch,
            // No on-screen touch controls on desktop — the embodied GUI is Android-only.
            touches: Default::default(),
            touch_count: 0,
            scroll: self.scroll,
        };

        // Clear one-shot state for the next accumulation window. `long_press` is NOT cleared here —
        // it is held/level state (true while F is down), mirroring `fire`/WASD; it is reset only by
        // the F-release event in `handle_window_event`.
        self.embody_latch = false;
        self.surface_latch = false;
        self.click_latch = false;
        self.command_latch = false;
        self.release_latch = false;
        self.command_slot = None;
        self.building_slot = None;
        self.train_slot = None;
        self.upgrade_latch = false;
        self.crouch_latch = false;
        self.reload_latch = false;
        self.jump_latch = false;
        self.select_fire_latch = false;
        self.look_dx = 0.0;
        self.look_dy = 0.0;
        self.scroll = 0.0;

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

/// Desktop [`ThermalSensor`] — a **STUB / synthetic source** (Phase 4 WS-C).
///
/// There is no real thermal sensor on the dev workstation (and reading laptop hwmon zones is
/// neither portable nor meaningful for the *target* mobile thermal budget), so this backend reports
/// a synthetic state rather than a live one. By default it reports [`ThermalState::Nominal`] on
/// external power — the desktop runs unthrottled — which keeps the render quality-tuning loop fully
/// wired on desktop without pretending to measure heat. The settable `forced` field is the test/dev
/// hook: drive it to `Serious`/`Critical` to exercise the render backoff (FPS cap + dyn-res floor)
/// end-to-end on desktop without a phone.
///
/// **OWED:** the real reading lives in `pal-android` (`PowerManager.getThermalStatus()` +
/// `BatteryManager` over JNI) — that is where the on-device numbers that may reopen D21 dual-rate
/// come from (phase-4-plan WS-C step 3). This desktop impl is deliberately inert.
#[derive(Clone, Copy, Debug, Default)]
pub struct DesktopThermalSensor {
    /// Synthetic state this stub reports. `None` → [`ThermalState::Nominal`]. Settable so the dev
    /// loop / tests can drive the backoff policy without real hardware.
    pub forced: Option<ThermalState>,
}

impl DesktopThermalSensor {
    /// A nominal, unthrottled desktop sensor (the default).
    pub fn new() -> Self {
        Self::default()
    }

    /// Build a stub that reports a forced thermal state — the dev/test hook for exercising the
    /// render backoff path on desktop (there is no real sensor here).
    pub fn forced(state: ThermalState) -> Self {
        DesktopThermalSensor {
            forced: Some(state),
        }
    }
}

impl ThermalSensor for DesktopThermalSensor {
    fn thermal_state(&self) -> ThermalState {
        // Synthetic: the forced override, else nominal. The desktop never throttles on its own.
        self.forced.unwrap_or(ThermalState::Nominal)
    }

    fn power_state(&self) -> PowerState {
        // The dev workstation is wall-powered; no battery telemetry.
        PowerState {
            on_external_power: true,
            charge: None,
        }
    }
}

#[cfg(test)]
mod input_tests {
    //! Tests for the pure input mappers (`on_key` / `on_mouse_button` / `on_cursor_*` /
    //! `on_mouse_motion`) + `drain_frame`. These cover the keymap and the latch-vs-held rules — the
    //! seam that the un-constructible winit `KeyEvent` previously hid from coverage. `KeyCode` and
    //! `MouseButton` are plain enums, so the mappers are driven directly.

    use super::*;

    /// THE regression guard: the F context key must be HELD (true while down), not a one-shot edge.
    /// A prior edge-latch made the radial command menu flash for a single frame and vanish.
    #[test]
    fn long_press_is_held_until_release() {
        let mut input = DesktopInput::new();
        assert!(!input.drain_frame().long_press, "idle: not pressed");
        input.on_key(KeyCode::KeyF, true, false);
        assert!(input.drain_frame().long_press, "held the frame F goes down");
        assert!(
            input.drain_frame().long_press,
            "STILL held with no new event (level state, not a cleared edge)"
        );
        input.on_key(KeyCode::KeyF, false, false);
        assert!(!input.drain_frame().long_press, "released → false");
    }

    #[test]
    fn embody_and_surface_are_one_shot_edges() {
        let mut input = DesktopInput::new();
        input.on_key(KeyCode::KeyE, true, false);
        assert!(input.drain_frame().embody_pressed, "embody fires on press");
        assert!(
            !input.drain_frame().embody_pressed,
            "and clears after one drain (one-shot)"
        );
        input.on_key(KeyCode::KeyQ, true, false);
        let f = input.drain_frame();
        assert!(f.surface_pressed && !f.embody_pressed);
    }

    #[test]
    fn edge_intents_ignore_key_repeat() {
        let mut input = DesktopInput::new();
        // An OS key-repeat (repeat = true) must NOT re-fire an edge intent.
        input.on_key(KeyCode::KeyE, true, true);
        assert!(
            !input.drain_frame().embody_pressed,
            "repeat doesn't latch embody"
        );
        input.on_key(KeyCode::Digit1, true, true);
        assert_eq!(
            input.drain_frame().command_slot,
            None,
            "repeat doesn't latch a slot"
        );
    }

    #[test]
    fn number_keys_map_to_zero_based_slots_then_clear() {
        let mut input = DesktopInput::new();
        input.on_key(KeyCode::Digit1, true, false);
        assert_eq!(input.drain_frame().command_slot, Some(0), "1 → slot 0");
        assert_eq!(input.drain_frame().command_slot, None, "slot is one-shot");
        input.on_key(KeyCode::Digit0, true, false);
        assert_eq!(input.drain_frame().command_slot, Some(9), "0 → slot 9");
    }

    #[test]
    fn wasd_is_held_level_state() {
        let mut input = DesktopInput::new();
        input.on_key(KeyCode::KeyW, true, false); // up → -Y (screen convention)
        assert_eq!(input.drain_frame().move_axis, (0.0, -1.0));
        assert_eq!(input.drain_frame().move_axis, (0.0, -1.0), "still held");
        input.on_key(KeyCode::KeyD, true, false); // + right → +X
        assert_eq!(input.drain_frame().move_axis, (1.0, -1.0));
        input.on_key(KeyCode::KeyW, false, false);
        input.on_key(KeyCode::KeyD, false, false);
        assert_eq!(
            input.drain_frame().move_axis,
            (0.0, 0.0),
            "released → centered"
        );
    }

    #[test]
    fn left_click_press_then_release_yields_down_then_up() {
        let mut input = DesktopInput::new();
        input.on_mouse_button(MouseButton::Left, true);
        let f = input.drain_frame();
        assert!(f.pointer_down && !f.pointer_up, "press → down");
        input.on_mouse_button(MouseButton::Left, false);
        let f = input.drain_frame();
        assert!(!f.pointer_down && f.pointer_up, "release → up");
        assert!(
            !input.drain_frame().pointer_up,
            "up latch cleared after one drain"
        );
    }

    #[test]
    fn fast_click_within_one_frame_is_not_dropped() {
        // Press AND release before the drain: still reads as a pointer-down + up that frame so a
        // quick tap is never lost (the click latch carries the down).
        let mut input = DesktopInput::new();
        input.on_mouse_button(MouseButton::Left, true);
        input.on_mouse_button(MouseButton::Left, false);
        let f = input.drain_frame();
        assert!(
            f.pointer_down && f.pointer_up,
            "fast click → down+up in one frame"
        );
    }

    #[test]
    fn left_click_fires_and_space_jumps_not_fires() {
        // Standard FPS bindings: fire rides the LEFT mouse button; Space is JUMP, not fire (and
        // never the RMB command edge). This is the "why is Space shoot?" fix.
        let mut input = DesktopInput::new();
        input.on_mouse_button(MouseButton::Left, true);
        assert!(input.drain_frame().fire, "LMB fires (embodied)");
        input.on_mouse_button(MouseButton::Left, false);
        assert!(!input.drain_frame().fire, "LMB released → no fire");
        // Space no longer fires — it latches a one-shot jump edge instead.
        input.on_key(KeyCode::Space, true, false);
        let f = input.drain_frame();
        assert!(!f.fire, "Space does NOT fire");
        assert!(f.jump_pressed, "Space latches a jump edge");
        // The jump edge is one-shot: cleared on the next drain.
        assert!(!input.drain_frame().jump_pressed, "jump edge clears after one drain");
    }

    #[test]
    fn r_reloads_while_x_selects_fire_mode() {
        // Standard reload is R (also queues a Rifleman in the command view — mode-exclusive), and X
        // toggles the fire mode. Both are one-shot edges cleared on drain.
        let mut input = DesktopInput::new();
        input.on_key(KeyCode::KeyR, true, false);
        let f = input.drain_frame();
        assert!(f.reload_pressed, "R reloads while embodied");
        assert_eq!(f.train_slot, Some(0), "and queues a Rifleman in the command view");
        assert!(!input.drain_frame().reload_pressed, "reload edge clears after one drain");
        input.on_key(KeyCode::KeyX, true, false);
        assert!(input.drain_frame().select_fire_pressed, "X toggles fire mode");
        assert!(!input.drain_frame().select_fire_pressed, "select-fire edge clears after one drain");
    }

    #[test]
    fn right_click_is_a_one_shot_command_edge_not_fire() {
        // RMB is the "command the selection" edge (D42): it latches command_click for exactly one
        // drain and never sets fire.
        let mut input = DesktopInput::new();
        input.on_mouse_button(MouseButton::Right, true);
        let f = input.drain_frame();
        assert!(f.command_click, "RMB press → command_click edge");
        assert!(!f.fire, "RMB does not fire");
        assert!(
            !input.drain_frame().command_click,
            "command_click latch cleared after one drain"
        );
        // The release edge produces nothing (the command already issued on press).
        input.on_mouse_button(MouseButton::Right, false);
        assert!(!input.drain_frame().command_click, "RMB release → no edge");
    }

    #[test]
    fn pointer_position_tracks_and_clears_on_leave() {
        let mut input = DesktopInput::new();
        assert_eq!(input.drain_frame().pointer, None);
        input.on_cursor_moved(120.0, 48.0);
        assert_eq!(input.drain_frame().pointer, Some((120.0, 48.0)));
        input.on_cursor_left();
        assert_eq!(
            input.drain_frame().pointer,
            None,
            "pointer dropped on leave"
        );
    }

    #[test]
    fn mouse_look_accumulates_then_clears_each_drain() {
        let mut input = DesktopInput::new();
        input.on_mouse_motion(3.0, -2.0);
        input.on_mouse_motion(1.0, 5.0);
        assert_eq!(
            input.drain_frame().look_axis,
            (4.0, 3.0),
            "deltas accumulate"
        );
        assert_eq!(
            input.drain_frame().look_axis,
            (0.0, 0.0),
            "cleared after drain"
        );
    }

    #[test]
    fn idle_frame_is_all_default() {
        let mut input = DesktopInput::new();
        let f = input.drain_frame();
        assert_eq!(f.pointer, None);
        assert!(!f.pointer_down && !f.pointer_up);
        assert!(!f.embody_pressed && !f.surface_pressed);
        assert!(!f.long_press && !f.fire);
        assert_eq!(f.command_slot, None);
        assert_eq!(f.building_slot, None);
        assert_eq!(f.train_slot, None);
        assert!(!f.upgrade_pressed);
        assert_eq!(f.move_axis, (0.0, 0.0));
        assert_eq!(f.look_axis, (0.0, 0.0));
    }

    #[test]
    fn build_train_upgrade_keys_latch_one_shot_then_clear() {
        // The Phase 2 production keys are edge-latched (one drain), exactly like command_slot:
        // B → build-palette slot 0 (Camp), R/H → train slots 0/1 (Rifleman/Heavy), U → upgrade.
        let mut input = DesktopInput::new();

        input.on_key(KeyCode::KeyB, true, false);
        let f = input.drain_frame();
        assert_eq!(f.building_slot, Some(0), "B → build slot 0 (Camp)");
        assert_eq!(
            input.drain_frame().building_slot,
            None,
            "build slot is one-shot"
        );

        input.on_key(KeyCode::KeyR, true, false);
        assert_eq!(input.drain_frame().train_slot, Some(0), "R → train Rifleman");
        input.on_key(KeyCode::KeyH, true, false);
        assert_eq!(input.drain_frame().train_slot, Some(1), "H → train Heavy");
        assert_eq!(
            input.drain_frame().train_slot,
            None,
            "train slot is one-shot"
        );

        input.on_key(KeyCode::KeyU, true, false);
        assert!(input.drain_frame().upgrade_pressed, "U → upgrade edge");
        assert!(
            !input.drain_frame().upgrade_pressed,
            "upgrade edge cleared after one drain"
        );
    }

    #[test]
    fn production_keys_ignore_key_repeat() {
        // OS key-repeat must not re-fire a production edge (same rule as embody / command_slot).
        let mut input = DesktopInput::new();
        input.on_key(KeyCode::KeyB, true, true);
        input.on_key(KeyCode::KeyR, true, true);
        input.on_key(KeyCode::KeyU, true, true);
        let f = input.drain_frame();
        assert_eq!(f.building_slot, None, "repeat doesn't latch a build");
        assert_eq!(f.train_slot, None, "repeat doesn't latch a train");
        assert!(!f.upgrade_pressed, "repeat doesn't latch an upgrade");
    }

    #[test]
    fn embodied_crouch_and_reload_keys_latch_one_shot_then_clear() {
        // The desktop mirror of the Android Crouch/Reload buttons: C → `crouch_pressed`, V →
        // `reload_pressed`. Both are ONE-SHOT edges (the engine's `crouch_toggle_command` inverts
        // posture off the authoritative sim state, so the host must emit a single edge, never a held
        // level — a held crouch would flip posture every frame).
        let mut input = DesktopInput::new();

        input.on_key(KeyCode::KeyC, true, false);
        let f = input.drain_frame();
        assert!(f.crouch_pressed && !f.reload_pressed, "C → one crouch edge");
        assert!(
            !input.drain_frame().crouch_pressed,
            "crouch edge cleared after one drain (a held key never re-toggles)"
        );

        input.on_key(KeyCode::KeyV, true, false);
        let f = input.drain_frame();
        assert!(f.reload_pressed && !f.crouch_pressed, "V → one reload edge");
        assert!(
            !input.drain_frame().reload_pressed,
            "reload edge cleared after one drain"
        );
    }

    #[test]
    fn embodied_crouch_and_reload_keys_ignore_key_repeat() {
        // Holding C/V (OS key-repeat) must NOT re-latch — same edge rule as embody/production, and
        // load-bearing for crouch (a re-latch each repeat frame would thrash the posture toggle).
        let mut input = DesktopInput::new();
        input.on_key(KeyCode::KeyC, true, true);
        input.on_key(KeyCode::KeyV, true, true);
        let f = input.drain_frame();
        assert!(!f.crouch_pressed, "repeat doesn't latch a crouch");
        assert!(!f.reload_pressed, "repeat doesn't latch a reload");
    }

    // --- look prefs (sensitivity + invert) ----------------------------------------------------

    #[test]
    fn scale_look_multiplies_and_inverts() {
        let approx = |a: (f32, f32), b: (f32, f32)| {
            (a.0 - b.0).abs() < 1e-6 && (a.1 - b.1).abs() < 1e-6
        };
        // Stock prefs are a pure pass-through.
        assert!(approx(scale_look((3.0, -2.0), 1.0, false), (3.0, -2.0)));
        // Sensitivity scales both axes.
        assert!(approx(scale_look((3.0, -2.0), 2.0, false), (6.0, -4.0)));
        // Invert flips pitch only (X is untouched), and composes with sensitivity.
        assert!(approx(scale_look((3.0, -2.0), 1.0, true), (3.0, 2.0)));
        assert!(approx(scale_look((3.0, -2.0), 0.5, true), (1.5, 1.0)));
    }

    #[test]
    fn drain_frame_applies_the_set_look_prefs() {
        let mut input = DesktopInput::new();
        // Default: look passes through unscaled.
        input.on_mouse_motion(2.0, -3.0);
        assert_eq!(input.drain_frame().look_axis, (2.0, -3.0));
        // With 2x sensitivity + invert-Y, the next drained delta is scaled and the pitch flipped.
        input.set_look_prefs(2.0, true);
        input.on_mouse_motion(2.0, -3.0);
        let f = input.drain_frame();
        assert!((f.look_axis.0 - 4.0).abs() < 1e-6, "x scaled 2x: {:?}", f.look_axis);
        assert!((f.look_axis.1 - 6.0).abs() < 1e-6, "y inverted+scaled: {:?}", f.look_axis);
    }
}

#[cfg(test)]
mod thermal_tests {
    use super::*;

    #[test]
    fn default_desktop_sensor_reports_nominal_on_external_power() {
        let s = DesktopThermalSensor::new();
        assert_eq!(s.thermal_state(), ThermalState::Nominal);
        let p = s.power_state();
        assert!(p.on_external_power);
        assert_eq!(p.charge, None);
    }

    #[test]
    fn forced_state_is_reported_for_the_dev_hook() {
        for forced in [
            ThermalState::Fair,
            ThermalState::Serious,
            ThermalState::Critical,
        ] {
            assert_eq!(DesktopThermalSensor::forced(forced).thermal_state(), forced);
        }
    }
}
