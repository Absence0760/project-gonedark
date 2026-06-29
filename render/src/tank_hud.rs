//! Embodied **tank** HUD overlay (tank embodiment P8, D55) — the gunner's-sight chrome drawn over the
//! dark embodied frame while the local player is *driving a tank*. It is the legible half of the
//! War-Thunder-flavoured vehicle (the plan's P8): four screen-space elements that turn the already-
//! simulated hull/turret/reload/ballistic state into something a driver can read and aim with.
//!
//! - **Hull-relative turret indicator** — a chevron on a top compass strip showing where the gun
//!   points relative to the hull, so a driver who has slewed the turret off-axis can still find
//!   "which way am I actually facing" to drive ([`turret_indicator_offset`] / [`turret_indicator_ndc_x`]).
//! - **Dispersion reticle** — a crosshair ring that BLOOMS while moving/traversing and SETTLES at
//!   rest, so the player waits for it to tighten before firing ([`dispersion_reticle_radius`]).
//! - **LEAD pip** — a small ring offset toward where to aim to hit a crossing target, given the
//!   shell's finite travel time ([`lead_pip_offset`]).
//! - **Reload ring** — an arc that fills clockwise as the gun reloads, full when loaded
//!   ([`reload_ring_fill`]).
//! - **Shell-selector readout** — the selected shell label, drawn through the shared text pass at the
//!   `lib.rs` boundary (W2 supplies the real `ShellKind`; see [`Renderer::render_tank_hud`]).
//!
//! Invariant #4: this is the **float side** — every number here is already `f32` host-side
//! presentation, never `core` sim state, and the renderer only READS the snapshot/world it is handed.
//! Like [`hud`](crate::hud) / [`touch_controls`](crate::touch_controls), all the geometry math lives
//! in pure free fns ([`tank_hud_instances`] and the placement helpers) so it is unit-testable without
//! a GPU — exactly the `marker_for` / `build_quads` pattern; only [`TankHudRenderer::render`] needs a
//! device.

use wgpu::util::DeviceExt;

/// Shader `shape` ids — must match the `fs_main` branches in `tank_hud.wgsl`.
const SHAPE_RETICLE: f32 = 0.0;
const SHAPE_RELOAD: f32 = 1.0;
const SHAPE_TURRET: f32 = 2.0;
const SHAPE_LEAD: f32 = 3.0;

/// Colors (RGB). Each element reads by shape + position; color is a secondary cue.
const RETICLE_COL: [f32; 3] = [0.78, 0.94, 0.82]; // pale gunsight green
const RELOAD_COL: [f32; 3] = [0.96, 0.74, 0.32]; // amber (matches the touch Reload button)
const TURRET_COL: [f32; 3] = [0.45, 0.85, 0.95]; // cyan compass tick
const LEAD_COL: [f32; 3] = [1.0, 0.55, 0.20]; // hot orange aim-ahead

const RETICLE_ALPHA: f32 = 0.85;
const RELOAD_ALPHA: f32 = 0.90;
const TURRET_ALPHA: f32 = 0.90;
const LEAD_ALPHA: f32 = 0.95;

/// Dispersion-reticle radius bounds, in NDC half-height units. A fully-settled gun shows the tight
/// [`RETICLE_MIN_R`]; bloom grows it by [`RETICLE_GAIN`] per unit of dispersion up to [`RETICLE_MAX_R`].
const RETICLE_MIN_R: f32 = 0.05;
const RETICLE_MAX_R: f32 = 0.34;
const RETICLE_GAIN: f32 = 0.55;

/// The reload ring rides just outside the widest the reticle can bloom, so the two never overlap.
const RELOAD_R: f32 = RETICLE_MAX_R + 0.06;

/// The hull-relative turret compass strip: vertical position (NDC, near the top) and half-width the
/// chevron can travel across. The chevron is small and fixed-size.
const TURRET_STRIP_Y: f32 = 0.80;
const TURRET_STRIP_HALF_W: f32 = 0.55;
const TURRET_CHEVRON_R: f32 = 0.05;

