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
use gonedark_core::alerts::AlertChannel;
use gonedark_core::commander::{self, CommanderConfig, COMMANDER_PERIOD};
use gonedark_core::components::{BuildingKind, EntityKind, Faction, Posture, Stance, UnitKind, Vec2};
use gonedark_core::detection::{
    self, detectable_embodiment, DetectionConfig, DetectionMemory, Tell,
};
use gonedark_core::economy::{self, Resources};
use gonedark_core::ecs::Entity;
use gonedark_core::event::SimEvent;
use gonedark_core::fixed::Fixed;
use gonedark_core::fog::{self, Visibility};
use gonedark_core::lockstep::Lockstep;
use gonedark_core::rng::Rng;
use gonedark_core::shell::{ConnectionStatus, LinkState, MatchOutcome};
use gonedark_core::sim::{Command, Sim, TICK_HZ};
use gonedark_core::snapshot::Snapshot;
use gonedark_core::territory::ControlPoint;
use gonedark_pal::{Audio, InputFrame, SoundId, Transport};
use gonedark_render::marquee::Marquee;
use gonedark_render::overlay::Overlay;
use gonedark_render::radial::RadialMenu;
use gonedark_render::{fixed_to_f32, Camera, Renderer};

use selection::{GestureScale, Selection};
use session_shell::{
    evaluate_outcome, EndStateRead, FactionForces, InSessionShell, ShellSurface,
};

/// Embodied audio mix (worker 3). Owns `mix_cues`: events + listener pose → positioned cues.
mod audio;
/// Order/stance command vocabulary (worker 5). Owns `commands_for`: UI intent → sim commands.
mod command_ui;
/// Build palette vocabulary. Owns `build_commands`: a chosen structure + placement tap → a
/// `Command::Build`, quantizing the placement point to `Fixed` at the boundary (invariant #1).
mod build_ui;
/// Troop-training command-UI seam (Phase 2). Owns `train_commands`: a camp + unit-type choice →
/// `Command::QueueProduction`, plus the `rally_point` quantization seam (no camp-rally sim command
/// exists yet — flagged follow-up). Pure presentation→intent, like `command_ui`. Public so the
/// `train_commands` / `rally_point` seams are reachable for the host to wire (mirrors the pub
/// `readout` / `train_panel` render seams); the integrator routes the commands into the loop.
pub mod train_ui;
/// Pre-match gunsmith loadout UI (WS-C, D60). Owns `LoadoutEditor`: the command-layer surface that
/// holds the player's current `core::gunsmith::Loadout` and turns a slot+direction UI input into a
/// new selection. Pure presentation/state — it NEVER touches the sim; the chosen loadout is handed
/// to the scenario seeder, which applies it to the spawned weapon at match start
/// (`core::gunsmith::Loadout::apply_to_weapon`). Public so the host can wire the on-screen gunsmith.
pub mod loadout_ui;
/// Camp-upgrade UI intent. Owns `upgrade_commands`: an "upgrade the selected camp" tap →
/// `Command::Upgrade` (the "growth" half of command-and-grow). Pure intent, never mutates the sim.
/// Public so a host/integrator can wire the on-screen Upgrade button into the command stream.
pub mod upgrade_ui;
/// Command-layer unit selection (worker 4). Owns `Selection`: which units the next order hits.
mod selection;
/// Embodied-fire input seam (W1). Owns `fire_command`: host yaw + trigger → `Command::Fire`,
/// quantizing the aim direction to `Fixed` at the boundary (invariant #1).
mod fire;
/// Embodied-locomotion input seam. Owns `locomote_command`: host `move_axis` + look yaw →
/// `Command::Locomote` (camera-relative twin-stick), quantizing the world heading to `Fixed` at
/// the boundary (invariant #1, exactly like `fire`).
mod locomote;
/// On-screen FPS touch controls (the COD-style embodied HUD). Owns the pure `TouchControls` seam:
/// raw multi-touch points → embodied intents (`move_axis`/look/fire/crouch/reload/surface) + the
/// screen-space layout the renderer draws. The testable logic `pal-android` can't host. Public so
/// the renderer (and a host) can read the layout/HUD geometry.
pub mod touch_controls;
/// In-session shell (Phase 4 WS-B): the in-engine pause / surrender / post-match-summary /
/// reconnect-prompt state machine + the host-side `MatchSummary` assembler. Pure presentation/
/// session state — never mutates sim state. Public so a host (and tests) can drive it.
pub mod session_shell;
/// Render quality tuning (Phase 4 WS-C). Owns `RenderTuning`: the tier + dyn-res + thermal-backoff
/// controller. A RENDERING choice only — never touches the sim (invariant #1/#4).
pub mod tuning;
/// Host-side RTT estimator + input-delay hysteresis (Phase 3 WS-B). Owns `RttDelayEstimator`: it
/// smooths measured RTT (host-side `f64` EWMA) and decides when to ask `core::lockstep` to change
/// the integer input delay. Floats stay here (engine glue), never `core`/sim (invariants #1/#2).
pub mod net_tuning;

pub use tuning::RenderTuning;
pub use net_tuning::{DelayPolicy, RttDelayEstimator};

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

/// Default embodied pitch (radians): a slight downward tilt so the weapon viewmodel and the near
/// ground are framed the instant you possess a unit. Vertical look adjusts up/down from here.
const EMBODIED_PITCH_DEFAULT: f32 = -0.15;
/// Pitch clamp (radians, ~80°): how far the embodied look can tilt up/down before it would gimbal-
/// flip past straight-up/down. Keeps `look_at` well-conditioned (the up vector never collinear).
const EMBODIED_PITCH_MAX: f32 = 1.4;

/// Command-camera zoom bounds (half-extent in world units). Floor = closest framing (read a single
/// skirmish), ceiling = widest (survey the whole playfield). [`TOPDOWN_HALF_EXTENT`] is the default
/// inside this band.
const CAM_HALF_EXTENT_MIN: f32 = 12.0;
const CAM_HALF_EXTENT_MAX: f32 = 120.0;
/// Multiplicative zoom per wheel notch: one notch toward the player shrinks the half-extent to
/// `1/CAM_ZOOM_PER_NOTCH` of its value (zoom in), away grows it. Geometric so each notch feels equal
/// at any zoom.
const CAM_ZOOM_PER_NOTCH: f32 = 1.12;
/// Command-camera pan rate as a fraction of the current half-extent per second at full stick
/// deflection. Scaling pan speed by the zoom keeps the felt pan velocity (screen-fractions/sec)
/// constant: zoomed out you sweep more ground, zoomed in you nudge precisely.
const CAM_PAN_RATE: f32 = 1.3;

/// Avatar-local prediction (D15): the predicted embodied eye eases toward the authoritative
/// target by this fraction each frame — high enough to feel responsive, low enough that a
/// per-tick correction reads as smooth rather than a snap. Presentation feel knob; tunable.
/// TODO(phase3 feel polish): this is a raw per-FRAME coefficient, so the ease rate is
/// frame-rate-dependent (120 fps converges faster than 30). Make it dt-independent (half-life /
/// `1-(1-base)^dt`) once embodied locomotion gives real motion to tune against across device tiers.
const AVATAR_RECONCILE_SMOOTHING: f32 = 0.5;

/// Avatar-local prediction (D15): if the predicted eye is more than this many world units from
/// the authoritative target, **snap** instead of easing — a large correction (snapshot resume,
/// a future teleport, gross misprediction) should resolve at once, not slide across the world.
const AVATAR_RECONCILE_SNAP_DIST: f32 = 5.0;

/// Cap on catch-up sim steps in one frame, so a huge first-frame / stall `dt` can't spiral
/// the sim ("spiral of death"). Excess time is dropped.
const MAX_CATCHUP_STEPS: u32 = 8;

/// Single-player lockstep session: one peer (us), local id 0, and **zero input delay** —
/// commands execute on the tick they're issued, so there's no added input latency and the
/// feel matches today's direct stepping (D27 step 4). With `peer_count == 1` the gate clears
/// on the local slot alone, so no real transport is needed (`NullTransport`).
const SP_PEER_COUNT: u32 = 1;
const SP_LOCAL: gonedark_core::lockstep::PeerId = 0;
const SP_DELAY: u64 = 0;

/// Hard match-length cap, in sim ticks. Past this the win-condition evaluator decides the match on
/// the territory/resource tiebreak rather than letting it run forever (a stalemate where neither
/// side can finish the other still has to end). 15 real minutes at the locked 60 Hz tick
/// (`TICK_HZ`). Host-side presentation policy only — the sim has no clock and this never folds into
/// the checksum (invariants #1/#4/#7).
const MATCH_TIMEOUT_TICKS: u64 = 15 * 60 * TICK_HZ as u64;

/// A transport that goes nowhere: `send` drops the frame, `poll` is always empty. This is the
/// single-player wiring — `peer_count == 1` means the lockstep gate never waits on a remote, so
/// the only frames in flight are our own echoes, which `Lockstep` already ignores. Having it
/// (rather than skipping the transport entirely) keeps `frame`'s drive loop multiplayer-ready:
/// swap a real `pal::Transport` in and the same loop is a 2-peer client. Lives HERE in `engine`
/// (not `pal-desktop`) so the layering stays `engine -> {core, render, pal}` (invariant #2).
///
/// Single-player runs the transport as `None` (zero per-frame overhead — the one-peer gate clears
/// on local input alone), so this is the documented, tested seam for the multiplayer wiring rather
/// than something the live loop constructs today; the tests drive the seam through it to prove the
/// transport-present branch is stream-identical.
#[cfg_attr(not(test), allow(dead_code))]
struct NullTransport;

impl Transport for NullTransport {
    fn send(&mut self, _frame: &[u8]) {}
    fn poll(&mut self) -> Vec<Vec<u8>> {
        Vec::new()
    }
}

/// Drive `lockstep` forward by up to `budget` ticks this frame, stepping `sim` with each ready
/// tick's merged command set — the wgpu-free seam under [`Game::frame`]'s fixed-tick accumulator
/// (D27 step 4). It mirrors the `net-sim-runner` reference drive loop, in order:
///
/// 1. **Submit** `budget` local sets — one per tick this frame intends to advance. The FIRST
///    carries this frame's `commands`; the rest are empty — exactly as the old accumulator applied
///    commands only on its first step and passed `&[]` to catch-up steps. (`Lockstep::submit`
///    stamps each to its own execution tick `delay + submitted`, so input delay is honoured
///    without the caller tracking tick numbers.)
/// 2. **Pump the transport** (if present): `drain_outbound -> send`, then `poll -> deliver`. With
///    the single-player `NullTransport` both are no-ops; with a real peer this is the wire pump.
/// 3. **Advance**: `while try_advance()` (clamped to `budget`) hand each ready tick's merged set
///    to `step` — a closure that snapshots `prev = curr`, calls `sim.step`, accumulates events,
///    and refreshes `curr` back in `Game`.
///
/// Returns the number of ticks advanced. For the single-player session (`peer_count == 1`,
/// `delay == 0`) the gate clears on the local slot alone with no warmup, so each submitted set is
/// returned by `try_advance` immediately and in order: the stepped checksum stream is
/// **bit-identical** to stepping `sim` directly with the same per-frame `commands` on the first
/// step and `&[]` after (invariant #7). `step` only ever sees a merged `&[Command]`, so the seam
/// stays wgpu-free and is unit-testable against a bare `Sim`.
///
/// Caller contract (held by `Game::frame`): never call with `budget == 0` while `commands` is
/// non-empty — the sub-tick fallback raises the budget to 1 first — so a frame's input is never
/// submitted to a tick it then declines to advance (which, at `delay == 0`, would strand it).
fn drive_lockstep(
    sim: &mut Sim,
    lockstep: &mut Lockstep,
    transport: Option<&mut (dyn Transport + '_)>,
    commands: Vec<Command>,
    budget: u32,
    mut step: impl FnMut(&mut Sim, &[Command]),
) -> u32 {
    // 1. Submit exactly `budget` local sets — the first carrying this frame's commands, the rest
    // empty. One submit per tick we intend to advance keeps `submitted` in step with the ticks
    // executed (no over-submission stranding input at delay 0).
    let mut commands = Some(commands);
    for _ in 0..budget {
        lockstep.submit(commands.take().unwrap_or_default());
    }

    // 2. Pump the transport: ship our outbound frames, then deliver anything inbound. No-op for
    // single-player (NullTransport); the real wire pump for a 2-peer client.
    if let Some(transport) = transport {
        for frame in lockstep.drain_outbound() {
            transport.send(&frame);
        }
        for frame in transport.poll() {
            // A malformed frame from the wire is an error to handle, not a crash. There is no
            // host-visible error channel here yet; drop it (a resend will carry a good copy) and
            // let the gate stall — the same loss-tolerant posture the protocol already takes.
            let _ = lockstep.deliver(&frame);
        }
    }

    // 3. Advance every ready tick into the sim, clamped to this frame's budget.
    let mut advanced = 0u32;
    while advanced < budget {
        match lockstep.try_advance() {
            Some(merged) => {
                step(sim, &merged);
                advanced += 1;
            }
            None => break,
        }
    }
    advanced
}

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

/// Command-view camera tilt above the horizon (D45). A three-quarter RTS pitch (think Company of
/// Heroes) so the 3D greybox tokens (D44) read as forms with visible fronts/sides instead of flat
/// tops. **Pitch only — the camera never yaws**, which is load-bearing: a pure tilt about the world
/// X axis keeps the ground↔screen mapping *separable* (screen-X depends only on world-X, screen-Y
/// only on world-Y), so band-select's world-space axis-aligned rectangle test stays exact. A yaw
/// would shear that and break picking. 90° here would be the old straight-down view.
const COMMAND_PITCH_DEG: f32 = 58.0;
/// Orthographic eye distance from the focus. With an orthographic projection this does **not** set
/// the on-screen size (the extents do) — it only positions the eye so the near/far planes bracket
/// the scene; it must stay larger than the scene's half-extent in Z + the framed ground radius.
const COMMAND_EYE_DIST: f32 = 120.0;

/// Command-view orthographic view-projection (free fn — viewport + camera state only, no
/// `Game`/device needed). World units are on the ground plane (z = 0; see `render/shader.wgsl`); the
/// camera looks down at a fixed [`COMMAND_PITCH_DEG`] tilt from the south (−Y). `(focus_x, focus_y)`
/// is the ground point centered on screen (camera PAN), and `half_extent` is the world radius framed
/// to the shorter screen edge (camera ZOOM — smaller = closer). Aspect-corrected on the long axis;
/// the tilt foreshortens the Y axis slightly. World +Y reads as "into the screen / north", +Z "up".
/// The tilt is pure pitch (no yaw) so the ground projection stays axis-separable — see
/// [`COMMAND_PITCH_DEG`]. Pan only translates the eye+target together, so it never shears that
/// separability (band-select stays exact); zoom only scales the ortho extents.
fn topdown_view_proj(width: u32, height: u32, focus_x: f32, focus_y: f32, half_extent: f32) -> Mat4 {
    let aspect = width.max(1) as f32 / height.max(1) as f32;
    let (hx, hy) = if aspect >= 1.0 {
        (half_extent * aspect, half_extent)
    } else {
        (half_extent, half_extent / aspect)
    };
    let pitch = COMMAND_PITCH_DEG.to_radians();
    // Focus point on the ground; eye sits south-and-above it; look straight at it (+Z up, no
    // roll/yaw). Translating both eye and target by the focus is a rigid pan — the view direction
    // and the pure-pitch tilt are unchanged, so screen-X still depends only on world-X (and Y on Y).
    let focus = Vec3::new(focus_x, focus_y, 0.0);
    let eye = focus
        + Vec3::new(
            0.0,
            -COMMAND_EYE_DIST * pitch.cos(),
            COMMAND_EYE_DIST * pitch.sin(),
        );
    let proj = Mat4::orthographic_rh(
        -hx,
        hx,
        -hy,
        hy,
        COMMAND_EYE_DIST - 100.0,
        COMMAND_EYE_DIST + 140.0,
    );
    let view = Mat4::look_at_rh(eye, focus, Vec3::Z);
    proj * view
}

/// The embodied camera's perspective parameters. Shared by [`embodied_view_proj`] (the world
/// camera) and [`embodied_proj`] (handed to the weapon viewmodel pass) so the gun's projection can
/// never drift from the world it sits in.
const EMBODIED_FOV_DEG: f32 = 60.0;
const EMBODIED_NEAR: f32 = 0.05;
const EMBODIED_FAR: f32 = 500.0;

/// The embodied perspective projection alone (no view), for the viewport. The weapon viewmodel is
/// placed in *view space*, so it needs the projection by itself (D44).
fn embodied_proj(width: u32, height: u32) -> Mat4 {
    let aspect = width.max(1) as f32 / height.max(1) as f32;
    Mat4::perspective_rh(EMBODIED_FOV_DEG.to_radians(), aspect, EMBODIED_NEAR, EMBODIED_FAR)
}

/// Whether the embodied first-person frame draws the handheld rifle viewmodel (W5/D44) for a
/// possessed unit of `kind`. The viewmodel is the rifleman's `weapon_rifle` greybox — an *infantry*
/// weapon — so it is drawn only when an infantry unit is possessed. A possessed tank (`Heavy`) has
/// no handheld weapon (and no cannon-viewmodel asset yet), so it shows none rather than a rifle
/// floating incongruously in the lower-right of the gun camera. PURE → unit-tested.
fn embodied_shows_rifle_viewmodel(kind: UnitKind) -> bool {
    match kind {
        UnitKind::Rifleman => true,
        // Vehicles have no handheld weapon; the Medic carries no offensive weapon at all (D65) — so
        // none of these show the rifle viewmodel.
        UnitKind::Heavy | UnitKind::Tank | UnitKind::Medic => false,
    }
}

/// Embodied perspective view-projection (free fn — eye position + yaw/pitch + viewport only, no
/// `Game`/device needed): eye at the possessed unit's position, raised by `EYE_HEIGHT`, looking out
/// along the current `yaw` (heading) and `pitch` (up/down tilt, radians; +up, −down).
fn embodied_view_proj(eye_x: f32, eye_y: f32, yaw: f32, pitch: f32, width: u32, height: u32) -> Mat4 {
    let eye = Vec3::new(eye_x, eye_y, EYE_HEIGHT);
    // Spherical look direction: pitch tilts the (yaw) heading up/down about the horizon. Already
    // unit-length (cos²+sin² folds to 1), but normalize defensively against fp drift.
    let (cp, sp) = (pitch.cos(), pitch.sin());
    let dir = Vec3::new(yaw.cos() * cp, yaw.sin() * cp, sp).normalize();
    let target = eye + dir;

    let proj = embodied_proj(width, height);
    let view = Mat4::look_at_rh(eye, target, Vec3::Z);
    proj * view
}

/// Unproject a pointer pixel onto the ground plane (z = 0) under the given command-view
/// `view_proj`, returning world `(x, y)`. Casts the pixel's view ray (the segment between its
/// near- and far-plane unprojections) and intersects it with `z = 0` — correct for the **tilted**
/// command camera (D45), where world XY now varies with depth, and for any future perspective
/// camera. `None` if the matrix is singular or the ray is parallel to the ground.
///
/// Accepted tradeoff under the tilt (D45): this returns the *ground* point under the cursor, so
/// tapping the visible top of a raised 3D token lands a touch north of its feet (≈ height·cotθ ≈
/// 0.94 wu for a ~1.5 wu token at 58°). The zoom-aware tap pick radius ([`selection::GestureScale`],
/// ~3.5 wu at the default command zoom) comfortably swallows that offset, so a tap still resolves
/// the unit; a true mesh-accurate pick (ray vs. token volume) is deferred until it's worth it.
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
    // Two points on the pixel's ray: NDC depth 0 (near) and 1 (far).
    let near4 = inv * Vec4::new(ndc_x, ndc_y, 0.0, 1.0);
    let far4 = inv * Vec4::new(ndc_x, ndc_y, 1.0, 1.0);
    if near4.w.abs() < f32::EPSILON || far4.w.abs() < f32::EPSILON {
        return None;
    }
    let a = near4.truncate() / near4.w;
    let b = far4.truncate() / far4.w;
    let dir = b - a;
    if dir.z.abs() < f32::EPSILON {
        return None; // ray parallel to the ground plane — no intersection
    }
    // a.z + t·dir.z = 0  →  the parameter where the ray meets z = 0.
    let t = -a.z / dir.z;
    let hit = a + dir * t;
    Some((hit.x, hit.y))
}

/// Map this frame's `input` + current `embodied` state into the sim commands it produces, for the
/// given `player` entity. PURE (no `Game`/device).
///
/// This handles ONLY the embodiment input-source swap. Unit *movement* is no longer a side effect
/// of any pointer-down — under the classic-RTS desktop scheme (D42) the **left-click selects** and
/// the **right-click commands the selection** (`command_ui::command_click_commands`, wired in
/// [`Game::frame`]); a bare click never moves a hard-wired avatar. (The old "any tap moves
/// `player`" behavior was the unintuitive feel this replaced.)
///
/// - Embody/surface (invariant #5): edge-triggered, mutually exclusive, *resolved by current
///   state* — `embody_pressed && !embodied` → [`Command::Embody`] (targeting the resolved
///   `embody_target`, see [`embody_target`]); `surface_pressed && embodied` → [`Command::Surface`]
///   on the current `avatar`. The Android two-finger tap sets BOTH flags; this state-resolution
///   turns it into the correct toggle.
///
/// `avatar` is the entity currently possessed (the surface target); `embody_target` is the entity
/// the player would possess this frame (`None` when there is no live unit to take, so the embody
/// press is a correct no-op rather than possessing a corpse).
fn map_input_commands(
    input: &InputFrame,
    embodied: bool,
    avatar: Entity,
    embody_target: Option<Entity>,
) -> Vec<Command> {
    let mut commands: Vec<Command> = Vec::new();

    // Embodiment input-source swap (invariant #5) — edge-triggered, mutually exclusive,
    // resolved by current state (so the two-finger BOTH-flags gesture toggles correctly).
    if input.embody_pressed && !embodied {
        // Only possess a real, live unit — a `None` target (no player unit to take) drops the
        // press so we never flip to embodied over nothing.
        if let Some(target) = embody_target {
            commands.push(Command::Embody { entity: target });
        }
    } else if input.surface_pressed && embodied {
        commands.push(Command::Surface { entity: avatar });
    }

    commands
}

/// Whether `e` is a live, possessable Player unit — alive, Player faction, and a unit (not a
/// building). Shared by [`embody_target`] and the embody picker. `is_alive` validates the
/// generation first, so a stale selected handle whose slot was reused fails here and is skipped.
fn is_live_player_unit(world: &gonedark_core::ecs::World, e: Entity) -> bool {
    world.is_alive(e)
        && world.faction[e.index as usize] == Faction::Player
        && world.kind[e.index as usize] == EntityKind::Unit
}

/// The live, possessable Player units in the current selection, in selection order — the rows of the
/// embody picker. Two or more means the player band-selected a mixed group and we ask *which* to
/// possess; zero or one falls through to [`embody_target`]'s direct path. PURE → unit-tested.
fn live_selected_player_units(
    selection: &Selection,
    world: &gonedark_core::ecs::World,
) -> Vec<Entity> {
    selection
        .units
        .iter()
        .copied()
        .filter(|&e| is_live_player_unit(world, e))
        .collect()
}

/// What an open embody picker should do with this frame's input.
#[derive(Debug, PartialEq)]
enum PickOutcome {
    /// Possess this unit (a row was chosen) and close the picker.
    Pick(Entity),
    /// Close the picker without possessing (a tap missed the list, or embody/surface was re-pressed).
    Cancel,
    /// Leave the picker open (no decisive input this frame).
    Stay,
}

/// Resolve an open embody picker against this frame's input (PURE → unit-tested). A number key
/// (`command_slot` `0`→row 0, i.e. the player's "1" key) or a `tap_row` hit picks that unit; a tap
/// that missed every row (`pointer_up` with `tap_row == None`), or a fresh embody/surface press,
/// cancels; anything else leaves it open. An out-of-range number key is ignored (the picker stays),
/// never a mis-pick.
fn embody_pick_outcome(
    rows: &[Entity],
    command_slot: Option<u8>,
    tap_row: Option<usize>,
    pointer_up: bool,
    embody_pressed: bool,
    surface_pressed: bool,
) -> PickOutcome {
    if let Some(s) = command_slot {
        if (s as usize) < rows.len() {
            return PickOutcome::Pick(rows[s as usize]);
        }
    }
    if let Some(r) = tap_row {
        if r < rows.len() {
            return PickOutcome::Pick(rows[r]);
        }
    }
    if pointer_up || embody_pressed || surface_pressed {
        return PickOutcome::Cancel;
    }
    PickOutcome::Stay
}

/// Build the picker's presentation description from the live selected entities — one labelled row per
/// unit (`Heavy`→"Tank", `Rifleman`→"Rifleman"), all possessable today. Render-only mapping.
fn embody_picker_view(
    rows: &[Entity],
    world: &gonedark_core::ecs::World,
) -> gonedark_render::picker::EmbodyPicker {
    use gonedark_render::picker::{EmbodyPicker, PickerRow};
    EmbodyPicker {
        rows: rows
            .iter()
            .map(|&e| PickerRow {
                label: unit_kind_name(world.unit_kind[e.index as usize]).to_string(),
                embodiable: true,
            })
            .collect(),
    }
}

/// Friendly display name for a unit kind, shared by the embody picker and the command panel.
fn unit_kind_name(k: UnitKind) -> &'static str {
    match k {
        UnitKind::Rifleman => "Rifleman",
        // Heavy was labelled "Tank" before a real Tank existed; now it reads as itself (D65).
        UnitKind::Heavy => "Heavy",
        UnitKind::Tank => "Tank",
        UnitKind::Medic => "Medic",
    }
}

/// Friendly display name for a stance (the command panel's troops summary).
fn stance_name(s: gonedark_core::components::Stance) -> &'static str {
    use gonedark_core::components::Stance;
    match s {
        Stance::HoldFire => "Hold Fire",
        Stance::ReturnFire => "Return Fire",
        Stance::FireAtWill => "Fire at Will",
    }
}

/// Derive the contextual command panel from the current selection (PURE → unit-tested). CoH-style:
///  - a selected **camp** (building) → its resources, train options, upgrade, and production queue;
///  - selected **troops** → their composition, average health, and stance (the order/stance
///    vocabulary is the unit "options" — invariant #3; in-match unit upgrades don't exist yet);
///  - **nothing** selected → the build palette + banked resources.
///
/// Reuses the same `train_options` / `upgrade_view` / `build_menu_entries` data the dedicated panels
/// used, so the numbers match the sim. Reads only presentation state (kind, level, queue, health,
/// stance) — never mutates or folds (invariant #4). When several buildings are selected the first
/// (selection order) drives the panel; a mixed building+troops selection shows the building.
fn command_panel_view(
    world: &gonedark_core::ecs::World,
    selection: &Selection,
    resources: i64,
    trainable: &[UnitKind],
) -> gonedark_render::command_panel::CommandPanelView {
    use gonedark_core::components::EntityKind;
    use gonedark_render::command_panel::{CommandPanelView, LineStyle, PanelLine};

    // Affordable rows read normal; rows you can't yet pay for dim out (mirrors the old panels).
    let afford = |cost: i64| {
        if resources >= cost {
            LineStyle::Normal
        } else {
            LineStyle::Dim
        }
    };

    // 1. A selected, live Player camp → its command panel (selection order picks the first).
    if let Some(camp) = selection.units.iter().copied().find(|&e| {
        world.is_alive(e)
            && world.faction[e.index as usize] == Faction::Player
            && world.kind[e.index as usize] == EntityKind::Building
    }) {
        let b = &world.building[camp.index as usize];
        let level = b.level;
        let mut lines = vec![PanelLine::new(
            format!("Resources: {resources}"),
            LineStyle::Normal,
        )];
        lines.push(PanelLine::new("TRAIN  [R/H]", LineStyle::Header));
        for o in gonedark_render::train_panel::train_options(trainable, level, resources) {
            // `unit_kind_name` (not the train-panel `label`) so a Heavy reads "Tank" here exactly as
            // it does in the QUEUE rows, the troops panel, and the embody picker.
            lines.push(PanelLine::new(
                format!("{}  {}  {:.1}s", unit_kind_name(o.kind), o.cost, o.eta_seconds),
                afford(o.cost),
            ));
        }
        lines.push(PanelLine::new("UPGRADE  [U]", LineStyle::Header));
        let uv = gonedark_render::upgrade_panel::upgrade_view(level, resources);
        let speed = if uv.prod_ticks_next < uv.prod_ticks_now {
            "  (faster build)"
        } else {
            ""
        };
        lines.push(PanelLine::new(
            format!("Next tier: {}{}", uv.next_cost, speed),
            afford(uv.next_cost),
        ));
        if !b.queue.is_empty() {
            lines.push(PanelLine::new("QUEUE", LineStyle::Header));
            for item in &b.queue {
                lines.push(PanelLine::new(
                    format!(
                        "{}  {:.1}s",
                        unit_kind_name(item.kind),
                        gonedark_render::train_panel::eta_seconds(item.ticks_left)
                    ),
                    LineStyle::Dim,
                ));
            }
        }
        return CommandPanelView {
            title: format!("CAMP — TIER {level}"),
            lines,
        };
    }

    // 2. Selected troops → composition + average health + stance.
    let units: Vec<Entity> = selection
        .units
        .iter()
        .copied()
        .filter(|&e| is_live_player_unit(world, e))
        .collect();
    if !units.is_empty() {
        let mut riflemen = 0usize;
        let mut heavies = 0usize;
        let mut tanks = 0usize;
        let mut medics = 0usize;
        let mut hp_sum = 0.0f32;
        for &e in &units {
            match world.unit_kind[e.index as usize] {
                UnitKind::Rifleman => riflemen += 1,
                UnitKind::Heavy => heavies += 1,
                UnitKind::Tank => tanks += 1,
                UnitKind::Medic => medics += 1,
            }
            hp_sum += fixed_to_f32(world.health[e.index as usize].fraction());
        }
        let n = units.len();
        let avg_hp = (hp_sum / n as f32 * 100.0).round() as i32;
        // A uniform stance reads by name; a mixed group reads "Mixed".
        let first_stance = world.stance[units[0].index as usize];
        let uniform = units
            .iter()
            .all(|&e| world.stance[e.index as usize] == first_stance);

        let mut lines = Vec::new();
        for (count, label) in [
            (riflemen, "Rifleman"),
            (heavies, "Heavy"),
            (tanks, "Tank"),
            (medics, "Medic"),
        ] {
            if count > 0 {
                lines.push(PanelLine::new(format!("{count}x {label}"), LineStyle::Normal));
            }
        }
        lines.push(PanelLine::new(format!("Avg HP: {avg_hp}%"), LineStyle::Normal));
        lines.push(PanelLine::new(
            format!(
                "Stance: {}",
                if uniform { stance_name(first_stance) } else { "Mixed" }
            ),
            LineStyle::Normal,
        ));
        lines.push(PanelLine::new("[E] embody   [1-9] orders", LineStyle::Dim));
        return CommandPanelView {
            title: format!("SELECTED — {} unit{}", n, if n == 1 { "" } else { "s" }),
            lines,
        };
    }

    // 3. Nothing selected → the build palette + banked resources.
    let mut lines = vec![PanelLine::new(
        format!("Resources: {resources}"),
        LineStyle::Normal,
    )];
    for e in gonedark_render::build_menu::build_menu_entries(resources) {
        lines.push(PanelLine::new(
            e.text,
            if e.affordable {
                LineStyle::Normal
            } else {
                LineStyle::Dim
            },
        ));
    }
    lines.push(PanelLine::new(
        "[B] place   select a unit/camp for more",
        LineStyle::Dim,
    ));
    CommandPanelView {
        title: "BUILD".to_string(),
        lines,
    }
}

