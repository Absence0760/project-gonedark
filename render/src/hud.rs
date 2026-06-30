//! The embodied alert HUD (invariant #6, game-design §6) — the *only* visual thread back to
//! command while the map is dark: directional pings ("taking fire to the south-east"), never a
//! map reveal. It draws ON TOP of the already-rendered embodied frame (a second pass that LOADs,
//! does not clear), so it is its own tiny screen-space pipeline + shader (`hud.wgsl`), kept
//! separate from the unit pipeline so the two never contend for the same shader/source.
//!
//! Data comes from `core::alerts::AlertChannel` (a presentation derivation, never sim state).
//! The HUD places each recent [`Alert`](gonedark_core::alerts::Alert) by the bearing of its
//! `pos` relative to the avatar's `yaw`, fading older ones out by tick age. Float boundary.
//!
//! ## Bearing → screen placement
//! Markers ride a ring near the screen edge. Bearing 0 (the alert lies straight ahead along
//! the avatar's facing) places the marker at top-center; a positive azimuth (the alert is to
//! the avatar's right) swings it clockwise toward the right edge, a negative one to the left.
//! An alert directly behind the avatar lands at bottom-center. The pure placement + fade math
//! lives in [`marker_for`] so it is unit-testable without a GPU.

use gonedark_core::alerts::{Alert, AlertChannel, AlertKind};
use wgpu::util::DeviceExt;

/// How many ticks an alert stays on the HUD before it has fully faded out. At 60 Hz this is a
/// ~2 s decay — long enough to read a direction, short enough that the thread stays *thin*.
const FADE_TICKS: u64 = 120;

/// The lowest alpha a still-live marker holds at (just before the hard cutoff at [`FADE_TICKS`]).
/// A linear ramp decays into <0.1 alpha that is invisible over a lit frame; flooring at a legible
/// value keeps the *whole* window readable, then the hard `None` at [`FADE_TICKS`] removes it.
const FADE_FLOOR: f32 = 0.35;

/// Half-size of a marker quad in NDC (markers are small screen-space chevrons/dots).
const MARKER_HALF_SIZE: f32 = 0.045;

/// Radius (in NDC) of the ring the markers sit on — near, but inside, the screen edge.
const RING_RADIUS: f32 = 0.82;

/// Initial GPU capacity (in markers) for the instance buffer.
const INITIAL_CAP: usize = 16;

/// How many ticks the embodied hitmarker flash stays up after a connecting shot, fading linearly
/// to nothing. At 60 Hz this is a ~0.17 s snap — long enough to register "I hit him", short enough
/// to feel crisp and not smear across sustained fire (WS-4 / roadmap game-feel polish).
pub const HITMARKER_TICKS: u64 = 10;

/// Half-size (NDC) of the centered hitmarker quad — a small crosshair flash at screen center.
const HITMARKER_HALF_SIZE: f32 = 0.085;

/// The hitmarker is a crisp bright white "X" at screen center. White is deliberately distinct from
/// every other embodied-frame element: the alert-marker palette (warm red/orange, teal, pale grey)
/// rides the screen *edge* ring; the muzzle flash is a *warm* yellow (low blue); the FPS world is a
/// muted, low-saturation blue-grey. A high-blue near-white pixel at center reads only as the
/// hitmarker — which is exactly what the viz pixel-assert keys on.
const HITMARKER_COLOR: [f32; 3] = [1.0, 1.0, 1.0];

/// The hitmarker glyph id matched by `glyph_coverage` in `hud.wgsl` (4 = centered "X").
const SHAPE_HITMARKER: f32 = 4.0;

/// The dot glyph id (`hud.wgsl` shape 0) — reused for the hip-fire crosshair ticks (WS-A).
const SHAPE_DOT: f32 = 0.0;

/// Resting half-gap (NDC-y) from screen-center to each crosshair arm tick at zero recoil — a tight,
/// readable reticle. The recoil **bloom** ([`gonedark_engine::recoil::crosshair_bloom`]) is added to
/// this so the arms spread under fire and pull back as the gun settles (WS-A, CP-2 game-feel bar).
const CROSSHAIR_GAP: f32 = 0.030;

/// Half-size (NDC) of each small crosshair tick (the four arm dots + the center pip).
const CROSSHAIR_DOT_HALF: f32 = 0.011;