/// The lead pip is a small fixed-size ring; its center is the (clamped) aim-ahead offset.
const LEAD_PIP_R: f32 = 0.035;
/// The pip is clamped to this NDC-y radius from center so a very fast crosser doesn't fly off-screen.
const LEAD_CLAMP_R: f32 = 0.45;

/// Hull-relative turret bearing — the signed angle (radians, wrapped to `(-π, π]`) the gun points
/// away from straight-ahead-of-the-hull. `0` = the gun is aligned with the hull; `+` = the turret is
/// swung counter-clockwise (toward `+Y`) of the hull; `-` = clockwise. Both inputs are absolute world
/// bearings in the renderer's convention (`+X = 0`, increasing CCW toward `+Y`), matching
/// [`interp_angle`](crate::interp_angle). Pure float math (the presentation boundary).
pub fn turret_indicator_offset(hull_rad: f32, turret_rad: f32) -> f32 {
    wrap_pi(turret_rad - hull_rad)
}

/// Wrap an angle (radians) into `(-π, π]`.
#[inline]
fn wrap_pi(a: f32) -> f32 {
    use std::f32::consts::{PI, TAU};
    let w = (a + PI).rem_euclid(TAU) - PI;
    // rem_euclid maps exactly -π to -π; canonicalise the seam to the (-π, π] half-open form.
    if w <= -PI {
        w + TAU
    } else {
        w
    }
}

/// Place the hull-relative turret chevron on the top compass strip: map the signed `offset` (radians,
/// `turret − hull`) to an NDC x in `[-strip_half_w, strip_half_w]`. A gun aligned with the hull sits
/// dead-center (`0`); a half-turn off-axis sits at the strip edge. The mapping is `−offset/π`-scaled
/// so a turret swung CCW (`+offset`, gun pointing to the driver's left) places the marker to the LEFT
/// (`−x`) — it reads like a top-down compass of where the gun is vs the hull. Pure, host-testable.
pub fn turret_indicator_ndc_x(offset: f32, strip_half_w: f32) -> f32 {
    use std::f32::consts::PI;
    (-offset / PI * strip_half_w).clamp(-strip_half_w, strip_half_w)
}

/// Reload-ring fill fraction in `[0, 1]`: `0` = the reload has just started, `1` = the gun is loaded
/// and ready. Derived from the weapon's `reload_left` (ticks remaining) vs `reload_ticks` (the total
/// reload duration). A weapon with no reload system (`reload_ticks == 0`, the infinite-ammo default)
/// reads as fully loaded. Clamped — a stale `reload_left > reload_ticks` still yields `0`. Pure,
/// float-free-input math (the testable seam for the reload ring).
pub fn reload_ring_fill(reload_left: u16, reload_ticks: u16) -> f32 {
    if reload_ticks == 0 {
        return 1.0;
    }
    let left = reload_left.min(reload_ticks);
    let done = reload_ticks - left;
    (done as f32 / reload_ticks as f32).clamp(0.0, 1.0)
}

/// Dispersion-reticle radius (NDC half-height units): a settled gun (`dispersion == 0`) shows the
/// tight `min_radius`; the radius grows linearly with `dispersion` at `gain`, capped at `max_radius`.
/// So the reticle BLOOMS while the tank moves/traverses and SETTLES when held — the player waits for
/// it to tighten before firing (plan §5, the skill-honest aim model). Negative dispersion is treated
/// as zero. Pure, host-testable.
pub fn dispersion_reticle_radius(dispersion: f32, min_radius: f32, gain: f32, max_radius: f32) -> f32 {
    (min_radius + gain * dispersion.max(0.0)).clamp(min_radius, max_radius)
}