/// Pick the Player unit to POSSESS this frame (invariant #5: embodiment is an input-source swap,
/// resolved over *live* units — never a hardwired avatar). The RTS "select, then possess" rule:
///  1. the first LIVE selected Player unit (what the command layer has highlighted), else
///  2. the `current` avatar if it is still alive (re-possess the same unit when nothing is
///     selected), else
///  3. the first live Player unit in stable entity-index order — so a dead avatar never *strands*
///     embodiment. (This is the bug it fixes: once your first avatar died, `E` re-targeted the
///     corpse, `Sim` ignored the dead entity, and auto-surface bounced you straight back to command
///     — possession looked dead because no live unit was ever taken.)
///
/// Returns `None` only when the player has NO live unit at all (every possession path is then a
/// correct no-op). PURE (no `Game`/device) → unit-tested. The chosen entity rides into the lockstep
/// [`Command::Embody`], so it is the local player's intent (like a tap target), applied
/// bit-identically on every peer (the sim swaps that one entity's `InputSource`). For a multi-unit
/// selection the host opens the embody picker instead, so the player chooses which (see `frame`).
fn embody_target(
    selection: &Selection,
    world: &gonedark_core::ecs::World,
    current: Entity,
) -> Option<Entity> {
    // 1. First live, selected Player unit.
    if let Some(&e) = selection
        .units
        .iter()
        .find(|&&e| is_live_player_unit(world, e))
    {
        return Some(e);
    }
    // 2. Keep the current avatar if it is still alive.
    if is_live_player_unit(world, current) {
        return Some(current);
    }
    // 3. Any live Player unit, in stable index order, so a death never permanently kills embodiment.
    for i in 0..world.capacity() {
        if world.is_index_alive(i)
            && world.faction[i] == Faction::Player
            && world.kind[i] == EntityKind::Unit
        {
            return world.entity(i);
        }
    }
    None
}

/// Whether the host should auto-surface this frame: the player is embodied but their avatar entity
/// is no longer present in the freshly-stepped snapshot (it died and the sim despawned it). PURE
/// (no `Game`/device) so it is unit-testable. This is local UI/camera state only — surfacing a
/// dead avatar emits NO sim `Command` (the entity is already gone; a `Surface` for it would be a
/// sim no-op and must not be double-emitted). `embodied` short-circuits so a never-embodied player
/// is never auto-surfaced by an unrelated absence.
#[inline]
fn should_auto_surface(embodied: bool, avatar_present: bool) -> bool {
    embodied && !avatar_present
}

/// Did the embodied avatar land a hit this tick? True iff the player is `embodied` AND some
/// `SimEvent::Damaged` in this frame's deterministic event stream names `avatar` as its `source` —
/// i.e. the avatar's OWN shot dealt damage. PURE (no `Game`/device) so it is the unit-testable seam
/// behind the WS-4 hit-feedback cue (the hitmarker + hit SFX).
///
/// Invariant #6: this keys STRICTLY on the avatar-as-source — feedback on the player's own action,
/// not intel about an unseen enemy. It reads only the event stream (already-checksummed state copied
/// out, never re-folded) + the local embodied flag; it mutates nothing and never enters `core`.
fn avatar_landed_hit(events: &[SimEvent], avatar: Entity, embodied: bool) -> bool {
    embodied
        && events.iter().any(|e| {
            matches!(*e, SimEvent::Damaged { source, .. } if source == avatar)
        })
}

/// The embodied crouch button → a `Command::Crouch` for `player`, or `None` when no press edge
/// fired this frame. PURE (no `Game`/device) so the toggle inversion is unit-testable. A press
/// **edge** flips posture off the avatar's CURRENT (authoritative sim) crouched state — the host
/// holds no toggle bit, so a desktop key and the on-screen Crouch button share one path and a
/// reconnecting peer's posture is never second-guessed. The caller guards that `player` is alive
/// before reading `currently_crouched`.
#[inline]
fn crouch_toggle_command(
    player: Entity,
    crouch_edge: bool,
    currently_crouched: bool,
) -> Option<Command> {
    crouch_edge.then_some(Command::Crouch {
        entity: player,
        crouched: !currently_crouched,
    })
}

/// The sim commands an embodied frame's resolved control intents produce, plus the two presentation
/// side effects the caller ([`Game::frame`]) must apply itself (it holds the `Game`, this fn does
/// not): `fired` → stamp the muzzle-flash tick, `surfaced` → flip the local camera back to command.
struct EmbodiedCommands {
    commands: Vec<Command>,
    /// A `Command::Fire` was emitted this frame (the trigger was held) — drives `last_fire_tick`.
    fired: bool,
    /// A `Command::Surface` was emitted via the on-screen eject button (touch) — the host flips its
    /// camera/embodiment state to match (desktop ejects through `map_input_commands` instead).
    surfaced: bool,
}

/// Compose one embodied frame's already-resolved control intents (merged from EITHER the Android
/// touch HUD or the desktop keyboard/mouse, upstream in [`Game::frame`]) into the sim commands they
/// produce, for the possessed `player`. PURE (no `Game`/device) so the whole input→command pipeline —
/// trigger→[`Command::Fire`] (aim quantized at the boundary), stick→[`Command::Locomote`], crouch
/// toggle, reload, surface — is exercised end to end by a test without a GPU, mirroring exactly how
/// `frame` wired these seams inline before. `yaw` is the post-look-integration heading;
/// `currently_crouched`/`player_alive` are the authoritative sim reads the crouch toggle inverts off
/// of (the caller guards the alive check, as `crouch_toggle_command` documents). Command order —
/// fire, locomote, crouch, reload, surface — is preserved so the lockstep stream is byte-unchanged.
#[allow(clippy::too_many_arguments)] // mirrors the per-control intents `Game::frame` resolves inline
fn embodied_input_commands(
    player: Entity,
    yaw: f32,
    move_axis: (f32, f32),
    fire: bool,
    crouch_edge: bool,
    reload_edge: bool,
    surface_edge: bool,
    currently_crouched: bool,
    player_alive: bool,
) -> EmbodiedCommands {
    let mut commands: Vec<Command> = Vec::new();

    // Embodied fire (W1, invariant #5/#1): a pressed trigger emits a `Command::Fire` whose aim is the
    // host yaw quantized to `Fixed` AT THE BOUNDARY (pure seam `fire::fire_command`). The cone-hitscan
    // hit resolves sim-side, bit-identically on every peer. Embodied units never auto-fire.
    let fired = if let Some(cmd) = fire::fire_command(player, yaw, fire) {
        commands.push(cmd);
        true
    } else {
        false
    };

    // Embodied locomotion (twin-stick): the WASD / virtual-stick `move_axis` becomes a
    // camera-relative `Command::Locomote` whose world heading is quantized to `Fixed` AT THE BOUNDARY
    // (pure seam `locomote::locomote_command`, exactly like the fire aim).
    if let Some(cmd) = locomote::locomote_command(player, yaw, move_axis) {
        commands.push(cmd);
    }

    // Crouch TOGGLE: derive the target posture from authoritative sim state (pure
    // `crouch_toggle_command`), so a desktop key and the on-screen Crouch button share one path. Only
    // when the avatar is alive — a dead handle has no posture to flip.
    if player_alive {
        if let Some(cmd) = crouch_toggle_command(player, crouch_edge, currently_crouched) {
            commands.push(cmd);
        }
    }

    // Reload: start a magazine reload (a no-op sim-side if there's no magazine / it's full / already
    // reloading — see `combat`).
    if reload_edge {
        commands.push(Command::Reload { entity: player });
    }

    // Surface via the on-screen button (touch): two fingers now mean move+look, so the Surface
    // button — not the two-finger gesture — ejects while embodied.
    let surfaced = surface_edge;
    if surfaced {
        commands.push(Command::Surface { entity: player });
    }

    EmbodiedCommands {
        commands,
        fired,
        surfaced,
    }
}

/// Translate the engine-side touch layout + per-frame HUD state into the renderer's own
/// [`TouchControlsHud`](gonedark_render::touch_controls::TouchControlsHud) description (px circles +
/// pressed flags). PURE → host-testable. Keeps the layering one-way (`engine -> render`, invariant
/// #2): the engine fills render's struct, render never sees `engine::touch_controls`. `crouched`
/// (authoritative sim posture) lights the Crouch button's sticky toggle highlight.
fn render_touch_hud(
    layout: &touch_controls::TouchLayout,
    hud: &touch_controls::TouchHud,
    viewport: (u32, u32),
    crouched: bool,
) -> gonedark_render::touch_controls::TouchControlsHud {
    use gonedark_render::touch_controls as r;
    let button = |c: &touch_controls::Circle, glyph, pressed, active| r::TouchButton {
        cx: c.cx,
        cy: c.cy,
        r: c.r,
        glyph,
        pressed,
        active,
    };
    r::TouchControlsHud {
        viewport,
        stick: hud.stick_active.then_some(r::StickView {
            base_x: hud.stick_origin.0,
            base_y: hud.stick_origin.1,
            radius: layout.stick_radius,
            thumb_x: hud.stick_thumb.0,
            thumb_y: hud.stick_thumb.1,
        }),
        fire: button(&layout.fire, r::TouchGlyph::Fire, hud.fire_pressed, false),
        crouch: button(&layout.crouch, r::TouchGlyph::Crouch, hud.crouch_pressed, crouched),
        reload: button(&layout.reload, r::TouchGlyph::Reload, hud.reload_pressed, false),
        surface: button(&layout.surface, r::TouchGlyph::Surface, hud.surface_pressed, false),
    }
}

/// Is `c` a ONE-SHOT/edge command — an intent that fires for a single input frame (embody, surface,
/// a tap-order, build/train/upgrade, a stance change) — as opposed to a HELD/continuous command
/// re-emitted every frame while a control is held ([`Command::Locomote`], [`Command::Fire`])?
///
/// Used by the sub-tick catch-up rule: a one-shot must force a tick if none elapsed this frame (or
/// it is lost), but a held command must NOT — forcing a tick per render frame makes the sim advance
/// at the render rate while the key is held, scaling avatar speed / fire rate with FPS. A dropped
/// held command is re-emitted next frame, so it costs nothing to skip. PURE → unit-testable.
#[inline]
fn is_oneshot_command(c: &Command) -> bool {
    !matches!(c, Command::Locomote { .. } | Command::Fire { .. })
}

/// Integrate one frame's horizontal mouse-look into the embodied yaw (radians). PURE so the
/// turn-direction is unit-testable without a window. The look delta is **subtracted**: with the
/// embodied basis (look dir `(cos yaw, sin yaw)`, world +Z up) the camera's screen-right is world
/// −Y, so a rightward mouse move (`look_dx > 0`) must *decrease* yaw to rotate the view toward −Y
/// (i.e. turn right). Adding it inverts the horizontal axis — the bug this fixes.
#[inline]
fn integrate_look_yaw(yaw: f32, look_dx: f32) -> f32 {
    yaw - look_dx * LOOK_SENSITIVITY
}

/// Integrate one frame's vertical mouse-look into the embodied pitch (radians), clamped to
/// ±[`EMBODIED_PITCH_MAX`] so the view can't flip past vertical. The delta is **subtracted** (winit
/// screen +Y points down, so moving the mouse UP gives a negative `look_dy` → pitch increases → the
/// view looks UP): non-inverted, consistent with [`integrate_look_yaw`]. PURE → unit-testable.
#[inline]
fn integrate_look_pitch(pitch: f32, look_dy: f32) -> f32 {
    (pitch - look_dy * LOOK_SENSITIVITY).clamp(-EMBODIED_PITCH_MAX, EMBODIED_PITCH_MAX)
}

/// Advance the command camera's ground focus (PAN) from the WASD / stick `move_axis` over `dt`
/// seconds. PURE → testable without a device. `move_axis` is the host screen-convention stick
/// (`+mx` = right/`D`, `+my` = down, so `W`/up is `−my`); the command ground maps screen-right →
/// world +X and screen-up/north → world +Y, so the world pan is `(+mx, −my)`. Speed scales with
/// `half_extent` ([`CAM_PAN_RATE`]) so the felt pan velocity is constant across zoom. Returns the new
/// `(focus_x, focus_y)`.
#[inline]
fn pan_focus(
    focus_x: f32,
    focus_y: f32,
    move_axis: (f32, f32),
    half_extent: f32,
    dt: f32,
) -> (f32, f32) {
    let (mx, my) = move_axis;
    let step = CAM_PAN_RATE * half_extent * dt;
    (focus_x + mx * step, focus_y - my * step)
}

/// Apply wheel `scroll` notches to the command camera's `half_extent` (ZOOM), clamped to
/// [`CAM_HALF_EXTENT_MIN`]..=[`CAM_HALF_EXTENT_MAX`]. PURE → testable. Positive scroll = zoom IN =
/// smaller extent. Geometric (`MIN^scroll`) so each notch scales by the same factor at any zoom and
/// the result never flips sign or hits zero.
#[inline]
fn zoom_half_extent(half_extent: f32, scroll: f32) -> f32 {
    let scaled = half_extent * CAM_ZOOM_PER_NOTCH.powf(-scroll);
    scaled.clamp(CAM_HALF_EXTENT_MIN, CAM_HALF_EXTENT_MAX)
}

/// The player's **active camp** for the per-camp command panels (train + upgrade) — the lowest-index
/// **built, operational** [`BuildingKind::Camp`] owned by `faction`, or `None` if it has none. A pure,
/// deterministic read over the world (stable entity-index order, identical on every peer): no autonomy
/// (invariant #3), no sim mutation, folds nothing into the checksum (invariants #1/#7). "Operational"
/// means construction finished (`build_ticks_left == 0`) so a half-built camp isn't offered for
/// production. Selecting a *specific* camp is a Stage-2 input concern; until then the primary camp is
/// the deterministic default — documented and tested.
fn active_player_camp(world: &gonedark_core::ecs::World, faction: Faction) -> Option<Entity> {
    for i in 0..world.capacity() {
        if !world.is_index_alive(i)
            || world.faction[i] != faction
            || world.kind[i] != EntityKind::Building
        {
            continue;
        }
        let b = &world.building[i];
        if b.kind == BuildingKind::Camp && b.build_ticks_left == 0 {
            return world.entity(i);
        }
    }
    None
}

/// Map this frame's command-view **production** intents — build / train / upgrade (Phase 2's "command
/// and grow your camps") — onto sim commands for `Faction::Player`. PURE (no `Game`/device), so it is
/// host-testable: the device-bound [`Game::frame`] only resolves the two inputs (the unprojected
/// cursor `pointer_world` and the deterministic `active_camp`) and calls this, gated on the command
/// view. Delegates to the three tested intent seams, each of which no-ops on a missing slot/edge/camp:
///  - [`build_ui::build_commands`]: `building_slot` + the cursor ground point → `Command::Build`
///    (placement quantized to `Fixed` at the boundary, invariant #1);
///  - [`train_ui::train_commands`]: `train_slot` + the active camp → `Command::QueueProduction`;
///  - [`upgrade_ui::upgrade_commands`]: `upgrade_pressed` + the active camp → `Command::Upgrade`.
///
/// The caller restricts this to the command view (never embodied — invariant #6: no command-layer
/// production while the map is dark); affordability/legality stays the sim's authoritative call.
fn command_view_production_commands(
    input: &InputFrame,
    pointer_world: Option<(f32, f32)>,
    active_camp: Option<Entity>,
) -> Vec<Command> {
    let mut commands = Vec::new();
    commands.extend(build_ui::build_commands(
        input.building_slot,
        Faction::Player,
        pointer_world,
    ));
    commands.extend(train_ui::train_commands(input.train_slot, active_camp));
    commands.extend(upgrade_ui::upgrade_commands(input.upgrade_pressed, active_camp));
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
    /// only — never written into the sim (D15). The aim half of the predicted avatar transform.
    yaw: f32,

    /// Accumulated embodied pitch (radians, +up/−down), integrated from the vertical look delta and
    /// clamped to ±[`EMBODIED_PITCH_MAX`]. Presentation only (camera tilt) — the sim aim is 2D, so
    /// pitch never enters fire/locomote, only the first-person view direction.
    pitch: f32,

    /// Command-camera ground focus (the world point centered on screen) and framed half-extent
    /// (zoom). Presentation only — the RTS camera pans (`cam_focus_*`) and zooms (`cam_half_extent`)
    /// with no effect on the sim. Updated from `move_axis`/`scroll` each command-view frame.
    cam_focus_x: f32,
    cam_focus_y: f32,
    cam_half_extent: f32,

    /// Avatar-local prediction (D15): the smooth, led embodied **eye** for the first-person
    /// camera + audio listener. PRESENTATION ONLY — see [`AvatarPrediction`]; it holds no sim
    /// state and the type can never reach `&mut Sim`, so it cannot desync lockstep.
    avatar: AvatarPrediction,

    /// Command-layer unit selection (worker 4) — which player units the next order targets.
    /// Presentation state; drives the order vocabulary, never the sim directly.
    selection: Selection,

    /// The radial command menu currently open on a held long-press: the action labels the player
    /// is choosing from this frame, empty when no menu is open. Pure presentation intent — the
    /// preview emits NO `Command`s (nothing reaches the sim until a slot is committed; invariant
    /// #3). Exposed via [`Game::radial_menu`] for a future on-screen radial renderer.
    radial_menu: Vec<&'static str>,

    /// The open embody-unit picker: the live selected Player units the player is choosing one of to
    /// possess, in selection order. `None` when closed. Opened by pressing embody with two or more
    /// units selected (a single selection embodies directly); a row pick / number key emits
    /// `Command::Embody`, a miss cancels. Pure presentation/intent state — never sim state.
    embody_picker: Option<Vec<Entity>>,

    /// The rolling embodied alert channel (worker 2's HUD reads this; `core::alerts` derives it).
    /// A presentation derivation from the event stream — never sim state (invariant #7).
    alerts: AlertChannel,

    /// The per-tick command exchange the sim is driven through (D27 step 4). Single-player runs a
    /// one-peer, zero-delay session, so the gate clears on local input alone and commands execute
    /// the tick they're issued (no added latency, today's feel). The frame loop submits this
    /// frame's mapped commands and steps the sim from `try_advance()`'s merged set rather than
    /// stepping it directly — making the loop multiplayer-ready without forking the path.
    lockstep: Lockstep,

    /// The wire transport for `lockstep`'s byte frames, or `None` for single-player. `peer_count
    /// == 1` means the gate never waits on a remote, so the single-player session needs no real
    /// transport; a real `pal::Transport` (loopback/UDP/relay) drops in here for multiplayer with
    /// no change to the drive loop. Boxed `dyn` so `engine` stays free of any concrete backend
    /// (the layering is `engine -> {core, render, pal}`, invariant #2).
    transport: Option<Box<dyn Transport>>,

    /// Host-side adaptive-input-delay estimator (Phase 3 WS-B). Smooths measured RTT and, when its
    /// hysteresis gate fires, hands `lockstep` an integer delay target via `propose_delay`. Consulted
    /// only on a NETWORKED session (`transport.is_some()`); single-player never proposes (no peer,
    /// delay stays 0). The float EWMA lives here in `engine` glue, never `core` (invariants #1/#2);
    /// `core` only ever sees the integer delay/guard. The RTT sample source is the host seam
    /// [`Game::observe_rtt`] — see that method and `net_tuning`'s docs for the production source.
    rtt_estimator: RttDelayEstimator,

    /// The in-session shell (Phase 4 WS-B): pause / surrender / post-match summary / reconnect
    /// prompt. Pure presentation/session state — it never mutates the sim. It drives the pause-
    /// halts-tick rule (single-player only) and which overlay `render` composites over the frame.
    shell: InSessionShell,

    /// This frame's accumulated sim events, kept on `Game` so the post-match summary assembler can
    /// count produced/lost/killed over the match. Presentation derivation only (the event stream is
    /// already-checksummed state copied out — never re-folded; invariant #7).
    match_events: Vec<SimEvent>,

    /// Render quality-tuning controller (Phase 4 WS-C): the active tier + the running
    /// dynamic-resolution scale + the thermal backoff. A RENDERING choice only — it reads frame
    /// timing + the host-reported thermal state and NEVER touches the sim, so the per-tick checksum
    /// stream is byte-identical at every tier (invariant #1/#4).
    tuning: RenderTuning,

    /// The latest thermal state the host reported through the PAL (invariant #2 — the platform
    /// signal crosses the seam, never `core`). The host calls [`Game::set_thermal_state`] from its
    /// `pal::ThermalSensor`; defaults to `Nominal` (the desktop stub's value) until it does.
    thermal: gonedark_pal::ThermalState,

    /// The enemy commander's OWN deterministic RNG (W3). Seeded `sim_seed ^ faction-id` so it is
    /// reproducible yet decoupled from the checksummed `Sim::rng()` stream — the commander must
    /// NEVER draw from `sim.rng()` (a host-side draw would advance that stream and desync
    /// lockstep, invariant #7). The commander's orders are pushed into the same `commands` Vec
    /// player commands ride, so they travel the lockstep stream and stay bit-identical on every
    /// peer; this RNG is host-side planning input only, never sim state.
    commander_rng: Rng,

    /// Tunables for the enemy commander (D — "tunable mechanism, not locked design"). Defaults to
    /// `CommanderConfig::default()` (`hunt_embodied = false`), so the commander reproduces its
    /// original behavior byte-for-byte and the default scenes' checksum streams are untouched. A
    /// host opts into the gone-dark hunt via [`Game::set_commander_hunts_embodied`].
    commander_config: CommanderConfig,

    /// Host-local linger memory for the commander's detection consult (W3 / D). **Presentation-side
    /// state, never sim state** — exactly like the HUD's `DetectionMemory`. The commander runs only
    /// on the local host and its orders ride the lockstep stream to peers, so this host-private
    /// memory can never desync lockstep (invariant #7). Only touched when `commander_config
    /// .hunt_embodied` is set; otherwise it stays empty and unused.
    commander_detection: DetectionMemory,

    /// The sim tick the embodied player last fired on, or `None` if they have not fired this match
    /// (W5). PRESENTATION ONLY — it drives the weapon viewmodel's muzzle-flash cue
    /// ([`gonedark_render::world::muzzle_flash_intensity`]); it is never read by the sim and never
    /// crosses into `core`. Set from the host-side `input.fire` edge in `frame`, alongside the
    /// `Command::Fire` the sim resolves authoritatively (invariant #4/#6: a render cue, not intel).
    last_fire_tick: Option<u64>,

    /// The sim tick the embodied avatar's own shot last *connected* (dealt damage), or `None` if it
    /// hasn't this match (WS-4). PRESENTATION ONLY — derived from the deterministic `SimEvent::Damaged`
    /// stream where `source` is the avatar (the pure [`avatar_landed_hit`] seam) and the player is
    /// embodied; it drives the centered hitmarker flash ([`gonedark_render::hud::hitmarker_marker`])
    /// plus a one-shot hit SFX. It is feedback on the player's OWN action — never intel about an
    /// unseen enemy — so it stays inside invariant #6, and never feeds the sim / never enters `core`.
    last_hit_tick: Option<u64>,

    /// Per-frame-persistent touch-control state (the Android FPS HUD): which finger owns the move
    /// stick / look region + button-edge history. Drives the embodied intents from
    /// `input.touches` while possessed. PRESENTATION/INPUT only — host-side floats, never sim state
    /// (the intents it yields are quantized to `Fixed` by the `fire`/`locomote` seams, invariant #1).
    touch: touch_controls::TouchControls,

    /// What the on-screen touch HUD should draw THIS frame, or `None` when no touch input drove the
    /// frame (e.g. desktop, or command view). Set in `frame`, read by the render step. Presentation
    /// only.
    touch_hud: Option<touch_controls::TouchHud>,

    /// Whether the debug hitbox / facet overlay is on (host **F3** toggle via
    /// [`Game::toggle_debug_hitboxes`]). Drawn ONLY in the command view (invariant #6) — surface to
    /// inspect the tanks. Defaults on for the duel debug scene, off otherwise. Pure presentation
    /// chrome: it reads the snapshot and never touches the sim.
    debug_hitboxes: bool,

    /// Tuning for the "gone dark" detection tell (`core::detection`, D33) — which mode, range, and
    /// linger this client surfaces. PRESENTATION/intel config only: it drives what the COMMAND view
    /// marks about hostile embodied enemies and is never folded into the checksum (invariants
    /// #1/#7). Defaults to the D33 `Subtle` baseline.
    detection: DetectionConfig,

    /// Per-client linger memory for `Subtle` detection tells (`core::detection::DetectionMemory`):
    /// the last-seen tick + position of each sensed hostile avatar, so a tell can fade in place after
    /// sight is lost. PRESENTATION state — each client holds its own for its own HUD; never sim state,
    /// never checksummed (invariant #6/#7). Mutated only by the read-only `detectable_embodiment`
    /// derivation in the command-view render path.
    detection_memory: DetectionMemory,
}

/// Which world [`Game::new_scene`] seeds. The default match is the Phase 2 demo skirmish; the
/// debug scenes are tiny, fully-deterministic sandboxes for exercising one mechanic — the
/// in-engine, *playable* counterpart to the headless `sim-runner` scenes, seeded from the SAME
/// `core::scenario` source so the thing you drive is the thing the harness validates.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum Scene {
    /// The Phase 2 demo skirmish: two rifle squads, producing camps, two neutral control points.
    /// A canned mid-action demo (units already advancing into contact).
    #[default]
    Default,
    /// The **playable two-base skirmish** ([`gonedark_core::scenario::seed_skirmish`]): two
    /// operational bases, **one starting troop each**, three neutral posts to capture. Booted in the
    /// command view — you select your troop, take posts to fund production, and grow an army; the
    /// scripted enemy commander does the same. The normal match the title screen's **Start** boots.
    Skirmish,
    /// The two-tank hitbox duel ([`gonedark_core::scenario::seed_duel`]), booted **embodied** in
    /// the player tank so you drop straight into first person, drive it, and fire the gun — the
    /// "load two tanks and see the hitboxes work" sandbox. A debug scene, not a real match.
    Duel,
    /// The infantry hitscan sandbox ([`gonedark_core::scenario::seed_infantry`]), booted **embodied**
    /// in the player rifleman: aim/crouch/fire at a row of dummies to feel range / cone / cover /
    /// line-of-sight. A debug scene, not a real match.
    Infantry,
}

impl Scene {
    /// Parse a scene name (e.g. an `app --scene <name>` CLI token). `None` for an unknown name so
    /// the host can report it. Pure + host-tested (no GPU), unlike `Game::new_scene` itself.
    pub fn parse(name: &str) -> Option<Scene> {
        match name {
            "default" | "demo" => Some(Scene::Default),
            "skirmish" | "match" => Some(Scene::Skirmish),
            "duel" => Some(Scene::Duel),
            "infantry" => Some(Scene::Infantry),
            _ => None,
        }
    }

    /// Whether this scene boots with the debug hitbox/facet overlay on — the debug sandboxes do,
    /// a real match does not.
    fn debug_overlay_default(self) -> bool {
        matches!(self, Scene::Duel | Scene::Infantry)
    }
}