/// Crosshair color — a pale green-white that stays legible over the muted blue-grey FPS world
/// without reading as the warm muzzle flash or the red/orange alert ring (invariant #6 legibility).
const CROSSHAIR_COLOR: [f32; 3] = [0.86, 0.96, 0.90];

/// Crosshair opacity (it is constant chrome, not a fading flash).
const CROSSHAIR_ALPHA: f32 = 0.82;

/// The RGB color for an alert kind. Color is only ONE of the two cues — [`shape_for`] carries a
/// redundant shape so the kinds stay distinct for CVD players (invariant #6: the thread back must
/// stay legible). The palette is spaced along lightness *and* the blue-yellow axis, not just warm
/// hue, so no pair collapses to the same muddy yellow-brown under deuteranopia/protanopia.
fn alert_color(kind: AlertKind) -> [f32; 3] {
    match kind {
        AlertKind::BaseUnderAttack => [1.0, 0.12, 0.12], // hot, dark urgent red
        AlertKind::TakingFire => [1.0, 0.62, 0.15],      // high-value orange (lighter than red)
        AlertKind::TerritoryLost => [0.15, 0.52, 0.58],  // darker desaturated teal (off warm axis)
        AlertKind::UnitLost => [0.88, 0.88, 0.92],       // pale grey (a death; brightest)
    }
}

/// A redundant, non-color shape cue per alert kind, threaded to the shader as an instance attr.
/// Color alone fails ~8% of CVD players and over same-hue frame regions; the shape glyph keeps the
/// four kinds distinguishable while dark (invariant #6). The id is matched by `fs_main` in
/// `hud.wgsl`: 0 = filled dot, 1 = chevron, 2 = triangle, 3 = hollow ring.
fn shape_for(kind: AlertKind) -> f32 {
    match kind {
        AlertKind::BaseUnderAttack => 2.0, // triangle — the loudest, most urgent glyph
        AlertKind::TakingFire => 1.0,      // chevron — a directional "incoming"
        AlertKind::TerritoryLost => 3.0,   // hollow ring — a place you no longer hold
        AlertKind::UnitLost => 0.0,        // dot — a single quiet loss
    }
}

/// One screen-space directional marker, ready to upload. `repr(C)` + `Pod` so it streams
/// straight into the per-instance vertex buffer; the field order/offsets MUST match the
/// instance attribute locations in `hud.wgsl` and the `vertex_attr_array` in [`HudRenderer::new`].
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, bytemuck::Pod, bytemuck::Zeroable)]
pub struct HudMarker {
    /// Marker center in normalized device coordinates ([-1,1], +y up).
    pub ndc_x: f32,
    pub ndc_y: f32,
    pub r: f32,
    pub g: f32,
    pub b: f32,
    /// Fade alpha in `[0,1]`.
    pub alpha: f32,
    /// Half-size of the marker quad in NDC.
    pub half_size: f32,
    /// Shape glyph id (redundant non-color cue): 0 dot, 1 chevron, 2 triangle, 3 ring.
    pub shape: f32,
}