/// Lead-pip screen offset (NDC), relative to the reticle center: where to aim to hit a target moving
/// at `target_rel_vel` given the shell's finite travel time.
///
/// `target_rel_vel` is the target's velocity relative to the shooter, **already resolved into the
/// gunner's screen axes** (`x` = screen-right, `y` = screen-up), in world units/tick — the camera→
/// screen rotation is the caller's (untestable) glue, leaving this fn the gameplay-meaningful leading
/// math. `range` is the shooter→target distance (world units), `muzzle_vel` the shell speed (world
/// units/tick), and `world_to_ndc` the NDC-per-world-unit screen scale. The lead is
/// `rel_vel · (range / muzzle_vel)` — more lead for a faster crosser, a slower shell, or a longer
/// shot. A hitscan gun (`muzzle_vel <= 0`) or a non-positive `range` yields no lead (the pip sits on
/// center). Pure, host-testable.
pub fn lead_pip_offset(
    target_rel_vel: (f32, f32),
    range: f32,
    muzzle_vel: f32,
    world_to_ndc: f32,
) -> (f32, f32) {
    if muzzle_vel <= 0.0 || range <= 0.0 {
        return (0.0, 0.0);
    }
    let flight_ticks = range / muzzle_vel;
    (
        target_rel_vel.0 * flight_ticks * world_to_ndc,
        target_rel_vel.1 * flight_ticks * world_to_ndc,
    )
}

/// Everything the tank HUD needs to draw this frame — a pure, `Copy` presentation description the host
/// fills from the embodied tank's (read-only) sim state and the camera. No `core` types cross this
/// boundary (invariant #2): the host does the `Fixed`/`Angle` → `f32` hops.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TankHudState {
    /// Hull bearing (radians, renderer convention) — the chassis facing.
    pub hull_rad: f32,
    /// Turret bearing (radians, renderer convention) — the absolute gun bearing.
    pub turret_rad: f32,
    /// Ticks left in an in-progress reload (`0` = loaded / not reloading).
    pub reload_left: u16,
    /// Total reload duration in ticks (`0` = no reload system → ring reads full).
    pub reload_ticks: u16,
    /// Current aim bloom. **W1 dependency:** until `Weapon::dispersion` (P5) merges, the host feeds
    /// `0.0` (a settled gun); once merged it is a one-line swap to the real per-tank field.
    pub dispersion: f32,
    /// Target velocity relative to the shooter, in the gunner's screen axes (world units/tick). `(0,0)`
    /// when there is no tracked target — the pip then sits on center.
    pub target_rel_vel: (f32, f32),
    /// Shooter→target distance (world units); `0` when no target is tracked.
    pub target_range: f32,
    /// Shell muzzle velocity (world units/tick); `<= 0` for a hitscan gun → no lead pip.
    pub muzzle_vel: f32,
    /// NDC-per-world-unit screen scale for the lead-pip projection.
    pub world_to_ndc: f32,
    /// Viewport aspect (width / height) — keeps the round elements circular per axis.
    pub aspect: f32,
}

impl Default for TankHudState {
    fn default() -> Self {
        TankHudState {
            hull_rad: 0.0,
            turret_rad: 0.0,
            reload_left: 0,
            reload_ticks: 0,
            dispersion: 0.0,
            target_rel_vel: (0.0, 0.0),
            target_range: 0.0,
            muzzle_vel: 0.0,
            world_to_ndc: 0.0,
            aspect: 1.0,
        }
    }
}

/// One tank-HUD element ready to upload. `repr(C)` + `Pod` so it streams into the per-instance vertex
/// buffer; the field order MUST match the instance attribute locations in `tank_hud.wgsl` and the
/// `vertex_attr_array` in [`TankHudRenderer::new`].
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, bytemuck::Pod, bytemuck::Zeroable)]
pub struct TankHudInstance {
    /// Center in NDC ([-1,1], +y up).
    pub ndc_x: f32,
    pub ndc_y: f32,
    /// Per-axis NDC half-size (so the round elements stay circular under aspect).
    pub half_x: f32,
    pub half_y: f32,
    pub r: f32,
    pub g: f32,
    pub b: f32,
    pub a: f32,
    /// Shape id: 0 reticle, 1 reload ring, 2 turret chevron, 3 lead pip.
    pub shape: f32,
    /// Per-element scalar — the reload fill fraction for the reload ring; unused (0) otherwise.
    pub param: f32,
}

/// An NDC-y radius → per-axis half-size that keeps a circle round on a non-square viewport. NDC spans
/// 2.0 on each axis; a `r`-tall circle is `r/aspect` wide (`aspect = w/h`).
#[inline]
fn round_half(r: f32, aspect: f32) -> (f32, f32) {
    let a = if aspect.abs() < 1e-6 { 1.0 } else { aspect };
    (r / a, r)
}