/// Seed the **Phase 2 demo skirmish** and return `(player, start_embodied)`: two rifle squads, a
/// producing player + enemy camp, two neutral control points. GPU-free (mutates only the `Sim`), so
/// it is host-tested directly — the renderer-bearing `Game::new_scene` stays the thin glue. Starts
/// in the command view (`start_embodied == false`).
fn seed_default_scene(sim: &mut Sim) -> (Entity, bool) {
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
    let player = spawn_unit(sim, -7, -2, Faction::Player, Stance::FireAtWill);
    let ally_a = spawn_unit(sim, -9, 4, Faction::Player, Stance::FireAtWill);
    let ally_b = spawn_unit(sim, -9, -7, Faction::Player, Stance::FireAtWill);

    // Enemy squad (right). They start IDLE (Stance::FireAtWill) — the enemy commander (W3)
    // takes over from the first commander tick and drives them: capture points, press the
    // player line, and reinforce from its camp. No one-shot spawn order; the AI is in charge
    // the whole match (the previous single AttackMove left the enemy inert forever).
    spawn_unit(sim, 8, 0, Faction::Enemy, Stance::FireAtWill);
    spawn_unit(sim, 10, 6, Faction::Enemy, Stance::FireAtWill);
    spawn_unit(sim, 9, -6, Faction::Enemy, Stance::FireAtWill);

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

    // An enemy camp too, so the commander has somewhere to reinforce from — making the
    // opponent a real economic actor, not just three units that trade and vanish.
    if let Some(camp) = economy::build(
        &mut sim.world,
        &mut sim.resources,
        Faction::Enemy,
        BuildingKind::Camp,
        Vec2::new(Fixed::from_int(22), Fixed::ZERO),
    ) {
        sim.world.building[camp.index as usize].build_ticks_left = 0; // skip construction
    }

    // Kick off the player squad's advance into contact (combat fires en route). The enemy is
    // NOT scripted here — its first move comes from the commander on the next 1 s gate.
    sim.step(&[
        Command::AttackMove {
            entity: ally_a,
            target: Vec2::new(Fixed::from_int(6), Fixed::from_int(2)),
        },
        Command::AttackMove {
            entity: ally_b,
            target: Vec2::new(Fixed::from_int(6), Fixed::from_int(-4)),
        },
    ]);

    (player, false)
}

/// Seed the **playable two-base skirmish** and return `(player, start_embodied)`. Seeds the shared
/// `core::scenario::seed_skirmish` scene (two operational bases, one troop each, three neutral posts)
/// and hands back the Player's starting troop as the embodiable/selectable `player`. Booted in the
/// command view (`start_embodied == false`): unlike the debug sandboxes you start commanding, not
/// possessing. No scripted opening order — the enemy is the commander's from its first plan, and the
/// match-end is the host's existing win-condition evaluator. GPU-free, so it is host-tested directly.
fn seed_skirmish_scene(sim: &mut Sim) -> (Entity, bool) {
    let s = gonedark_core::scenario::seed_skirmish(sim);
    (s.player_troop, false)
}

/// Seed the **two-tank hitbox duel** and return `(player, start_embodied)`. Seeds the shared
/// `core::scenario::seed_duel` scene, then possesses the player tank so the sandbox boots in first
/// person (`start_embodied == true`) — the input-source swap (invariant #5) is the `Command::Embody`
/// stepped here, and the matching host-side `embodied`/camera state is set by the caller from the
/// returned flag. GPU-free, so it is host-tested directly.
fn seed_duel_scene(sim: &mut Sim) -> (Entity, bool) {
    let duel = gonedark_core::scenario::seed_duel(sim);
    // Drop straight into the tank: the embodied input-source swap a ballistic `Fire` needs.
    sim.step(&[Command::Embody {
        entity: duel.player,
    }]);
    // Mirror the telemetry the normal Command::Embody handler logs, so the embodiment event still
    // shows in the trace at duel launch (the host-side flag is set by the caller, not that handler).
    log::info!("[tick {}] EMBODY (duel boot) — world goes dark", sim.tick_count());
    (duel.player, true)
}

/// Seed the **infantry hitscan sandbox** and return `(player, start_embodied)`. Seeds the shared
/// `core::scenario::seed_infantry` scene, then possesses the player rifleman so the sandbox boots in
/// first person (`start_embodied == true`). GPU-free, so it is host-tested directly.
fn seed_infantry_scene(sim: &mut Sim) -> (Entity, bool) {
    let inf = gonedark_core::scenario::seed_infantry(sim);
    sim.step(&[Command::Embody { entity: inf.player }]);
    log::info!(
        "[tick {}] EMBODY (infantry boot) — world goes dark",
        sim.tick_count()
    );
    (inf.player, true)
}

/// Faction tint for an infantry unit's debug range ring.
fn faction_ring_color(f: Faction) -> [f32; 3] {
    match f {
        Faction::Player => [0.30, 0.55, 1.0],
        Faction::Enemy => [1.0, 0.40, 0.35],
        Faction::Neutral => [0.70, 0.70, 0.72],
    }
}

/// Compose the **command-view debug overlay** line list (the F3 overlay) from a snapshot + terrain.
/// Tanks (Heavy) get armour-facet hitbox rings + shell tracers; infantry (Rifleman) get a weapon
/// **range** ring + firing-**cone** wedge; and every Player→Enemy pair within the player's weapon
/// range gets a **line-of-sight** connector — green when the sightline is clear, red when Heavy
/// cover blocks it. GPU-free + pure (reads the snapshot + terrain, mutates nothing), so it is
/// host-tested without a device; the renderer just draws the returned world-space lines (invariant
/// #4 — presentation floats, never the sim; invariant #6 — the caller draws it command-view only).
// The command-view muzzle flash (core's snapshot `firing` window) and the embodied first-person
// viewmodel flash (render::world) must last the same wall-clock time. Invariant #2 bars `core` from
// depending on `render`, so the two windows are declared apart; pin them equal at compile time here
// in `engine` — the one crate that sees both — so they can never silently drift.
const _: () = assert!(
    gonedark_core::snapshot::MUZZLE_FLASH_TICKS as u64 == gonedark_render::world::MUZZLE_FLASH_TICKS
);

/// Count living `Unit`-kind entities of `faction` in `sim`. The testable seam behind
/// [`Game::alive_unit_count`] (the method is GPU-bound through `Game`; this free fn is driven
/// directly in tests). Read-only — no sim mutation, no checksum surface.
fn alive_units_of(sim: &Sim, faction: Faction) -> usize {
    (0..sim.world.capacity())
        .filter(|&i| {
            sim.world.is_index_alive(i)
                && sim.world.kind[i] == EntityKind::Unit
                && sim.world.faction[i] == faction
        })
        .count()
}

fn debug_overlay_lines(
    curr: &Snapshot,
    terrain: &gonedark_core::terrain::Terrain,
) -> Vec<gonedark_render::debug::DebugVertex> {
    use gonedark_render::debug::{self as dbg, DebugVertex};
    use gonedark_render::{fixed_to_f32 as fx, interp_angle};

    let mut verts: Vec<DebugVertex> = Vec::new();
    let yaw = |a| interp_angle(a, a, 0.0); // Angle → f32 radians (no interpolation needed)

    // Tanks: armour-facet hitbox rings + spokes, and a tracer behind every in-flight shell.
    let radius = fx(gonedark_core::projectile::HIT_RADIUS);
    let tanks: Vec<dbg::DebugUnit> = curr
        .units
        .iter()
        .filter(|u| {
            // Vehicle tokens (Heavy + the produced Tank, D65) get the hull/turret hitbox ring.
            !u.building && matches!(u.unit_kind, UnitKind::Heavy | UnitKind::Tank)
        })
        .map(|u| dbg::DebugUnit {
            x: fx(u.pos.x),
            y: fx(u.pos.y),
            hull_yaw: yaw(u.hull_heading),
            radius,
            is_tank: true,
        })
        .collect();
    verts.extend(dbg::hitbox_lines(&tanks));
    let shells: Vec<dbg::DebugShell> = curr
        .projectiles
        .iter()
        .map(|p| dbg::DebugShell {
            x: fx(p.pos.x),
            y: fx(p.pos.y),
            vx: fx(p.vel.x),
            vy: fx(p.vel.y),
        })
        .collect();
    verts.extend(dbg::tracer_lines(&shells));

    // Infantry: weapon range ring + firing-cone wedge (the produced Rifleman range + cone).
    let rifle = gonedark_core::economy::unit_stats(UnitKind::Rifleman).1;
    let cone = fx(gonedark_core::combat::FIRE_CONE_COS_HALF);
    let infantry: Vec<dbg::DebugInfantry> = curr
        .units
        .iter()
        .filter(|u| !u.building && u.unit_kind == UnitKind::Rifleman)
        .map(|u| dbg::DebugInfantry {
            x: fx(u.pos.x),
            y: fx(u.pos.y),
            facing: yaw(u.hull_heading),
            range: fx(rifle.range),
            cone_cos_half: cone,
            ring_color: faction_ring_color(u.faction),
        })
        .collect();
    verts.extend(dbg::infantry_lines(&infantry));

    // Muzzle flashes: any non-building unit that fired within the last few ticks (the snapshot
    // `firing` flag, derived from the weapon cooldown in `core::snapshot`) lights a bright burst —
    // the command-view analogue of the embodied viewmodel flash, so AI firing reads at a glance. The
    // spike points down the gun bearing (`turret_yaw`, which tracks the hull for turret-less units).
    const MUZZLE_FLASH_SIZE: f32 = 1.5;
    let flashes: Vec<dbg::DebugMuzzle> = curr
        .units
        .iter()
        .filter(|u| !u.building && u.firing)
        .map(|u| dbg::DebugMuzzle {
            x: fx(u.pos.x),
            y: fx(u.pos.y),
            facing: yaw(u.turret_yaw),
            size: MUZZLE_FLASH_SIZE,
        })
        .collect();
    verts.extend(dbg::muzzle_flash_lines(&flashes));

    // Line-of-sight connectors: from each Player unit to each Enemy unit within the player's weapon
    // range — green if the sightline is clear, red if a Heavy-cover wall blocks it (the LoS mechanic
    // made visible). Distances + LoS are read in fixed-point against the snapshot/terrain. The range
    // gate uses the archetype `unit_stats` range (the snapshot carries no per-entity weapon range);
    // exact for the debug scenes' produced units, and a no-op where it slightly differs.
    for p in curr
        .units
        .iter()
        .filter(|u| !u.building && u.faction == Faction::Player)
    {
        let prange = gonedark_core::economy::unit_stats(p.unit_kind).1.range;
        for e in curr
            .units
            .iter()
            .filter(|u| !u.building && u.faction == Faction::Enemy)
        {
            if (e.pos - p.pos).len_sq() > prange * prange {
                continue;
            }
            let color = if terrain.line_of_sight(p.pos, e.pos) {
                [0.25, 1.0, 0.40] // clear sightline
            } else {
                [1.0, 0.30, 0.30] // blocked by Heavy cover
            };
            verts.push(DebugVertex {
                world: [fx(p.pos.x), fx(p.pos.y)],
                color,
            });
            verts.push(DebugVertex {
                world: [fx(e.pos.x), fx(e.pos.y)],
                color,
            });
        }
    }

    verts
}

/// The faintest a `Subtle` detection tell fades to at the end of its linger window — kept above zero
/// so an aging, last-known marker is still legible (a ghost, not gone) right until it expires.
const MIN_TELL_ALPHA: f32 = 0.25;

/// Marker opacity for a tell aged `age_ticks` into a `linger_ticks` window. A fresh / in-sight /
/// `Marked` tell (`age_ticks == 0`) is fully opaque; a `Subtle` linger fades **linearly** from 1.0
/// toward [`MIN_TELL_ALPHA`] as it ages, so the commander reads "this is stale" at a glance. Pure
/// (the float side, invariant #4): floats are fine here — this is presentation, never sim math.
fn tell_alpha(age_ticks: u32, linger_ticks: u32) -> f32 {
    if linger_ticks == 0 || age_ticks == 0 {
        return 1.0;
    }
    let frac = (age_ticks as f32 / linger_ticks as f32).clamp(0.0, 1.0);
    1.0 - frac * (1.0 - MIN_TELL_ALPHA)
}

/// Map the per-observer "gone dark" detection [`Tell`]s (`core::detection::detectable_embodiment`)
/// into command-view render markers. PURE + GPU-free — the testable seam (mirrors
/// [`debug_overlay_lines`] / `render::interpolate_instances`): the only host-side glue is the
/// `Fixed -> f32` hop and the age → fade-alpha mapping.
///
/// ## Fairness (invariant #6) — the load-bearing guard
///
/// `world_dark` is the **local** player's embodiment. While the local player is embodied the command
/// view is gone (avatar-only fog), so this **refuses to emit any marker**: the detection tell is
/// command-view intel for the *commander*, and must never paint over the dark embodied frame. The
/// host also gates the call to the command view, so this is defense in depth — but the gate living
/// in the pure seam is what lets a headless test *prove* the tell stays dark while embodied, with no
/// GPU. The tell itself is "alerts, not intel": each marker is one already-earned, sensed unit's
/// live-or-last-seen point, never a reveal of the rest of the map (`core::detection` does the range +
/// line-of-sight gating and the last-seen lingering upstream).
fn detection_markers(
    tells: &[Tell],
    world_dark: bool,
    linger_ticks: u32,
) -> Vec<gonedark_render::detection::DetectionMarker> {
    use gonedark_render::detection::DetectionMarker;
    if world_dark {
        return Vec::new(); // embodied: no command-view intel can leak (invariant #6)
    }
    tells
        .iter()
        .map(|t| DetectionMarker {
            x: fixed_to_f32(t.pos.x),
            y: fixed_to_f32(t.pos.y),
            alpha: tell_alpha(t.age_ticks, linger_ticks),
        })
        .collect()
}

impl Game {
    /// Build the game against a live GPU device into the default [`Scene`] (the Phase 2 demo
    /// skirmish). The returned `player` is a Player-faction unit you can embody. `seed` drives the
    /// deterministic sim — pass [`DEFAULT_SEED`] for the shared scene.
    pub fn new(device: &wgpu::Device, surface_format: wgpu::TextureFormat, seed: u64) -> Self {
        Self::new_scene(device, surface_format, seed, Scene::Default)
    }