/// Compute the screen-space marker for one `alert` from the avatar's pose, or `None` if the
/// alert has faded out (age beyond [`FADE_TICKS`], or its tick is in the future).
///
/// Bearing is the signed angle from the avatar's facing (`yaw`) to the alert direction:
/// `0` → straight ahead (top-center), `+` → to the right, `-` → to the left, `±π` → behind
/// (bottom-center). Pure float math (presentation boundary), so it is unit-testable.
pub fn marker_for(
    alert: &Alert,
    avatar_world: (f32, f32),
    yaw: f32,
    tick: u64,
) -> Option<HudMarker> {
    // Age-based fade. A future-stamped alert (tick < alert.tick) is treated as not-yet-live.
    if tick < alert.tick {
        return None;
    }
    let age = tick - alert.tick;
    if age >= FADE_TICKS {
        return None;
    }
    // Ease-out fade: reads full-bright immediately, then decays on an ease-out curve that holds a
    // legible [`FADE_FLOOR`] until the hard `None` at [`FADE_TICKS`] removes it. (A linear ramp
    // kept old and new pings near-equal for half their life then sank into invisible <0.1 alpha.)
    let t = age as f32 / FADE_TICKS as f32; // 0 at fresh → 1 at the cutoff
    let eased = (1.0 - t) * (1.0 - t); // ease-out: steep early read, gentle tail
    let alpha = FADE_FLOOR + (1.0 - FADE_FLOOR) * eased;

    // Direction from the avatar to the alert, in world space.
    let (ax, ay) = avatar_world;
    let dx = crate::fixed_to_f32(alert.pos.x) - ax;
    let dy = crate::fixed_to_f32(alert.pos.y) - ay;

    // World bearing of the alert, then relative to the avatar's facing. We use atan2 so a zero
    // vector (alert on top of the avatar) still yields a stable bearing (0 → straight ahead).
    let world_bearing = dy.atan2(dx);
    // Signed azimuth in (-π, π], 0 = dead ahead, + = to the right. atan2 grows
    // counter-clockwise, so "to the right" (clockwise from facing) is `yaw - world_bearing`.
    let mut azimuth = yaw - world_bearing;
    let two_pi = std::f32::consts::TAU;
    azimuth = ((azimuth + std::f32::consts::PI).rem_euclid(two_pi)) - std::f32::consts::PI;

    // Map azimuth → a point on the ring. azimuth 0 must land at top-center (ndc = (0, +R)),
    // positive azimuth must swing to the right (ndc_x > 0). With +y up that is:
    //   ndc_x =  R * sin(azimuth)
    //   ndc_y =  R * cos(azimuth)
    let ndc_x = RING_RADIUS * azimuth.sin();
    let ndc_y = RING_RADIUS * azimuth.cos();

    let [r, g, b] = alert_color(alert.kind);
    Some(HudMarker {
        ndc_x,
        ndc_y,
        r,
        g,
        b,
        alpha,
        half_size: MARKER_HALF_SIZE,
        shape: shape_for(alert.kind),
    })
}

/// Build the centered hitmarker for this frame from `last_hit_tick` (the tick the embodied avatar's
/// own shot last connected) and the current `tick`, or `None` when no hit is live (never hit, a
/// future-stamped tick, or aged past [`HITMARKER_TICKS`]).
///
/// The hitmarker is presentation feedback on the player's OWN action — it draws iff the avatar
/// itself dealt damage — so it reveals nothing about unseen enemies and stays inside invariant #6
/// (WS-4). A pure float fn (the presentation boundary) so it is unit-testable without a GPU. Placed
/// dead-center (NDC `(0, 0)`) so it never collides with the edge-ring alert markers.
pub fn hitmarker_marker(last_hit_tick: Option<u64>, tick: u64) -> Option<HudMarker> {
    let fired = last_hit_tick?;
    if tick < fired {
        return None; // future-stamped hit is not yet live
    }
    let age = tick - fired;
    if age >= HITMARKER_TICKS {
        return None;
    }
    // Linear fade: full-bright on the connecting frame, gone by HITMARKER_TICKS.
    let alpha = 1.0 - age as f32 / HITMARKER_TICKS as f32;
    let [r, g, b] = HITMARKER_COLOR;
    Some(HudMarker {
        ndc_x: 0.0,
        ndc_y: 0.0,
        r,
        g,
        b,
        alpha,
        half_size: HITMARKER_HALF_SIZE,
        shape: SHAPE_HITMARKER,
    })
}

/// Build the hip-fire **dynamic crosshair** — four arm ticks (up/down/left/right) plus a center pip —
/// for the current recoil `bloom` and viewport `aspect` (WS-A, CP-2 game-feel bar). The arms sit a
/// half-gap of `CROSSHAIR_GAP + bloom` from screen-center, so at rest (`bloom == 0`) the reticle is
/// tight and under fire it **spreads** outward and settles back as the recoil decays. The horizontal
/// gap is divided by `aspect` so the cross stays square on a wide window (the raw-NDC chrome footgun —
/// the alert markers ride a ring and dodge it, but a crosshair's symmetry needs the correction).
///
/// Pure float math (presentation boundary), so it is unit-testable without a GPU. Reveals nothing —
/// it is screen-center chrome with no world position (invariant #6).
pub fn crosshair_markers(bloom: f32, aspect: f32) -> Vec<HudMarker> {
    let aspect = if aspect.abs() < 1.0e-6 { 1.0 } else { aspect };
    let gap = (CROSSHAIR_GAP + bloom.max(0.0)).max(0.0);
    let gx = gap / aspect; // square the cross on a non-1:1 viewport
    let [r, g, b] = CROSSHAIR_COLOR;
    let tick = |ndc_x: f32, ndc_y: f32| HudMarker {
        ndc_x,
        ndc_y,
        r,
        g,
        b,
        alpha: CROSSHAIR_ALPHA,
        half_size: CROSSHAIR_DOT_HALF,
        shape: SHAPE_DOT,
    };
    vec![
        tick(0.0, 0.0),  // center pip
        tick(gx, 0.0),   // right arm
        tick(-gx, 0.0),  // left arm
        tick(0.0, gap),  // top arm
        tick(0.0, -gap), // bottom arm
    ]
}

