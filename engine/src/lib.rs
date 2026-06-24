//! gonedark-engine — the platform-agnostic game-loop driver.
//!
//! This is the shared spine that used to live inside the desktop winit host. It owns the
//! deterministic sim, the renderer, the two latest snapshots, the fixed-tick accumulator,
//! and the embodiment/camera state, and exposes a single [`Game::frame`] that BOTH hosts
//! drive:
//!  - desktop (`app`): a winit `ApplicationHandler` polls input + acquires a surface frame,
//!    then calls `game.frame(...)`;
//!  - android (`pal-android::android_main`): the `android-activity` loop does the same.
//!
//! It depends on `core`/`render`/`pal` (+ `wgpu`/`glam` — the render-side wiring layer, D19)
//! but on NO windowing/platform crate, so the loop is identical everywhere (invariant #2
//! spirit). The per-platform surface/input/lifecycle stays in the `pal-*` backends.
//!
//! Host-side floats are fine HERE (this crate is not the sim): the camera matrices (glam),
//! the wall-clock `dt`, and the pointer-unproject are all `f32`. The one value that crosses
//! into `core` — the command-layer tap target — is **quantized to `Fixed` AT THE INPUT
//! BOUNDARY** ([`world_to_fixed`]) so the `Command` carries Fixed bits into the deterministic
//! sim and no float ever leaks into `core` (invariant #1).

use glam::{Mat4, Vec3, Vec4};
use gonedark_core::components::{BuildingKind, Faction, Stance, UnitKind, Vec2};
use gonedark_core::ecs::Entity;
use gonedark_core::economy::{self, Resources};
use gonedark_core::fixed::Fixed;
use gonedark_core::sim::{Command, Sim, TICK_HZ};
use gonedark_core::snapshot::Snapshot;
use gonedark_core::territory::ControlPoint;
use gonedark_pal::InputFrame;
use gonedark_render::{Camera, Renderer};

/// The seed both hosts start the sim with, so desktop and Android run the bit-identical
/// deterministic scene (invariant #1 / #7).
pub const DEFAULT_SEED: u64 = 0x00C0_FFEE;

/// Half-extent (world units) the top-down command camera covers from center to the shorter
/// screen edge. Framed on the Phase 2 demo scene (units clustered within ~±25) so the
/// skirmish, the camp, and the control points read at a usable size.
const TOPDOWN_HALF_EXTENT: f32 = 40.0;

/// Eye height (world units) of the embodied perspective camera above the ground plane.
const EYE_HEIGHT: f32 = 1.5;

/// Mouse-look sensitivity (radians of yaw per accumulated raw look-delta unit).
const LOOK_SENSITIVITY: f32 = 0.0025;

/// Cap on catch-up sim steps in one frame, so a huge first-frame / stall `dt` can't spiral
/// the sim ("spiral of death"). Excess time is dropped.
const MAX_CATCHUP_STEPS: u32 = 8;

/// Which camera the host is presenting through.
#[derive(Clone, Copy, PartialEq, Eq)]
enum CameraMode {
    /// RTS command view: orthographic, looking straight down at the playfield.
    TopDown,
    /// First-person view from the possessed unit, yaw driven by look input.
    Embodied,
}

/// Quantize a host-side world coordinate to exact Q16.16 `Fixed` bits — the mirror of
/// `render::fixed_to_f32`. THE input boundary: the float never enters `core`; the `Command`
/// it produces carries Fixed bits into the deterministic sim (invariant #1).
#[inline]
pub fn world_to_fixed(world_coord: f32) -> Fixed {
    Fixed::from_bits((world_coord * Fixed::SCALE as f32).round() as i32)
}

/// Top-down orthographic view-projection (free fn — viewport only, no `Game`/device needed).
/// World units are on the ground plane (z = 0; see `render/shader.wgsl`); the camera looks
/// straight down onto it, framing `±TOPDOWN_HALF_EXTENT` (aspect-corrected on the long axis).
fn topdown_view_proj(width: u32, height: u32) -> Mat4 {
    let aspect = width.max(1) as f32 / height.max(1) as f32;
    let (hx, hy) = if aspect >= 1.0 {
        (TOPDOWN_HALF_EXTENT * aspect, TOPDOWN_HALF_EXTENT)
    } else {
        (TOPDOWN_HALF_EXTENT, TOPDOWN_HALF_EXTENT / aspect)
    };
    let proj = Mat4::orthographic_rh(-hx, hx, -hy, hy, -10.0, 10.0);
    let view = Mat4::look_at_rh(
        Vec3::new(0.0, 0.0, 5.0),
        Vec3::new(0.0, 0.0, 0.0),
        Vec3::new(0.0, 1.0, 0.0),
    );
    proj * view
}

