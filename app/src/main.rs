//! Host: wires core + pal-desktop + render and owns the run loop / sim–render split.
//!
//! This is Phase-1 build-order step 5 (docs/phase-1-plan.md §5): a real `winit` 0.30
//! `ApplicationHandler` run loop that drives the deterministic core, the desktop wgpu
//! backend, and the renderer. It carries the four Phase-1 seams the scaffold documented:
//!  - a **fixed-tick accumulator** advancing the deterministic sim (invariant #4),
//!  - **render interpolation** between the last two snapshots (invariant #4),
//!  - the **embodiment input-source swap** (invariant #5) — possess/release one entity,
//!  - the **avatar-local-prediction seam** (D15) — prediction is computed in the
//!    PRESENTATION path and never written back into sim state.
//!
//! Host-side floats are fine HERE (this crate is not the sim): the wall clock, the camera
//! matrices (glam), and the pointer-unproject all use `f32`. The one place a float would
//! otherwise leak into `core` is the command-layer tap target — that is **quantized to
//! `Fixed` AT THE INPUT BOUNDARY** (see `world_to_fixed`) so the `Command` carries Fixed
//! bits into the deterministic sim and no float ever crosses into `core` (invariant #1). In
//! real lockstep netcode every peer receives the identical Fixed-bits command.
//!
//! Android does NOT share this loop (its entry is `android_main` in `pal-android`, built by
//! cargo-ndk); this binary is desktop-only and the shared android wiring is Phase 2.

use glam::{Mat4, Vec3, Vec4};
use gonedark_core::components::Vec2;
use gonedark_core::ecs::Entity;
use gonedark_core::fixed::Fixed;
use gonedark_core::sim::{Command, Sim, TICK_HZ};
use gonedark_core::snapshot::Snapshot;
use gonedark_pal::InputFrame;
use gonedark_pal_desktop::{DesktopInput, DesktopRenderSurface};
use std::sync::Arc;
use std::time::Instant;
use winit::application::ApplicationHandler;
use winit::event::{DeviceEvent, DeviceId, WindowEvent};
use winit::event_loop::{ActiveEventLoop, EventLoop};
use winit::window::{Window, WindowAttributes, WindowId};

/// Half-extent (in world units) the top-down command camera covers from center to edge on
/// the shorter axis. The flow-field playfield is roughly `[-64, 64]`; this frames its
/// neighbourhood with a little margin. Render-only — never touches the sim.
const TOPDOWN_HALF_EXTENT: f32 = 72.0;

/// Eye height (world units) of the embodied perspective camera above the ground plane.
const EYE_HEIGHT: f32 = 1.5;

/// Mouse-look sensitivity (radians of yaw per accumulated raw look-delta unit).
const LOOK_SENSITIVITY: f32 = 0.0025;

/// Cap on catch-up sim steps in a single frame, so a huge first-frame / stall `dt` can't
/// spiral the sim (the classic "spiral of death"). Excess time is simply dropped.
const MAX_CATCHUP_STEPS: u32 = 8;

/// Which camera the host is presenting through.
#[derive(Clone, Copy, PartialEq, Eq)]
enum CameraMode {
    /// RTS command view: orthographic, looking straight down at the playfield.
    TopDown,
    /// First-person view from the possessed unit, yaw driven by mouse-look.
    Embodied,
}

/// Quantize a host-side world coordinate to exact Q16.16 `Fixed` bits — the mirror of
/// `render::fixed_to_f32`. This is THE input boundary: the float never enters `core`; the
/// `Command` it produces carries Fixed bits into the deterministic sim (invariant #1).
#[inline]
fn world_to_fixed(world_coord: f32) -> Fixed {
    Fixed::from_bits((world_coord * Fixed::SCALE as f32).round() as i32)
}

/// The host application: the deterministic sim, the possessed entity, the (lazily created
/// in `resumed`) GPU surface + renderer, the input accumulator, the two latest snapshots
/// for interpolation, the fixed-tick accumulator, and the embodiment/camera state.
struct App {
    sim: Sim,
    player: Entity,

    surface: Option<DesktopRenderSurface>,
    renderer: Option<gonedark_render::Renderer>,
    input: DesktopInput,

    /// The previous and current sim snapshots — the renderer interpolates between them.
    prev: Snapshot,
    curr: Snapshot,

    /// Fixed-tick accumulator: wall-clock leftover seconds carried between frames, plus the
    /// timestamp of the last frame. Host wall clock — floats are fine here.
    last_frame: Instant,
    acc: f32,

    embodied: bool,
    camera: CameraMode,

    /// Accumulated embodied yaw (radians), integrated from raw mouse-look deltas. Presentation
    /// only — never written into the sim (D15).
    yaw: f32,
}