/// Build the tank HUD's screen-space instances from `state`, in a stable order (reticle, reload ring,
/// turret indicator, lead pip). PURE float math — host-testable without a GPU; [`TankHudRenderer::
/// render`] just uploads + draws whatever this returns. The shell-selector *label* is text and is
/// drawn separately through the shared text pass (it carries no geometry here).
pub fn tank_hud_instances(state: &TankHudState) -> Vec<TankHudInstance> {
    let mut out = Vec::with_capacity(4);

    // 1. Dispersion reticle — a crosshair ring at screen center sized by current bloom.
    let reticle_r =
        dispersion_reticle_radius(state.dispersion, RETICLE_MIN_R, RETICLE_GAIN, RETICLE_MAX_R);
    let (hx, hy) = round_half(reticle_r, state.aspect);
    out.push(TankHudInstance {
        ndc_x: 0.0,
        ndc_y: 0.0,
        half_x: hx,
        half_y: hy,
        r: RETICLE_COL[0],
        g: RETICLE_COL[1],
        b: RETICLE_COL[2],
        a: RETICLE_ALPHA,
        shape: SHAPE_RETICLE,
        param: 0.0,
    });

    // 2. Reload ring — concentric, just outside the widest reticle bloom, filling as the gun reloads.
    let fill = reload_ring_fill(state.reload_left, state.reload_ticks);
    let (rhx, rhy) = round_half(RELOAD_R, state.aspect);
    out.push(TankHudInstance {
        ndc_x: 0.0,
        ndc_y: 0.0,
        half_x: rhx,
        half_y: rhy,
        r: RELOAD_COL[0],
        g: RELOAD_COL[1],
        b: RELOAD_COL[2],
        a: RELOAD_ALPHA,
        shape: SHAPE_RELOAD,
        param: fill,
    });

    // 3. Hull-relative turret indicator — a chevron on the top compass strip.
    let offset = turret_indicator_offset(state.hull_rad, state.turret_rad);
    let tx = turret_indicator_ndc_x(offset, TURRET_STRIP_HALF_W);
    let (thx, thy) = round_half(TURRET_CHEVRON_R, state.aspect);
    out.push(TankHudInstance {
        ndc_x: tx,
        ndc_y: TURRET_STRIP_Y,
        half_x: thx,
        half_y: thy,
        r: TURRET_COL[0],
        g: TURRET_COL[1],
        b: TURRET_COL[2],
        a: TURRET_ALPHA,
        shape: SHAPE_TURRET,
        param: 0.0,
    });

    // 4. Lead pip — only when a real ballistic target is tracked (a finite shell + a positive range).
    let (lx, ly) = lead_pip_offset(
        state.target_rel_vel,
        state.target_range,
        state.muzzle_vel,
        state.world_to_ndc,
    );
    if lx != 0.0 || ly != 0.0 {
        // Clamp the pip to a sane on-screen radius so a very fast crosser stays visible.
        let (cx, cy) = clamp_to_radius(lx, ly, LEAD_CLAMP_R);
        let (lhx, lhy) = round_half(LEAD_PIP_R, state.aspect);
        out.push(TankHudInstance {
            ndc_x: cx,
            ndc_y: cy,
            half_x: lhx,
            half_y: lhy,
            r: LEAD_COL[0],
            g: LEAD_COL[1],
            b: LEAD_COL[2],
            a: LEAD_ALPHA,
            shape: SHAPE_LEAD,
            param: 0.0,
        });
    }

    out
}

/// Clamp a 2D NDC offset to a maximum length `max_r`, preserving its direction.
#[inline]
fn clamp_to_radius(x: f32, y: f32, max_r: f32) -> (f32, f32) {
    let len = (x * x + y * y).sqrt();
    if len <= max_r || len < 1e-9 {
        (x, y)
    } else {
        let s = max_r / len;
        (x * s, y * s)
    }
}

/// A unit-quad corner in [-1, 1]^2 (the shader scales it by the per-instance half-size).
#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct QuadVertex {
    corner: [f32; 2],
}