/// Embodied perspective view-projection (free fn — eye position + yaw + viewport only, no
/// `Game`/device needed): eye at the possessed unit's position, raised by `EYE_HEIGHT`,
/// looking out across the ground plane along the current yaw.
fn embodied_view_proj(eye_x: f32, eye_y: f32, yaw: f32, width: u32, height: u32) -> Mat4 {
    let eye = Vec3::new(eye_x, eye_y, EYE_HEIGHT);
    let dir = Vec3::new(yaw.cos(), yaw.sin(), -0.15).normalize();
    let target = eye + dir;

    let aspect = width.max(1) as f32 / height.max(1) as f32;
    let proj = Mat4::perspective_rh(60_f32.to_radians(), aspect, 0.05, 500.0);
    let view = Mat4::look_at_rh(eye, target, Vec3::Z);
    proj * view
}

/// Unproject a pointer pixel onto the ground plane (z = 0) under the given TOP-DOWN
/// `view_proj`, returning world `(x, y)`. For the orthographic camera the world XY is
/// independent of NDC depth, so we invert `view_proj` at the near plane. `None` if the
/// matrix is singular.
fn unproject_topdown(
    view_proj: &Mat4,
    px: f32,
    py: f32,
    width: u32,
    height: u32,
) -> Option<(f32, f32)> {
    let inv = view_proj.inverse();
    if !inv.is_finite() {
        return None;
    }
    // Pixel → NDC. Pixel origin is top-left, +y down; NDC +y is up, so flip y.
    let ndc_x = (px / width.max(1) as f32) * 2.0 - 1.0;
    let ndc_y = 1.0 - (py / height.max(1) as f32) * 2.0;
    let world = inv * Vec4::new(ndc_x, ndc_y, 0.0, 1.0);
    if world.w.abs() < f32::EPSILON {
        return None;
    }
    Some((world.x / world.w, world.y / world.w))
}

/// Map this frame's `input` + current `embodied` state into the sim commands it produces,
/// for the given `player` entity and `viewport`. PURE (no `Game`/device): builds the
/// top-down camera and unprojects the tap internally, quantizing the target to `Fixed` AT
/// THE INPUT BOUNDARY (invariant #1).
///
/// - Command-layer tap: only in top-down (`!embodied`), on a pointer-down edge with a known
///   position → one [`Command::Move`].
/// - Embody/surface (invariant #5): edge-triggered, mutually exclusive, *resolved by current
///   state* — `embody_pressed && !embodied` → [`Command::Embody`]; `surface_pressed &&
///   embodied` → [`Command::Surface`]. The Android two-finger tap sets BOTH flags; this
///   state-resolution turns it into the correct toggle.
fn map_input_commands(
    input: &InputFrame,
    embodied: bool,
    player: Entity,
    width: u32,
    height: u32,
) -> Vec<Command> {
    let mut commands: Vec<Command> = Vec::new();

    // Command-layer tap: only in top-down, on a pointer-down edge with a known position.
    // The target is unprojected then quantized to Fixed AT THIS BOUNDARY (invariant #1).
    if !embodied && input.pointer_down {
        if let Some((px, py)) = input.pointer {
            let view_proj = topdown_view_proj(width, height);
            if let Some((wx, wy)) = unproject_topdown(&view_proj, px, py, width, height) {
                commands.push(Command::Move {
                    entity: player,
                    target: Vec2::new(world_to_fixed(wx), world_to_fixed(wy)),
                });
            }
        }
    }

    // Embodiment input-source swap (invariant #5) — edge-triggered, mutually exclusive,
    // resolved by current state (so the two-finger BOTH-flags gesture toggles correctly).
    if input.embody_pressed && !embodied {
        commands.push(Command::Embody { entity: player });
    } else if input.surface_pressed && embodied {
        commands.push(Command::Surface { entity: player });
    }

    commands
}