impl App {
    /// Build the host, spawn the one Phase-1 unit, and issue its initial move order — the
    /// same bootstrap the scaffold used, now feeding the real loop.
    fn new() -> Self {
        let mut sim = Sim::new(0x00C0FFEE);
        let player = sim.world.spawn();
        // Initial order: walk the unit out into the field so there is motion to interpolate.
        sim.step(&[Command::Move {
            entity: player,
            target: Vec2::new(Fixed::from_int(20), Fixed::from_int(8)),
        }]);

        let curr = sim.snapshot();
        let prev = curr.clone();

        App {
            sim,
            player,
            surface: None,
            renderer: None,
            input: DesktopInput::new(),
            prev,
            curr,
            last_frame: Instant::now(),
            acc: 0.0,
            embodied: false,
            camera: CameraMode::TopDown,
            yaw: 0.0,
        }
    }

    /// The player's authoritative world position, read straight from the sim world (read
    /// only — the host never mutates sim state outside `Sim::step`). Used to place the
    /// embodied camera; the snapshot carries no entity identity, so we read by index.
    fn player_pos(&self) -> Vec2 {
        self.sim.world.pos[self.player.index as usize]
    }

    /// Build the top-down orthographic view-projection. World units live on the ground plane
    /// (z = 0; see `shader.wgsl`); the camera looks straight down -Z onto it. Centered on the
    /// playfield origin, framing `±TOPDOWN_HALF_EXTENT` (aspect-corrected on the long axis).
    fn topdown_view_proj(&self, width: u32, height: u32) -> Mat4 {
        let aspect = width.max(1) as f32 / height.max(1) as f32;
        let (hx, hy) = if aspect >= 1.0 {
            (TOPDOWN_HALF_EXTENT * aspect, TOPDOWN_HALF_EXTENT)
        } else {
            (TOPDOWN_HALF_EXTENT, TOPDOWN_HALF_EXTENT / aspect)
        };
        // Orthographic box centered on origin. Near/far straddle the z=0 plane.
        let proj = Mat4::orthographic_rh(-hx, hx, -hy, hy, -10.0, 10.0);
        // Eye above the plane looking down; +Y world stays "up" on screen.
        let view = Mat4::look_at_rh(
            Vec3::new(0.0, 0.0, 5.0),
            Vec3::new(0.0, 0.0, 0.0),
            Vec3::new(0.0, 1.0, 0.0),
        );
        proj * view
    }

    /// Build the embodied perspective view-projection: eye at the possessed unit's position,
    /// raised by `EYE_HEIGHT`, looking out across the ground plane along the current yaw.
    /// Minimal but real — enough to validate the embodiment camera swap.
    fn embodied_view_proj(&self, width: u32, height: u32) -> Mat4 {
        let p = self.player_pos();
        let px = gonedark_render::fixed_to_f32(p.x);
        let py = gonedark_render::fixed_to_f32(p.y);

        let eye = Vec3::new(px, py, EYE_HEIGHT);
        // Look direction in the ground plane from yaw; aim slightly downward at the field.
        let dir = Vec3::new(self.yaw.cos(), self.yaw.sin(), -0.15).normalize();
        let target = eye + dir;

        let aspect = width.max(1) as f32 / height.max(1) as f32;
        let proj = Mat4::perspective_rh(60_f32.to_radians(), aspect, 0.05, 500.0);
        let view = Mat4::look_at_rh(eye, target, Vec3::Z);
        proj * view
    }

    /// Unproject a pointer pixel onto the ground plane (z = 0) under the current TOP-DOWN
    /// camera, returning world `(x, y)`. For the orthographic top-down camera the world XY is
    /// independent of NDC depth, so we invert `view_proj` at the near plane. Returns `None`
    /// if the matrix is singular.
    fn unproject_topdown(&self, px: f32, py: f32, width: u32, height: u32) -> Option<(f32, f32)> {
        let view_proj = self.topdown_view_proj(width, height);
        let inv = view_proj.inverse();
        if !inv.is_finite() {
            return None;
        }
        // Pixel → NDC. winit pixel origin is top-left, +y down; NDC +y is up, so flip y.
        let ndc_x = (px / width.max(1) as f32) * 2.0 - 1.0;
        let ndc_y = 1.0 - (py / height.max(1) as f32) * 2.0;
        let world = inv * Vec4::new(ndc_x, ndc_y, 0.0, 1.0);
        if world.w.abs() < f32::EPSILON {
            return None;
        }
        Some((world.x / world.w, world.y / world.w))
    }