/// A unit-quad corner in [-1, 1]^2 (the shader scales it by the per-marker half-size).
#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct QuadVertex {
    corner: [f32; 2],
}

/// The two triangles of a unit quad.
const QUAD_VERTS: [QuadVertex; 6] = [
    QuadVertex {
        corner: [-1.0, -1.0],
    },
    QuadVertex {
        corner: [1.0, -1.0],
    },
    QuadVertex { corner: [1.0, 1.0] },
    QuadVertex {
        corner: [-1.0, -1.0],
    },
    QuadVertex { corner: [1.0, 1.0] },
    QuadVertex {
        corner: [-1.0, 1.0],
    },
];

/// Screen-space directional-alert overlay for the embodied view. Owns its own pipeline +
/// buffers (a separate pipeline from the unit pass so the two never contend for a shader).
pub struct HudRenderer {
    pipeline: wgpu::RenderPipeline,
    quad_buf: wgpu::Buffer,
    /// Per-instance GPU buffer of [`HudMarker`]; reallocated only when it must grow.
    instance_buf: wgpu::Buffer,
    /// Capacity (in markers) currently allocated in `instance_buf`.
    instance_cap: usize,
}

impl HudRenderer {
    /// Build the HUD pipeline against the swapchain `surface_format`. The `device` is borrowed
    /// (D19). Uses alpha blending so markers composite over the already-rendered frame.
    pub fn new(device: &wgpu::Device, surface_format: wgpu::TextureFormat) -> Self {
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("gonedark.hud_shader"),
            source: wgpu::ShaderSource::Wgsl(include_str!("hud.wgsl").into()),
        });

        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("gonedark.hud_pipeline_layout"),
            bind_group_layouts: &[],
            immediate_size: 0,
        });

        let quad_layout = wgpu::VertexBufferLayout {
            array_stride: std::mem::size_of::<QuadVertex>() as u64,
            step_mode: wgpu::VertexStepMode::Vertex,
            attributes: &wgpu::vertex_attr_array![0 => Float32x2],
        };
        let instance_layout = wgpu::VertexBufferLayout {
            array_stride: std::mem::size_of::<HudMarker>() as u64,
            step_mode: wgpu::VertexStepMode::Instance,
            // 1=center(vec2), 2=color(vec3), 3=alpha(f32), 4=half_size(f32), 5=shape(f32).
            attributes: &wgpu::vertex_attr_array![
                1 => Float32x2,
                2 => Float32x3,
                3 => Float32,
                4 => Float32,
                5 => Float32
            ],
        };

        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("gonedark.hud_pipeline"),
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
            label: Some("gonedark.hud_quad_vbo"),
            contents: bytemuck::cast_slice(&QUAD_VERTS),
            usage: wgpu::BufferUsages::VERTEX,
        });

        let instance_cap = INITIAL_CAP;
        let instance_buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("gonedark.hud_instance_vbo"),
            size: (instance_cap * std::mem::size_of::<HudMarker>()) as u64,
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        HudRenderer {
            pipeline,
            quad_buf,
            instance_buf,
            instance_cap,
        }
    }

    /// Draw this frame's directional alert markers on top of `view` (a LOAD pass — never clears).
    ///
    /// - `alerts`: the rolling alert channel (most recent last).
    /// - `avatar_world`: the listener/avatar position in world units.
    /// - `yaw`: avatar facing (radians) — markers are placed by bearing relative to this.
    /// - `_viewport`: surface size in pixels — unused (placement is pure NDC ring math); kept on
    ///   this signature only because the `lib.rs` boundary still threads it. [`marker_for`] no
    ///   longer takes it. (If aspect correction is ever wanted, re-add a real param there.)
    /// - `tick`: the current sim tick (to fade alerts by age).
    ///
    /// Builds the live marker set via [`marker_for`], uploads it, and records a single LOAD
    /// render pass so the markers composite over the embodied frame. No-op if nothing is live.
    #[allow(clippy::too_many_arguments)]
    pub fn render(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        view: &wgpu::TextureView,
        alerts: &AlertChannel,
        avatar_world: (f32, f32),
        yaw: f32,
        _viewport: (u32, u32),
        tick: u64,
    ) {
        let markers: Vec<HudMarker> = alerts
            .recent
            .iter()
            .filter_map(|a| marker_for(a, avatar_world, yaw, tick))
            .collect();

        // No live markers → nothing to draw (and the frame must stay untouched).
        self.draw_markers(device, queue, view, &markers);
    }

    /// Draw the embodied hitmarker — the centered "X" flash confirming the player's OWN shot
    /// connected (WS-4). Builds the single live marker via [`hitmarker_marker`] from `last_hit_tick`
    /// and `tick`, then composites it as a LOAD pass over the embodied frame (same pipeline/shader as
    /// the alert markers, glyph 4). No-op when no hit is live, so it leaves the frame untouched.
    ///
    /// Invariant #6: this is feedback on the avatar's own action (the caller only stamps
    /// `last_hit_tick` when the avatar itself dealt damage), not intel about an unseen enemy — it
    /// reveals nothing the player could not already see.
    pub fn render_hitmarker(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        view: &wgpu::TextureView,
        last_hit_tick: Option<u64>,
        tick: u64,
    ) {
        let Some(marker) = hitmarker_marker(last_hit_tick, tick) else {
            return;
        };
        self.draw_markers(device, queue, view, &[marker]);
    }

    /// Draw the hip-fire **dynamic crosshair** (WS-A) — the four arm ticks + center pip, spread by the
    /// recoil `bloom` — as a LOAD pass over the embodied frame (same pipeline/shader as the alert
    /// markers, glyph 0). `aspect` keeps the cross square on a wide viewport. The host calls this only
    /// while embodied in a unit that hip-fires (infantry); the tank shows its own gun-sight reticle
    /// instead. Screen-center chrome with no world position — reveals nothing (invariant #6).
    pub fn render_crosshair(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        view: &wgpu::TextureView,
        bloom: f32,
        aspect: f32,
    ) {
        let markers = crosshair_markers(bloom, aspect);
        self.draw_markers(device, queue, view, &markers);
    }

    /// Upload `markers` and composite them as one LOAD pass over `view` (never clears). Shared by
    /// the alert overlay ([`Self::render`]) and the hitmarker ([`Self::render_hitmarker`]) so both
    /// drive the one screen-space pipeline. No-op on an empty set so the frame stays untouched.
    fn draw_markers(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        view: &wgpu::TextureView,
        markers: &[HudMarker],
    ) {
        if markers.is_empty() {
            return;
        }

        if markers.len() > self.instance_cap {
            let new_cap = markers.len().next_power_of_two();
            self.instance_buf = device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("gonedark.hud_instance_vbo"),
                size: (new_cap * std::mem::size_of::<HudMarker>()) as u64,
                usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });
            self.instance_cap = new_cap;
        }
        queue.write_buffer(&self.instance_buf, 0, bytemuck::cast_slice(markers));

        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("gonedark.hud_encoder"),
        });
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("gonedark.hud_pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        // LOAD, never Clear — this draws on top of the embodied frame.
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
            pass.draw(0..QUAD_VERTS.len() as u32, 0..markers.len() as u32);
        }

        queue.submit(std::iter::once(encoder.finish()));
    }
}