/// Spawn one Rifleman of `faction` at integer world `(x, y)` with the given stance, taking its
/// health + weapon from the shared [`economy::unit_stats`] table (so it matches a produced
/// unit). Demo-scene setup only — the sim itself is seeded the same on every peer.
fn spawn_unit(sim: &mut Sim, x: i32, y: i32, faction: Faction, stance: Stance) -> Entity {
    let (health, weapon) = economy::unit_stats(UnitKind::Rifleman);
    let e = sim.world.spawn();
    let i = e.index as usize;
    sim.world.faction[i] = faction;
    sim.world.pos[i] = Vec2::new(Fixed::from_int(x), Fixed::from_int(y));
    sim.world.health[i] = health;
    sim.world.weapon[i] = weapon;
    sim.world.stance[i] = stance;
    e
}

/// The shared game: the deterministic sim, the possessed entity, the renderer, the two
/// latest snapshots for interpolation, the fixed-tick accumulator, and embodiment/camera
/// state. Construct once a GPU device exists; drive [`Game::frame`] once per presented frame.
pub struct Game {
    sim: Sim,
    player: Entity,
    renderer: Renderer,

    /// The previous and current sim snapshots — the renderer interpolates between them.
    prev: Snapshot,
    curr: Snapshot,

    /// Fixed-tick accumulator: leftover seconds carried between frames. Host wall clock —
    /// floats are fine here.
    acc: f32,

    embodied: bool,
    camera: CameraMode,

    /// Accumulated embodied yaw (radians), integrated from raw look deltas. Presentation
    /// only — never written into the sim (D15).
    yaw: f32,
}

impl Game {
    /// Build the game against a live GPU device and set up the Phase 2 demo scene: two rifle
    /// squads skirmishing, a player camp producing reinforcements, and two neutral control
    /// points to capture. The returned `player` is a Player-faction unit you can embody.
    /// `seed` drives the deterministic sim — pass [`DEFAULT_SEED`] for the shared scene.
    pub fn new(device: &wgpu::Device, surface_format: wgpu::TextureFormat, seed: u64) -> Self {
        let mut sim = Sim::new(seed);
        sim.resources = Resources::new(500);

        // Two neutral control points to fight over.
        sim.territory
            .points
            .push(ControlPoint::neutral(Vec2::new(Fixed::ZERO, Fixed::ZERO)));
        sim.territory.points.push(ControlPoint::neutral(Vec2::new(
            Fixed::from_int(-16),
            Fixed::from_int(10),
        )));

        // Player squad (left). The first is the embodiable avatar; it holds and returns fire
        // (Idle order + FireAtWill stance), the allies attack-move into the enemy line.
        let player = spawn_unit(&mut sim, -7, -2, Faction::Player, Stance::FireAtWill);
        let ally_a = spawn_unit(&mut sim, -9, 4, Faction::Player, Stance::FireAtWill);
        let ally_b = spawn_unit(&mut sim, -9, -7, Faction::Player, Stance::FireAtWill);

        // Enemy squad (right), attack-moving toward the player line.
        let foe_a = spawn_unit(&mut sim, 8, 0, Faction::Enemy, Stance::FireAtWill);
        let foe_b = spawn_unit(&mut sim, 10, 6, Faction::Enemy, Stance::FireAtWill);
        let foe_c = spawn_unit(&mut sim, 9, -6, Faction::Enemy, Stance::FireAtWill);

        // A player camp, pre-built for the demo, producing reinforcements you can watch spawn.
        if let Some(camp) = economy::build(
            &mut sim.world,
            &mut sim.resources,
            Faction::Player,
            BuildingKind::Camp,
            Vec2::new(Fixed::from_int(-22), Fixed::ZERO),
        ) {
            sim.world.building[camp.index as usize].build_ticks_left = 0; // skip construction
            economy::queue_production(&mut sim.world, &mut sim.resources, camp, UnitKind::Rifleman);
            economy::queue_production(&mut sim.world, &mut sim.resources, camp, UnitKind::Rifleman);
        }

        // Kick off the skirmish: both squads advance into contact (combat fires en route).
        sim.step(&[
            Command::AttackMove {
                entity: ally_a,
                target: Vec2::new(Fixed::from_int(6), Fixed::from_int(2)),
            },
            Command::AttackMove {
                entity: ally_b,
                target: Vec2::new(Fixed::from_int(6), Fixed::from_int(-4)),
            },
            Command::AttackMove {
                entity: foe_a,
                target: Vec2::new(Fixed::from_int(-6), Fixed::ZERO),
            },
            Command::AttackMove {
                entity: foe_b,
                target: Vec2::new(Fixed::from_int(-6), Fixed::from_int(4)),
            },
            Command::AttackMove {
                entity: foe_c,
                target: Vec2::new(Fixed::from_int(-6), Fixed::from_int(-4)),
            },
        ]);

        let curr = sim.snapshot();
        let prev = curr.clone();
        let renderer = Renderer::new(device, surface_format);

        Game {
            sim,
            player,
            renderer,
            prev,
            curr,
            acc: 0.0,
            embodied: false,
            camera: CameraMode::TopDown,
            yaw: 0.0,
        }
    }