    /// Build the game against a live GPU device into a chosen [`Scene`]. The world seeding is the
    /// only thing that varies; everything else (renderer, interpolation snapshots, lockstep,
    /// shell, tuning) is identical across scenes. A debug scene may boot **embodied** (the duel
    /// sandbox does), which this reflects in the initial `embodied`/`camera` state.
    pub fn new_scene(
        device: &wgpu::Device,
        surface_format: wgpu::TextureFormat,
        seed: u64,
        scene: Scene,
    ) -> Self {
        let mut sim = Sim::new(seed);
        let (player, start_embodied) = match scene {
            Scene::Default => seed_default_scene(&mut sim),
            Scene::Skirmish => seed_skirmish_scene(&mut sim),
            Scene::Duel => seed_duel_scene(&mut sim),
            Scene::Infantry => seed_infantry_scene(&mut sim),
        };
        // The debug overlay defaults on for the sandboxes (their whole point), off for a real
        // match; F3 toggles it either way.
        let debug_hitboxes = scene.debug_overlay_default();

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
            // A debug scene may boot already possessing a unit (the duel sandbox does); the camera
            // follows that initial embodiment so first person is live from frame one.
            embodied: start_embodied,
            camera: if start_embodied {
                CameraMode::Embodied
            } else {
                CameraMode::TopDown
            },
            yaw: 0.0,
            pitch: EMBODIED_PITCH_DEFAULT,
            cam_focus_x: 0.0,
            cam_focus_y: 0.0,
            cam_half_extent: TOPDOWN_HALF_EXTENT,
            avatar: AvatarPrediction::default(),
            selection: Selection::new(),
            radial_menu: Vec::new(),
            embody_picker: None,
            alerts: AlertChannel::new(),
            // Single-player lockstep: one peer (us), local id 0, zero input delay (D27 step 4).
            lockstep: Lockstep::new(SP_PEER_COUNT, SP_LOCAL, SP_DELAY),
            // No remote → no real transport needed; the one-peer gate clears on local input alone.
            transport: None,
            // Adaptive-delay estimator with the default policy. Inert until RTT samples arrive AND
            // a transport is present (single-player never proposes a delay change). WS-B.
            rtt_estimator: RttDelayEstimator::new(DelayPolicy::default()),
            // Single-player session (one peer), so a pause may halt the local tick accumulator.
            shell: InSessionShell::new(SP_PEER_COUNT == 1),
            match_events: Vec::new(),
            // Render quality tuning (Phase 4 WS-C). Default to the High tier — the flagship profile
            // Phase 1 validated on (D22); a host wires its device-class tier (and the Settings
            // "graphics tiers" surface) via `set_tier`. RENDER-only state (invariant #1/#4).
            tuning: RenderTuning::new(gonedark_render::tiers::QualityTier::High),
            // Until the host reports through its `pal::ThermalSensor`, assume no thermal pressure.
            thermal: gonedark_pal::ThermalState::Nominal,
            // Enemy commander RNG: own stream seeded `sim_seed ^ faction-id` (W3) — decoupled from
            // the checksummed sim RNG so a host-side draw can never advance/desync it.
            commander_rng: Rng::new(seed ^ Faction::Enemy.index() as u64),
            // Commander tunables default OFF (byte-identical to the original commander); a host
            // enables the gone-dark hunt via `set_commander_hunts_embodied`. The detection linger
            // memory starts empty and is only used when that hunt is enabled.
            commander_config: CommanderConfig::default(),
            commander_detection: DetectionMemory::new(),
            // No shot fired yet → no muzzle flash (W5, presentation only).
            last_fire_tick: None,
            // No connecting shot yet → no hitmarker (WS-4, presentation only).
            last_hit_tick: None,
            // No touches tracked yet; the HUD is only built on embodied touch frames.
            touch: touch_controls::TouchControls::new(),
            touch_hud: None,
            debug_hitboxes,
            // The "gone dark" detection tell: D33 `Subtle` baseline, with its own per-client linger
            // memory. Presentation/intel only — never sim state, never checksummed (invariant #6/#7).
            detection: DetectionConfig::default(),
            detection_memory: DetectionMemory::new(),
        }
    }

    /// Toggle the debug hitbox / facet overlay (the host's **F3**). Visible only in the command
    /// view; pure presentation state, never the sim.
    pub fn toggle_debug_hitboxes(&mut self) {
        self.debug_hitboxes = !self.debug_hitboxes;
    }

    /// Whether the debug hitbox overlay is currently on (for a host indicator / test).
    pub fn debug_hitboxes(&self) -> bool {
        self.debug_hitboxes
    }

    /// The current in-session shell surface (pause / surrender-ended / reconnect / playing) — a
    /// read-only window for a host or test (e.g. to confirm a pause overlay is up). Presentation
    /// state only; no sim impact.
    pub fn shell_surface(&self) -> &ShellSurface {
        self.shell.surface()
    }

    /// Whether the player is currently embodied (possessing a unit; the strategic map is dark).
    /// A read-only host query for presentation concerns — e.g. the desktop host locks+hides the OS
    /// cursor only while embodied so mouse motion drives the FPS look instead of drifting onto UI.
    /// Never mutates state and never reaches the sim.
    pub fn is_embodied(&self) -> bool {
        self.embodied
    }

    /// Apply a resolved in-session [`SessionAction`](gonedark_core::shell::SessionAction) to the
    /// shell — the host calls this after `core::shell::resolve_intent` returns the
    /// `ResolvedIntent::Session` arm. Pause/Resume flip the overlay (and, single-player, the tick
    /// halt); Surrender ends the session with a freshly-assembled summary (invariant #5: no sim
    /// mutation — it never enters the lockstep stream). This is the only place the shell consumes a
    /// session action; it never touches `&mut Sim`.
    pub fn apply_session_action(&mut self, action: gonedark_core::shell::SessionAction) {
        let summary = self.assemble_summary();
        self.shell.apply(action, &summary);
    }

    /// Toggle the in-session pause overlay from the host (desktop **Esc**; the natural Android
    /// back-gesture binding too): open the pause menu while playing, dismiss it while paused. A
    /// no-op once the match has ended or a reconnect prompt owns the screen — those surfaces are
    /// dismissed by their own buttons, not the pause key (see [`pause_toggle_action`]). Once the
    /// pause overlay is up, the existing `overlay_click` seam reaches its **Resume** / **Surrender**
    /// buttons, so this trigger is all that was missing for the pause + in-match surrender loop.
    ///
    /// Pure session state: it routes through [`Self::apply_session_action`] and so never touches
    /// `&mut Sim` — a pause is host/session state that never enters the lockstep input stream, so
    /// the per-tick checksum is untouched (invariants #1/#4). The single-player tick halt follows
    /// from [`InSessionShell::halts_local_tick`], which `frame` reads.
    pub fn toggle_pause(&mut self) {
        if let Some(action) = pause_toggle_action(self.shell.surface()) {
            self.apply_session_action(action);
        }
    }

    /// Whether any in-session shell overlay (pause / reconnect prompt / post-match summary) is
    /// currently up — i.e. the match is *not* in the plain `Playing` surface. The host reads this
    /// to free the OS cursor (so the overlay's buttons are clickable) and to stop feeding
    /// world-driving input to the match frozen underneath. Read-only presentation state; the
    /// decision is the pure [`overlay_active`] seam.
    pub fn shell_overlay_active(&self) -> bool {
        overlay_active(self.shell.surface())
    }

    /// Hit-test a pointer click (given in normalized device coordinates — `x` rightward, `y` upward,
    /// the same screen space the overlay is drawn in) against the current in-session shell overlay's
    /// buttons, and resolve it to a host action. Returns `None` when no overlay is up or the click
    /// misses every button.
    ///
    /// This is the missing seam between a tap and the shell: the renderer lays the buttons out
    /// ([`overlay::button_slot_at`](gonedark_render::overlay::button_slot_at)) and this maps the hit
    /// slot to its action for the live surface. Read-only — the host applies the result (an
    /// [`OverlayClick::Session`] via [`Self::apply_session_action`]; an [`OverlayClick::Dismiss`] by
    /// tearing the match down and returning to its out-of-match screen, which the engine has no
    /// concept of).
    pub fn overlay_click(&self, ndc: (f32, f32)) -> Option<OverlayClick> {
        let overlay = overlay_for_surface(self.shell.surface());
        let slot = gonedark_render::overlay::button_slot_at(&overlay, ndc.0, ndc.1)?;
        overlay_click_action(&overlay, slot)
    }

    /// One faction's standing [`FactionForces`] — alive units/buildings + territory + purse — read
    /// off the checksummed sim world in the stable [`Faction::ALL`] index space. A read-only scan of
    /// already-checksummed state: it folds nothing new, so deriving it can never perturb the per-tick
    /// checksum or desync (invariants #1/#7). The inputs the host-side win-condition evaluator reads.
    fn faction_forces(&self, faction: Faction) -> FactionForces {
        let w = &self.sim.world;
        let mut alive_units = 0u32;
        let mut buildings = 0u32;
        for i in 0..w.capacity() {
            if !w.is_index_alive(i) || w.faction[i] != faction {
                continue;
            }
            match w.kind[i] {
                EntityKind::Unit => alive_units += 1,
                EntityKind::Building => buildings += 1,
            }
        }
        FactionForces {
            alive_units,
            buildings,
            // Territory points this faction holds (the timeout primary tiebreak).
            territory_held: self
                .sim
                .territory
                .points
                .iter()
                .filter(|cp| cp.owner == faction)
                .count() as u32,
            // The per-faction banked purse (economy `amounts` is `[i64; FACTION_COUNT]`) — no float
            // money (invariant #1). The timeout secondary tiebreak.
            resources_total: self.sim.resources.get(faction),
        }
    }

    /// The match outcome *right now*, or `None` while the match is still ongoing. A pure host-side
    /// read: derives each combatant's [`FactionForces`] from checksummed sim state and hands them to
    /// the unit-tested [`evaluate_outcome`] (elimination, then a territory/resource timeout
    /// tiebreak). No sim mutation, nothing folded — it cannot desync (invariants #1/#7).
    fn match_outcome(&self) -> Option<MatchOutcome> {
        evaluate_outcome(
            self.faction_forces(Faction::Player),
            self.faction_forces(Faction::Enemy),
            self.sim.tick_count(),
            MATCH_TIMEOUT_TICKS,
        )
    }

    /// Build the post-match [`MatchSummary`](gonedark_core::shell::MatchSummary) from the match's
    /// accumulated events + end-of-match reads of checksummed sim state (territory held, resource
    /// purse), stamped with the real [`MatchOutcome`] from [`Self::match_outcome`] (elimination /
    /// timeout tiebreak; D34 keeps the evaluator host-side, not in `core`). Float-free, host-side;
    /// the assembler and the evaluator are each unit-tested in `session_shell`. `outcome` falls back
    /// to `Draw` only on a surrender before either side is eliminated (the match was not won).
    fn assemble_summary(&self) -> gonedark_core::shell::MatchSummary {
        let mut reads: [EndStateRead; gonedark_core::components::FACTION_COUNT] =
            Default::default();
        for f in Faction::ALL {
            reads[f.index()] = EndStateRead {
                territory_held: self
                    .sim
                    .territory
                    .points
                    .iter()
                    .filter(|cp| cp.owner == f)
                    .count() as u32,
                // The per-faction banked purse (economy `amounts` is `[i64; FACTION_COUNT]`) — no
                // float money (invariant #1).
                resources_total: self.sim.resources.get(f),
            };
        }
        let outcome = self.match_outcome().unwrap_or(MatchOutcome::Draw);
        session_shell::assemble_summary(
            &self.match_events,
            self.sim.tick_count(),
            outcome,
            &reads,
        )
    }

    /// Set the active device-class render quality tier (Phase 4 WS-C; the Settings "graphics tiers"
    /// surface, surface 3, drives this). RENDER-only — re-clamps the running dyn-res scale into the
    /// new tier's band and never touches the sim (invariant #1/#4).
    pub fn set_tier(&mut self, tier: gonedark_render::tiers::QualityTier) {
        self.tuning.set_tier(tier);
    }

    /// The active render quality tier.
    pub fn tier(&self) -> gonedark_render::tiers::QualityTier {
        self.tuning.tier()
    }

    /// The current dynamic-resolution scale `(0,1]` the render target is drawn at — observation
    /// for a host that owns an intermediate scaled target. RENDER-only.
    pub fn resolution_scale(&self) -> f32 {
        self.tuning.resolution_scale()
    }

    /// The current FPS cap presentation should pace to (`None` = uncapped), driven by thermal
    /// backoff. The SIM still ticks at 60 Hz regardless (invariant #1/#4) — this only throttles how
    /// often the host presents.
    pub fn fps_cap(&self) -> Option<u32> {
        self.tuning.fps_cap()
    }

    /// Report the platform thermal state, read by the host from its `pal::ThermalSensor` (invariant
    /// #2: the platform signal crosses the PAL seam, never `core`). Consulted by the render-cost
    /// backoff on the next [`Game::frame`]. Storing it is presentation-only; it never reaches the sim.
    pub fn set_thermal_state(&mut self, thermal: gonedark_pal::ThermalState) {
        self.thermal = thermal;
    }

    /// Feed one measured network round-trip-time sample (seconds) into the adaptive-input-delay
    /// estimator (Phase 3 WS-B). The host calls this with a transport-level RTT measurement; the
    /// estimator smooths it (`f64` EWMA, host-side — never `core`) and, on a networked session,
    /// `frame` may turn a sustained shift into a `Lockstep::propose_delay`. A no-op until a real
    /// sample arrives, so an unmeasured session never changes its delay.
    ///
    /// **Sample source (the one stubbed seam):** production RTT comes from a transport-level
    /// ping/pong measured in `pal-desktop`, NOT from a new `core::lockstep` wire frame (adding one
    /// is out of WS-B scope — it touches the protocol). Until that ping/pong exists this method is
    /// simply never called, leaving the estimator inert. See `net_tuning`'s module docs.
    pub fn observe_rtt(&mut self, rtt_secs: f64) {
        self.rtt_estimator.observe_rtt(rtt_secs);
    }

    /// The estimator's current smoothed RTT (seconds), or `None` if no sample has been observed —
    /// a read-only host/test window into the adaptive-delay state.
    pub fn smoothed_rtt_secs(&self) -> Option<f64> {
        self.rtt_estimator.smoothed_rtt_secs()
    }

    /// Opt the enemy commander into (or out of) the **gone-dark hunt** (D). When enabled, on each
    /// commander tick the host derives the Enemy's detection tells (range + LoS bounded, honest) and
    /// lets the commander chase a player who has gone dark. Default OFF — keeping it off reproduces
    /// the original commander byte-for-byte (no checksum churn). A pure host-side knob; never sim
    /// state, so flipping it perturbs only future planning, not the running checksum stream.
    pub fn set_commander_hunts_embodied(&mut self, hunt: bool) {
        self.commander_config.hunt_embodied = hunt;
    }

    /// The player's authoritative world position, read straight from the sim world (read
    /// only — the host never mutates sim state outside `Sim::step`). The snapshot carries no
    /// entity identity, so we read by index for the embodied camera.
    fn player_pos(&self) -> Vec2 {
        self.sim.world.pos[self.player.index as usize]
    }

    /// Every living Player-faction unit (not buildings) as `(handle, world_xy)` — the candidate
    /// set the command-layer [`Selection`] (worker 4) tests the pointer against. Read-only over
    /// the sim world; positions cross the float boundary via [`fixed_to_f32`] for the UI math.
    fn selectable_player_units(&self) -> Vec<(Entity, (f32, f32))> {
        let w = &self.sim.world;
        let mut out = Vec::new();
        for i in 0..w.capacity() {
            if !w.is_index_alive(i)
                || w.faction[i] != Faction::Player
                || w.kind[i] != EntityKind::Unit
            {
                continue;
            }
            if let Some(e) = w.entity(i) {
                let p = w.pos[i];
                out.push((e, (fixed_to_f32(p.x), fixed_to_f32(p.y))));
            }
        }
        out
    }

    /// Whether the command-view world point `target` lands on a living **non-Player** unit — the
    /// hit-test that turns a right-click into an *attack* rather than a *move* (D42). Read-only over
    /// the sim world; a presentation derivation (the resulting `AttackMove` carries a Fixed-quantized
    /// point, so no float reaches the sim — invariant #1). The pick radius is generous enough that a
    /// click *near* an enemy reads as "attack that one" (units render at half-extent ~0.5).
    fn enemy_unit_at(&self, target: (f32, f32)) -> bool {
        const ENEMY_PICK_RADIUS: f32 = 1.25;
        let w = &self.sim.world;
        for i in 0..w.capacity() {
            if !w.is_index_alive(i)
                || w.faction[i] == Faction::Player
                || w.kind[i] != EntityKind::Unit
            {
                continue;
            }
            let p = w.pos[i];
            let (dx, dy) = (fixed_to_f32(p.x) - target.0, fixed_to_f32(p.y) - target.1);
            if dx * dx + dy * dy <= ENEMY_PICK_RADIUS * ENEMY_PICK_RADIUS {
                return true;
            }
        }
        false
    }

    /// The radial command menu open this frame: the action labels a held long-press is offering for
    /// the current selection, or empty when no menu is open. Presentation intent only — reading it
    /// never mutates the sim, and a preview emits no `Command`s. A host's on-screen radial renderer
    /// reads this to draw the wedges; it is recomputed every frame from input + selection.
    pub fn radial_menu(&self) -> &[&'static str] {
        &self.radial_menu
    }

    /// The sim's current tick count — a read-only window onto the deterministic clock so a
    /// host can surface sim progress (e.g. the on-device heartbeat) without reaching into
    /// private sim state. Observation only: never mutates the sim, no determinism impact.
    pub fn tick_count(&self) -> u64 {
        self.sim.tick_count()
    }

    /// Read-only: how many living `Unit`-kind entities of `faction` there are right now. A
    /// presentation/test query over the sim world — it mutates nothing and never enters the
    /// checksum, so it has no determinism impact. Used by the offscreen viz harness to assert that
    /// embodied fire actually *kills* (TF-1) without leaning on fragile screen-pixel counts.
    pub fn alive_unit_count(&self, faction: Faction) -> usize {
        alive_units_of(&self.sim, faction)
    }

    /// The sim's current per-tick checksum — a read-only window onto deterministic state so a
    /// host can eyeball lockstep determinism on-device (the heartbeat logs it alongside the
    /// frame rate). Observation only: never mutates the sim, no determinism impact.
    pub fn checksum(&self) -> u64 {
        self.sim.checksum()
    }

    /// Embodied perspective view-projection for the active player — thin wrapper over the free
    /// [`embodied_view_proj`]. The eye is the **predicted** avatar position (D15, smooth + led)
    /// once prediction is anchored; before the first embodied frame anchors it, falls back to the
    /// raw authoritative position so the very first frame is never off at the origin.
    fn embodied_view_proj(&self, width: u32, height: u32) -> Mat4 {
        let (px, py) = if self.avatar.valid {
            self.avatar.eye
        } else {
            let p = self.player_pos();
            (fixed_to_f32(p.x), fixed_to_f32(p.y))
        };
        embodied_view_proj(px, py, self.yaw, self.pitch, width, height)
    }

    /// Command-view orthographic view-projection at the current pan focus + zoom — thin wrapper
    /// over the free [`topdown_view_proj`] threading `self.cam_focus_*` / `self.cam_half_extent`.
    /// Single source of the command matrix so picking (`unproject_topdown`) and rendering always
    /// agree on the framing.
    fn command_view_proj(&self, width: u32, height: u32) -> Mat4 {
        topdown_view_proj(
            width,
            height,
            self.cam_focus_x,
            self.cam_focus_y,
            self.cam_half_extent,
        )
    }

    /// Advance and present one frame: map this frame's `input` → sim commands, drain the
    /// fixed-tick accumulator by `dt_secs`, build the camera, and render the interpolated
    /// snapshot into `view`. `viewport` is the surface size in pixels. The host owns acquiring
    /// `view` and presenting afterward; this method never touches the platform surface.
    ///
    /// All host-float work; the only thing crossing into the sim is the Fixed-quantized
    /// command set (invariant #1).
    #[allow(clippy::too_many_arguments)]
    pub fn frame(
        &mut self,
        input: &InputFrame,
        dt_secs: f32,
        viewport: (u32, u32),
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        view: &wgpu::TextureView,
        audio: &mut dyn Audio,
    ) {
        let (width, height) = viewport;
        // A local, mutable copy of this frame's input so the embody picker (below) can CONSUME the
        // tap / number-key / embody edges it reacts to — clearing them here so the selection + order
        // layers downstream never also handle a tap the player aimed at the picker.
        let mut input = input.clone();

        // 0. Render quality tuning (Phase 4 WS-C): observe this frame's wall-clock `dt` + the
        // host-reported thermal state and ease the dynamic-resolution scale / FPS cap to hold the
        // frame budget. PURELY a rendering decision (invariant #1/#4) — it reads frame timing and a
        // PAL-reported thermal signal (invariant #2), touches only `self.tuning`, and never the sim,
        // so the per-tick checksum stream below is byte-identical at every tier. The budget paces to
        // the thermal FPS cap when one is active, else the 60 Hz baseline.
        let budget_secs = match self.tuning.fps_cap() {
            Some(cap) if cap > 0 => 1.0 / cap as f32,
            _ => 1.0 / TICK_HZ as f32,
        };
        self.tuning
            .observe_frame(dt_secs, self.thermal, budget_secs);

        // 0b. Command-camera pan + zoom (presentation only — never touches the sim). While in the
        // command view the WASD/stick `move_axis` pans the ground focus and the wheel `scroll` zooms
        // the framing; both feed the SAME `topdown_view_proj` used for picking below, so selection,
        // the pick radius (zoom-aware via `gesture_scale`), and what's drawn stay consistent. Gated
        // to !embodied so `move_axis` drives the avatar (not the camera) while possessed.
        if !self.embodied {
            let (fx, fy) = pan_focus(
                self.cam_focus_x,
                self.cam_focus_y,
                input.move_axis,
                self.cam_half_extent,
                dt_secs,
            );
            self.cam_focus_x = fx;
            self.cam_focus_y = fy;
            self.cam_half_extent = zoom_half_extent(self.cam_half_extent, input.scroll);
        }

        // 0c. Embody picker (command view): with two or more units selected, pressing embody opens a
        // small on-screen list so the player chooses WHICH to possess (e.g. the tank in a mixed
        // troops+tanks band) instead of the engine silently taking the first. Resolved BEFORE
        // selection below, and it CONSUMES the tap / number-key / embody edges it uses (clearing them
        // on the local `input`) so the same tap never also drives selection or the order vocabulary.
        // Command-view only — being embodied force-closes it. Pure host UX over `Command::Embody`;
        // the sim and embodiment semantics (invariant #5) are unchanged — only *which* entity is
        // chosen differs.
        let mut picker_embody: Option<Entity> = None;
        if self.embodied {
            self.embody_picker = None;
        } else {
            // Drop dead handles so a stale pick never targets a corpse; if fewer than two possessable
            // units remain there is nothing to choose between, so just close it (the player presses
            // embody again to possess the lone survivor directly).
            if let Some(rows) = self.embody_picker.as_mut() {
                rows.retain(|&e| is_live_player_unit(&self.sim.world, e));
                if rows.len() < 2 {
                    self.embody_picker = None;
                }
            }
            if let Some(rows) = self.embody_picker.clone() {
                // Open: this frame's number key / tap picks a row, or cancels. `width.max(1)` mirrors
                // `unproject_topdown` so a zero-size viewport can't divide by zero.
                let tap_row = if input.pointer_up {
                    input.pointer.and_then(|(px, py)| {
                        let nx = px / width.max(1) as f32 * 2.0 - 1.0;
                        let ny = 1.0 - py / height.max(1) as f32 * 2.0;
                        gonedark_render::picker::picker_row_at(rows.len(), nx, ny)
                    })
                } else {
                    None
                };
                match embody_pick_outcome(
                    &rows,
                    input.command_slot,
                    tap_row,
                    input.pointer_up,
                    input.embody_pressed,
                    input.surface_pressed,
                ) {
                    PickOutcome::Pick(e) => {
                        picker_embody = Some(e);
                        self.embody_picker = None;
                    }
                    PickOutcome::Cancel => self.embody_picker = None,
                    PickOutcome::Stay => {}
                }
                // Consume the inputs the picker reacted to so the selection / order layer below does
                // not also handle this tap or key.
                input.pointer = None;
                input.pointer_up = false;
                input.command_slot = None;
                input.embody_pressed = false;
                input.surface_pressed = false;
            } else if input.embody_pressed {
                // Closed + embody pressed: open the picker for a multi-unit selection; otherwise fall
                // through to the existing direct-embody path (0/1 selected → first / avatar / any).
                let rows = live_selected_player_units(&self.selection, &self.sim.world);
                if rows.len() >= 2 {
                    self.embody_picker = Some(rows);
                    input.embody_pressed = false; // the picker — not this press — will embody
                }
            }
        }

        // 1. Map input → sim commands (applied on the first step of this frame). The pure
        // mapping (tap-to-move + state-resolved embody/surface toggle) lives in the free
        // `map_input_commands`; here we apply the resulting embodiment state transition.
        // Resolve WHICH unit an embody press would possess this frame — the first live selected
        // unit, else the current avatar, else any live player unit (see `embody_target`). Computed
        // only on the embody edge; the selection read is last frame's highlight (you select, then
        // press E). `None` ⇒ no live unit, so the press is dropped rather than possessing a corpse.
        // (When the picker is open it has already cleared `embody_pressed`, so this stays `None`.)
        let target = if input.embody_pressed && !self.embodied {
            embody_target(&self.selection, &self.sim.world, self.player)
        } else {
            None
        };
        let mut commands = map_input_commands(&input, self.embodied, self.player, target);
        if let Some(e) = picker_embody {
            commands.push(Command::Embody { entity: e });
        }

        // 1b. Touch-UI layer (workers 4 + 5): in the command view, the pointer drives unit
        // SELECTION and the on-screen vocabulary issues orders to that selection. Both are pure
        // presentation→intent layers; the float target is quantized to Fixed at the boundary
        // inside `command_ui` (invariant #1). With nothing selected they emit nothing and the
        // legacy single-avatar tap-to-move above still applies (back-compat).
        // Zoom context for gesture thresholds: world units spanned by one screen pixel at the
        // command-view center. Derived from the same top-down unprojection the pointer uses, so a
        // fixed-pixel finger jitter reads as a tap (and the pick radius stays a usable hit target)
        // regardless of camera zoom. Float geometry at the input boundary — never enters the sim.
        let (pointer_world, gesture_scale) = if !self.embodied {
            let vp = self.command_view_proj(width, height);
            let pw = input
                .pointer
                .and_then(|(px, py)| unproject_topdown(&vp, px, py, width, height));
            let cx = width as f32 / 2.0;
            let cy = height as f32 / 2.0;
            let scale = match (
                unproject_topdown(&vp, cx, cy, width, height),
                unproject_topdown(&vp, cx + 1.0, cy, width, height),
            ) {
                (Some((x0, y0)), Some((x1, y1))) => {
                    GestureScale::new(((x1 - x0).powi(2) + (y1 - y0).powi(2)).sqrt())
                }
                _ => GestureScale::world_floor(),
            };
            (pw, scale)
        } else {
            (None, GestureScale::world_floor())
        };
        let candidates = self.selectable_player_units();
        // `additive` (grow the selection instead of replacing it) has no PAL modifier plumbed yet,
        // so it is `false` today — the legacy clear-then-select feel is preserved bit-for-bit while
        // the zoom-stable thresholds take effect via `gesture_scale`.
        let gesture = self.selection.update_ex(
            pointer_world,
            input.pointer_down,
            input.pointer_up,
            self.embodied,
            false,
            input.command_tap,
            gesture_scale,
            &candidates,
        );
        // Resolve the selection to live `(handle, world_pos)` pairs for the vocabulary layer
        // (Patrol anchors a leg at each unit's current position). Read-only over the sim world;
        // skip the work entirely when nothing is selected.
        let selected: Vec<(Entity, (f32, f32))> = if self.selection.is_empty() {
            Vec::new()
        } else {
            self.selection
                .units
                .iter()
                .filter(|&&e| self.sim.world.is_alive(e))
                .map(|&e| {
                    let p = self.sim.world.pos[e.index as usize];
                    (e, (fixed_to_f32(p.x), fixed_to_f32(p.y)))
                })
                .collect()
        };
        // Long-press opens a radial menu over the vocabulary; a slot picked while it is held
        // commits. This gate (invariant #3: depth in the vocabulary, never unit autonomy) leaves
        // the direct quick-slot path — a slot tapped without a long-press — to `commands_for`, so
        // the sim-visible commands are byte-identical to before in every case:
        //   - no long-press            → RadialIntent::None  → direct quick-slot path runs as before
        //   - long-press + a slot      → RadialIntent::Commit → same Commands as the quick-slot path
        //   - long-press, no slot yet  → RadialIntent::Preview → menu captured, NO Commands emitted
        match command_ui::radial_intent(
            input.command_slot,
            input.long_press,
            &selected,
            pointer_world,
        ) {
            command_ui::RadialIntent::Commit(cmds) => {
                self.radial_menu.clear();
                commands.extend(cmds);
            }
            command_ui::RadialIntent::Preview(menu) => {
                self.radial_menu = menu;
            }
            command_ui::RadialIntent::None => {
                self.radial_menu.clear();
                commands.extend(command_ui::commands_for(
                    input.command_slot,
                    input.long_press,
                    &selected,
                    pointer_world,
                ));
            }
        }

        // Right-click "command here" (classic-RTS scheme, D42): the primary, no-modifier order to
        // the current selection — Move onto empty ground, AttackMove onto an enemy. Command view
        // only; ignored while embodied (right-click has no command-layer meaning in first person).
        if input.command_click && !self.embodied {
            let on_enemy = pointer_world.is_some_and(|t| self.enemy_unit_at(t));
            commands.extend(command_ui::command_click_commands(
                &selected,
                pointer_world,
                on_enemy,
            ));
        }

        // Single-pointer "tap commands" (the touch scheme, D43): on a one-button screen there is no
        // right-click, so a TAP that lands OFF any friendly unit — while a selection is active —
        // issues the same default order to the kept selection (the selection layer left it intact
        // in `tap_commands` mode). A tap ON a unit selected it instead (outcome carries the hit), so
        // it never doubles as a command. Mirrors the D42 emission exactly (Move / AttackMove).
        if input.command_tap && !self.embodied {
            if let selection::GestureOutcome::Tapped { hit: None, at } = gesture {
                if !selected.is_empty() {
                    let on_enemy = self.enemy_unit_at(at);
                    commands.extend(command_ui::command_click_commands(
                        &selected,
                        Some(at),
                        on_enemy,
                    ));
                }
            }
        }

        // 1b'. Command-view PRODUCTION intents (Phase 2 "command and grow your camps"): the build /
        // train / upgrade keys (desktop B/R/H/U; touch on-screen buttons are the deferred PAL slice).
        // Command view only — never while embodied (invariant #6: no command-layer production with the
        // map dark). The two inputs are resolved here (the unprojected cursor `pointer_world`, and the
        // deterministic `active_player_camp` over the pre-step world) and handed to the pure, tested
        // `command_view_production_commands`; the emitted Commands enter the SAME lockstep stream as
        // taps, so a placement/queue/upgrade applies bit-identically on every peer (invariant #1/#7).
        if !self.embodied {
            let active_camp = active_player_camp(&self.sim.world, Faction::Player);
            commands.extend(command_view_production_commands(
                &input,
                pointer_world,
                active_camp,
            ));
        }

        // Embodiment input-source swap (invariant #5): mirror the toggle the mapping resolved.
        for cmd in &commands {
            match cmd {
                Command::Embody { entity } => {
                    // Follow the possessed entity: the avatar may be a freshly-selected unit (not
                    // the original spawn), so locomotion/fire/fog/camera all re-point at it here.
                    self.player = *entity;
                    self.embodied = true;
                    self.camera = CameraMode::Embodied;
                    // Fresh possession → forget any stale finger ownership from the command view so
                    // the move stick / look region re-capture cleanly on the next touch.
                    self.touch.reset();
                    log::info!("[tick {}] EMBODY — world goes dark", self.sim.tick_count());
                }
                Command::Surface { .. } => {
                    self.embodied = false;
                    self.camera = CameraMode::TopDown;
                    self.touch.reset();
                    log::info!("[tick {}] SURFACE — back to command", self.sim.tick_count());
                }
                _ => {}
            }
        }

        // Embodied control intents (invariant #2/#5): a SINGLE platform-agnostic set of intents
        // feeds the look integration + the Fire/Locomote/Crouch/Reload/Surface emission below,
        // sourced from EITHER the Android on-screen FPS HUD (the pure `touch_controls` seam over
        // `input.touches`) or the desktop keyboard/mouse `InputFrame` fields. The touch seam runs
        // only while embodied and only when fingers are down; otherwise the on-screen HUD is cleared
        // (the GUI is Android-only, and never drawn in the command view).
        let (look_axis, move_axis, fire, crouch_edge, reload_edge, surface_edge) =
            if self.embodied && input.touch_count > 0 {
                let layout = touch_controls::TouchLayout::new(width, height);
                let n = (input.touch_count as usize).min(input.touches.len());
                let out = self.touch.update(&layout, &input.touches[..n]);
                self.touch_hud = Some(out.hud);
                (
                    out.look_delta,
                    out.move_axis,
                    out.fire,
                    out.crouch_edge,
                    out.reload_edge,
                    out.surface_edge,
                )
            } else {
                self.touch_hud = None;
                (
                    input.look_axis,
                    input.move_axis,
                    input.fire,
                    input.crouch_pressed,
                    input.reload_pressed,
                    false, // desktop ejects via the Q-key surface path in `map_input_commands`
                )
            };

        // Integrate look into presentation-only yaw + pitch (D15: never into the sim). Both
        // subtract the delta so the view is non-inverted (mouse/drag right → look right, up → look
        // up); pitch is clamped shy of vertical (see `integrate_look_*`).
        self.yaw = integrate_look_yaw(self.yaw, look_axis.0);
        self.pitch = integrate_look_pitch(self.pitch, look_axis.1);

        if self.embodied {
            // The whole embodied input→command pipeline lives in the pure `embodied_input_commands`
            // seam (GPU-free, host-tested end to end): trigger→Fire (aim quantized at the boundary),
            // stick→Locomote, crouch toggle, reload, surface — same lockstep stream as taps, the
            // cone-hitscan / move resolved sim-side bit-identically on every peer. `frame` resolves
            // only the authoritative sim reads (alive + posture) the seam can't, then applies the two
            // presentation side effects it returns (the seam holds no `Game`).
            let player_alive = self.sim.world.is_alive(self.player);
            let crouched = player_alive
                && self.sim.world.posture[self.player.index as usize] == Posture::Crouched;
            let out = embodied_input_commands(
                self.player,
                self.yaw,
                move_axis,
                fire,
                crouch_edge,
                reload_edge,
                surface_edge,
                crouched,
                player_alive,
            );
            commands.extend(out.commands);
            if out.fired {
                // Stamp the muzzle-flash cue (W5, presentation only): the weapon viewmodel flares
                // for a few ticks after this shot. Never read by the sim — it rides the host clock
                // alongside the authoritative `Command::Fire`, not in place of it (invariant #4/#6).
                self.last_fire_tick = Some(self.sim.tick_count());
            }
            if out.surfaced {
                // The transition loop already ran THIS frame, so flip the camera state here directly
                // (mirroring it) now that the on-screen Surface button emitted its eject.
                self.embodied = false;
                self.camera = CameraMode::TopDown;
                self.touch.reset();
            }
        }

        // 1c. Enemy commander (W3). On a once-per-second gate (`tick % COMMANDER_PERIOD == 0`) the
        // scripted commander surveys the (checksummed) world and emits ORDERS for its faction —
        // capture points, press the player line, reinforce — using its OWN seeded RNG, never
        // `sim.rng()` (that stream is checksummed; a host draw would desync, invariant #7). Its
        // commands are pushed into THIS frame's `commands` Vec, so they enter the same lockstep
        // stream as player taps and are applied bit-identically on every peer. Units stay literal
        // executors (invariant #3); the commander only *chooses* their orders. Gating on
        // `tick_count()` (the next tick to step) keeps the cadence a pure function of sim state, so
        // it is identical across peers regardless of frame pacing.
        if self.sim.tick_count().is_multiple_of(COMMANDER_PERIOD) {
            // Gone-dark hunt (config-gated, default OFF): when enabled, derive the commander's
            // permitted intel from the SAME detection channel the player's HUD uses — for the Enemy
            // as observer, so it learns only what range + LoS honestly reveal (invariant #6, no
            // omniscient peek). Off → no detection call at all and an empty slice, so the emitted
            // command stream is byte-identical to before (no checksum churn).
            let tells = if self.commander_config.hunt_embodied {
                detection::detectable_embodiment(
                    &self.sim.world,
                    &self.sim.terrain,
                    &DetectionConfig::default(),
                    Faction::Enemy,
                    self.sim.tick_count(),
                    &mut self.commander_detection,
                )
            } else {
                Vec::new()
            };
            let cmds = commander::commander_orders(
                &self.sim.world,
                &self.sim.territory,
                &self.sim.resources,
                &mut self.commander_rng,
                &self.commander_config,
                &tells,
                Faction::Enemy,
                self.sim.tick_count(),
            );
            commands.extend(cmds);
        }

        // 2. Fixed-tick accumulator → lockstep drive. The deterministic sim advances in whole
        // ticks, but each tick is now driven through `core::lockstep` (D27 step 4) instead of
        // stepped directly: this frame's commands are submitted onto the FIRST advancing tick and
        // catch-up ticks submit none — exactly as before, but the sim steps from the merged set
        // `try_advance()` returns, so the path is multiplayer-ready. The accumulator (clamped so a
        // huge first-frame / stall dt can't spiral) only decides HOW MANY ticks to advance.
        let tick_dt = 1.0 / TICK_HZ as f32;
        self.acc += dt_secs;

        // This frame's emitted sim events, accumulated across however many ticks stepped (each
        // `Sim::step` clears its own stream). Drives the alert channel + the embodied audio mix
        // below — both pure presentation derivations, neither touches sim state (invariant #7).
        let mut frame_events: Vec<SimEvent> = Vec::new();

        // THE pause rule (WS-B): in SINGLE-PLAYER a pause halts the local tick — we hold the
        // accumulator (don't grow it) and advance zero ticks, so the sim stops and resumes bit-
        // identically (pause mutates no sim state). In LOCKSTEP a local pause is overlay-only and
        // `halts_local_tick` is false, so the sim keeps stepping from the shared gate (the protocol
        // has no peer-agreed pause). `halts_local_tick` is the single point that encodes this.
        if self.shell.halts_local_tick() {
            self.acc = 0.0;
            commands.clear();
        }

        // Drain the accumulator into a whole-tick budget (clamped). Each whole tick consumes
        // exactly `tick_dt`; the excess past the clamp is dropped so the sim can't spiral.
        let mut budget = 0u32;
        while self.acc >= tick_dt && budget < MAX_CATCHUP_STEPS {
            self.acc -= tick_dt;
            budget += 1;
        }
        if budget == MAX_CATCHUP_STEPS && self.acc >= tick_dt {
            self.acc = 0.0;
        }
        // Sub-tick frame: if no whole tick elapsed this frame (render faster than TICK_HZ) but a
        // ONE-SHOT/edge intent fired (tap / embody / build / order — each lives for exactly one
        // drained input frame), advance ONE tick anyway so it is not dropped. (At delay 0 a
        // submitted-but-not-advanced tick would strand the input.)
        //
        // CRUCIALLY this must NOT bump for HELD/continuous commands (locomote, fire), which are
        // re-emitted every frame. Bumping on those forced a tick on every render frame, so the sim
        // advanced at the *render* rate while a key was held — movement/fire scaled with FPS (a
        // 2x/4x avatar overspeed at 120/240 Hz). A held command dropped on a sub-tick frame is
        // harmless: next frame re-emits it, and it applies on the next whole tick at the true 60 Hz.
        if budget == 0 && commands.iter().any(is_oneshot_command) {
            budget = 1;
        }

        // Drive the lockstep loop for `budget` ticks. The per-tick `step` closure preserves the
        // prev→curr snapshot, the event accumulation, and the sim advance the old accumulator did.
        let prev = &mut self.prev;
        let curr = &mut self.curr;
        let events = &mut frame_events;
        let advanced = drive_lockstep(
            &mut self.sim,
            &mut self.lockstep,
            self.transport.as_deref_mut(),
            commands,
            budget,
            |sim, merged| {
                *prev = curr.clone();
                sim.step(merged);
                events.extend_from_slice(sim.events());
                *curr = sim.snapshot();
            },
        );
        // The lockstep gate stalled this frame iff we couldn't advance the whole budget — a ready
        // tick's per-peer input wasn't in hand (the seam's `stalled` observation; single-player at
        // delay 0 never stalls, so this is always false there). Feeds the reconnect prompt below.
        let lockstep_stalled = advanced < budget;

        // Adaptive input delay (WS-B): on a NETWORKED session, fold the latest RTT into the
        // estimator's decision and, when its hysteresis gate fires, ask lockstep to propose the new
        // integer delay. The float EWMA lives in the estimator (engine glue); lockstep receives only
        // the integer target + guard, so `core` stays float-free and platform-free (invariants
        // #1/#2). The agreed change commits at a shipped effective tick identically on every peer.
        // Single-player (transport `None`) never proposes — there is no peer and delay stays 0.
        if self.transport.is_some() {
            // The lockstep frontier drives the dwell clock (sim ticks, not wall-clock).
            let now_tick = self.lockstep.submit_tick().max(self.lockstep.next_tick());
            if let Some(target) = self
                .rtt_estimator
                .poll_decision(self.lockstep.delay(), now_tick)
            {
                let guard = self.rtt_estimator.guard_ticks();
                // `AlreadyPending` just means a prior agreed change is still in flight; the next
                // poll (after the dwell) retries, so the error is safely dropped here.
                let _ = self.lockstep.propose_delay(target, guard);
            }
        }

        // Auto-surface on avatar death (invariant #5): if the possessed unit died this frame the
        // sim despawned it, so it is gone from the freshly-stepped snapshot (`self.curr`, refreshed
        // inside `drive_lockstep`'s step closure). Eject back to command rather than stranding the
        // first-person camera staring at a corpse — mirroring the manual Surface path's local state
        // flip EXACTLY (embodied off + camera TopDown; the fog reverts to `command_visibility`
        // automatically in the visibility step below, which keys off `self.embodied`). This is
        // host UI/camera state only: the entity is already gone, so we emit NO `Command::Surface`
        // (it would be a sim no-op and must never be double-emitted). Liveness is read against the
        // same snapshot the avatar prediction below probes.
        let avatar_present = self
            .curr
            .units
            .iter()
            .any(|u| u.entity_index == self.player.index);
        if should_auto_surface(self.embodied, avatar_present) {
            self.embodied = false;
            self.camera = CameraMode::TopDown;
            log::info!(
                "[tick {}] AUTO-SURFACE — embodied avatar died, back to command",
                self.sim.tick_count()
            );
        }

        // Interpolation factor for this frame (invariant #4): how far into the next tick the
        // render clock sits. Drives both the avatar-prediction lead just below and the renderer.
        let alpha = (self.acc / tick_dt).clamp(0.0, 1.0);

        // Avatar-local prediction (D15): lead + reconcile the embodied eye from the authoritative
        // snapshot. PRESENTATION ONLY — reads `curr` by shared ref, mutates only `self.avatar`,
        // never the sim. While embodied, find the avatar in the latest snapshot and update it;
        // when not embodied, drop the prediction so the next embody re-anchors cleanly.
        if self.embodied {
            if let Some(u) = self
                .curr
                .units
                .iter()
                .find(|u| u.entity_index == self.player.index)
            {
                let pos = (fixed_to_f32(u.pos.x), fixed_to_f32(u.pos.y));
                let vel = (fixed_to_f32(u.vel.x), fixed_to_f32(u.vel.y));
                // Lead by this frame's sub-tick fraction. Multiplayer adds the input-delay lead
                // (`delay * tick_dt`) once a 2-peer session runs delay > 0; the single-player
                // delay-0 session leads only by the sub-tick, which simply smooths the 60 Hz eye.
                self.avatar.update(pos, vel, alpha * tick_dt);
            }
        } else {
            self.avatar.clear();
        }

        // Fold this frame's events into the embodied thread-back: the alert channel (worker 2's
        // HUD) and the positioned audio mix (worker 3). "Alerts, not intel" — observed as the
        // local Player faction (invariant #6). Both read-only over the sim.
        let tick = self.sim.tick_count();
        self.alerts
            .ingest(&frame_events, &self.sim.world, Faction::Player, tick);

        // WS-4 — local hit feedback. The "I hit him" signal the game never sent: if the embodied
        // avatar's OWN shot dealt damage this frame (the pure `avatar_landed_hit` seam over the
        // deterministic event stream), stamp the hitmarker clock and fire a one-shot hit SFX. This
        // is presentation feedback on the player's own action — keyed STRICTLY on the avatar as the
        // damage `source`, never on intel about an unseen enemy — so it is invariant-#6-safe; it
        // folds nothing into the sim (the events are already-checksummed copies, invariant #1/#7).
        if avatar_landed_hit(&frame_events, self.player, self.embodied) {
            self.last_hit_tick = Some(tick);
            audio.play_oneshot(SoundId::HitConfirm as u32);
        }
        // Accumulate this frame's events over the match so the post-match summary assembler can
        // tally produced/lost/killed (a presentation derivation; the events are already-checksummed
        // state copied out — never re-folded, invariant #7).
        self.match_events.extend_from_slice(&frame_events);

        // Evaluate the win/lose condition from the (already-checksummed) end-state this frame and,
        // once it is decided, end the match into the post-match summary surface. `match_outcome` is
        // a pure read — derives each combatant's forces and runs the unit-tested `evaluate_outcome`
        // (elimination, then a territory/resource timeout tiebreak); it folds nothing and so cannot
        // desync (invariants #1/#7). `end_match` is idempotent, so the first decided outcome sticks
        // (a later tick can't overwrite the summary), and it is skipped once any overlay has already
        // ended the match (e.g. a surrender).
        if !self.shell.is_ended() && self.match_outcome().is_some() {
            let summary = self.assemble_summary();
            self.shell.end_match(summary);
        }

        // Surface the reconnect prompt when the lockstep link is unhealthy (D28 reconnect path).
        // Pure: a `core::shell::ConnectionStatus` projection of the lockstep state + the WS-B
        // `should_prompt_reconnect` predicate; no I/O, no sim mutation. Single-player (one peer,
        // null transport) never stalls or desyncs, so this only fires in a real multiplayer session.
        //
        // We DRAIN any confirmed cross-client desync each frame (invariant #7): an undrained desync
        // queue would let the most-severe link signal go unseen and accumulate unchecked. A drained
        // desync dominates a stall in the projection (the more severe signal wins → the warning-
        // accented prompt). We surface the prompt over ANY non-ended overlay — a lockstep pause is
        // a local-only overlay while the shared clock keeps ticking, so a stall/desync while the
        // pause menu is open must still reach the player (`request_reconnect` already transitions
        // Paused → ReconnectPrompt; it only refuses an ended match).
        if !self.shell.is_ended() {
            let recent_desync = self.lockstep.take_desyncs().into_iter().next();
            let status: ConnectionStatus =
                ConnectionStatus::project(&self.lockstep, lockstep_stalled, recent_desync);
            if session_shell::should_prompt_reconnect(&status) {
                self.shell.request_reconnect(status.state);
            }
        }
        // The listener follows the PREDICTED eye while embodied (so the positioned mix lines up
        // with the first-person camera), else the raw authoritative position.
        let listener = if self.embodied && self.avatar.valid {
            self.avatar.eye
        } else {
            let p = self.player_pos();
            (fixed_to_f32(p.x), fixed_to_f32(p.y))
        };
        let cues = audio::mix_cues(
            &frame_events,
            self.embodied,
            listener,
            self.yaw,
            &self.sim.world,
        );
        audio.submit_mix(&cues);

        // 4. Build the camera for the active view (alpha computed above for the avatar lead).
        let view_proj = match self.camera {
            CameraMode::TopDown => self.command_view_proj(width, height),
            CameraMode::Embodied => self.embodied_view_proj(width, height),
        };
        let camera = Camera {
            view_proj: view_proj.to_cols_array_2d(),
        };

        // 5. Compute the visibility mask for the active viewpoint (worker 1 applies it in
        // render). Embodied → only the avatar's sight (the map goes dark); command view → the
        // Player faction's union vision. A pure derivation over the world — never sim state.
        let visibility: Visibility = if self.embodied {
            fog::embodied_visibility(&self.sim.world, &self.sim.terrain, self.player)
        } else {
            fog::command_visibility(&self.sim.world, &self.sim.terrain, Faction::Player)
        };

        // 6. Interpolate prev→curr into render instances (the float boundary lives in render).
        // The command-layer selection (presentation only) is handed in as world indices so the
        // renderer rims the selected units. It is a *command-view* affordance, so we pass none
        // while embodied — the selection set may linger across an embody, but its rims have no
        // place in the first-person view.
        let selected_indices: Vec<u32> = if self.embodied {
            Vec::new()
        } else {
            self.selection.units.iter().map(|e| e.index).collect()
        };
        self.renderer
            .prepare(&self.prev, &self.curr, alpha, &selected_indices);

        // 6b. Embodied first-person WORLD (W5): paint a real ground/sky UNDER the avatar so "world
        // goes dark" means losing INTEL, not staring at a black void (invariant #6). This is the
        // CLEARING pass of the embodied frame; the unit pass below then LOADs the avatar over it.
        // The world is a pure function of the *camera* (inverse view-proj + eye) — it has no access
        // to sim entities, so it cannot reveal enemy units/buildings/control points; the fog filter
        // stays the fairness boundary. Skipped entirely in command view (it never clears that path).
        if self.embodied {
            // Eye = the predicted listener position (x,y) raised to EYE_HEIGHT — the same eye the
            // embodied camera uses. The host owns glam, so the matrix inverse is computed HERE (the
            // render crate stays glam-free, D19) and handed in as plain arrays.
            let eye = [listener.0, listener.1, EYE_HEIGHT];
            let flash = gonedark_render::world::muzzle_flash_intensity(self.last_fire_tick, tick);
            let world_uniform = gonedark_render::world::WorldUniform::new(
                view_proj.inverse().to_cols_array_2d(),
                eye,
                flash,
            );
            self.renderer
                .render_world_sky(device, queue, view, &world_uniform);

            // 6c. First-person world meshes: the static scenery/cover props AND the dynamic sim
            // units the avatar can SEE, drawn over the sky/ground (one shared depth pass so they
            // occlude each other) and before the avatar pass so the embodied view reads as a *place*
            // with the enemies actually in it. Fairness (invariant #6): "world goes dark" strips the
            // strategic MAP, not the soldier in your line of sight — the renderer applies the
            // avatar-only `visibility` mask, so only units the avatar can physically see are drawn
            // (the avatar's own body is dropped); props are a fixed cosmetic layout and carry no
            // intel. The renderer picks a LOD tier per mesh from the eye distance; we hand in the same
            // eye + camera the sky used (matrix math stays host-side, D19 — render is glam-free).
            self.renderer.render_world_meshes(
                device,
                queue,
                view,
                &view_proj.to_cols_array_2d(),
                eye,
                &visibility,
                width,
                height,
            );
        }

        // Economy readout (the resource/income lines of the command readout): read banked credits +
        // derive income from held control points off the (checksummed) sim — a pure read, folds
        // nothing, so it can't desync (invariants #1/#7). Built ONLY in the command view (the readout
        // never draws over the dark embodied frame — invariant #6), so the embodied branch passes
        // `None` and skips the `territory.points` scan entirely. `resources.get` is `i64` (no float
        // money — invariant #1); `clamp` into the `u32` the readout displays (truncating `as` would
        // wrap a value past `u32::MAX`).
        let economy = (!self.embodied).then(|| {
            let held_points = self
                .sim
                .territory
                .points
                .iter()
                .filter(|cp| cp.owner == Faction::Player)
                .count() as u32;
            gonedark_render::readout::EconomyReadout {
                resources: self.sim.resources.get(Faction::Player).clamp(0, u32::MAX as i64) as u32,
                income_per_tick: gonedark_render::readout::income_per_tick(held_points),
            }
        });

        // 7. Render the interpolated snapshot, fog-filtered (world goes dark while embodied). In the
        // embodied branch this LOADs over the world drawn in 6b; in command view it CLEARS.
        self.renderer.render(
            device,
            queue,
            view,
            &camera,
            /* world_dark = */ self.embodied,
            &visibility,
            width,
            height,
            economy,
        );

        // 7c. Contextual command panel ("command and grow your camps"), top-right. Command view only
        // — never over the dark embodied frame (invariant #6). Its rows are derived from the current
        // SELECTION (the unit-tested `command_panel_view`): a selected camp shows its train / upgrade
        // / resources, selected troops show their composition + stance, and an empty selection shows
        // the build palette. A pure read over the (checksummed) sim that folds nothing, so it cannot
        // perturb the per-tick checksum (invariants #1/#7).
        if !self.embodied {
            let panel_view = command_panel_view(
                &self.sim.world,
                &self.selection,
                self.sim.resources.get(Faction::Player),
                &[UnitKind::Rifleman, UnitKind::Heavy],
            );
            self.renderer
                .render_command_panel(device, queue, view, &panel_view);
        }

        // 7a''. Embody picker (command view): when open, draw the list of selected units the player
        // is choosing one of to possess. Text chrome only, on top of the command panels; gated to the
        // command view so it never paints over the dark embodied frame (invariant #6).
        if !self.embodied {
            if let Some(rows) = &self.embody_picker {
                let view_desc = embody_picker_view(rows, &self.sim.world);
                self.renderer
                    .render_embody_picker(device, queue, view, &view_desc);
            }
        }

        // 7a. Embodied weapon viewmodel (W5/D44): the first-person gun — the real `weapon_rifle`
        // greybox 3D mesh — over the world + avatar, with a muzzle flash that flares + recoils for a
        // few ticks after the player fires. Anchored in view space, so the host hands in the
        // projection ALONE (the model matrix is the view-space placement). No world position →
        // reveals no intel (invariant #6). Only an *infantry* avatar shows it: the viewmodel is an
        // infantry rifle, so a possessed tank (`Heavy`) draws none rather than a rifle floating in
        // the cannon view (see `embodied_shows_rifle_viewmodel`).
        if self.embodied
            && embodied_shows_rifle_viewmodel(self.sim.world.unit_kind[self.player.index as usize])
        {
            let proj = embodied_proj(width, height).to_cols_array_2d();
            let flash = gonedark_render::world::muzzle_flash_intensity(self.last_fire_tick, tick);
            self.renderer
                .render_world_weapon(device, queue, view, &proj, flash, width, height);
        }

        // 7b. Command-view band-select marquee: while a drag is in flight, draw the selection box
        // the player is sweeping. Project the world-space drag anchor and the live pointer through
        // the active view to NDC (the top-down camera is axis-aligned, so the world band maps to an
        // axis-aligned screen rect). Presentation only — screen-space chrome, no sim mutation, and
        // gated to the command view so it never paints over the dark embodied frame (invariant #6).
        if !self.embodied {
            if let (Some((ax, ay)), Some((px, py))) = (self.selection.drag_anchor(), pointer_world)
            {
                let a = view_proj.project_point3(Vec3::new(ax, ay, 0.0));
                let b = view_proj.project_point3(Vec3::new(px, py, 0.0));
                let marquee = Marquee {
                    min: [a.x, a.y],
                    max: [b.x, b.y],
                };
                self.renderer.render_marquee(device, queue, view, &marquee);
            }
        }

        // 7d. Debug overlay (command view, F3): for tanks, armour-facet hitbox rings + hull spoke +
        // shell tracers; for infantry, the weapon range ring + firing-cone wedge; plus Player→Enemy
        // line-of-sight connectors (green clear / red blocked). Composed by the pure, host-tested
        // `debug_overlay_lines` from the curr snapshot + terrain (no interpolation — debug chrome).
        // Command-view only (invariant #6) and reuses the command view-proj the step-7 `render` just
        // uploaded; a pure read of the snapshot + terrain, never the sim.
        if !self.embodied && self.debug_hitboxes {
            let verts = debug_overlay_lines(&self.curr, &self.sim.terrain);
            self.renderer.render_debug(device, queue, view, &verts);
        }

        // 7e. Detection "gone dark" tell (command view): mark each hostile EMBODIED enemy the Player
        // commander can currently SENSE — Subtle reveals one only while an own unit holds range +
        // line of sight, then a fading linger at the last-seen point; Marked is persistent (D33,
        // `core::detection`). The fairness boundary (invariant #6): this is "alerts, not intel" for
        // the COMMANDER — a marker on an already-earned, sensed unit, never a reveal of un-sensed
        // units or the rest of the map. It is gated to the command view here (`!self.embodied`) AND
        // the pure `detection_markers` seam refuses to emit while the local player is embodied, so it
        // can never paint over the dark embodied frame. `detectable_embodiment` is a read-only,
        // checksum-excluded derivation over the live world + terrain (it never mutates the sim or
        // touches the checksum — invariants #1/#7); the linger memory is per-client presentation
        // state. Reuses the command view-projection the step-7 `render` just uploaded.
        if !self.embodied {
            let tells = detectable_embodiment(
                &self.sim.world,
                &self.sim.terrain,
                &self.detection,
                Faction::Player,
                tick,
                &mut self.detection_memory,
            );
            let markers = detection_markers(&tells, self.embodied, self.detection.tell_linger_ticks);
            let verts = gonedark_render::detection::detection_vertices(&markers);
            self.renderer.render_detection(device, queue, view, &verts);
        }

        // 8. While embodied, draw the directional alert HUD over the dark frame (worker 2) — the
        // only thread back to command (invariant #6).
        if self.embodied {
            self.renderer.render_hud(
                device,
                queue,
                view,
                &self.alerts,
                listener,
                self.yaw,
                viewport,
                tick,
            );
            // 8'. WS-4 — the embodied hitmarker: a centered "X" flash confirming the player's OWN
            // connecting shot, drawn over the dark frame. A no-op unless a hit is live in the fade
            // window (`last_hit_tick`). Feedback on the player's own action, never map intel about an
            // unseen enemy (invariant #6) — and screen-space chrome with no world position, so it
            // widens no fog beneath it.
            self.renderer
                .render_hitmarker(device, queue, view, self.last_hit_tick, tick);
        }

        // 8a'. On a touch device, draw the on-screen FPS controls over the dark frame (the COD-style
        // move stick + Fire/Crouch/Reload/Surface). Gated to embodied AND `touch_hud.is_some()` —
        // set only when this frame's input came through `input.touches` — so the desktop keyboard/
        // mouse path never draws a GUI (the controls are Android-only). The Crouch button lights from
        // authoritative sim posture. Screen-space chrome with no world position (invariant #6).
        if self.embodied {
            if let Some(hud_state) = self.touch_hud {
                let layout = touch_controls::TouchLayout::new(width, height);
                let crouched = self.sim.world.is_alive(self.player)
                    && self.sim.world.posture[self.player.index as usize] == Posture::Crouched;
                let rhud = render_touch_hud(&layout, &hud_state, viewport, crouched);
                self.renderer
                    .render_touch_controls(device, queue, view, &rhud);
            }
        }

        // 8b. In the command view, draw the radial command menu when a held long-press has one open
        // (engine::command_ui's radial preview). It is NDC chrome with no world position and is
        // gated to `!embodied`, so it never paints over the dark frame (invariant #6). The menu
        // anchors at the pointer (mapped pixels → NDC) or the screen center when none is known.
        if !self.embodied && !self.radial_menu.is_empty() {
            let center = input
                .pointer
                .map(|(px, py)| {
                    [
                        px / width as f32 * 2.0 - 1.0,
                        1.0 - py / height as f32 * 2.0,
                    ]
                })
                .unwrap_or([0.0, 0.0]);
            let menu = RadialMenu {
                center,
                slots: self.radial_menu.len(),
                highlight: None,
            };
            self.renderer.render_radial(device, queue, view, &menu);
        }

        // 9. Draw the in-session shell overlay (Phase 4 WS-B) LAST, over everything else — so the
        // pause / reconnect prompt / post-match summary dims and sits above the (possibly dark)
        // frame, the alert HUD, and the radial menu. It is screen-space chrome with no world
        // position, so it never widens the avatar-only fog beneath it (invariant #6). `Overlay::None`
        // is a no-op.
        let overlay = overlay_for_surface(self.shell.surface());
        self.renderer.render_overlay(device, queue, view, &overlay);
    }
}