#[cfg(test)]
mod tests {
    //! `render` is the float boundary (invariant #1: floats live only in rendering), so `f32`
    //! math and epsilon comparisons are fair game. `HudRenderer::new` needs a real `wgpu::Device`
    //! (no display in CI), so the pipeline path is untested; the testable placement/fade math is
    //! factored into [`marker_for`].

    use super::*;
    use gonedark_core::components::Vec2;
    use gonedark_core::fixed::Fixed;

    fn alert(kind: AlertKind, x: i32, y: i32, tick: u64) -> Alert {
        Alert {
            kind,
            pos: Vec2::new(Fixed::from_int(x), Fixed::from_int(y)),
            tick,
        }
    }

    // ---- bearing → placement ----

    #[test]
    fn dead_ahead_is_top_center() {
        // Avatar at origin facing +x (yaw = 0); alert straight ahead along +x.
        let a = alert(AlertKind::TakingFire, 10, 0, 0);
        let m = marker_for(&a, (0.0, 0.0), 0.0, 0).expect("fresh alert is live");
        // Top-center: small |ndc_x|, ndc_y near +RING_RADIUS (top of screen).
        assert!(m.ndc_x.abs() < 1e-3, "ndc_x={} should be ~0", m.ndc_x);
        assert!(m.ndc_y > 0.5, "ndc_y={} should be near top", m.ndc_y);
    }