    /// The player's authoritative world position, read straight from the sim world (read
    /// only — the host never mutates sim state outside `Sim::step`). The snapshot carries no
    /// entity identity, so we read by index for the embodied camera.
    fn player_pos(&self) -> Vec2 {
        self.sim.world.pos[self.player.index as usize]
    }

    /// The sim's current tick count — a read-only window onto the deterministic clock so a
    /// host can surface sim progress (e.g. the on-device heartbeat) without reaching into
    /// private sim state. Observation only: never mutates the sim, no determinism impact.
    pub fn tick_count(&self) -> u64 {
        self.sim.tick_count()
    }

    /// The sim's current per-tick checksum — a read-only window onto deterministic state so a
    /// host can eyeball lockstep determinism on-device (the heartbeat logs it alongside the
    /// frame rate). Observation only: never mutates the sim, no determinism impact.
    pub fn checksum(&self) -> u64 {
        self.sim.checksum()
    }

    /// Embodied perspective view-projection for the active player — thin wrapper over the
    /// free [`embodied_view_proj`] that reads this player's authoritative position.
    fn embodied_view_proj(&self, width: u32, height: u32) -> Mat4 {
        let p = self.player_pos();
        let px = gonedark_render::fixed_to_f32(p.x);
        let py = gonedark_render::fixed_to_f32(p.y);
        embodied_view_proj(px, py, self.yaw, width, height)
    }

    /// Advance and present one frame: map this frame's `input` → sim commands, drain the
    /// fixed-tick accumulator by `dt_secs`, build the camera, and render the interpolated
    /// snapshot into `view`. `viewport` is the surface size in pixels. The host owns acquiring
    /// `view` and presenting afterward; this method never touches the platform surface.
    ///
    /// All host-float work; the only thing crossing into the sim is the Fixed-quantized
    /// command set (invariant #1).
    pub fn frame(
        &mut self,
        input: &InputFrame,
        dt_secs: f32,
        viewport: (u32, u32),
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        view: &wgpu::TextureView,
    ) {
        let (width, height) = viewport;

        // 1. Map input → sim commands (applied on the first step of this frame). The pure
        // mapping (tap-to-move + state-resolved embody/surface toggle) lives in the free
        // `map_input_commands`; here we apply the resulting embodiment state transition.
        let commands = map_input_commands(input, self.embodied, self.player, width, height);

        // Embodiment input-source swap (invariant #5): mirror the toggle the mapping resolved.
        for cmd in &commands {
            match cmd {
                Command::Embody { .. } => {
                    self.embodied = true;
                    self.camera = CameraMode::Embodied;
                    log::info!("[tick {}] EMBODY — world goes dark", self.sim.tick_count());
                }
                Command::Surface { .. } => {
                    self.embodied = false;
                    self.camera = CameraMode::TopDown;
                    log::info!("[tick {}] SURFACE — back to command", self.sim.tick_count());
                }
                _ => {}
            }
        }

        // Integrate look into presentation-only yaw (D15: never into the sim).
        self.yaw += input.look_axis.0 * LOOK_SENSITIVITY;

        // 2. Fixed-tick accumulator: advance the deterministic sim in whole ticks. This
        // frame's commands apply on the FIRST step; catch-up steps pass none. Clamped so a
        // huge first-frame / stall dt can't spiral.
        let tick_dt = 1.0 / TICK_HZ as f32;
        self.acc += dt_secs;

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
        if steps == MAX_CATCHUP_STEPS && self.acc >= tick_dt {
            self.acc = 0.0;
        }
        // Sub-tick frame: if no whole tick elapsed this frame (render faster than TICK_HZ) but
        // input produced commands, apply them now on an extra step so the edge-triggered
        // tap/embody intent — which fires for exactly one drained input frame — is not dropped.
        if first_step && !commands.is_empty() {
            self.prev = self.curr.clone();
            self.sim.step(&commands);
            self.curr = self.sim.snapshot();
        }

        // 3. Interpolation factor for the renderer (invariant #4).
        let alpha = (self.acc / tick_dt).clamp(0.0, 1.0);

        // 4. Build the camera for the active view.
        let view_proj = match self.camera {
            CameraMode::TopDown => topdown_view_proj(width, height),
            CameraMode::Embodied => self.embodied_view_proj(width, height),
        };
        let camera = Camera {
            view_proj: view_proj.to_cols_array_2d(),
        };

        // 5. Interpolate prev→curr into render instances (the float boundary lives in render).
        self.renderer.prepare(&self.prev, &self.curr, alpha);

        // 6. Render the interpolated snapshot (world goes dark while embodied) into `view`.
        self.renderer.render(
            device,
            queue,
            view,
            &camera,
            /* world_dark = */ self.embodied,
        );

        // 7. Avatar-local prediction seam (D15): presentation-only, NEVER writes sim state.
        predict_avatar(&self.curr, input, self.embodied);
    }
}