const QUAD_VERTS: [QuadVertex; 6] = [
    QuadVertex { corner: [-1.0, -1.0] },
    QuadVertex { corner: [1.0, -1.0] },
    QuadVertex { corner: [1.0, 1.0] },
    QuadVertex { corner: [-1.0, -1.0] },
    QuadVertex { corner: [1.0, 1.0] },
    QuadVertex { corner: [-1.0, 1.0] },
];

const INITIAL_CAP: usize = 4;

/// Screen-space embodied-tank HUD overlay (its own pipeline + buffers, like [`hud`](crate::hud) and
/// [`touch_controls`](crate::touch_controls)). Recorded as a LOAD pass so it composites over the dark
/// embodied frame, never clearing it.
pub struct TankHudRenderer {
    pipeline: wgpu::RenderPipeline,
    quad_buf: wgpu::Buffer,
    instance_buf: wgpu::Buffer,
    instance_cap: usize,
}

impl TankHudRenderer {
    /// Build the pipeline against the swapchain `surface_format` (alpha-blended LOAD overlay).
    pub fn new(device: &wgpu::Device, surface_format: wgpu::TextureFormat) -> Self {
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("gonedark.tank_hud_shader"),
            source: wgpu::ShaderSource::Wgsl(include_str!("tank_hud.wgsl").into()),
        });

        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("gonedark.tank_hud_pipeline_layout"),
            bind_group_layouts: &[],
            immediate_size: 0,
        });

        let quad_layout = wgpu::VertexBufferLayout {
            array_stride: std::mem::size_of::<QuadVertex>() as u64,
            step_mode: wgpu::VertexStepMode::Vertex,
            attributes: &wgpu::vertex_attr_array![0 => Float32x2],
        };
        let instance_layout = wgpu::VertexBufferLayout {
            array_stride: std::mem::size_of::<TankHudInstance>() as u64,
            step_mode: wgpu::VertexStepMode::Instance,
            // 1=center(vec2), 2=half(vec2), 3=color(vec4), 4=shape(f32), 5=param(f32).
            attributes: &wgpu::vertex_attr_array![
                1 => Float32x2,
                2 => Float32x2,
                3 => Float32x4,
                4 => Float32,
                5 => Float32
            ],
        };

        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("gonedark.tank_hud_pipeline"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_main"),
                buffers: &[quad_layout, instance_layout],
                compilation_options: wgpu::PipelineCompilationOptions::default(),
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs_main"),
                targets: &[Some(wgpu::ColorTargetState {
                    format: surface_format,
                    blend: Some(wgpu::BlendState::ALPHA_BLENDING),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
                compilation_options: wgpu::PipelineCompilationOptions::default(),
            }),
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::TriangleList,
                ..Default::default()
            },
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            multiview_mask: None,
            cache: None,
        });

        let quad_buf = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("gonedark.tank_hud_quad_vbo"),
            contents: bytemuck::cast_slice(&QUAD_VERTS),
            usage: wgpu::BufferUsages::VERTEX,
        });

        let instance_cap = INITIAL_CAP;
        let instance_buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("gonedark.tank_hud_instance_vbo"),
            size: (instance_cap * std::mem::size_of::<TankHudInstance>()) as u64,
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        TankHudRenderer {
            pipeline,
            quad_buf,
            instance_buf,
            instance_cap,
        }
    }

    /// Draw the embodied-tank HUD geometry for `state` over `view` (a LOAD pass — never clears).
    /// Builds the live instance set via [`tank_hud_instances`], uploads it, and records one render
    /// pass. The shell-selector *label* is drawn separately (text pass) by the `lib.rs` boundary.
    pub fn render(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        view: &wgpu::TextureView,
        state: &TankHudState,
    ) {
        let instances = tank_hud_instances(state);
        if instances.is_empty() {
            return;
        }
        if instances.len() > self.instance_cap {
            let new_cap = instances.len().next_power_of_two();
            self.instance_buf = device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("gonedark.tank_hud_instance_vbo"),
                size: (new_cap * std::mem::size_of::<TankHudInstance>()) as u64,
                usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });
            self.instance_cap = new_cap;
        }
        queue.write_buffer(&self.instance_buf, 0, bytemuck::cast_slice(&instances));

        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("gonedark.tank_hud_encoder"),
        });
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("gonedark.tank_hud_pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Load,
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                multiview_mask: None,
                timestamp_writes: None,
                occlusion_query_set: None,
            });
            pass.set_pipeline(&self.pipeline);
            pass.set_vertex_buffer(0, self.quad_buf.slice(..));
            pass.set_vertex_buffer(1, self.instance_buf.slice(..));
            pass.draw(0..QUAD_VERTS.len() as u32, 0..instances.len() as u32);
        }
        queue.submit(std::iter::once(encoder.finish()));
    }
}