/// What a click on an in-session shell-overlay button resolves to for the host. `Session` is a
/// [`SessionAction`](gonedark_core::shell::SessionAction) the host feeds back through
/// [`Game::apply_session_action`] (pause/resume/surrender flips the live overlay); `Dismiss`
/// acknowledges the terminal post-match summary — the host tears the match down and returns to its
/// own out-of-match screen, which the engine deliberately knows nothing about (invariant #2).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OverlayClick {
    Session(gonedark_core::shell::SessionAction),
    Dismiss,
}

/// Resolve a hit button `slot` on `overlay` to its [`OverlayClick`]. The slot order mirrors the
/// renderer's per-surface button vocabulary (`overlay::surface_choices`): on the pause / reconnect
/// surfaces slot 0 is the affirmative **Resume** and slot 1 is **Surrender**/leave; the post-match
/// summary has a single **Dismiss**. Pure (no `Game`, no GPU) — unit-tested below. An unknown
/// (overlay, slot) pair yields `None` rather than a wrong action.
fn overlay_click_action(overlay: &Overlay, slot: usize) -> Option<OverlayClick> {
    use gonedark_core::shell::SessionAction;
    match (overlay, slot) {
        (Overlay::Paused, 0) | (Overlay::ReconnectPrompt { .. }, 0) => {
            Some(OverlayClick::Session(SessionAction::Resume))
        }
        (Overlay::Paused, 1) | (Overlay::ReconnectPrompt { .. }, 1) => {
            Some(OverlayClick::Session(SessionAction::Surrender))
        }
        (Overlay::Summary(_), 0) => Some(OverlayClick::Dismiss),
        _ => None,
    }
}

/// Whether a shell `surface` warrants an overlay — anything but the plain `Playing` surface. Pure
/// seam behind [`Game::shell_overlay_active`], so the host's cursor-freeing / input-freezing
/// predicate is unit-tested without constructing a GPU `Game`.
fn overlay_active(surface: &ShellSurface) -> bool {
    !matches!(surface, ShellSurface::Playing)
}

/// Map the current shell surface to the host pause-toggle action: **Playing → Pause** (open the
/// menu), **Paused → Resume** (close it), and `None` on the terminal/blocking surfaces
/// (`Ended` / `ReconnectPrompt`), which own the screen and are dismissed by their own buttons
/// rather than by the pause key. Pure (no `Game`, no GPU) so it is unit-tested below — this toggle
/// *decision* is the only logic in [`Game::toggle_pause`], the rest being thin host glue.
fn pause_toggle_action(surface: &ShellSurface) -> Option<gonedark_core::shell::SessionAction> {
    use gonedark_core::shell::SessionAction;
    match surface {
        ShellSurface::Playing => Some(SessionAction::Pause),
        ShellSurface::Paused => Some(SessionAction::Resume),
        ShellSurface::Ended(_) | ShellSurface::ReconnectPrompt(_) => None,
    }
}

/// Map the in-session shell surface to the render-side [`Overlay`] description (Phase 4 WS-B). Pure
/// (no `Game`, no GPU) so it is unit-testable: `Playing` → nothing; `Paused` → the pause overlay;
/// `ReconnectPrompt` → the prompt (severity from the [`LinkState`]); `Ended` → the post-match
/// summary panel (the integer-only `MatchSummary`, full-info — shown only once the match is over).
fn overlay_for_surface(surface: &ShellSurface) -> Overlay {
    match surface {
        ShellSurface::Playing => Overlay::None,
        ShellSurface::Paused => Overlay::Paused,
        ShellSurface::ReconnectPrompt(state) => Overlay::ReconnectPrompt {
            desynced: *state == LinkState::Desynced,
        },
        ShellSurface::Ended(summary) => Overlay::Summary(summary.clone()),
    }
}

/// Extrapolate the embodied avatar's eye to the current render instant: its latest authoritative
/// position carried forward along its authoritative velocity by `lead_secs` — the render sub-tick
/// fraction plus, in multiplayer, the input-delay lead. This is the **predict** half of D15: the
/// one entity you twitch-control leads the discrete authoritative ticks so it reads as responsive,
/// while every other unit stays pure interpolated lockstep. The float boundary lives HERE — Fixed
/// authoritative state crosses to f32 for presentation and never crosses back (invariant #1).
fn extrapolate_avatar(pos: (f32, f32), vel: (f32, f32), lead_secs: f32) -> (f32, f32) {
    (pos.0 + vel.0 * lead_secs, pos.1 + vel.1 * lead_secs)
}

/// Reconcile the running predicted eye toward a fresh authoritative `target`: ease by `smoothing`
/// (clamped to `[0,1]`), but **snap** when the error meets/exceeds `snap_dist` so a large
/// correction resolves at once instead of sliding. Pure; returns the new predicted eye. This is
/// the **reconcile against the tick** half of D15 — misprediction (and, in multiplayer, the
/// authoritative T+D resolution differing from the local lead) decays smoothly, never as a jolt.
fn reconcile_avatar(
    predicted: (f32, f32),
    target: (f32, f32),
    smoothing: f32,
    snap_dist: f32,
) -> (f32, f32) {
    let (dx, dy) = (target.0 - predicted.0, target.1 - predicted.1);
    if dx * dx + dy * dy >= snap_dist * snap_dist {
        return target; // too far to ease — snap to the authoritative target
    }
    let s = smoothing.clamp(0.0, 1.0);
    (predicted.0 + dx * s, predicted.1 + dy * s)
}

/// Avatar-local prediction state (D15) — the predicted embodied **eye position**, living entirely
/// in the PRESENTATION path. It is fed the authoritative avatar pose (read from the snapshot by
/// shared reference) and leads + reconciles a smooth eye for the first-person camera + audio
/// listener. It holds **no** `Sim` and is never handed `&mut Sim`, so it *structurally cannot*
/// feed back into deterministic state — the silent-desync risk invariant #1 exists to prevent.
/// Aim (yaw) is the other half of the predicted transform and is integrated locally in
/// [`Game::yaw`]; together they are the predicted avatar transform. (Authoritative hit
/// resolution still happens in the sim at tick T+D.)
#[derive(Clone, Copy, Default)]
struct AvatarPrediction {
    /// Predicted eye position (world XY, f32). Meaningful only while `valid`.
    eye: (f32, f32),
    /// False until the first embodied frame anchors `eye` to the authoritative position (so the
    /// camera never eases in from a stale/origin value); reset to false on surfacing.
    valid: bool,
}

impl AvatarPrediction {
    /// Drop the prediction (call when not embodied) — the next embodied frame re-anchors.
    fn clear(&mut self) {
        self.valid = false;
    }