/// Avatar-local prediction (D15) lives HERE, in the presentation path. It reads sim state
/// plus the latest input to predict the embodied unit's transform for a responsive local
/// view, and MUST NOT feed back into the sim (or lockstep desyncs silently — invariant #1).
/// Authoritative resolution still happens in the sim at tick T+D. Stub for Phase 1.
fn predict_avatar(_snapshot: &Snapshot, _input: &InputFrame, _embodied: bool) {
    // TODO(phase3): integrate local aim/move from `_input`; reconcile against the tick.
}

#[cfg(test)]
mod tests {
    use super::*;
    use gonedark_core::ecs::World;
    use gonedark_render::fixed_to_f32;

    /// A throwaway player handle for the command-mapping tests — a real generational handle
    /// from a `World`, so the produced commands carry a valid entity.
    fn test_player() -> Entity {
        let mut world = World::new();
        world.spawn()
    }

    /// `world_to_fixed` is the input-boundary quantizer; round-tripping a representable world
    /// coordinate (integers, halves — exact in Q16.16) through `render::fixed_to_f32` must be
    /// lossless.
    #[test]
    fn world_to_fixed_round_trips_representable_coords() {
        for &w in &[0.0_f32, 1.0, -1.0, 20.0, -8.0, 0.5, -0.5, 12.25, -3.75] {
            let back = fixed_to_f32(world_to_fixed(w));
            assert!((back - w).abs() < 1e-4, "round-trip {w} -> {back}");
        }
    }

    /// World `(0,0)` projects to screen center under the top-down ortho camera.
    #[test]
    fn topdown_projects_origin_to_screen_center() {
        let (width, height) = (1920u32, 1080u32);
        let vp = topdown_view_proj(width, height);
        let clip = vp * Vec4::new(0.0, 0.0, 0.0, 1.0);
        let ndc_x = clip.x / clip.w;
        let ndc_y = clip.y / clip.w;
        // NDC center -> screen center.
        let px = (ndc_x * 0.5 + 0.5) * width as f32;
        let py = (1.0 - (ndc_y * 0.5 + 0.5)) * height as f32;
        assert!((px - width as f32 / 2.0).abs() < 1e-2, "center x = {px}");
        assert!((py - height as f32 / 2.0).abs() < 1e-2, "center y = {py}");
    }

    /// Unprojecting the center pixel returns ~`(0,0)`.
    #[test]
    fn unproject_center_pixel_is_origin() {
        let (width, height) = (1920u32, 1080u32);
        let vp = topdown_view_proj(width, height);
        let (wx, wy) =
            unproject_topdown(&vp, width as f32 / 2.0, height as f32 / 2.0, width, height).unwrap();
        assert!(wx.abs() < 1e-3, "center world x = {wx}");
        assert!(wy.abs() < 1e-3, "center world y = {wy}");
    }