    /// One presented frame: drain input → commands, drain the fixed-tick accumulator, build
    /// the camera, and render the interpolated snapshot. All host-float work; the only thing
    /// crossing into the sim is the Fixed-quantized command set.
    fn frame(&mut self) {
        // Bail before any work if the surface/renderer aren't up yet (pre-`resumed`).
        let Some((width, height)) = self.surface.as_ref().map(|s| s.size()) else {
            return;
        };
        if self.renderer.is_none() {
            return;
        }

        // 1. Drain this frame's accumulated input into one engine-neutral frame.
        let f = self.input.drain_frame();

        // 2. Map input → sim commands (applied on the first step of this frame).
        let mut commands: Vec<Command> = Vec::new();

        // Command-layer tap: only in the top-down view, and only on a pointer-down edge with
        // a known pointer position. The tap target is unprojected to a world XY then quantized
        // to Fixed AT THIS BOUNDARY — no float crosses into the sim (invariant #1).
        if !self.embodied && f.pointer_down {
            if let Some((px, py)) = f.pointer {
                if let Some((wx, wy)) = self.unproject_topdown(px, py, width, height) {
                    commands.push(Command::Move {
                        entity: self.player,
                        target: Vec2::new(world_to_fixed(wx), world_to_fixed(wy)),
                    });
                }
            }
        }

        // Embodiment input-source swap (invariant #5) — edge-triggered, mutually exclusive.
        if f.embody_pressed && !self.embodied {
            commands.push(Command::Embody {
                entity: self.player,
            });
            self.embodied = true;
            self.camera = CameraMode::Embodied;
            eprintln!("[tick {}] EMBODY — world goes dark", self.sim.tick_count());
        } else if f.surface_pressed && self.embodied {
            commands.push(Command::Surface {
                entity: self.player,
            });
            self.embodied = false;
            self.camera = CameraMode::TopDown;
            eprintln!("[tick {}] SURFACE — back to command", self.sim.tick_count());
        }

        // Integrate mouse-look into the presentation-only yaw (D15: never into the sim).
        self.yaw += f.look_axis.0 * LOOK_SENSITIVITY;

        // 3. Fixed-tick accumulator: advance the deterministic sim in whole ticks. This frame's
        // commands apply on the FIRST step; catch-up steps pass no commands. Clamped so a huge
        // first-frame / stall dt can't spiral.
        let now = Instant::now();
        let tick_dt = 1.0 / TICK_HZ as f32;
        self.acc += now.duration_since(self.last_frame).as_secs_f32();
        self.last_frame = now;

        let mut steps = 0u32;
        let mut first_step = true;
        while self.acc >= tick_dt && steps < MAX_CATCHUP_STEPS {
            self.prev = self.curr.clone();
            if first_step {
                self.sim.step(&commands);
                first_step = false;
            } else {
                self.sim.step(&[]);
            }
            self.curr = self.sim.snapshot();
            self.acc -= tick_dt;
            steps += 1;
        }
        // If we hit the catch-up cap, drop the backlog rather than carry it (anti-spiral).
        if steps == MAX_CATCHUP_STEPS && self.acc >= tick_dt {
            self.acc = 0.0;
        }
        // Sub-tick frame: if no whole tick elapsed this frame (render faster than TICK_HZ)
        // but input produced commands, apply them now on an extra step so the edge-triggered
        // tap/embody intent — which fires for exactly one drained input frame — is never
        // dropped. The accumulator keeps the cadence honest for the steady-state case.
        if first_step && !commands.is_empty() {
            self.prev = self.curr.clone();
            self.sim.step(&commands);
            self.curr = self.sim.snapshot();
        }

        // 4. Interpolation factor for the renderer (invariant #4).
        let alpha = (self.acc / tick_dt).clamp(0.0, 1.0);

        // 5. Build the camera for the active view.
        let view_proj = match self.camera {
            CameraMode::TopDown => self.topdown_view_proj(width, height),
            CameraMode::Embodied => self.embodied_view_proj(width, height),
        };
        let camera = gonedark_render::Camera {
            view_proj: view_proj.to_cols_array_2d(),
        };

        // Now take the mutable GPU borrows — all the `&self` camera/unproject work above is
        // done, so this can't conflict with it.
        let (Some(surface), Some(renderer)) = (self.surface.as_mut(), self.renderer.as_mut())
        else {
            return;
        };

        // 6. Interpolate prev→curr into render instances (the float boundary lives in render).
        renderer.prepare(&self.prev, &self.curr, alpha);

        // 7. Acquire → render (world goes dark while embodied) → present.
        if let Some((frame, view)) = surface.acquire() {
            renderer.render(
                surface.device(),
                surface.queue(),
                &view,
                &camera,
                /* world_dark = */ self.embodied,
            );
            surface.present(frame);
        }

        // 8. Avatar-local prediction seam (D15): presentation-only, NEVER writes sim state.
        predict_avatar(&self.curr, &f, self.embodied);
    }
}

impl ApplicationHandler for App {
    /// Create the window + GPU surface + renderer once the event loop is ready. On desktop
    /// `resumed` fires once at startup; we guard against a redundant second create.
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.surface.is_some() {
            return;
        }
        let window = event_loop
            .create_window(WindowAttributes::default().with_title("Going Dark"))
            .expect("create winit window");
        let window: Arc<Window> = Arc::new(window);

        let surface = DesktopRenderSurface::new(window);
        let renderer = gonedark_render::Renderer::new(surface.device(), surface.format());

        self.surface = Some(surface);
        self.renderer = Some(renderer);
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

/// Avatar-local prediction (D15) lives HERE, in the presentation path. It reads sim state
/// plus the latest input to predict the embodied unit's transform for a responsive local
/// view, and MUST NOT feed back into the sim (or lockstep desyncs silently — invariant #1).
/// Authoritative resolution still happens in the sim at tick T+D. Stub for Phase 1.
fn predict_avatar(_snapshot: &Snapshot, _frame: &InputFrame, _embodied: bool) {
    // TODO(phase3): integrate local aim/move from `_frame`; reconcile against the tick.
}