#[cfg(test)]
mod tests {
    //! `render` is the float boundary (invariant #1: floats live only in rendering), so f32 math and
    //! epsilon compares are fair game. `TankHudRenderer::new` needs a real `wgpu::Device` (no display
    //! in CI), so the pipeline path is untested; the testable placement/sizing math is factored into
    //! the pure free fns below — exactly the `marker_for` / `build_quads` pattern.

    use super::*;
    use std::f32::consts::{FRAC_PI_2, PI};

    // ---- turret indicator ----

    #[test]
    fn aligned_turret_offset_is_zero_and_centered() {
        // Gun pointed the same way as the hull → offset 0 → chevron dead-center on the strip.
        let off = turret_indicator_offset(1.3, 1.3);
        assert!(off.abs() < 1e-6, "aligned turret has zero offset, got {off}");
        assert!(turret_indicator_ndc_x(off, TURRET_STRIP_HALF_W).abs() < 1e-6);
    }

    #[test]
    fn turret_offset_wraps_the_short_way() {
        // hull at +179°, turret at -179° (i.e. 181°): the short way is +2°, not -358°.
        let hull = 179.0_f32.to_radians();
        let turret = (-179.0_f32).to_radians();
        let off = turret_indicator_offset(hull, turret);
        assert!(off > 0.0 && off < 0.1, "short-arc +2°, got {off} rad");
    }

    #[test]
    fn turret_offset_is_signed_and_wrapped() {
        // A quarter-turn CCW reads +π/2; a quarter-turn CW reads -π/2.
        let ccw = turret_indicator_offset(0.0, FRAC_PI_2);
        let cw = turret_indicator_offset(0.0, -FRAC_PI_2);
        assert!((ccw - FRAC_PI_2).abs() < 1e-5);
        assert!((cw + FRAC_PI_2).abs() < 1e-5);
        // Every offset stays within (-π, π].
        for deg in (-360..=360).step_by(7) {
            let o = turret_indicator_offset(0.0, (deg as f32).to_radians());
            assert!(o > -PI - 1e-4 && o <= PI + 1e-4, "{deg}° → {o} out of range");
        }
    }

    #[test]
    fn turret_ccw_places_marker_left_cw_right() {
        // +offset (turret swung CCW, gun to the driver's left) → marker to the LEFT (-x); mirror for CW.
        let left = turret_indicator_ndc_x(FRAC_PI_2, TURRET_STRIP_HALF_W);
        let right = turret_indicator_ndc_x(-FRAC_PI_2, TURRET_STRIP_HALF_W);
        assert!(left < 0.0, "CCW gun marks left, got {left}");
        assert!(right > 0.0, "CW gun marks right, got {right}");
        assert!((left + right).abs() < 1e-6, "symmetric about center");
    }

    #[test]
    fn turret_marker_clamps_to_strip_edges() {
        // A near-half-turn can't push the chevron past the strip ends.
        let x = turret_indicator_ndc_x(PI, TURRET_STRIP_HALF_W);
        assert!(x.abs() <= TURRET_STRIP_HALF_W + 1e-6);
        let xn = turret_indicator_ndc_x(-PI, TURRET_STRIP_HALF_W);
        assert!(xn.abs() <= TURRET_STRIP_HALF_W + 1e-6);
    }

    // ---- reload ring ----

    #[test]
    fn reload_just_started_is_empty_done_is_full() {
        // Full reload duration remaining → 0; none remaining → 1.
        assert!((reload_ring_fill(60, 60) - 0.0).abs() < 1e-6, "fresh reload empty");
        assert!((reload_ring_fill(0, 60) - 1.0).abs() < 1e-6, "finished reload full");
    }