    /// Update the predicted eye from the authoritative avatar pose (`pos`/`vel`, world f32),
    /// leading by `lead_secs` and reconciling against the tick. Presentation-only — touches only
    /// `self`. The first embodied frame anchors (no ease-in); subsequent frames reconcile.
    fn update(&mut self, pos: (f32, f32), vel: (f32, f32), lead_secs: f32) {
        let target = extrapolate_avatar(pos, vel, lead_secs);
        self.eye = if self.valid {
            reconcile_avatar(
                self.eye,
                target,
                AVATAR_RECONCILE_SMOOTHING,
                AVATAR_RECONCILE_SNAP_DIST,
            )
        } else {
            self.valid = true;
            target
        };
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gonedark_core::components::{BuildingKind, EntityKind};
    use gonedark_core::economy::{self, Resources};
    use gonedark_core::ecs::World;
    use gonedark_render::fixed_to_f32;

    /// Scene-name parsing for the `app --scene <name>` host flag — pure, GPU-free.
    #[test]
    fn scene_parse_known_and_unknown() {
        assert_eq!(Scene::parse("default"), Some(Scene::Default));
        assert_eq!(Scene::parse("demo"), Some(Scene::Default));
        assert_eq!(Scene::parse("skirmish"), Some(Scene::Skirmish));
        assert_eq!(Scene::parse("match"), Some(Scene::Skirmish));
        assert_eq!(Scene::parse("duel"), Some(Scene::Duel));
        assert_eq!(Scene::parse("infantry"), Some(Scene::Infantry));
        assert_eq!(Scene::parse("nope"), None);
        assert_eq!(Scene::default(), Scene::Default);
        // The debug sandboxes default the overlay on; a real match (skirmish/demo) leaves it off.
        assert!(Scene::Duel.debug_overlay_default());
        assert!(Scene::Infantry.debug_overlay_default());
        assert!(!Scene::Default.debug_overlay_default());
        assert!(!Scene::Skirmish.debug_overlay_default());
    }

    // --- detection "gone dark" tell → render markers (the pure seam) -------------------------------

    fn tell(index: u32, x: i32, y: i32, age_ticks: u32) -> Tell {
        Tell {
            unit: gonedark_core::ecs::Entity {
                index,
                generation: 0,
            },
            pos: Vec2::new(Fixed::from_int(x), Fixed::from_int(y)),
            age_ticks,
        }
    }

    /// The fairness gate (invariant #6): while the LOCAL player is embodied the command view is dark,
    /// so the seam emits NO marker — even when the observer has live tells. Proven headlessly here.
    #[test]
    fn detection_markers_empty_while_locally_embodied() {
        let tells = [tell(0, 5, 0, 0), tell(1, -3, 2, 0)];
        let markers = detection_markers(&tells, /* world_dark = */ true, 90);
        assert!(
            markers.is_empty(),
            "no command-view detection intel may leak while the local player is embodied"
        );
    }

    /// No tells in → no markers out (the Hidden mode / nothing-sensed case the host hands through).
    #[test]
    fn detection_markers_empty_with_no_tells() {
        assert!(detection_markers(&[], false, 90).is_empty());
    }

    /// In the command view, each fresh (in-sight / Marked) tell maps to one fully-opaque marker at
    /// the tell's f32 position — correct count + positions.
    #[test]
    fn detection_markers_map_count_and_positions() {
        let tells = [tell(0, 5, 0, 0), tell(1, -3, 7, 0), tell(2, 12, -4, 0)];
        let markers = detection_markers(&tells, false, 90);
        assert_eq!(markers.len(), 3, "one marker per sensed tell");
        for (m, t) in markers.iter().zip(tells.iter()) {
            assert_eq!(m.x, fixed_to_f32(t.pos.x));
            assert_eq!(m.y, fixed_to_f32(t.pos.y));
            assert_eq!(m.alpha, 1.0, "a fresh / in-sight tell is fully opaque");
        }
    }

    /// The `Subtle` linger surfaces as a fade: a marker grows fainter as the tell ages out of its
    /// linger window, down to (but not below) `MIN_TELL_ALPHA` at the edge of the window.
    #[test]
    fn detection_markers_linger_fades_with_age() {
        let linger = 100;
        let fresh = detection_markers(&[tell(0, 1, 1, 0)], false, linger)[0].alpha;
        let mid = detection_markers(&[tell(0, 1, 1, 50)], false, linger)[0].alpha;
        let old = detection_markers(&[tell(0, 1, 1, 100)], false, linger)[0].alpha;
        assert_eq!(fresh, 1.0, "age 0 is fully opaque");
        assert!(mid < fresh && mid > old, "alpha falls monotonically as the tell ages");
        assert!((old - MIN_TELL_ALPHA).abs() < 1e-6, "fades to the floor at the window edge");
        assert!(old > 0.0, "a last-known marker stays legible until it expires");
    }

    /// `tell_alpha` edge cases: a zero-linger window (every present tell is in-sight) never fades, and
    /// an age past the window clamps to the floor rather than going negative.
    #[test]
    fn tell_alpha_edge_cases() {
        assert_eq!(tell_alpha(0, 0), 1.0);
        assert_eq!(tell_alpha(5, 0), 1.0, "zero linger → no fade (only in-sight tells exist)");
        assert_eq!(tell_alpha(0, 90), 1.0);
        assert_eq!(tell_alpha(200, 90), MIN_TELL_ALPHA, "past the window clamps to the floor");
    }

    /// End-to-end through the render geometry: command-view markers produce the fixed per-marker
    /// vertex count carrying the tell's alpha, and the embodied (world-dark) case produces none.
    #[test]
    fn detection_markers_feed_render_geometry() {
        use gonedark_render::detection::{detection_vertices, VERTS_PER_MARKER};
        let tells = [tell(0, 5, 0, 0), tell(1, -3, 7, 30)];
        let markers = detection_markers(&tells, false, 60);
        let verts = detection_vertices(&markers);
        assert_eq!(verts.len(), 2 * VERTS_PER_MARKER);
        // The embodied path renders nothing (no markers → empty vertex list).
        let dark = detection_vertices(&detection_markers(&tells, true, 60));
        assert!(dark.is_empty());
    }

    /// The skirmish boots in the **command view** (not embodied) with the Player's single starting
    /// troop as the selectable `player`, two operational bases, three neutral posts, and the small
    /// scenario purse. GPU-free seam under `Game::new_scene`, so it covers the wiring without a
    /// device (the seeding itself is unit-tested in `core::scenario`).
    #[test]
    fn skirmish_scene_boots_in_command_view_with_one_player_troop() {
        let mut sim = Sim::new(DEFAULT_SEED);
        let (player, start_embodied) = seed_skirmish_scene(&mut sim);
        assert!(!start_embodied, "the skirmish boots commanding, not possessing");

        // The handed-back `player` is a live Player Rifleman, order-driven (not embodied).
        let i = player.index as usize;
        assert_eq!(sim.world.faction[i], Faction::Player);
        assert_eq!(sim.world.unit_kind[i], UnitKind::Rifleman);
        assert_eq!(sim.world.kind[i], EntityKind::Unit);
        assert_eq!(
            sim.world.input_source[i],
            gonedark_core::components::InputSource::Orders,
        );

        // One troop and one base per side, three neutral posts — the scene shape the host renders.
        let units = |f: Faction| {
            (0..sim.world.capacity())
                .filter(|&j| {
                    sim.world.is_index_alive(j)
                        && sim.world.kind[j] == EntityKind::Unit
                        && sim.world.faction[j] == f
                })
                .count()
        };
        assert_eq!(units(Faction::Player), 1);
        assert_eq!(units(Faction::Enemy), 1);
        assert_eq!(sim.territory.points.len(), 3);
    }

    /// The infantry sandbox boots **embodied** in a Player Rifleman with the input source swapped
    /// to `Embodied` (invariant #5). GPU-free seam under `Game::new_scene`.
    #[test]
    fn infantry_scene_boots_embodied_in_a_player_rifleman() {
        let mut sim = Sim::new(DEFAULT_SEED);
        let (player, start_embodied) = seed_infantry_scene(&mut sim);
        assert!(start_embodied, "the infantry sandbox boots in first person");
        let i = player.index as usize;
        assert_eq!(sim.world.faction[i], Faction::Player);
        assert_eq!(sim.world.unit_kind[i], UnitKind::Rifleman);
        assert_eq!(
            sim.world.input_source[i],
            gonedark_core::components::InputSource::Embodied,
        );
    }

    /// The infantry overlay composer draws the firing cone + a RED (blocked) LoS connector to the
    /// walled dummy and GREEN (clear) connectors to the others — the line-of-sight mechanic made
    /// visible. Pure, GPU-free seam under the F3 render block.
    #[test]
    fn infantry_overlay_draws_cone_and_a_blocked_los_connector() {
        let mut sim = Sim::new(DEFAULT_SEED);
        let _ = gonedark_core::scenario::seed_infantry(&mut sim);
        let snap = sim.snapshot();
        let verts = debug_overlay_lines(&snap, &sim.terrain);
        assert!(!verts.is_empty());
        // The firing-cone wedge color (render::debug COLOR_CONE) appears.
        assert!(
            verts.iter().any(|v| v.color == [1.0, 0.65, 0.20]),
            "the firing-cone wedge is drawn",
        );
        // A blocked (red) connector to `walled` and clear (green) connectors to the open dummies.
        assert!(
            verts.iter().any(|v| v.color == [1.0, 0.30, 0.30]),
            "the Heavy wall yields a blocked (red) LoS connector",
        );
        assert!(
            verts.iter().any(|v| v.color == [0.25, 1.0, 0.40]),
            "a clear sightline yields a green LoS connector",
        );
    }

    /// `alive_units_of` (the seam behind `Game::alive_unit_count`) counts only living units of the
    /// asked faction — buildings and the other side excluded. Driven over a headless `Sim` (no GPU),
    /// since `Game` itself needs a device.
    #[test]
    fn alive_units_of_counts_living_units_per_faction() {
        let mut sim = Sim::new(7);
        spawn_unit(&mut sim, 0, 0, Faction::Enemy, Stance::HoldFire);
        let e2 = spawn_unit(&mut sim, 1, 0, Faction::Enemy, Stance::HoldFire);
        spawn_unit(&mut sim, 2, 0, Faction::Player, Stance::HoldFire);
        assert_eq!(alive_units_of(&sim, Faction::Enemy), 2);
        assert_eq!(alive_units_of(&sim, Faction::Player), 1);
        // A despawned unit drops out of the count.
        sim.world.despawn(e2);
        assert_eq!(alive_units_of(&sim, Faction::Enemy), 1);
    }

    /// The muzzle-flash overlay lights only firing, non-building units (the `!u.building && u.firing`
    /// filter in `debug_overlay_lines`): a building is a damageable target, never a shooter, and an
    /// idle unit draws no flash.
    #[test]
    fn muzzle_flash_overlay_lights_firing_units_but_not_buildings() {
        use gonedark_core::components::{Faction, UnitKind, Vec2};
        use gonedark_core::fixed::Fixed;
        use gonedark_core::snapshot::{Snapshot, UnitSnapshot};
        use gonedark_core::trig::Angle;

        const COLOR_MUZZLE: [f32; 3] = [1.0, 0.95, 0.55]; // render::debug COLOR_MUZZLE
        let terrain = gonedark_core::terrain::Terrain::open();

        let mk = |index: u32, building: bool, firing: bool| UnitSnapshot {
            entity_index: index,
            pos: Vec2::new(Fixed::from_int(index as i32 * 5), Fixed::ZERO),
            vel: Vec2::ZERO,
            embodied: false,
            faction: Faction::Player,
            health: Fixed::ONE,
            building,
            unit_kind: UnitKind::Rifleman,
            hull_heading: Angle(0),
            turret_yaw: Angle(0),
            firing,
        };
        let snap = |units| Snapshot {
            tick: 0,
            units,
            control_points: Vec::new(),
            projectiles: Vec::new(),
        };
        let has_muzzle =
            |s: &Snapshot| debug_overlay_lines(s, &terrain).iter().any(|v| v.color == COLOR_MUZZLE);

        assert!(has_muzzle(&snap(vec![mk(0, false, true)])), "a firing unit flashes");
        assert!(
            !has_muzzle(&snap(vec![mk(1, true, true), mk(2, false, false)])),
            "a firing building and an idle unit draw no muzzle flash",
        );
    }

    /// The duel sandbox boots **embodied** in a Player Heavy tank, with the sim input-source already
    /// swapped to `Embodied` (invariant #5) — the state a ballistic `Fire` needs. This is the
    /// GPU-free seam under `Game::new_scene`, so it covers the new logic without a device.
    #[test]
    fn duel_scene_boots_embodied_in_a_player_tank() {
        let mut sim = Sim::new(DEFAULT_SEED);
        let (player, start_embodied) = seed_duel_scene(&mut sim);
        assert!(start_embodied, "the duel sandbox boots in first person");
        let i = player.index as usize;
        assert_eq!(sim.world.faction[i], Faction::Player);
        assert_eq!(sim.world.unit_kind[i], UnitKind::Heavy);
        assert_eq!(
            sim.world.input_source[i],
            gonedark_core::components::InputSource::Embodied,
            "the Embody step swapped the input source",
        );
    }

    /// The default demo skirmish starts in the command view (not embodied) and hands back a live
    /// Player unit to possess.
    #[test]
    fn default_scene_starts_in_command_view() {
        let mut sim = Sim::new(DEFAULT_SEED);
        let (player, start_embodied) = seed_default_scene(&mut sim);
        assert!(!start_embodied, "the demo skirmish starts in command view");
        assert_eq!(sim.world.faction[player.index as usize], Faction::Player);
        assert_eq!(
            sim.world.input_source[player.index as usize],
            gonedark_core::components::InputSource::Orders,
            "an unembodied avatar is still order-driven",
        );
    }

    /// A throwaway player handle for the command-mapping tests — a real generational handle
    /// from a `World`, so the produced commands carry a valid entity.
    fn test_player() -> Entity {
        let mut world = World::new();
        world.spawn()
    }

    /// `active_player_camp` returns the player's lowest-index built camp, skips a half-built one,
    /// ignores enemy camps, and is `None` when the player has none.
    #[test]
    fn active_player_camp_picks_first_built_player_camp() {
        let mut world = World::new();
        let mut res = Resources::new(10_000);

        // No camp yet → None.
        assert!(active_player_camp(&world, Faction::Player).is_none());

        // An ENEMY camp (built) must be ignored.
        let enemy = economy::build(
            &mut world,
            &mut res,
            Faction::Enemy,
            BuildingKind::Camp,
            Vec2::new(Fixed::from_int(9), Fixed::from_int(9)),
        )
        .expect("enemy camp affordable");
        world.building[enemy.index as usize].build_ticks_left = 0;
        assert!(
            active_player_camp(&world, Faction::Player).is_none(),
            "enemy camps don't count"
        );

        // A still-CONSTRUCTING player camp must be skipped (build_ticks_left > 0 by default).
        let building = economy::build(
            &mut world,
            &mut res,
            Faction::Player,
            BuildingKind::Camp,
            Vec2::new(Fixed::from_int(1), Fixed::from_int(1)),
        )
        .expect("player camp affordable");
        assert!(
            active_player_camp(&world, Faction::Player).is_none(),
            "a half-built camp is not operational"
        );

        // Finish it → now it's the active camp, and it's a Camp building owned by the player.
        world.building[building.index as usize].build_ticks_left = 0;
        let got = active_player_camp(&world, Faction::Player).expect("a built player camp exists");
        assert_eq!(got, building);
        let i = got.index as usize;
        assert_eq!(world.faction[i], Faction::Player);
        assert_eq!(world.kind[i], EntityKind::Building);
        assert_eq!(world.building[i].kind, BuildingKind::Camp);

        // A second built player camp doesn't displace the first (lowest-index is deterministic).
        let second = economy::build(
            &mut world,
            &mut res,
            Faction::Player,
            BuildingKind::Camp,
            Vec2::new(Fixed::from_int(5), Fixed::from_int(5)),
        )
        .expect("second player camp affordable");
        world.building[second.index as usize].build_ticks_left = 0;
        assert_eq!(
            active_player_camp(&world, Faction::Player),
            Some(building),
            "the lowest-index built camp stays the deterministic active camp"
        );
    }

    /// `command_view_production_commands` maps the InputFrame's build/train/upgrade edges onto the
    /// matching sim commands: a build places at the (quantized) cursor for the player, train/upgrade
    /// route at the active camp, an idle frame emits nothing, and without a camp only the build (which
    /// needs none) survives.
    #[test]
    fn production_intents_map_to_build_train_upgrade_commands() {
        let mut world = World::new();
        let mut res = Resources::new(10_000);
        let camp = economy::build(
            &mut world,
            &mut res,
            Faction::Player,
            BuildingKind::Camp,
            Vec2::new(Fixed::from_int(2), Fixed::from_int(3)),
        )
        .expect("player camp affordable");
        world.building[camp.index as usize].build_ticks_left = 0;

        // Idle frame → nothing, even with a cursor + active camp available.
        let idle = InputFrame::default();
        assert!(command_view_production_commands(&idle, Some((1.0, 2.0)), Some(camp)).is_empty());

        // Build: slot 0 + a cursor point → one Build at the quantized point, for the player.
        let build = InputFrame {
            building_slot: Some(0),
            ..Default::default()
        };
        let cmds = command_view_production_commands(&build, Some((12.5, -4.25)), Some(camp));
        assert_eq!(cmds.len(), 1, "exactly one Build");
        match &cmds[0] {
            Command::Build { faction, kind, pos } => {
                assert_eq!(*faction, Faction::Player);
                assert_eq!(*kind, BuildingKind::Camp);
                assert_eq!(pos.x.to_bits(), world_to_fixed(12.5).to_bits());
                assert_eq!(pos.y.to_bits(), world_to_fixed(-4.25).to_bits());
            }
            other => panic!("expected Build, got {other:?}"),
        }

        // Train: the slot routes a QueueProduction at the active camp (slot 1 = Heavy).
        let train = InputFrame {
            train_slot: Some(1),
            ..Default::default()
        };
        let cmds = command_view_production_commands(&train, None, Some(camp));
        assert_eq!(cmds.len(), 1);
        match &cmds[0] {
            Command::QueueProduction { camp: c, unit } => {
                assert_eq!(*c, camp);
                assert_eq!(*unit, UnitKind::Heavy);
            }
            other => panic!("expected QueueProduction, got {other:?}"),
        }

        // Upgrade: the edge upgrades the active camp.
        let up = InputFrame {
            upgrade_pressed: true,
            ..Default::default()
        };
        let cmds = command_view_production_commands(&up, None, Some(camp));
        assert_eq!(cmds.len(), 1);
        assert!(
            matches!(&cmds[0], Command::Upgrade { camp: c } if *c == camp),
            "upgrade targets the active camp"
        );

        // No active camp: train + upgrade emit nothing (no camp to act on), but a build still places
        // (a build needs only a slot + a point, not a camp).
        let all = InputFrame {
            building_slot: Some(0),
            train_slot: Some(0),
            upgrade_pressed: true,
            ..Default::default()
        };
        let cmds = command_view_production_commands(&all, Some((0.0, 0.0)), None);
        assert_eq!(cmds.len(), 1, "only the build survives without a camp");
        assert!(matches!(&cmds[0], Command::Build { .. }));
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
        let vp = topdown_view_proj(width, height, 0.0, 0.0, TOPDOWN_HALF_EXTENT);
        let clip = vp * Vec4::new(0.0, 0.0, 0.0, 1.0);
        let ndc_x = clip.x / clip.w;
        let ndc_y = clip.y / clip.w;
        // NDC center -> screen center.
        let px = (ndc_x * 0.5 + 0.5) * width as f32;
        let py = (1.0 - (ndc_y * 0.5 + 0.5)) * height as f32;
        assert!((px - width as f32 / 2.0).abs() < 1e-2, "center x = {px}");
        assert!((py - height as f32 / 2.0).abs() < 1e-2, "center y = {py}");
    }

    /// `embodied_proj` is the single source of the embodied perspective constants (D44 shares it
    /// with the weapon viewmodel pass), so pin it: it must equal a direct `perspective_rh` with the
    /// documented FOV/near/far, and produce a sane 4:3 frustum. Guards the constants against drift —
    /// if they ever diverge, the gun's projection silently stops matching the world it sits in.
    #[test]
    fn embodied_proj_matches_documented_constants() {
        let (width, height) = (800u32, 600u32); // 4:3
        let got = embodied_proj(width, height);
        let expected = Mat4::perspective_rh(
            EMBODIED_FOV_DEG.to_radians(),
            width as f32 / height as f32,
            EMBODIED_NEAR,
            EMBODIED_FAR,
        );
        assert_eq!(
            got.to_cols_array(),
            expected.to_cols_array(),
            "embodied_proj must be the documented perspective verbatim"
        );
        // Sanity on the diagonal: m11 = 1/tan(fov/2); m00 = m11 / aspect.
        let m = got.to_cols_array_2d();
        let m11 = 1.0 / (EMBODIED_FOV_DEG.to_radians() / 2.0).tan();
        assert!((m[1][1] - m11).abs() < 1e-4, "m11 = {}", m[1][1]);
        assert!(
            (m[0][0] - m11 / (width as f32 / height as f32)).abs() < 1e-4,
            "m00 = {}",
            m[0][0]
        );
    }

    /// The handheld rifle viewmodel is infantry-only: a possessed rifleman shows it, a possessed
    /// tank does not (no infantry rifle floating in the cannon view).
    #[test]
    fn rifle_viewmodel_is_infantry_only() {
        assert!(
            embodied_shows_rifle_viewmodel(UnitKind::Rifleman),
            "an embodied rifleman carries the rifle viewmodel"
        );
        assert!(
            !embodied_shows_rifle_viewmodel(UnitKind::Heavy),
            "an embodied tank has no handheld rifle viewmodel"
        );
    }

    /// Unprojecting the center pixel returns ~`(0,0)`.
    #[test]
    fn unproject_center_pixel_is_origin() {
        let (width, height) = (1920u32, 1080u32);
        let vp = topdown_view_proj(width, height, 0.0, 0.0, TOPDOWN_HALF_EXTENT);
        let (wx, wy) =
            unproject_topdown(&vp, width as f32 / 2.0, height as f32 / 2.0, width, height).unwrap();
        assert!(wx.abs() < 1e-3, "center world x = {wx}");
        assert!(wy.abs() < 1e-3, "center world y = {wy}");
    }

    /// Unproject round-trips against project for the tilted command camera (D45): the right edge of
    /// the centre row still maps to `(+hx, 0)` (the X extent + centre-row separability), and a spread
    /// of ground points survive a project→unproject round-trip — the ground-plane ray cast is exact.
    #[test]
    fn unproject_roundtrips_on_the_tilted_camera() {
        let (width, height) = (1000u32, 1000u32); // square -> symmetric extent
        let vp = topdown_view_proj(width, height, 0.0, 0.0, TOPDOWN_HALF_EXTENT);

        // Right edge, vertical centre -> (+half_extent, 0). The centre row stays y=0 (separability),
        // and the X extent is unchanged by the pitch tilt.
        let (wx, wy) = unproject_topdown(&vp, width as f32, height as f32 / 2.0, width, height)
            .expect("right-edge unproject");
        assert!((wx - TOPDOWN_HALF_EXTENT).abs() < 1e-2, "right x = {wx}");
        assert!(wy.abs() < 1e-2, "centre row stays y=0, got {wy}");

        // Project a ground point to its pixel, then unproject back — must return the same point.
        let project = |x: f32, y: f32| {
            let c = vp * Vec4::new(x, y, 0.0, 1.0);
            let (nx, ny) = (c.x / c.w, c.y / c.w);
            (
                (nx * 0.5 + 0.5) * width as f32,
                (1.0 - (ny * 0.5 + 0.5)) * height as f32,
            )
        };
        for (x, y) in [(0.0, 0.0), (12.0, -7.0), (-20.0, 15.0), (33.0, 4.0)] {
            let (px, py) = project(x, y);
            let (ux, uy) = unproject_topdown(&vp, px, py, width, height).expect("roundtrip");
            assert!(
                (ux - x).abs() < 1e-2 && (uy - y).abs() < 1e-2,
                "round-trip ({x},{y}) -> ({ux},{uy})"
            );
        }
    }

    /// The command camera is tilted (so 3D tokens read) yet axis-separable (so band-select's
    /// world-AABB stays exact) — the load-bearing property of the D45 pure-pitch tilt. Ground points
    /// sharing a world axis share that screen axis; raising a point in +Z moves it up-screen.
    #[test]
    fn command_camera_is_tilted_and_axis_separable() {
        let (width, height) = (1000u32, 1000u32);
        let vp = topdown_view_proj(width, height, 0.0, 0.0, TOPDOWN_HALF_EXTENT);
        let project = |x: f32, y: f32, z: f32| {
            let c = vp * Vec4::new(x, y, z, 1.0);
            (c.x / c.w, c.y / c.w)
        };
        // No yaw: ground points sharing world-x share screen-x; sharing world-y share screen-y.
        let (ax, _) = project(5.0, 0.0, 0.0);
        let (bx, _) = project(5.0, 18.0, 0.0);
        assert!((ax - bx).abs() < 1e-4, "same world-x → same screen-x ({ax} vs {bx})");
        let (_, cy) = project(0.0, 7.0, 0.0);
        let (_, dy) = project(22.0, 7.0, 0.0);
        assert!((cy - dy).abs() < 1e-4, "same world-y → same screen-y ({cy} vs {dy})");
        // Tilted, not straight down: a point raised in +Z reads higher up the screen than its base.
        let (_, ground_y) = project(0.0, 0.0, 0.0);
        let (_, up_y) = project(0.0, 0.0, 5.0);
        assert!(
            up_y > ground_y + 1e-3,
            "height reads as up-screen under the tilt ({ground_y} → {up_y})"
        );
    }

    /// A bare left-click (`pointer_down`) no longer moves a hard-wired avatar (D42): movement comes
    /// from the right-click "command the selection" path, not from `map_input_commands`. The
    /// selection gesture rides `pointer_down` separately (see `Selection`).
    #[test]
    fn map_input_bare_click_emits_no_movement() {
        let player = test_player();
        let input = InputFrame {
            pointer: Some((900.0, 300.0)),
            pointer_down: true,
            ..Default::default()
        };
        let cmds = map_input_commands(&input, false, player, None);
        assert!(
            cmds.is_empty(),
            "a left-click selects; it must not emit a Move/AttackMove, got {cmds:?}"
        );
    }

    /// `embody_pressed && !embodied` -> contains `Embody`, not `Surface`.
    #[test]
    fn map_input_embody_when_surfaced() {
        let player = test_player();
        let input = InputFrame {
            embody_pressed: true,
            ..Default::default()
        };
        let cmds = map_input_commands(&input, false, player, Some(player));
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
        let cmds = map_input_commands(&input, true, player, None);
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

        let surfaced = map_input_commands(&both, false, player, Some(player));
        assert!(surfaced.iter().any(|c| matches!(c, Command::Embody { .. })));
        assert!(!surfaced
            .iter()
            .any(|c| matches!(c, Command::Surface { .. })));

        let embodied = map_input_commands(&both, true, player, None);
        assert!(embodied
            .iter()
            .any(|c| matches!(c, Command::Surface { .. })));
        assert!(!embodied.iter().any(|c| matches!(c, Command::Embody { .. })));
    }

    /// An embody press with NO live unit to take (`embody_target == None`) drops the edge — it must
    /// not emit a possession over nothing (the corpse-guard: a dead avatar never re-possesses).
    #[test]
    fn map_input_drops_embody_with_no_target() {
        let player = test_player();
        let input = InputFrame {
            embody_pressed: true,
            ..Default::default()
        };
        let cmds = map_input_commands(&input, false, player, None);
        assert!(
            cmds.is_empty(),
            "no live target → no Embody command, got {cmds:?}"
        );
    }

    /// The embody command carries the RESOLVED target, not the current avatar — possessing the unit
    /// the player picked, not the hard-wired original.
    #[test]
    fn map_input_embody_targets_the_resolved_unit() {
        let avatar = Entity { index: 1, generation: 0 };
        let picked = Entity { index: 7, generation: 3 };
        let input = InputFrame {
            embody_pressed: true,
            ..Default::default()
        };
        let cmds = map_input_commands(&input, false, avatar, Some(picked));
        assert!(
            cmds.iter()
                .any(|c| matches!(c, Command::Embody { entity } if *entity == picked)),
            "Embody must carry the resolved target {picked:?}, got {cmds:?}"
        );
    }

    /// Spawn `n` live Player units into a fresh `World`, returning their handles in index order.
    fn world_with_player_units(n: usize) -> (gonedark_core::ecs::World, Vec<Entity>) {
        let mut world = World::new();
        let es = (0..n)
            .map(|_| {
                let e = world.spawn();
                world.faction[e.index as usize] = Faction::Player;
                world.kind[e.index as usize] = EntityKind::Unit;
                e
            })
            .collect();
        (world, es)
    }

    fn selection_of(units: &[Entity]) -> Selection {
        let mut sel = Selection::new();
        sel.units.extend_from_slice(units);
        sel
    }

    /// Rule 1: the first LIVE selected Player unit wins (the RTS "select, then possess" path) over
    /// the current avatar.
    #[test]
    fn embody_target_prefers_first_live_selected_unit() {
        let (world, e) = world_with_player_units(3);
        let sel = selection_of(&[e[1], e[2]]);
        assert_eq!(embody_target(&sel, &world, e[0]), Some(e[1]));
    }

    /// A dead selected handle and an enemy selected unit are both skipped; the first *live Player*
    /// selection is taken.
    #[test]
    fn embody_target_skips_dead_and_non_player_selections() {
        let (mut world, e) = world_with_player_units(3);
        world.despawn(e[0]); // dead selected handle
        world.faction[e[1].index as usize] = Faction::Enemy; // enemy selected unit
        let sel = selection_of(&[e[0], e[1], e[2]]);
        assert_eq!(
            embody_target(&sel, &world, e[2]),
            Some(e[2]),
            "skip the corpse and the enemy, take the live player unit"
        );
    }

    /// Rule 2: with nothing selected, re-possess the current avatar while it is still alive.
    #[test]
    fn embody_target_keeps_live_current_when_nothing_selected() {
        let (world, e) = world_with_player_units(2);
        let empty = Selection::new();
        assert_eq!(embody_target(&empty, &world, e[1]), Some(e[1]));
    }

    /// Rule 3 — the bug fix: when the avatar has DIED and nothing is selected, fall back to any live
    /// Player unit (stable index order) so embodiment is never permanently stranded.
    #[test]
    fn embody_target_falls_back_to_a_live_unit_after_avatar_death() {
        let (mut world, e) = world_with_player_units(3);
        world.despawn(e[0]); // the original avatar died
        let empty = Selection::new();
        assert_eq!(
            embody_target(&empty, &world, e[0]),
            Some(e[1]),
            "a dead avatar must not strand embodiment — take the next live player unit"
        );
    }

    /// `None` only when the player has no live unit at all (an enemy-only / empty world): every
    /// possession path is then a correct no-op.
    #[test]
    fn embody_target_is_none_without_a_live_player_unit() {
        let mut world = World::new();
        let enemy = world.spawn();
        world.faction[enemy.index as usize] = Faction::Enemy;
        world.kind[enemy.index as usize] = EntityKind::Unit;
        let empty = Selection::new();
        assert_eq!(embody_target(&empty, &world, enemy), None);
    }

    // ---- embody picker (multi-select "which unit to possess") ----

    /// The picker rows are the LIVE PLAYER units in the selection, in selection order — a corpse and
    /// an enemy in the selection are filtered out.
    #[test]
    fn live_selected_player_units_filters_to_live_player_units() {
        let (mut world, e) = world_with_player_units(4);
        world.despawn(e[1]); // a dead selected handle
        world.faction[e[2].index as usize] = Faction::Enemy; // an enemy in the band
        let sel = selection_of(&[e[0], e[1], e[2], e[3]]);
        assert_eq!(
            live_selected_player_units(&sel, &world),
            vec![e[0], e[3]],
            "only the live player units survive, in selection order"
        );
    }

    /// A number key picks its row (the player's "1" key is `command_slot` 0); an out-of-range key is
    /// ignored (the picker stays open), never a mis-pick.
    #[test]
    fn embody_pick_outcome_number_key_picks_row_in_range() {
        let (_w, e) = world_with_player_units(3);
        assert_eq!(
            embody_pick_outcome(&e, Some(0), None, false, false, false),
            PickOutcome::Pick(e[0])
        );
        assert_eq!(
            embody_pick_outcome(&e, Some(2), None, false, false, false),
            PickOutcome::Pick(e[2])
        );
        // Out of range (only 3 rows) → not a pick, and with no other signal the picker stays.
        assert_eq!(
            embody_pick_outcome(&e, Some(7), None, false, false, false),
            PickOutcome::Stay
        );
    }

    /// A tap on a row picks it; a tap that hit no row (a miss) cancels.
    #[test]
    fn embody_pick_outcome_tap_picks_or_cancels() {
        let (_w, e) = world_with_player_units(3);
        assert_eq!(
            embody_pick_outcome(&e, None, Some(1), true, false, false),
            PickOutcome::Pick(e[1])
        );
        assert_eq!(
            embody_pick_outcome(&e, None, None, true, false, false),
            PickOutcome::Cancel,
            "a tap that missed every row closes the picker"
        );
    }

    /// Re-pressing embody (or surface) while the picker is open cancels it; an idle frame leaves it
    /// open.
    #[test]
    fn embody_pick_outcome_embody_or_surface_cancels_and_idle_stays() {
        let (_w, e) = world_with_player_units(2);
        assert_eq!(
            embody_pick_outcome(&e, None, None, false, true, false),
            PickOutcome::Cancel
        );
        assert_eq!(
            embody_pick_outcome(&e, None, None, false, false, true),
            PickOutcome::Cancel
        );
        assert_eq!(
            embody_pick_outcome(&e, None, None, false, false, false),
            PickOutcome::Stay
        );
    }

    /// The picker view labels each row by unit kind (Heavy→"Heavy", Rifleman→"Rifleman"; D65 added
    /// the real Tank/Medic, so Heavy reads as itself now).
    #[test]
    fn embody_picker_view_labels_rows_by_kind() {
        let (mut world, e) = world_with_player_units(2);
        world.unit_kind[e[0].index as usize] = UnitKind::Heavy;
        world.unit_kind[e[1].index as usize] = UnitKind::Rifleman;
        let view = embody_picker_view(&e, &world);
        assert_eq!(view.rows.len(), 2);
        assert_eq!(view.rows[0].label, "Heavy");
        assert_eq!(view.rows[1].label, "Rifleman");
        assert!(view.rows.iter().all(|r| r.embodiable));
    }

    // ---- contextual command panel ----

    /// Build a live, operational Player camp and return its handle.
    fn build_player_camp(world: &mut World, res: &mut Resources) -> Entity {
        let camp = economy::build(
            world,
            res,
            Faction::Player,
            BuildingKind::Camp,
            Vec2::new(Fixed::from_int(1), Fixed::from_int(1)),
        )
        .expect("camp affordable");
        world.building[camp.index as usize].build_ticks_left = 0; // operational
        camp
    }

    fn has_line(view: &gonedark_render::command_panel::CommandPanelView, needle: &str) -> bool {
        view.lines.iter().any(|l| l.text.contains(needle))
    }

    /// A selected camp shows its train + upgrade options and resources.
    #[test]
    fn command_panel_view_camp_shows_train_and_upgrade() {
        let mut world = World::new();
        let mut res = Resources::new(10_000);
        let camp = build_player_camp(&mut world, &mut res);
        let sel = selection_of(&[camp]);
        let view = command_panel_view(&world, &sel, res.get(Faction::Player), &[UnitKind::Rifleman, UnitKind::Heavy]);
        assert!(view.title.starts_with("CAMP"), "title names the camp: {}", view.title);
        assert!(has_line(&view, "TRAIN"), "shows a TRAIN section");
        assert!(has_line(&view, "UPGRADE"), "shows an UPGRADE section");
        assert!(has_line(&view, "Rifleman"), "lists a trainable unit");
    }

    /// A selected troop group shows its composition, average health, and stance.
    #[test]
    fn command_panel_view_troops_shows_composition_and_stance() {
        let (mut world, e) = world_with_player_units(2);
        world.unit_kind[e[0].index as usize] = UnitKind::Heavy;
        world.unit_kind[e[1].index as usize] = UnitKind::Rifleman;
        let sel = selection_of(&[e[0], e[1]]);
        let view = command_panel_view(&world, &sel, 500, &[UnitKind::Rifleman, UnitKind::Heavy]);
        assert_eq!(view.title, "SELECTED — 2 units");
        // Heavy now reads as itself; a real Tank/Medic reads "Tank"/"Medic" (D65).
        assert!(has_line(&view, "1x Heavy"));
        assert!(has_line(&view, "1x Rifleman"));
        assert!(has_line(&view, "Stance:"));
        assert!(has_line(&view, "Avg HP:"));
    }

    /// An empty selection shows the build palette + resources.
    #[test]
    fn command_panel_view_empty_shows_build_palette() {
        let world = World::new();
        let empty = Selection::new();
        let view = command_panel_view(&world, &empty, 500, &[UnitKind::Rifleman, UnitKind::Heavy]);
        assert_eq!(view.title, "BUILD");
        assert!(has_line(&view, "Resources:"));
        assert!(has_line(&view, "Camp"), "lists the placeable camp");
    }

    /// A selection mixing a camp and troops shows the CAMP panel (building takes priority).
    #[test]
    fn command_panel_view_building_takes_priority_over_troops() {
        let mut world = World::new();
        let mut res = Resources::new(10_000);
        let unit = world.spawn();
        world.faction[unit.index as usize] = Faction::Player;
        world.kind[unit.index as usize] = EntityKind::Unit;
        let camp = build_player_camp(&mut world, &mut res);
        let sel = selection_of(&[unit, camp]); // troop first, camp second
        let view = command_panel_view(&world, &sel, res.get(Faction::Player), &[UnitKind::Rifleman, UnitKind::Heavy]);
        assert!(view.title.starts_with("CAMP"), "a building in the selection wins over troops");
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
        let cmds = map_input_commands(&input, true, player, None);
        assert!(!cmds.iter().any(|c| matches!(c, Command::Move { .. })));
    }

    /// Auto-surface fires only when embodied AND the avatar is gone from the snapshot — never
    /// otherwise. A surfaced player, or a live embodied avatar, must stay put.
    #[test]
    fn should_auto_surface_only_when_embodied_avatar_is_absent() {
        assert!(
            should_auto_surface(true, false),
            "embodied + avatar despawned → eject to command"
        );
        assert!(
            !should_auto_surface(true, true),
            "embodied + avatar alive → stay embodied"
        );
        assert!(
            !should_auto_surface(false, false),
            "not embodied → an unrelated absence never surfaces"
        );
        assert!(
            !should_auto_surface(false, true),
            "not embodied + present → nothing to do"
        );
    }

    /// WS-4 hit-feedback seam: `avatar_landed_hit` fires only when the player is embodied AND a
    /// `Damaged` event names the avatar as its `source` (its own shot connected). It must ignore
    /// damage dealt by OTHER units (no false hitmarker for an ally's kill) and damage TAKEN by the
    /// avatar (being shot is not "I hit him"), and never fire while commanding.
    #[test]
    fn avatar_landed_hit_fires_only_on_avatar_source_while_embodied() {
        use gonedark_core::fixed::Fixed;
        let avatar = Entity { index: 3, generation: 1 };
        let other = Entity { index: 9, generation: 0 };
        let target = Entity { index: 12, generation: 2 };
        let pos = Vec2::new(Fixed::from_int(4), Fixed::from_int(2));
        let dmg = |source: Entity, entity: Entity| SimEvent::Damaged {
            entity,
            faction: Faction::Enemy,
            source,
            amount: Fixed::from_int(5),
            pos,
        };

        // The canonical case: embodied, the avatar's own shot dealt damage.
        assert!(
            avatar_landed_hit(&[dmg(avatar, target)], avatar, true),
            "embodied + avatar is the damage source → hit lands"
        );
        // Damage from someone else (an ally / another unit) is NOT the player's hit.
        assert!(
            !avatar_landed_hit(&[dmg(other, target)], avatar, true),
            "another unit's damage must not register as the avatar's hit"
        );
        // Damage TAKEN by the avatar (avatar is the target, not the source) is not "I hit him".
        assert!(
            !avatar_landed_hit(&[dmg(other, avatar)], avatar, true),
            "being shot is not a landed hit"
        );
        // Same avatar-source event, but commanding (not embodied) → no embodied hit cue.
        assert!(
            !avatar_landed_hit(&[dmg(avatar, target)], avatar, false),
            "no hitmarker while commanding (the cue is an embodied-view affordance)"
        );
        // Mixed stream still detects the avatar's own hit among other events.
        assert!(
            avatar_landed_hit(
                &[
                    dmg(other, target),
                    SimEvent::Killed { entity: target, faction: Faction::Enemy, source: other, pos },
                    dmg(avatar, target),
                ],
                avatar,
                true
            ),
            "the avatar's hit is found among unrelated events"
        );
        // Empty stream / non-Damaged events → no hit.
        assert!(!avatar_landed_hit(&[], avatar, true));
        assert!(!avatar_landed_hit(
            &[SimEvent::UnitProduced { faction: Faction::Player, pos }],
            avatar,
            true
        ));
    }

    /// The crouch button toggles posture off the avatar's CURRENT sim state: standing → crouch,
    /// crouched → stand. No edge → no command, regardless of current posture.
    #[test]
    fn crouch_toggle_inverts_current_posture_only_on_an_edge() {
        let e = test_player();
        // No press edge → nothing, whatever the posture.
        assert!(crouch_toggle_command(e, false, false).is_none());
        assert!(crouch_toggle_command(e, false, true).is_none());
        // Standing + edge → crouch.
        match crouch_toggle_command(e, true, false) {
            Some(Command::Crouch { entity, crouched }) => {
                assert_eq!(entity, e);
                assert!(crouched, "standing → crouch");
            }
            other => panic!("expected a Crouch command, got {other:?}"),
        }
        // Crouched + edge → stand.
        match crouch_toggle_command(e, true, true) {
            Some(Command::Crouch { crouched, .. }) => assert!(!crouched, "crouched → stand"),
            other => panic!("expected a Crouch command, got {other:?}"),
        }
    }

    /// The engine→render touch-HUD mapping: an active stick + a crouched avatar produce a stick view
    /// and a lit Crouch toggle, and the button circles carry over from the layout in pixels.
    #[test]
    fn render_touch_hud_maps_layout_state_and_crouch_highlight() {
        let layout = touch_controls::TouchLayout::new(1280, 720);
        let hud = touch_controls::TouchHud {
            stick_active: true,
            stick_origin: (120.0, 600.0),
            stick_thumb: (150.0, 580.0),
            fire_pressed: true,
            crouch_pressed: false,
            reload_pressed: false,
            surface_pressed: false,
        };
        let r = render_touch_hud(&layout, &hud, (1280, 720), /* crouched = */ true);
        assert!(r.stick.is_some(), "active stick → a stick view");
        let s = r.stick.unwrap();
        assert_eq!((s.base_x, s.base_y), (120.0, 600.0));
        assert_eq!(s.radius, layout.stick_radius);
        assert!(r.fire.pressed, "held fire carries the pressed flash");
        assert!(r.crouch.active, "crouched avatar lights the Crouch toggle");
        assert!(!r.crouch.pressed);
        // Button circles pass straight through from the layout (pixels).
        assert_eq!((r.fire.cx, r.fire.cy, r.fire.r), (layout.fire.cx, layout.fire.cy, layout.fire.r));

        // No active stick → no stick view drawn.
        let hud2 = touch_controls::TouchHud::default();
        let r2 = render_touch_hud(&layout, &hud2, (1280, 720), false);
        assert!(r2.stick.is_none());
        assert!(!r2.crouch.active);
    }

    /// One-shot/edge commands force a sub-tick catch-up; held/continuous ones (locomote, fire) do
    /// not — the distinction that keeps avatar speed framerate-independent.
    #[test]
    fn only_oneshot_commands_force_a_subtick_step() {
        let e = test_player();
        let dir = Vec2::new(Fixed::ONE, Fixed::ZERO);
        assert!(is_oneshot_command(&Command::Embody { entity: e }));
        assert!(is_oneshot_command(&Command::Surface { entity: e }));
        assert!(is_oneshot_command(&Command::Move { entity: e, target: dir }));
        assert!(!is_oneshot_command(&Command::Locomote { entity: e, dir }));
        assert!(!is_oneshot_command(&Command::Fire { entity: e, dir }));
    }

    /// Headlessly replay the frame() accumulator + sub-tick bump + `drive_lockstep` path for one
    /// embodied unit holding "forward", returning how far it travels in `seconds` at `fps`. The
    /// distance MUST be framerate-independent (the bug fixed here scaled it with fps).
    fn held_locomotion_distance(fps: f32, seconds: f32) -> f32 {
        let mut sim = Sim::new(99);
        let e = sim.world.spawn();
        sim.world.kind[e.index as usize] = EntityKind::Unit;
        sim.world.faction[e.index as usize] = Faction::Player;
        let mut ls = Lockstep::new(SP_PEER_COUNT, SP_LOCAL, SP_DELAY);
        let tick_dt = 1.0 / TICK_HZ as f32;
        let dt = 1.0 / fps;
        let mut acc = 0.0f32;
        let mut embodied = false;
        let frames = (seconds * fps) as u32;
        for f in 0..frames {
            let mut commands: Vec<Command> = Vec::new();
            if f == 0 {
                commands.push(Command::Embody { entity: e });
                embodied = true;
            } else if embodied {
                // Held "W" (forward at yaw 0 → +x). Re-emitted every frame, exactly like the host.
                if let Some(c) = locomote::locomote_command(e, 0.0, (0.0, -1.0)) {
                    commands.push(c);
                }
            }
            acc += dt;
            let mut budget = 0u32;
            while acc >= tick_dt && budget < MAX_CATCHUP_STEPS {
                acc -= tick_dt;
                budget += 1;
            }
            if budget == MAX_CATCHUP_STEPS && acc >= tick_dt {
                acc = 0.0;
            }
            if budget == 0 && commands.iter().any(is_oneshot_command) {
                budget = 1;
            }
            drive_lockstep(&mut sim, &mut ls, None, commands, budget, |s, m| s.step(m));
        }
        fixed_to_f32(sim.world.pos[e.index as usize].x)
    }

    #[test]
    fn held_locomotion_speed_is_framerate_independent() {
        let d60 = held_locomotion_distance(60.0, 1.0);
        let d120 = held_locomotion_distance(120.0, 1.0);
        let d240 = held_locomotion_distance(240.0, 1.0);
        // ~7.5 wu over 1s at the 60 Hz tick; every framerate lands within one tick's worth (1/8 wu).
        let tol = 0.2;
        assert!(
            (d60 - d120).abs() < tol && (d120 - d240).abs() < tol,
            "avatar speed must not scale with fps: 60={d60} 120={d120} 240={d240}"
        );
        assert!(d60 > 7.0 && d60 < 7.6, "and the 60 Hz baseline is ~7.5 wu/s: {d60}");
    }

    /// Mouse-look must not be inverted: a rightward delta (`look_dx > 0`) turns the view to the
    /// player's right. With look dir `(cos yaw, sin yaw)` and screen-right = world −Y, "turn right"
    /// means the heading rotates toward −Y, i.e. `sin(yaw)` goes negative. A leftward delta mirrors.
    #[test]
    fn look_is_not_inverted() {
        let right = integrate_look_yaw(0.0, 10.0);
        assert!(right < 0.0, "rightward mouse decreases yaw: {right}");
        assert!(right.sin() < 0.0, "view heading turns toward world −Y (screen right)");

        let left = integrate_look_yaw(0.0, -10.0);
        assert!(left > 0.0, "leftward mouse increases yaw: {left}");
        assert!(left.sin() > 0.0, "view heading turns toward world +Y (screen left)");

        assert_eq!(integrate_look_yaw(1.234, 0.0), 1.234, "no delta → yaw unchanged");
    }

    /// Vertical look must work, be non-inverted, and clamp shy of vertical. winit screen +Y is down,
    /// so a mouse-UP delta is negative `look_dy` → pitch increases (look up); down mirrors.
    #[test]
    fn pitch_look_is_non_inverted_and_clamped() {
        let up = integrate_look_pitch(0.0, -10.0);
        assert!(up > 0.0, "mouse up tilts the view up (pitch +): {up}");
        let down = integrate_look_pitch(0.0, 10.0);
        assert!(down < 0.0, "mouse down tilts the view down (pitch −): {down}");
        assert_eq!(integrate_look_pitch(0.3, 0.0), 0.3, "no delta → pitch unchanged");
        // Clamp: a huge up/down delta saturates at ±MAX, never flipping past vertical.
        assert_eq!(integrate_look_pitch(0.0, -100_000.0), EMBODIED_PITCH_MAX);
        assert_eq!(integrate_look_pitch(0.0, 100_000.0), -EMBODIED_PITCH_MAX);
    }

    /// Pitch actually steers the embodied camera: a point straight ahead at EYE LEVEL sits at screen
    /// centre when looking level, drops below centre when you pitch UP, and rises above it when you
    /// pitch DOWN — so its NDC y strictly decreases as pitch increases.
    #[test]
    fn embodied_pitch_changes_the_view_direction() {
        let (w, h) = (800u32, 600u32);
        // Straight ahead (+x) at eye height — dead centre at pitch 0.
        let ahead = Vec3::new(10.0, 0.0, EYE_HEIGHT);
        let clip_y = |pitch: f32| {
            let vp = embodied_view_proj(0.0, 0.0, 0.0, pitch, w, h);
            let c = vp * Vec4::new(ahead.x, ahead.y, ahead.z, 1.0);
            c.y / c.w
        };
        assert!(clip_y(0.0).abs() < 1e-4, "level look centres an eye-level point ahead");
        assert!(
            clip_y(0.6) < clip_y(0.0) && clip_y(0.0) < clip_y(-0.6),
            "pitch up drops the point below centre, pitch down raises it: up={} level={} down={}",
            clip_y(0.6),
            clip_y(0.0),
            clip_y(-0.6)
        );
    }

    // --- WS-3: embodied input-pipeline INTEGRATION tests ------------------------------------------
    //
    // These exercise the REAL mouse/key → `Command::Fire` composition the combat harnesses skip
    // (they construct `Command::Fire { dir }` directly). They drive the same seams `frame` wires —
    // `integrate_look_yaw` → `embodied_input_commands` (→ `fire::fire_command`) and the camera
    // `embodied_view_proj` — so a regression in the aim convention or trigger mapping fails here, with
    // no GPU/display. Host-side floats are intentional (this crate is not the sim; the only value that
    // crosses into `core` is the `Fixed`-quantized aim, invariant #1 unaffected).

    /// One Fixed quantisation step, in world units — the tolerance a `(cos, sin)` aim is preserved to
    /// at the float→`Fixed` boundary (`fire::fire_command`). A couple of steps of slack absorbs the
    /// round-to-nearest on both components.
    const QUANT_TOL: f32 = 2.0 / gonedark_core::fixed::Fixed::SCALE as f32;

    /// Pull the `(x, y)` aim out of the single `Command::Fire` an embodied frame emitted, asserting it
    /// targets `player`. Panics if the frame emitted no fire (the composition under test should).
    fn fire_dir_of(out: &EmbodiedCommands, player: Entity) -> (f32, f32) {
        let dir = out
            .commands
            .iter()
            .find_map(|c| match c {
                Command::Fire { entity, dir } => {
                    assert_eq!(*entity, player, "the Fire must target the possessed unit");
                    Some(*dir)
                }
                _ => None,
            })
            .expect("a held trigger must emit exactly one Command::Fire");
        (fixed_to_f32(dir.x), fixed_to_f32(dir.y))
    }

    /// A held-fire `InputFrame` at a given yaw emits a `Command::Fire` whose quantized `dir` matches
    /// the yaw's unit vector `(cos yaw, sin yaw)` to the `Fixed` tolerance — across several headings,
    /// through the real `embodied_input_commands` seam `frame` calls. A released trigger emits none.
    #[test]
    fn held_fire_emits_aim_matching_yaw_through_the_seam() {
        let player = test_player();
        for &yaw in &[0.0_f32, 0.5, 1.0, 2.3, -1.2, 3.0, std::f32::consts::FRAC_PI_2] {
            let out = embodied_input_commands(
                player, yaw, (0.0, 0.0), true, false, false, false, false, true,
            );
            assert!(out.fired, "a held trigger sets the muzzle-flash `fired` flag");
            let (ax, ay) = fire_dir_of(&out, player);
            assert!(
                (ax - yaw.cos()).abs() < QUANT_TOL && (ay - yaw.sin()).abs() < QUANT_TOL,
                "yaw {yaw}: aim ({ax}, {ay}) must match (cos, sin) = ({}, {})",
                yaw.cos(),
                yaw.sin(),
            );
        }

        // Trigger released → no Fire command, no muzzle-flash stamp.
        let none = embodied_input_commands(
            player, 1.0, (0.0, 0.0), false, false, false, false, false, true,
        );
        assert!(!none.fired);
        assert!(!none.commands.iter().any(|c| matches!(c, Command::Fire { .. })));
    }

    /// THE load-bearing guarantee behind the targeting report: the embodied camera's forward
    /// (`embodied_view_proj`'s look dir) AGREES with the `Command::Fire` aim. A world point placed at
    /// the muzzle range *along the fire dir* must project to screen centre (NDC ≈ origin) under the
    /// camera built from the SAME yaw — i.e. you hit what's under the crosshair. Swept across yaws so
    /// an axis swap / sign flip / convention drift between the two seams fails loudly.
    #[test]
    fn camera_forward_agrees_with_fire_direction() {
        let player = test_player();
        let (w, h) = (800u32, 600u32);
        for &yaw in &[0.0_f32, 0.4, 1.1, 2.0, -0.8, 3.1, -2.7] {
            // Integrate a (zero) look delta exactly as `frame` does, then compose the fire command —
            // the aim the sim will act on.
            let yaw = integrate_look_yaw(yaw, 0.0);
            let out = embodied_input_commands(
                player, yaw, (0.0, 0.0), true, false, false, false, false, true,
            );
            let (ax, ay) = fire_dir_of(&out, player);

            // Eye at an arbitrary spot; the camera looks level (pitch 0). A target one muzzle-range
            // out ALONG THE FIRE DIR, at eye height, must sit dead centre.
            let (ex, ey) = (3.0_f32, -2.0_f32);
            let dist = 12.0_f32;
            let vp = embodied_view_proj(ex, ey, yaw, 0.0, w, h);
            let clip = vp * Vec4::new(ex + ax * dist, ey + ay * dist, EYE_HEIGHT, 1.0);
            assert!(clip.w > 0.0, "yaw {yaw}: the aimed point must be in front of the camera");
            let (ndc_x, ndc_y) = (clip.x / clip.w, clip.y / clip.w);
            assert!(
                ndc_x.abs() < 1e-2 && ndc_y.abs() < 1e-2,
                "yaw {yaw}: the fire dir must land under the crosshair, got NDC ({ndc_x}, {ndc_y})",
            );

            // Negative control: the OPPOSITE bearing is behind the camera, never under the crosshair.
            let behind = vp * Vec4::new(ex - ax * dist, ey - ay * dist, EYE_HEIGHT, 1.0);
            assert!(
                behind.w <= 0.0 || (behind.x / behind.w).abs() > 0.5,
                "yaw {yaw}: aiming backwards must NOT centre on screen",
            );
        }
    }

    /// The look convention end to end, not just per-fn: a rightward look delta (`look_dx > 0`) swings
    /// the *fire aim* toward world −Y (screen-right), a leftward delta toward +Y — `integrate_look_yaw`
    /// composed with `fire::fire_command` via the real seam. Mirrors `frame`'s order (integrate, then
    /// emit) and the standalone `look_is_not_inverted` per-fn test.
    #[test]
    fn rightward_look_aims_toward_minus_y_through_the_pipeline() {
        let player = test_player();
        let aim_after_look = |look_dx: f32| {
            let yaw = integrate_look_yaw(0.0, look_dx);
            let out = embodied_input_commands(
                player, yaw, (0.0, 0.0), true, false, false, false, false, true,
            );
            fire_dir_of(&out, player)
        };

        // From level (yaw 0, aim +X): rightward look turns the aim toward −Y, still mostly forward.
        let (rx, ry) = aim_after_look(50.0);
        assert!(rx > 0.0 && ry < 0.0, "rightward look aims forward-and-right (−Y): ({rx}, {ry})");
        // Leftward mirrors: toward +Y.
        let (lx, ly) = aim_after_look(-50.0);
        assert!(lx > 0.0 && ly > 0.0, "leftward look aims forward-and-left (+Y): ({lx}, {ly})");
    }

    /// Crouch (tighter cone) at the COMPOSITION level: the real input seams feed `core::combat`. The
    /// aim comes from `fire::fire_command` and the posture flip from `crouch_toggle_command`; an
    /// off-axis enemy inside the ~30° standing cone but outside the ~18° crouched one is clipped
    /// standing and missed crouched. This stitches the input pipeline to the sim's cone resolution
    /// (the combat harness's `crouched_cone_is_tighter` test drives `resolve_fire` with a hand-built
    /// `dir`; this one proves the host's fire/crouch seams produce that behaviour).
    #[test]
    fn crouch_tightens_the_cone_through_the_input_pipeline() {
        use gonedark_core::components::{Faction, InputSource, Posture, Vec2};
        use gonedark_core::fixed::Fixed;

        let mut world = World::new();
        let terrain = gonedark_core::terrain::Terrain::open();

        // Embodied player rifleman at the origin, armed from the shared stats. Disable the magazine
        // and clear cooldown so the ONLY variable between the two shots is posture → cone width.
        let shooter = world.spawn();
        let si = shooter.index as usize;
        let (health, weapon) = economy::unit_stats(UnitKind::Rifleman);
        world.faction[si] = Faction::Player;
        world.health[si] = health;
        world.weapon[si] = weapon;
        world.weapon[si].mag_size = 0;
        world.input_source[si] = InputSource::Embodied;

        // Off-axis enemy at (10, 5) — bearing ≈ 26.6° off the +X aim: inside the wide standing cone,
        // outside the tight crouched one. Distance ≈ 11.18 < range 14 (and < crouched 17.5), so range
        // is never the limiter — the cone is.
        let enemy = world.spawn();
        let ei = enemy.index as usize;
        let (eh, _) = economy::unit_stats(UnitKind::Rifleman);
        world.faction[ei] = Faction::Enemy;
        world.pos[ei] = Vec2::new(Fixed::from_int(10), Fixed::from_int(5));
        world.health[ei] = eh;
        let full = world.health[ei].cur;

        // The REAL input seam produces the aim from yaw 0 (aim +X).
        let Command::Fire { dir, .. } = fire::fire_command(shooter, 0.0, true).unwrap() else {
            panic!("a held trigger emits Fire");
        };

        // Standing: the wide hip-fire cone clips the off-axis enemy.
        world.posture[si] = Posture::Standing;
        world.weapon[si].cooldown_left = 0;
        let mut events = Vec::new();
        gonedark_core::combat::resolve_fire(&mut world, &terrain, si, dir, &mut events);
        assert!(
            world.health[ei].cur < full,
            "standing cone is wide enough to clip the 26.6°-off enemy",
        );

        // Crouch via the real toggle seam (Standing → Crouched), reset the target + weapon, fire again.
        let Command::Crouch { crouched, .. } = crouch_toggle_command(shooter, true, false).unwrap()
        else {
            panic!("a crouch edge emits Crouch");
        };
        assert!(crouched, "toggling off Standing crouches");
        world.posture[si] = Posture::Crouched;
        world.health[ei].cur = full;
        world.weapon[si].cooldown_left = 0;
        events.clear();
        gonedark_core::combat::resolve_fire(&mut world, &terrain, si, dir, &mut events);
        assert_eq!(
            world.health[ei].cur, full,
            "crouch tightens the cone past the off-axis bearing — same aim now misses",
        );
        assert!(events.is_empty(), "a missed crouched shot deals no damage");
    }

    /// Command pan maps the screen stick to world ground motion: `D` (+mx) pans +X, `W` (−my) pans
    /// +Y (north), and the step scales with both `dt` and the zoom half-extent so the felt velocity
    /// is constant across zoom.
    #[test]
    fn pan_focus_moves_with_the_stick_and_scales_with_zoom() {
        // D held for the frame: focus slides +X, Y untouched.
        let (x, y) = pan_focus(0.0, 0.0, (1.0, 0.0), TOPDOWN_HALF_EXTENT, 0.1);
        assert!(x > 0.0 && (y - 0.0).abs() < 1e-6, "D pans +X only: ({x}, {y})");

        // W is screen-up = −my: north is +Y.
        let (_, ny) = pan_focus(0.0, 0.0, (0.0, -1.0), TOPDOWN_HALF_EXTENT, 0.1);
        assert!(ny > 0.0, "W pans +Y (north): {ny}");

        // S (+my) pans −Y, A (−mx) pans −X — opposite signs.
        let (sx, sy) = pan_focus(0.0, 0.0, (-1.0, 1.0), TOPDOWN_HALF_EXTENT, 0.1);
        assert!(sx < 0.0 && sy < 0.0, "A/S pan −X/−Y: ({sx}, {sy})");

        // Zoomed out (larger half-extent) sweeps more ground for the same stick + dt.
        let near = pan_focus(0.0, 0.0, (1.0, 0.0), 20.0, 0.1).0;
        let far = pan_focus(0.0, 0.0, (1.0, 0.0), 80.0, 0.1).0;
        assert!(far > near, "pan speed scales with zoom: far {far} > near {near}");

        // Neutral stick is a no-op.
        assert_eq!(pan_focus(3.0, 5.0, (0.0, 0.0), 40.0, 0.1), (3.0, 5.0));
    }

    /// Wheel zoom shrinks the half-extent on a positive (zoom-in) notch, grows it on a negative one,
    /// and clamps hard to the configured band so it can never invert or run away.
    #[test]
    fn zoom_half_extent_is_geometric_and_clamped() {
        let start = 40.0;
        let zin = zoom_half_extent(start, 1.0);
        assert!(zin < start, "positive scroll zooms in (smaller extent): {zin}");
        let zout = zoom_half_extent(start, -1.0);
        assert!(zout > start, "negative scroll zooms out (larger extent): {zout}");

        // Geometric/symmetric: one notch in then one out returns (within fp) to start.
        let roundtrip = zoom_half_extent(zin, -1.0);
        assert!((roundtrip - start).abs() < 1e-3, "in then out round-trips: {roundtrip}");

        // Clamps: a huge zoom-in floors at MIN, a huge zoom-out ceils at MAX.
        assert_eq!(zoom_half_extent(start, 100.0), CAM_HALF_EXTENT_MIN);
        assert_eq!(zoom_half_extent(start, -100.0), CAM_HALF_EXTENT_MAX);
        assert_eq!(zoom_half_extent(start, 0.0), start, "no scroll → unchanged");
    }

    /// Pan is a rigid translation, so the command projection stays axis-separable at a non-zero
    /// focus: ground points sharing world-X still share screen-X (and world-Y → screen-Y). This is
    /// the load-bearing property band-select relies on (mirrors the origin-focus test below).
    #[test]
    fn panned_command_view_stays_axis_separable() {
        let (w, h) = (1280u32, 720u32);
        let vp = topdown_view_proj(w, h, 17.0, -9.0, 55.0);
        let clip = |x: f32, y: f32| {
            let c = vp * Vec4::new(x, y, 0.0, 1.0);
            (c.x / c.w, c.y / c.w)
        };
        // Same world-X, different world-Y → identical screen-X.
        let (sx0, _) = clip(20.0, -30.0);
        let (sx1, _) = clip(20.0, 25.0);
        assert!((sx0 - sx1).abs() < 1e-5, "world-X alone fixes screen-X: {sx0} vs {sx1}");
        // Same world-Y, different world-X → identical screen-Y.
        let (_, sy0) = clip(-40.0, 12.0);
        let (_, sy1) = clip(33.0, 12.0);
        assert!((sy0 - sy1).abs() < 1e-5, "world-Y alone fixes screen-Y: {sy0} vs {sy1}");
    }

    // ---- lockstep drive seam (D27 step 4) ----

    const DRIVE_SEED: u64 = 0x5EED_1234_ABCD_F00D;

    /// A small two-faction scene built into `sim`, returning a handle to drive. Spawn order is
    /// fixed, so the handles are bit-identical across every sim built this way — exactly the
    /// determinism the lockstep path leans on.
    fn drive_scene(sim: &mut Sim) -> Entity {
        sim.resources = Resources::new(100_000);
        let (health, weapon) = economy::unit_stats(UnitKind::Rifleman);
        let mut spawn = |x: i32, y: i32, faction: Faction| {
            let e = sim.world.spawn();
            let i = e.index as usize;
            sim.world.kind[i] = EntityKind::Unit;
            sim.world.faction[i] = faction;
            sim.world.pos[i] = Vec2::new(Fixed::from_int(x), Fixed::from_int(y));
            sim.world.health[i] = health;
            sim.world.weapon[i] = weapon;
            sim.world.stance[i] = Stance::FireAtWill;
            e
        };
        let player = spawn(-5, 0, Faction::Player);
        let _ = spawn(-5, 3, Faction::Player);
        let _ = spawn(5, 0, Faction::Enemy);
        let _ = spawn(5, 3, Faction::Enemy);
        economy::build(
            &mut sim.world,
            &mut sim.resources,
            Faction::Player,
            BuildingKind::Camp,
            Vec2::new(Fixed::from_int(-20), Fixed::from_int(20)),
        )
        .expect("camp affordable at 100k");
        player
    }

    /// A scripted sequence of per-frame `(commands, budget)` — a few active frames with commands,
    /// catch-up frames advancing several ticks at once, and quiet frames. Mirrors what the
    /// accumulator hands `drive_lockstep`: commands ride the FIRST tick of a frame, catch-up ticks
    /// are empty.
    fn drive_script(player: Entity) -> Vec<(Vec<Command>, u32)> {
        let v = |x: i32, y: i32| Vec2::new(Fixed::from_int(x), Fixed::from_int(y));
        vec![
            // Frame 0: one tick, a Move command on it.
            (
                vec![Command::Move {
                    entity: player,
                    target: v(3, 1),
                }],
                1,
            ),
            // Frame 1: quiet single tick.
            (Vec::new(), 1),
            // Frame 2: a catch-up frame — 3 ticks, AttackMove on the first, empty after.
            (
                vec![Command::AttackMove {
                    entity: player,
                    target: v(4, 0),
                }],
                3,
            ),
            // Frame 3: budget 0 + a command → the sub-tick fallback raises it to 1 (Embody).
            (vec![Command::Embody { entity: player }], 0),
            // Frame 4: several quiet catch-up ticks.
            (Vec::new(), 4),
            // Frame 5: Surface, single tick.
            (vec![Command::Surface { entity: player }], 1),
            // Frame 6: budget 0, no commands → advances nothing.
            (Vec::new(), 0),
            // Frame 7: a longer quiet stretch.
            (Vec::new(), 6),
        ]
    }

    /// Step `sim` directly the way the OLD accumulator did: commands on the first tick of the
    /// frame, `&[]` on catch-up ticks; the sub-tick fallback forces one tick when budget 0 carries
    /// commands. Returns the per-tick checksum stream — the reference the lockstep path must match.
    fn direct_reference(script: &[(Vec<Command>, u32)]) -> Vec<u64> {
        let mut sim = Sim::new(DRIVE_SEED);
        let _ = drive_scene(&mut sim);
        let mut sums = Vec::new();
        for (commands, budget) in script {
            let budget = if *budget == 0 && !commands.is_empty() {
                1
            } else {
                *budget
            };
            let mut first = true;
            for _ in 0..budget {
                if first {
                    sim.step(commands);
                    first = false;
                } else {
                    sim.step(&[]);
                }
                sums.push(sim.checksum());
            }
        }
        sums
    }

    /// Drive the SAME scripted sequence through a fresh single-player `Lockstep` via the seam,
    /// optionally with a `transport` present, collecting the per-tick checksum stream.
    fn lockstep_stream(script: &[(Vec<Command>, u32)], with_transport: bool) -> Vec<u64> {
        let mut sim = Sim::new(DRIVE_SEED);
        let _ = drive_scene(&mut sim);
        let mut ls = Lockstep::new(SP_PEER_COUNT, SP_LOCAL, SP_DELAY);
        let mut null = NullTransport;
        let mut sums = Vec::new();
        for (commands, budget) in script {
            let budget = if *budget == 0 && !commands.is_empty() {
                1
            } else {
                *budget
            };
            let transport: Option<&mut (dyn Transport + '_)> = if with_transport {
                Some(&mut null)
            } else {
                None
            };
            drive_lockstep(
                &mut sim,
                &mut ls,
                transport,
                commands.clone(),
                budget,
                |sim, merged| {
                    sim.step(merged);
                    sums.push(sim.checksum());
                },
            );
        }
        sums
    }

    /// THE load-bearing guard: the single-player lockstep path (peer_count=1, delay=0) produces a
    /// checksum stream bit-identical to stepping the sim directly with the same per-frame commands
    /// (invariant #7). If lockstep stamped, gated, or merged differently, this diverges.
    #[test]
    fn lockstep_single_player_matches_direct_stepping() {
        let player = drive_scene(&mut Sim::new(DRIVE_SEED));
        let script = drive_script(player);
        let reference = direct_reference(&script);
        let through = lockstep_stream(&script, /* with_transport = */ false);
        assert!(!reference.is_empty(), "the script must advance some ticks");
        assert_eq!(
            through, reference,
            "single-player lockstep stream must equal direct stepping"
        );
    }

    /// Catch-up (a frame advancing multiple ticks: commands on the first, empty after) matches
    /// direct stepping — isolates the multi-tick-per-frame path the headline test also covers.
    #[test]
    fn lockstep_catch_up_frame_matches_direct_stepping() {
        let player = drive_scene(&mut Sim::new(DRIVE_SEED));
        let cmd = Command::AttackMove {
            entity: player,
            target: Vec2::new(Fixed::from_int(4), Fixed::from_int(0)),
        };
        // One frame, five ticks: the command rides tick 0, the next four are empty.
        let script = vec![(vec![cmd], 5u32)];
        assert_eq!(
            lockstep_stream(&script, false),
            direct_reference(&script),
            "a 5-tick catch-up frame must match direct stepping"
        );
        // And the stream is genuinely five ticks long.
        assert_eq!(lockstep_stream(&script, false).len(), 5);
    }

    /// The transport-present path (here a no-op `NullTransport`: drains/sends our echoes, polls
    /// nothing) advances identically — the single-peer gate clears on local input regardless, so
    /// pumping a transport must not change the stream. Exercises the `Some(transport)` branch.
    #[test]
    fn lockstep_with_null_transport_matches_no_transport() {
        let player = drive_scene(&mut Sim::new(DRIVE_SEED));
        let script = drive_script(player);
        assert_eq!(
            lockstep_stream(&script, /* with_transport = */ true),
            lockstep_stream(&script, /* with_transport = */ false),
            "a no-op transport must not change the single-player stream"
        );
    }

    /// `drive_lockstep` returns the number of ticks advanced and equals the budget for the
    /// single-player session (the gate never stalls with one peer at delay 0).
    #[test]
    fn drive_lockstep_advances_exactly_the_budget() {
        let mut sim = Sim::new(DRIVE_SEED);
        let player = drive_scene(&mut sim);
        let mut ls = Lockstep::new(SP_PEER_COUNT, SP_LOCAL, SP_DELAY);
        let advanced = drive_lockstep(
            &mut sim,
            &mut ls,
            None,
            vec![Command::Move {
                entity: player,
                target: Vec2::new(Fixed::from_int(1), Fixed::from_int(1)),
            }],
            3,
            |sim, merged| {
                sim.step(merged);
            },
        );
        assert_eq!(advanced, 3, "single-player advances exactly the budget");
        assert_eq!(ls.next_tick(), 3, "three ticks executed");
    }

    /// Budget 0 with no commands advances nothing and leaves the lockstep clock untouched.
    #[test]
    fn drive_lockstep_zero_budget_no_commands_is_a_noop() {
        let mut sim = Sim::new(DRIVE_SEED);
        let _ = drive_scene(&mut sim);
        let mut ls = Lockstep::new(SP_PEER_COUNT, SP_LOCAL, SP_DELAY);
        let before = sim.checksum();
        let advanced = drive_lockstep(&mut sim, &mut ls, None, Vec::new(), 0, |sim, merged| {
            sim.step(merged);
        });
        assert_eq!(advanced, 0);
        assert_eq!(ls.next_tick(), 0, "no tick executed");
        assert_eq!(sim.checksum(), before, "sim untouched");
    }

    // --- Avatar-local prediction (D15, workstream B step 5) ---

    #[test]
    fn extrapolate_avatar_leads_along_velocity() {
        // Position carried forward by velocity × lead time.
        assert_eq!(extrapolate_avatar((1.0, 2.0), (3.0, -1.0), 0.5), (2.5, 1.5));
        // Zero velocity (an embodied unit holding position) → no lead, eye sits on the unit.
        assert_eq!(extrapolate_avatar((4.0, 4.0), (0.0, 0.0), 1.0), (4.0, 4.0));
        // Zero lead → identity.
        assert_eq!(
            extrapolate_avatar((7.0, -3.0), (9.0, 9.0), 0.0),
            (7.0, -3.0)
        );
    }

    #[test]
    fn reconcile_avatar_eases_toward_target() {
        // Within the snap distance, ease by `smoothing` — halfway at 0.5, then closer again.
        let a = reconcile_avatar((0.0, 0.0), (4.0, 0.0), 0.5, 100.0);
        assert_eq!(a, (2.0, 0.0));
        let b = reconcile_avatar(a, (4.0, 0.0), 0.5, 100.0);
        assert_eq!(b, (3.0, 0.0));
        // It converges toward, never past, the target (no overshoot).
        assert!(b.0 < 4.0 && b.0 > a.0);
    }

    #[test]
    fn reconcile_avatar_snaps_past_threshold() {
        // Error ≥ snap_dist → snap straight to the target (a big correction resolves at once,
        // rather than sliding across the world).
        let snapped = reconcile_avatar((0.0, 0.0), (10.0, 0.0), 0.5, 5.0);
        assert_eq!(snapped, (10.0, 0.0));
        // Exactly at the threshold also snaps (>= boundary).
        assert_eq!(
            reconcile_avatar((0.0, 0.0), (5.0, 0.0), 0.5, 5.0),
            (5.0, 0.0)
        );
    }

    #[test]
    fn reconcile_avatar_clamps_smoothing() {
        // A smoothing > 1 is clamped to 1 — reach the target in one step (within snap dist),
        // never overshoot past it.
        assert_eq!(
            reconcile_avatar((0.0, 0.0), (3.0, 0.0), 2.0, 100.0),
            (3.0, 0.0)
        );
        // A negative smoothing clamps to 0 — hold position.
        assert_eq!(
            reconcile_avatar((1.0, 1.0), (3.0, 3.0), -1.0, 100.0),
            (1.0, 1.0)
        );
    }

    #[test]
    fn avatar_prediction_anchors_then_reconciles_and_clears() {
        let mut p = AvatarPrediction::default();
        assert!(!p.valid, "starts invalid");
        // First embodied frame ANCHORS to the extrapolated target (no ease-in from origin).
        p.update((10.0, 0.0), (0.0, 0.0), 0.5);
        assert!(p.valid);
        assert_eq!(p.eye, (10.0, 0.0), "first frame anchors exactly");
        // A subsequent frame with a moved authoritative target reconciles (eases), not snaps.
        p.update((12.0, 0.0), (0.0, 0.0), 0.0);
        assert_eq!(p.eye, (11.0, 0.0), "eases halfway toward the new target");
        // Clearing (surfacing) resets so the next embody re-anchors.
        p.clear();
        assert!(!p.valid);
        p.update((-3.0, 7.0), (0.0, 0.0), 0.0);
        assert_eq!(p.eye, (-3.0, 7.0), "re-anchors after clear");
    }

    /// THE load-bearing guard (invariant #1, D15): running avatar prediction exactly as `frame`
    /// does — reading each tick's snapshot and updating — must leave the sim's per-tick checksum
    /// stream **byte-identical** to a run that never touches prediction. Prediction is
    /// presentation-only and structurally cannot reach `&mut Sim`; this fails loudly if that ever
    /// regresses (e.g. someone threads the sim into the prediction path).
    #[test]
    fn avatar_prediction_never_perturbs_the_sim_checksum() {
        fn run(with_prediction: bool) -> Vec<u64> {
            let mut sim = Sim::new(DRIVE_SEED);
            let player = drive_scene(&mut sim);
            // Give the avatar a move order so it actually carries velocity for the prediction
            // to read (a non-trivial input to the seam, not a frozen zero).
            sim.step(&[Command::Move {
                entity: player,
                target: Vec2::new(Fixed::from_int(12), Fixed::from_int(0)),
            }]);
            let mut pred = AvatarPrediction::default();
            let tick_dt = 1.0 / TICK_HZ as f32;
            let mut stream = vec![sim.checksum()];
            for _ in 0..120 {
                sim.step(&[]);
                if with_prediction {
                    let snap = sim.snapshot();
                    if let Some(u) = snap.units.iter().find(|u| u.entity_index == player.index) {
                        let pos = (fixed_to_f32(u.pos.x), fixed_to_f32(u.pos.y));
                        let vel = (fixed_to_f32(u.vel.x), fixed_to_f32(u.vel.y));
                        pred.update(pos, vel, 0.5 * tick_dt);
                    }
                }
                stream.push(sim.checksum());
            }
            assert!(pred.eye.0.is_finite(), "prediction produced a usable eye");
            stream
        }
        assert_eq!(
            run(true),
            run(false),
            "avatar prediction must not perturb the deterministic sim"
        );
    }

    // --- in-session shell overlay mapping (Phase 4 WS-B) ---

    use gonedark_core::shell::{MatchOutcome, SessionAction};
    use session_shell::{assemble_summary, EndStateRead};

    fn empty_reads() -> [EndStateRead; gonedark_core::components::FACTION_COUNT] {
        Default::default()
    }

    #[test]
    fn overlay_for_surface_maps_each_surface() {
        // Playing → no overlay.
        assert_eq!(overlay_for_surface(&ShellSurface::Playing), Overlay::None);
        // Paused → the pause overlay.
        assert_eq!(overlay_for_surface(&ShellSurface::Paused), Overlay::Paused);
        // Reconnect prompt: stalled vs desynced map to the prompt severity.
        assert_eq!(
            overlay_for_surface(&ShellSurface::ReconnectPrompt(LinkState::Reconnecting)),
            Overlay::ReconnectPrompt { desynced: false }
        );
        assert_eq!(
            overlay_for_surface(&ShellSurface::ReconnectPrompt(LinkState::Desynced)),
            Overlay::ReconnectPrompt { desynced: true }
        );
    }

    /// The pause-key trigger (`Game::toggle_pause`'s only logic): Playing opens the menu, Paused
    /// closes it, and the terminal/blocking surfaces refuse — they own the screen and are dismissed
    /// by their own buttons, not Esc. This is the seam that closes "pause + in-match surrender" —
    /// once Paused is reachable, the existing `overlay_click_action` slots already reach Surrender.
    #[test]
    fn pause_toggle_action_maps_each_surface() {
        assert_eq!(
            pause_toggle_action(&ShellSurface::Playing),
            Some(SessionAction::Pause),
            "the pause key opens the menu while playing"
        );
        assert_eq!(
            pause_toggle_action(&ShellSurface::Paused),
            Some(SessionAction::Resume),
            "the pause key closes the menu while paused"
        );
        // An ended match: its summary owns the screen (Dismiss-only), never re-pausable.
        let summary = assemble_summary(&[], 0, MatchOutcome::Draw, &empty_reads());
        assert_eq!(pause_toggle_action(&ShellSurface::Ended(summary)), None);
        // A reconnect prompt is dismissed by its own Resume/leave buttons, not the pause key.
        assert_eq!(
            pause_toggle_action(&ShellSurface::ReconnectPrompt(LinkState::Reconnecting)),
            None
        );
        assert_eq!(
            pause_toggle_action(&ShellSurface::ReconnectPrompt(LinkState::Desynced)),
            None
        );
    }

    /// The host's overlay-active predicate (cursor-freeing + world-input-freezing): false only on
    /// the plain `Playing` surface, true on every overlay. The negation + every discriminant matters
    /// because the host frees the cursor / freezes input on the truthy branch.
    #[test]
    fn overlay_active_is_true_for_every_non_playing_surface() {
        assert!(!overlay_active(&ShellSurface::Playing), "playing has no overlay");
        assert!(overlay_active(&ShellSurface::Paused));
        assert!(overlay_active(&ShellSurface::ReconnectPrompt(LinkState::Reconnecting)));
        assert!(overlay_active(&ShellSurface::ReconnectPrompt(LinkState::Desynced)));
        let summary = assemble_summary(&[], 0, MatchOutcome::Draw, &empty_reads());
        assert!(overlay_active(&ShellSurface::Ended(summary)));
    }

    #[test]
    fn overlay_for_surface_ended_carries_the_summary() {
        let summary = assemble_summary(&[], 1234, MatchOutcome::Draw, &empty_reads());
        match overlay_for_surface(&ShellSurface::Ended(summary.clone())) {
            Overlay::Summary(s) => assert_eq!(s, summary),
            other => panic!("Ended must map to Overlay::Summary, got {other:?}"),
        }
    }

    /// Each surface's button slots resolve to the right host action — and the post-match summary's
    /// lone slot resolves to `Dismiss` (the reported "dismiss button does nothing" path: the click
    /// must produce an actionable result, not `None`).
    #[test]
    fn overlay_click_action_maps_each_slot() {
        let summary = assemble_summary(&[], 0, MatchOutcome::Draw, &empty_reads());
        // Pause: slot 0 resumes, slot 1 surrenders.
        assert_eq!(
            overlay_click_action(&Overlay::Paused, 0),
            Some(OverlayClick::Session(SessionAction::Resume))
        );
        assert_eq!(
            overlay_click_action(&Overlay::Paused, 1),
            Some(OverlayClick::Session(SessionAction::Surrender))
        );
        // Reconnect prompt: same Resume / leave vocabulary.
        assert_eq!(
            overlay_click_action(&Overlay::ReconnectPrompt { desynced: true }, 0),
            Some(OverlayClick::Session(SessionAction::Resume))
        );
        assert_eq!(
            overlay_click_action(&Overlay::ReconnectPrompt { desynced: false }, 1),
            Some(OverlayClick::Session(SessionAction::Surrender))
        );
        // Post-match summary: the single DISMISS button.
        assert_eq!(
            overlay_click_action(&Overlay::Summary(summary), 0),
            Some(OverlayClick::Dismiss)
        );
        // No overlay, and out-of-range slots, resolve to nothing (never a wrong action).
        assert_eq!(overlay_click_action(&Overlay::None, 0), None);
        assert_eq!(overlay_click_action(&Overlay::Paused, 2), None);
    }

    /// `Game::overlay_click` ties the geometry seam to the action map: a click on the live overlay's
    /// drawn button center resolves; a miss does not. Exercised on the terminal post-match summary
    /// (the reported broken path) without constructing a GPU `Game` — `overlay_click` only reads the
    /// shell surface, so we drive the same two pure seams it composes.
    #[test]
    fn overlay_click_resolves_summary_dismiss_at_button_center() {
        let summary = assemble_summary(&[], 0, MatchOutcome::Draw, &empty_reads());
        let overlay = overlay_for_surface(&ShellSurface::Ended(summary));
        // The drawn DISMISS button's center, taken from the renderer's own layout.
        let button = gonedark_render::overlay::overlay_quads(&overlay)
            .into_iter()
            .find(|q| q.role == gonedark_render::overlay::QuadRole::ButtonPrimary)
            .expect("summary draws a dismiss button");
        let slot = gonedark_render::overlay::button_slot_at(&overlay, button.cx, button.cy);
        assert_eq!(slot, Some(0), "the button center hit-tests to slot 0");
        assert_eq!(
            overlay_click_action(&overlay, slot.unwrap()),
            Some(OverlayClick::Dismiss),
            "clicking DISMISS resolves to a host Dismiss, not a no-op"
        );
        // A click far outside the panel resolves to nothing.
        assert_eq!(
            gonedark_render::overlay::button_slot_at(&overlay, 5.0, 5.0),
            None
        );
    }

    /// End-to-end the reconnect wire-up as `frame` runs it (minus the GPU glue): a confirmed desync
    /// drained from the lockstep handle → projected to `LinkState::Desynced` → raised over a PAUSED
    /// overlay → mapped to the warning-accented overlay. Locks both review fixes: the desync is
    /// drained/projected (invariant #7) and the prompt supersedes a lockstep pause.
    #[test]
    fn desync_supersedes_pause_and_maps_to_warning_overlay() {
        use gonedark_core::lockstep::{Desync, Lockstep};
        // A lockstep (multi-peer) session, paused locally.
        let mut shell = InSessionShell::new(/* single_player = */ false);
        shell.apply(
            SessionAction::Pause,
            &assemble_summary(&[], 0, MatchOutcome::Draw, &empty_reads()),
        );
        assert!(shell.is_paused());

        // Project a confirmed desync exactly as the call site does (the Desync stands in for what
        // `take_desyncs().into_iter().next()` yields on a real cross-client divergence).
        let ls = Lockstep::new(2, 0, 4);
        let recent_desync = Some(Desync {
            tick: 7,
            peer: 1,
            local: 0x1111,
            remote: 0x2222,
        });
        let status = ConnectionStatus::project(&ls, /* stalled = */ false, recent_desync);
        assert_eq!(status.state, LinkState::Desynced);
        assert!(session_shell::should_prompt_reconnect(&status));

        // The call-site guard is `!is_ended()`, so a paused session still surfaces the prompt.
        assert!(!shell.is_ended());
        shell.request_reconnect(status.state);
        assert_eq!(
            *shell.surface(),
            ShellSurface::ReconnectPrompt(LinkState::Desynced)
        );
        assert_eq!(
            overlay_for_surface(shell.surface()),
            Overlay::ReconnectPrompt { desynced: true },
            "a desync over a pause must read as the warning-accented prompt"
        );
    }

    /// THE shell-determinism guard (invariant #1/#7): driving the in-session shell state machine
    /// AND assembling the post-match summary every tick — exactly the work `frame` does — must
    /// leave the sim's per-tick checksum stream byte-identical to a run that never touches the
    /// shell. The shell holds no `Sim` and can't be handed one, so this fails loudly only if that
    /// ever regresses. Mirrors `avatar_prediction_never_perturbs_the_sim_checksum`.
    #[test]
    fn shell_and_summary_never_perturb_the_sim_checksum() {
        fn run(with_shell: bool) -> Vec<u64> {
            let mut sim = Sim::new(DRIVE_SEED);
            let _player = drive_scene(&mut sim);
            let mut shell = InSessionShell::new(true);
            let mut events: Vec<SimEvent> = Vec::new();
            let mut stream = vec![sim.checksum()];
            for t in 0..120u64 {
                sim.step(&[]);
                if with_shell {
                    events.extend_from_slice(sim.events());
                    // Exercise the state machine and the assembler against live sim reads.
                    let reads = [
                        EndStateRead {
                            territory_held: sim.territory.points.len() as u32,
                            resources_total: sim.resources.get(Faction::Player),
                        },
                        EndStateRead::default(),
                        EndStateRead::default(),
                    ];
                    let summary =
                        assemble_summary(&events, sim.tick_count(), MatchOutcome::Draw, &reads);
                    // Toggle pause/resume to walk transitions; surrender near the end.
                    if t == 10 {
                        shell.apply(SessionAction::Pause, &summary);
                    } else if t == 20 {
                        shell.apply(SessionAction::Resume, &summary);
                    } else if t == 110 {
                        shell.apply(SessionAction::Surrender, &summary);
                    }
                    let _ = overlay_for_surface(shell.surface());
                }
                stream.push(sim.checksum());
            }
            stream
        }
        assert_eq!(
            run(true),
            run(false),
            "the in-session shell + summary assembler must not perturb the deterministic sim"
        );
    }

    // --- Render quality tiers / dyn-res / thermal (Phase 4 WS-C) ---

    use gonedark_pal::ThermalState;
    use gonedark_render::tiers::QualityTier;
    use tuning::RenderTuning;

    /// THE load-bearing WS-C guard (invariant #1/#4): a quality tier — and the dynamic-resolution
    /// scale + thermal backoff it drives — is a RENDERING choice, never a sim input. Stepping the
    /// SAME scripted sim while running the full `RenderTuning` controller at each of Low/Mid/High,
    /// under each thermal state, must produce a per-tick checksum stream that is byte-identical
    /// across every tier and identical to a run with NO tuning at all. If a tier ever leaked into
    /// the sim (a float, a tick-rate change), this diverges loudly.
    #[test]
    fn tier_choice_is_sim_independent() {
        /// Step the drive script through a fresh sim, optionally running the tuning controller at
        /// `tier` under `thermal` exactly as `Game::frame` does (observe `dt`, no sim feedback).
        fn run(tuning: Option<(QualityTier, ThermalState)>) -> Vec<u64> {
            let mut sim = Sim::new(DRIVE_SEED);
            let player = drive_scene(&mut sim);
            let script = drive_script(player);
            let mut ctrl = tuning.map(|(t, _)| RenderTuning::new(t));
            let tick_dt = 1.0 / TICK_HZ as f32;
            let mut sums = Vec::new();
            for (commands, budget) in &script {
                let budget = if *budget == 0 && !commands.is_empty() {
                    1
                } else {
                    *budget
                };
                let mut first = true;
                for _ in 0..budget {
                    // Drive the tuning controller every "frame" with a realistic dt — purely a
                    // render decision; it must not touch the sim at all.
                    if let (Some(ctrl), Some((_, thermal))) = (ctrl.as_mut(), tuning) {
                        let cap = ctrl.fps_cap();
                        let budget_secs = match cap {
                            Some(c) if c > 0 => 1.0 / c as f32,
                            _ => tick_dt,
                        };
                        ctrl.observe_frame(0.018, thermal, budget_secs);
                    }
                    if first {
                        sim.step(commands);
                        first = false;
                    } else {
                        sim.step(&[]);
                    }
                    sums.push(sim.checksum());
                }
            }
            sums
        }

        let baseline = run(None);
        assert!(!baseline.is_empty(), "the script must advance some ticks");
        for tier in [QualityTier::Low, QualityTier::Mid, QualityTier::High] {
            for thermal in [
                ThermalState::Nominal,
                ThermalState::Fair,
                ThermalState::Serious,
                ThermalState::Critical,
            ] {
                assert_eq!(
                    run(Some((tier, thermal))),
                    baseline,
                    "tier {tier:?} under {thermal:?} must not perturb the sim checksum stream"
                );
            }
        }
    }

    /// `Game::set_tier` is render-only: changing the tier re-clamps the running scale into the new
    /// band but reports the new tier and never errors. (The full `Game` needs a GPU device, so the
    /// controller is exercised directly here — the same `RenderTuning` `Game` owns.)
    #[test]
    fn set_tier_switches_render_band_only() {
        let mut ctrl = RenderTuning::new(QualityTier::High);
        assert_eq!(ctrl.tier(), QualityTier::High);
        ctrl.set_tier(QualityTier::Low);
        assert_eq!(ctrl.tier(), QualityTier::Low);
        assert!(ctrl.resolution_scale() <= QualityTier::Low.params().res_scale_ceiling + 1e-5);
    }

    // --- Enemy commander integration (W3) --------------------------------------------------
    //
    // `Game::frame` needs a GPU device, so the commander's host-side wiring (the once-per-second
    // gate, the own RNG seeded `sim_seed ^ faction`, pushing its orders into the lockstep command
    // stream BEFORE the step) is exercised here against a raw `Sim` set up like the demo scene —
    // the same shape `frame()` drives. This is the testable seam for the otherwise-GPU-bound glue.

    /// Drive one tick exactly as `Game::frame` does for the commander path: on the gate, plan with
    /// the OWN rng and feed the orders into the same command set that steps the sim.
    fn commander_drive(sim: &mut Sim, rng: &mut Rng, faction: Faction, extra: &[Command]) {
        let mut commands: Vec<Command> = extra.to_vec();
        if sim.tick_count().is_multiple_of(COMMANDER_PERIOD) {
            commands.extend(commander::commander_orders(
                &sim.world,
                &sim.territory,
                &sim.resources,
                rng,
                &CommanderConfig::default(),
                &[],
                faction,
                sim.tick_count(),
            ));
        }
        sim.step(&commands);
    }

    fn enemy_demo_sim() -> Sim {
        let mut sim = Sim::new(DEFAULT_SEED);
        sim.resources = Resources::new(500);
        sim.territory
            .points
            .push(ControlPoint::neutral(Vec2::new(Fixed::ZERO, Fixed::ZERO)));
        sim.territory.points.push(ControlPoint::neutral(Vec2::new(
            Fixed::from_int(-16),
            Fixed::from_int(10),
        )));
        // Enemy squad + camp, mirroring `Game::new` (the player half is irrelevant to the AI).
        spawn_unit(&mut sim, 8, 0, Faction::Enemy, Stance::FireAtWill);
        spawn_unit(&mut sim, 10, 6, Faction::Enemy, Stance::FireAtWill);
        spawn_unit(&mut sim, 9, -6, Faction::Enemy, Stance::FireAtWill);
        spawn_unit(&mut sim, -7, -2, Faction::Player, Stance::FireAtWill); // a foe to press
        if let Some(camp) = economy::build(
            &mut sim.world,
            &mut sim.resources,
            Faction::Enemy,
            BuildingKind::Camp,
            Vec2::new(Fixed::from_int(22), Fixed::ZERO),
        ) {
            sim.world.building[camp.index as usize].build_ticks_left = 0;
        }
        sim
    }

    /// Snapshot each enemy unit's position so we can prove movement.
    fn enemy_unit_positions(sim: &Sim) -> Vec<Vec2> {
        let mut v = Vec::new();
        for i in 0..sim.world.capacity() {
            if sim.world.is_index_alive(i)
                && sim.world.kind[i] == EntityKind::Unit
                && sim.world.faction[i] == Faction::Enemy
            {
                v.push(sim.world.pos[i]);
            }
        }
        v
    }

    /// Over a 300-tick run the commander makes the enemy DO something visible: its units move from
    /// their spawn (it tasks them to capture/attack) and it reinforces (an enemy unit count above
    /// the 3 it spawned with). Previously the enemy was inert after one spawn order.
    #[test]
    fn enemy_commander_makes_the_enemy_act_over_300_ticks() {
        let mut sim = enemy_demo_sim();
        let mut rng = Rng::new(DEFAULT_SEED ^ Faction::Enemy.index() as u64);
        let start = enemy_unit_positions(&sim);
        let start_count = start.len();
        assert_eq!(start_count, 3, "demo enemy starts with three units");

        for _ in 0..300 {
            commander_drive(&mut sim, &mut rng, Faction::Enemy, &[]);
        }

        // 1. The original three enemies moved (the commander tasked them) — not all still at spawn.
        let mut any_moved = false;
        for (i, &p) in enemy_unit_positions(&sim).iter().take(3).enumerate() {
            if i < start.len() && p != start[i] {
                any_moved = true;
            }
        }
        assert!(any_moved, "the commander should have moved its units off their spawn");

        // 2. It reinforced: more enemy units alive than it started with (camp produced from income).
        let end_count = enemy_unit_positions(&sim).len();
        assert!(
            end_count > start_count,
            "commander should reinforce: {start_count} -> {end_count} enemy units"
        );
    }

    /// The commander wiring is deterministic end-to-end: two identical 300-tick runs (same seed,
    /// same own-RNG seeding) produce the bit-identical per-tick checksum stream. This is the
    /// host-side guarantee behind lockstep — the orders are a pure function of sim state + the
    /// own RNG, so every peer's stream agrees.
    #[test]
    fn commander_driven_run_is_deterministic() {
        fn run() -> Vec<u64> {
            let mut sim = enemy_demo_sim();
            let mut rng = Rng::new(DEFAULT_SEED ^ Faction::Enemy.index() as u64);
            let mut stream = Vec::with_capacity(300);
            for _ in 0..300 {
                commander_drive(&mut sim, &mut rng, Faction::Enemy, &[]);
                stream.push(sim.checksum());
            }
            stream
        }
        assert_eq!(run(), run(), "commander-driven checksum stream must be reproducible");
    }
}