    #[test]
    fn alert_to_the_right_has_positive_ndc_x() {
        // Avatar at origin facing +x; alert to the avatar's right is at -y (screen-right when
        // facing +x with +y "left/up" in world). Direction (0,-10) relative to yaw 0 → azimuth
        // -π/2 maps to... use a direction that is unambiguously to the right of facing:
        // facing +x, "right" in world for +y-up convention is -y.
        let a = alert(AlertKind::TakingFire, 0, -10, 0);
        let m = marker_for(&a, (0.0, 0.0), 0.0, 0).unwrap();
        assert!(m.ndc_x > 0.0, "ndc_x={} should be on the right", m.ndc_x);
    }

    #[test]
    fn alert_to_the_left_has_negative_ndc_x() {
        // Facing +x; alert at +y is to the avatar's left.
        let a = alert(AlertKind::TakingFire, 0, 10, 0);
        let m = marker_for(&a, (0.0, 0.0), 0.0, 0).unwrap();
        assert!(m.ndc_x < 0.0, "ndc_x={} should be on the left", m.ndc_x);
    }

    #[test]
    fn alert_behind_is_bottom_center() {
        // Facing +x; alert at -x is directly behind → azimuth ±π → bottom-center.
        let a = alert(AlertKind::TakingFire, -10, 0, 0);
        let m = marker_for(&a, (0.0, 0.0), 0.0, 0).unwrap();
        assert!(m.ndc_x.abs() < 1e-3, "ndc_x={} should be ~0", m.ndc_x);
        assert!(m.ndc_y < -0.5, "ndc_y={} should be near bottom", m.ndc_y);
    }

    #[test]
    fn yaw_rotates_placement() {
        // Alert along +x, but avatar now faces +x rotated by +π/2 (faces +y). The alert is now
        // to the avatar's right → ndc_x > 0.
        let a = alert(AlertKind::TakingFire, 10, 0, 0);
        let m = marker_for(&a, (0.0, 0.0), std::f32::consts::FRAC_PI_2, 0).unwrap();
        assert!(
            m.ndc_x > 0.0,
            "ndc_x={} should swing right under yaw",
            m.ndc_x
        );
    }

    // ---- fade by age ----

    #[test]
    fn fresh_alert_is_full_alpha() {
        let a = alert(AlertKind::UnitLost, 5, 5, 100);
        let m = marker_for(&a, (0.0, 0.0), 0.0, 100).unwrap();
        assert!(
            (m.alpha - 1.0).abs() < 1e-4,
            "alpha={} should be ~1",
            m.alpha
        );
    }

    #[test]
    fn aging_alert_fades_toward_floor() {
        // The ease-out curve must decrease monotonically with age, but never below FADE_FLOOR
        // while the marker is still live — it stays legible right up to the hard cutoff.
        let a = alert(AlertKind::UnitLost, 5, 5, 0);
        let young = marker_for(&a, (0.0, 0.0), 0.0, 10).unwrap();
        let mid = marker_for(&a, (0.0, 0.0), 0.0, FADE_TICKS / 2).unwrap();
        let old = marker_for(&a, (0.0, 0.0), 0.0, FADE_TICKS - 1).unwrap();
        assert!(young.alpha > mid.alpha, "alpha should decrease with age");
        assert!(mid.alpha > old.alpha, "alpha should keep decreasing");
        // Floored: even the oldest live marker stays >= the legibility floor (not invisible).
        assert!(
            old.alpha >= FADE_FLOOR,
            "near-end alpha {} should hold the floor {}",
            old.alpha,
            FADE_FLOOR
        );
        // ...and never exceeds 1.0.
        assert!(young.alpha <= 1.0, "alpha must stay <= 1");
    }

    #[test]
    fn faded_out_alert_is_none() {
        let a = alert(AlertKind::TakingFire, 5, 5, 0);
        assert!(marker_for(&a, (0.0, 0.0), 0.0, FADE_TICKS).is_none());
        assert!(marker_for(&a, (0.0, 0.0), 0.0, FADE_TICKS + 50).is_none());
    }

    #[test]
    fn future_alert_is_none() {
        // tick < alert.tick (not yet live) yields None rather than a negative age.
        let a = alert(AlertKind::TakingFire, 5, 5, 100);
        assert!(marker_for(&a, (0.0, 0.0), 0.0, 50).is_none());
    }