    #[test]
    fn reload_fill_is_monotonic_in_progress() {
        // As ticks tick down, the ring fills.
        let early = reload_ring_fill(50, 60);
        let mid = reload_ring_fill(30, 60);
        let late = reload_ring_fill(10, 60);
        assert!(early < mid && mid < late, "{early} < {mid} < {late}");
        assert!((mid - 0.5).abs() < 1e-6, "half-way is half-full");
    }

    #[test]
    fn reload_fill_clamps_to_unit_interval() {
        // No reload system (reload_ticks == 0) → loaded; a stale over-large reload_left → 0; never out of [0,1].
        assert_eq!(reload_ring_fill(0, 0), 1.0, "no reload system reads loaded");
        assert_eq!(reload_ring_fill(999, 60), 0.0, "stale over-large left clamps to empty");
        for left in 0..=120u16 {
            let f = reload_ring_fill(left, 60);
            assert!((0.0..=1.0).contains(&f), "fill {f} out of [0,1] at left={left}");
        }
    }

    // ---- dispersion reticle ----

    #[test]
    fn settled_gun_shows_the_tight_reticle() {
        // Zero dispersion → the minimum (tight) radius.
        let r = dispersion_reticle_radius(0.0, RETICLE_MIN_R, RETICLE_GAIN, RETICLE_MAX_R);
        assert!((r - RETICLE_MIN_R).abs() < 1e-6, "settled gun is tight, got {r}");
    }

    #[test]
    fn reticle_grows_with_dispersion() {
        // More bloom → strictly larger reticle, until the cap.
        let a = dispersion_reticle_radius(0.1, RETICLE_MIN_R, RETICLE_GAIN, RETICLE_MAX_R);
        let b = dispersion_reticle_radius(0.3, RETICLE_MIN_R, RETICLE_GAIN, RETICLE_MAX_R);
        assert!(b > a && a > RETICLE_MIN_R, "{a} < {b}, both above min");
    }

    #[test]
    fn reticle_caps_and_floors() {
        // A huge dispersion saturates at max; a negative one is treated as settled.
        let big = dispersion_reticle_radius(100.0, RETICLE_MIN_R, RETICLE_GAIN, RETICLE_MAX_R);
        assert!((big - RETICLE_MAX_R).abs() < 1e-6, "saturates at max, got {big}");
        let neg = dispersion_reticle_radius(-5.0, RETICLE_MIN_R, RETICLE_GAIN, RETICLE_MAX_R);
        assert!((neg - RETICLE_MIN_R).abs() < 1e-6, "negative reads as settled");
    }

    // ---- lead pip ----

    #[test]
    fn no_lead_for_hitscan_or_stationary_target() {
        // muzzle_vel <= 0 (hitscan) → center; a stationary target → center; no range → center.
        assert_eq!(lead_pip_offset((5.0, 0.0), 10.0, 0.0, 1.0), (0.0, 0.0));
        assert_eq!(lead_pip_offset((0.0, 0.0), 10.0, 8.0, 1.0), (0.0, 0.0));
        assert_eq!(lead_pip_offset((5.0, 0.0), 0.0, 8.0, 1.0), (0.0, 0.0));
    }

    #[test]
    fn lead_grows_with_crossing_speed_range_and_slower_shell() {
        // Lead = rel_vel * (range / muzzle_vel) * world_to_ndc.
        let base = lead_pip_offset((2.0, 0.0), 10.0, 5.0, 0.1);
        assert!((base.0 - (2.0 * (10.0 / 5.0) * 0.1)).abs() < 1e-6, "exact lead, got {:?}", base);
        // Faster crosser → more lead.
        let fast = lead_pip_offset((4.0, 0.0), 10.0, 5.0, 0.1);
        assert!(fast.0 > base.0);
        // Longer shot → more lead.
        let far = lead_pip_offset((2.0, 0.0), 20.0, 5.0, 0.1);
        assert!(far.0 > base.0);
        // Slower shell → more lead.
        let slow = lead_pip_offset((2.0, 0.0), 10.0, 2.5, 0.1);
        assert!(slow.0 > base.0);
    }