    /// Unprojecting a known off-center pixel returns the expected world point. With a square
    /// viewport the ortho extent is `±TOPDOWN_HALF_EXTENT` on both axes, so the right edge maps
    /// to `+half_extent` in x and the top edge to `+half_extent` in y.
    #[test]
    fn unproject_offcenter_pixel_matches_expected_world() {
        let (width, height) = (1000u32, 1000u32); // square -> symmetric extent
        let vp = topdown_view_proj(width, height);

        // Right edge, vertical center -> (+half_extent, 0).
        let (wx, wy) = unproject_topdown(&vp, width as f32, height as f32 / 2.0, width, height)
            .expect("right-edge unproject");
        assert!((wx - TOPDOWN_HALF_EXTENT).abs() < 1e-2, "right x = {wx}");
        assert!(wy.abs() < 1e-2, "right y = {wy}");

        // Top edge (py = 0, +y up), horizontal center -> (0, +half_extent).
        let (wx, wy) = unproject_topdown(&vp, width as f32 / 2.0, 0.0, width, height)
            .expect("top-edge unproject");
        assert!(wx.abs() < 1e-2, "top x = {wx}");
        assert!((wy - TOPDOWN_HALF_EXTENT).abs() < 1e-2, "top y = {wy}");
    }

    /// Top-down + `pointer_down` with a pointer set -> exactly one `Move`, target ≈ the
    /// unprojected world point.
    #[test]
    fn map_input_topdown_tap_produces_single_move() {
        let (width, height) = (1280u32, 720u32);
        let player = test_player();
        let (px, py) = (900.0_f32, 300.0_f32);

        let input = InputFrame {
            pointer: Some((px, py)),
            pointer_down: true,
            ..Default::default()
        };
        let cmds = map_input_commands(&input, false, player, width, height);
        assert_eq!(cmds.len(), 1, "exactly one command");

        let vp = topdown_view_proj(width, height);
        let (wx, wy) = unproject_topdown(&vp, px, py, width, height).unwrap();
        match cmds[0] {
            Command::Move { entity, target } => {
                assert_eq!(entity, player);
                // Compare via the same Fixed bits the mapping produced.
                assert_eq!(target.x.to_bits(), world_to_fixed(wx).to_bits());
                assert_eq!(target.y.to_bits(), world_to_fixed(wy).to_bits());
            }
            ref other => panic!("expected Move, got {other:?}"),
        }
    }

    /// `embody_pressed && !embodied` -> contains `Embody`, not `Surface`.
    #[test]
    fn map_input_embody_when_surfaced() {
        let player = test_player();
        let input = InputFrame {
            embody_pressed: true,
            ..Default::default()
        };
        let cmds = map_input_commands(&input, false, player, 800, 600);
        assert!(cmds.iter().any(|c| matches!(c, Command::Embody { .. })));
        assert!(!cmds.iter().any(|c| matches!(c, Command::Surface { .. })));
    }

    /// `surface_pressed && embodied` -> contains `Surface`.
    #[test]
    fn map_input_surface_when_embodied() {
        let player = test_player();
        let input = InputFrame {
            surface_pressed: true,
            ..Default::default()
        };
        let cmds = map_input_commands(&input, true, player, 800, 600);
        assert!(cmds.iter().any(|c| matches!(c, Command::Surface { .. })));
        assert!(!cmds.iter().any(|c| matches!(c, Command::Embody { .. })));
    }

    /// The Android two-finger gesture sets BOTH flags; the mapping resolves it by current
    /// state — `Embody` when surfaced, `Surface` when embodied.
    #[test]
    fn map_input_both_flags_resolve_by_state() {
        let player = test_player();
        let both = InputFrame {
            embody_pressed: true,
            surface_pressed: true,
            ..Default::default()
        };

        let surfaced = map_input_commands(&both, false, player, 800, 600);
        assert!(surfaced.iter().any(|c| matches!(c, Command::Embody { .. })));
        assert!(!surfaced
            .iter()
            .any(|c| matches!(c, Command::Surface { .. })));

        let embodied = map_input_commands(&both, true, player, 800, 600);
        assert!(embodied
            .iter()
            .any(|c| matches!(c, Command::Surface { .. })));
        assert!(!embodied.iter().any(|c| matches!(c, Command::Embody { .. })));
    }

    /// Embodied suppresses tap-to-move: a pointer-down while embodied produces no `Move`.
    #[test]
    fn map_input_embodied_suppresses_tap_to_move() {
        let player = test_player();
        let input = InputFrame {
            pointer: Some((100.0, 100.0)),
            pointer_down: true,
            ..Default::default()
        };
        let cmds = map_input_commands(&input, true, player, 800, 600);
        assert!(!cmds.iter().any(|c| matches!(c, Command::Move { .. })));
    }
}