    #[test]
    fn color_tracks_kind() {
        let p = (0.0, 0.0);
        let kinds = [
            AlertKind::TakingFire,
            AlertKind::UnitLost,
            AlertKind::BaseUnderAttack,
            AlertKind::TerritoryLost,
        ];
        // Each kind's marker stamps exactly its `alert_color`.
        let cols: Vec<[f32; 3]> = kinds
            .iter()
            .map(|&k| {
                let m = marker_for(&alert(k, 1, 0, 0), p, 0.0, 0).unwrap();
                assert_eq!([m.r, m.g, m.b], alert_color(k), "color must track {:?}", k);
                [m.r, m.g, m.b]
            })
            .collect();
        // All four colors are pairwise distinct (no two kinds share an RGB).
        for i in 0..cols.len() {
            for j in (i + 1)..cols.len() {
                assert_ne!(
                    cols[i], cols[j],
                    "{:?} and {:?} must not share a color",
                    kinds[i], kinds[j]
                );
            }
        }
    }

    /// Relative luminance (Rec. 709) of an RGB triple — the channel a CVD viewer still reads.
    fn luminance([r, g, b]: [f32; 3]) -> f32 {
        0.2126 * r + 0.7152 * g + 0.0722 * b
    }

    #[test]
    fn alert_palette_separates_by_luminance() {
        // The accessibility fix: every pair of kinds must differ in lightness, not just hue, so
        // they don't collapse under red-green CVD (invariant #6's thread back must stay legible).
        let kinds = [
            AlertKind::TakingFire,
            AlertKind::UnitLost,
            AlertKind::BaseUnderAttack,
            AlertKind::TerritoryLost,
        ];
        for i in 0..kinds.len() {
            for j in (i + 1)..kinds.len() {
                let li = luminance(alert_color(kinds[i]));
                let lj = luminance(alert_color(kinds[j]));
                assert!(
                    (li - lj).abs() > 0.05,
                    "{:?} (L={:.3}) and {:?} (L={:.3}) need a luminance spread",
                    kinds[i],
                    li,
                    kinds[j],
                    lj
                );
            }
        }
    }

    #[test]
    fn shape_tracks_kind() {
        // The redundant non-color cue: each kind stamps a distinct shape glyph id, so the four
        // kinds stay distinguishable for CVD players / same-hue frame regions (invariant #6).
        let p = (0.0, 0.0);
        let kinds = [
            AlertKind::TakingFire,
            AlertKind::UnitLost,
            AlertKind::BaseUnderAttack,
            AlertKind::TerritoryLost,
        ];
        let shapes: Vec<f32> = kinds
            .iter()
            .map(|&k| {
                let m = marker_for(&alert(k, 1, 0, 0), p, 0.0, 0).unwrap();
                assert_eq!(m.shape, shape_for(k), "shape must track {:?}", k);
                m.shape
            })
            .collect();
        // All four shape ids are pairwise distinct.
        for i in 0..shapes.len() {
            for j in (i + 1)..shapes.len() {
                assert_ne!(
                    shapes[i], shapes[j],
                    "{:?} and {:?} must not share a shape",
                    kinds[i], kinds[j]
                );
            }
        }
    }

    // ---- hitmarker (WS-4) ----

    #[test]
    fn no_hit_yet_is_no_marker() {
        assert!(hitmarker_marker(None, 0).is_none());
        assert!(hitmarker_marker(None, 500).is_none());
    }

    #[test]
    fn fresh_hit_is_centered_full_bright_x() {
        let m = hitmarker_marker(Some(100), 100).expect("a hit this tick is live");
        // Dead center so it never collides with the edge-ring alert markers.
        assert!(m.ndc_x.abs() < 1e-6 && m.ndc_y.abs() < 1e-6, "centered");
        assert!((m.alpha - 1.0).abs() < 1e-4, "alpha={} should be ~1", m.alpha);
        // The distinct hitmarker glyph + bright white color the viz pixel-assert keys on.
        assert_eq!(m.shape, SHAPE_HITMARKER);
        assert_eq!([m.r, m.g, m.b], HITMARKER_COLOR);
    }