    #[test]
    fn lead_carries_both_screen_axes() {
        // A target crossing up-and-right leads up-and-right.
        let (x, y) = lead_pip_offset((3.0, -1.5), 10.0, 5.0, 0.1);
        assert!(x > 0.0 && y < 0.0, "lead follows the screen-axis rel velocity: {x},{y}");
    }

    // ---- instance builder ----

    fn state() -> TankHudState {
        TankHudState {
            aspect: 2.0,
            ..TankHudState::default()
        }
    }

    #[test]
    fn builder_always_emits_reticle_reload_and_turret() {
        // With no tracked target the builder still draws the three resident elements (no lead pip).
        let inst = tank_hud_instances(&state());
        assert_eq!(inst.len(), 3, "reticle + reload + turret, no lead");
        assert_eq!(inst[0].shape, SHAPE_RETICLE);
        assert_eq!(inst[1].shape, SHAPE_RELOAD);
        assert_eq!(inst[2].shape, SHAPE_TURRET);
    }

    #[test]
    fn builder_adds_lead_pip_for_a_tracked_ballistic_target() {
        let mut s = state();
        s.muzzle_vel = 6.0;
        s.target_range = 12.0;
        s.target_rel_vel = (3.0, 0.0);
        s.world_to_ndc = 0.05;
        let inst = tank_hud_instances(&s);
        assert_eq!(inst.len(), 4, "lead pip added");
        assert_eq!(inst[3].shape, SHAPE_LEAD);
        assert!(inst[3].ndc_x > 0.0, "pip leads in the crossing direction");
    }

    #[test]
    fn reticle_quad_grows_with_dispersion() {
        let mut tight = state();
        tight.dispersion = 0.0;
        let mut bloom = state();
        bloom.dispersion = 0.4;
        let t = tank_hud_instances(&tight)[0];
        let b = tank_hud_instances(&bloom)[0];
        assert!(b.half_y > t.half_y, "blooming reticle quad is taller");
    }

    #[test]
    fn round_elements_stay_circular_via_per_axis_half() {
        // aspect 2.0 (twice as wide as tall) → the x half-size is half the y half-size, so the ring
        // renders as a circle, not an ellipse.
        let inst = tank_hud_instances(&state());
        let reticle = inst[0];
        assert!((reticle.half_x - reticle.half_y / 2.0).abs() < 1e-6, "per-axis half keeps it round");
    }

    #[test]
    fn reload_ring_param_carries_the_fill_fraction() {
        let mut s = state();
        s.reload_ticks = 60;
        s.reload_left = 15; // 3/4 done
        let reload = tank_hud_instances(&s)[1];
        assert!((reload.param - 0.75).abs() < 1e-6, "reload param is the fill fraction");
    }

    #[test]
    fn lead_pip_clamps_to_a_visible_radius() {
        // A wildly fast crosser would fly off-screen; the pip is clamped to LEAD_CLAMP_R.
        let mut s = state();
        s.muzzle_vel = 1.0;
        s.target_range = 100.0;
        s.target_rel_vel = (50.0, 0.0);
        s.world_to_ndc = 1.0;
        let pip = *tank_hud_instances(&s).last().unwrap();
        let len = (pip.ndc_x * pip.ndc_x + pip.ndc_y * pip.ndc_y).sqrt();
        assert!(len <= LEAD_CLAMP_R + 1e-6, "pip clamped to {LEAD_CLAMP_R}, got {len}");
    }

    /// Validate `tank_hud.wgsl` offline with naga (the compiler wgpu uses), so a WGSL regression fails
    /// the suite instead of only blowing up at pipeline creation on a real GPU (mirrors `hud.rs`).
    #[test]
    fn tank_hud_wgsl_parses_and_validates() {
        let src = include_str!("tank_hud.wgsl");
        let module = naga::front::wgsl::parse_str(src).expect("tank_hud.wgsl must parse");
        let mut validator = naga::valid::Validator::new(
            naga::valid::ValidationFlags::all(),
            naga::valid::Capabilities::all(),
        );
        validator.validate(&module).expect("tank_hud.wgsl must validate");
    }
}