    #[test]
    fn hitmarker_fades_monotonically_then_vanishes() {
        let young = hitmarker_marker(Some(0), 1).unwrap();
        let mid = hitmarker_marker(Some(0), HITMARKER_TICKS / 2).unwrap();
        let old = hitmarker_marker(Some(0), HITMARKER_TICKS - 1).unwrap();
        assert!(young.alpha > mid.alpha && mid.alpha > old.alpha, "fades with age");
        assert!(old.alpha > 0.0 && young.alpha <= 1.0, "stays in (0, 1]");
        // Aged past the window → gone (the frame is left untouched).
        assert!(hitmarker_marker(Some(0), HITMARKER_TICKS).is_none());
        assert!(hitmarker_marker(Some(0), HITMARKER_TICKS + 50).is_none());
    }

    #[test]
    fn future_stamped_hit_is_none() {
        // tick < last_hit_tick (clock not yet there) yields None rather than a negative age.
        assert!(hitmarker_marker(Some(100), 50).is_none());
    }

    // ---- dynamic crosshair (WS-A) ----

    #[test]
    fn crosshair_has_center_pip_plus_four_arms() {
        let m = crosshair_markers(0.0, 1.0);
        assert_eq!(m.len(), 5, "center + 4 arms");
        // The first is the center pip.
        assert!(m[0].ndc_x.abs() < 1e-9 && m[0].ndc_y.abs() < 1e-9, "center pip at origin");
        // Every tick is the dot glyph in the crosshair color.
        for t in &m {
            assert_eq!(t.shape, SHAPE_DOT);
            assert_eq!([t.r, t.g, t.b], CROSSHAIR_COLOR);
        }
        // At rest the arms sit at the resting gap.
        let right = m.iter().find(|t| t.ndc_x > 0.0).unwrap();
        assert!((right.ndc_x - CROSSHAIR_GAP).abs() < 1e-6, "rest gap at aspect 1");
    }

    #[test]
    fn crosshair_blooms_outward_with_recoil() {
        let rest = crosshair_markers(0.0, 1.0);
        let fired = crosshair_markers(0.05, 1.0);
        let rest_right = rest.iter().find(|t| t.ndc_x > 0.0).unwrap().ndc_x;
        let fired_right = fired.iter().find(|t| t.ndc_x > 0.0).unwrap().ndc_x;
        let rest_top = rest.iter().find(|t| t.ndc_y > 0.0).unwrap().ndc_y;
        let fired_top = fired.iter().find(|t| t.ndc_y > 0.0).unwrap().ndc_y;
        assert!(fired_right > rest_right, "horizontal arms spread under fire");
        assert!(fired_top > rest_top, "vertical arms spread under fire");
        // The bloom adds exactly the recoil bloom to the gap (aspect 1).
        assert!((fired_top - (CROSSHAIR_GAP + 0.05)).abs() < 1e-6);
    }

    #[test]
    fn crosshair_stays_square_on_a_wide_window() {
        // aspect 16:9 → the horizontal gap is divided by aspect so the on-screen cross is square
        // (the same raw-NDC chrome footgun the alert ring dodges). Multiply the x gap back by aspect
        // and it matches the y gap.
        let aspect = 16.0 / 9.0;
        let m = crosshair_markers(0.02, aspect);
        let right = m.iter().find(|t| t.ndc_x > 0.0).unwrap().ndc_x;
        let top = m.iter().find(|t| t.ndc_y > 0.0).unwrap().ndc_y;
        assert!((right * aspect - top).abs() < 1e-6, "x·aspect ({right}) matches y ({top})");
    }

    #[test]
    fn crosshair_negative_bloom_does_not_pull_arms_inside_rest() {
        // A stray negative recoil can never collapse the reticle past its resting gap.
        let m = crosshair_markers(-1.0, 1.0);
        let right = m.iter().find(|t| t.ndc_x > 0.0).unwrap().ndc_x;
        assert!((right - CROSSHAIR_GAP).abs() < 1e-6, "floors at the resting gap");
    }

    /// Validate `hud.wgsl` offline with naga (the compiler wgpu uses), so a WGSL regression fails
    /// the test suite instead of only blowing up at pipeline creation on a real GPU.
    #[test]
    fn hud_wgsl_parses_and_validates() {
        let src = include_str!("hud.wgsl");
        let module = naga::front::wgsl::parse_str(src).expect("hud.wgsl must parse");
        let mut validator = naga::valid::Validator::new(
            naga::valid::ValidationFlags::all(),
            naga::valid::Capabilities::all(),
        );
        validator.validate(&module).expect("hud.wgsl must validate");
    }
}
