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
use gonedark_core::fixed::Fixed;
use wgpu::util::DeviceExt;

/// Convert a Q16.16 fixed value to `f32` (the sanctioned fixed→float hop; presentation-only).
#[inline]
fn fixed_to_f32(v: Fixed) -> f32 {
    v.to_bits() as f32 / Fixed::SCALE as f32
}

/// How many ticks an alert stays on the HUD before it has fully faded out. At 60 Hz this is a
/// ~2 s decay — long enough to read a direction, short enough that the thread stays *thin*.
const FADE_TICKS: u64 = 120;

/// Half-size of a marker quad in NDC (markers are small screen-space chevrons/dots).
const MARKER_HALF_SIZE: f32 = 0.045;

/// Radius (in NDC) of the ring the markers sit on — near, but inside, the screen edge.
const RING_RADIUS: f32 = 0.82;

/// Initial GPU capacity (in markers) for the instance buffer.
const INITIAL_CAP: usize = 16;

/// The RGB color for an alert kind. Hostile-warm for fire, dim for losses, urgent for base.
fn alert_color(kind: AlertKind) -> [f32; 3] {
    match kind {
        AlertKind::TakingFire => [1.0, 0.35, 0.2],      // hot orange-red
        AlertKind::UnitLost => [0.85, 0.85, 0.9],       // pale grey (a death)
        AlertKind::BaseUnderAttack => [1.0, 0.15, 0.15], // urgent red
        AlertKind::TerritoryLost => [0.9, 0.7, 0.2],    // amber
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
    _viewport: (u32, u32),
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
    let alpha = 1.0 - (age as f32 / FADE_TICKS as f32);

    // Direction from the avatar to the alert, in world space.
    let (ax, ay) = avatar_world;
    let dx = fixed_to_f32(alert.pos.x) - ax;
    let dy = fixed_to_f32(alert.pos.y) - ay;

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
    })
}

/// A unit-quad corner in [-1, 1]^2 (the shader scales it by the per-marker half-size).
#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct QuadVertex {
    corner: [f32; 2],
}

/// The two triangles of a unit quad.
const QUAD_VERTS: [QuadVertex; 6] = [
    QuadVertex { corner: [-1.0, -1.0] },
    QuadVertex { corner: [1.0, -1.0] },
    QuadVertex { corner: [1.0, 1.0] },
    QuadVertex { corner: [-1.0, -1.0] },
    QuadVertex { corner: [1.0, 1.0] },
    QuadVertex { corner: [-1.0, 1.0] },
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
            // 1=center(vec2), 2=color(vec3), 3=alpha(f32), 4=half_size(f32).
            attributes: &wgpu::vertex_attr_array![
                1 => Float32x2,
                2 => Float32x3,
                3 => Float32,
                4 => Float32
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
    /// - `viewport`: surface size in pixels (for the screen-space projection).
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
        viewport: (u32, u32),
        tick: u64,
    ) {
        let markers: Vec<HudMarker> = alerts
            .recent
            .iter()
            .filter_map(|a| marker_for(a, avatar_world, yaw, viewport, tick))
            .collect();

        // No live markers → nothing to draw (and the frame must stay untouched).
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
        queue.write_buffer(&self.instance_buf, 0, bytemuck::cast_slice(&markers));

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

    const VIEWPORT: (u32, u32) = (1920, 1080);

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
        let m = marker_for(&a, (0.0, 0.0), 0.0, VIEWPORT, 0).expect("fresh alert is live");
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
        let m = marker_for(&a, (0.0, 0.0), 0.0, VIEWPORT, 0).unwrap();
        assert!(m.ndc_x > 0.0, "ndc_x={} should be on the right", m.ndc_x);
    }

    #[test]
    fn alert_to_the_left_has_negative_ndc_x() {
        // Facing +x; alert at +y is to the avatar's left.
        let a = alert(AlertKind::TakingFire, 0, 10, 0);
        let m = marker_for(&a, (0.0, 0.0), 0.0, VIEWPORT, 0).unwrap();
        assert!(m.ndc_x < 0.0, "ndc_x={} should be on the left", m.ndc_x);
    }

    #[test]
    fn alert_behind_is_bottom_center() {
        // Facing +x; alert at -x is directly behind → azimuth ±π → bottom-center.
        let a = alert(AlertKind::TakingFire, -10, 0, 0);
        let m = marker_for(&a, (0.0, 0.0), 0.0, VIEWPORT, 0).unwrap();
        assert!(m.ndc_x.abs() < 1e-3, "ndc_x={} should be ~0", m.ndc_x);
        assert!(m.ndc_y < -0.5, "ndc_y={} should be near bottom", m.ndc_y);
    }

    #[test]
    fn yaw_rotates_placement() {
        // Alert along +x, but avatar now faces +x rotated by +π/2 (faces +y). The alert is now
        // to the avatar's right → ndc_x > 0.
        let a = alert(AlertKind::TakingFire, 10, 0, 0);
        let m = marker_for(&a, (0.0, 0.0), std::f32::consts::FRAC_PI_2, VIEWPORT, 0).unwrap();
        assert!(m.ndc_x > 0.0, "ndc_x={} should swing right under yaw", m.ndc_x);
    }

    // ---- fade by age ----

    #[test]
    fn fresh_alert_is_full_alpha() {
        let a = alert(AlertKind::UnitLost, 5, 5, 100);
        let m = marker_for(&a, (0.0, 0.0), 0.0, VIEWPORT, 100).unwrap();
        assert!((m.alpha - 1.0).abs() < 1e-4, "alpha={} should be ~1", m.alpha);
    }

    #[test]
    fn aging_alert_fades_toward_zero() {
        let a = alert(AlertKind::UnitLost, 5, 5, 0);
        let young = marker_for(&a, (0.0, 0.0), 0.0, VIEWPORT, 10).unwrap();
        let old = marker_for(&a, (0.0, 0.0), 0.0, VIEWPORT, FADE_TICKS - 1).unwrap();
        assert!(young.alpha > old.alpha, "alpha should decrease with age");
        assert!(old.alpha > 0.0 && old.alpha < 0.1, "near-end alpha low");
    }

    #[test]
    fn faded_out_alert_is_none() {
        let a = alert(AlertKind::TakingFire, 5, 5, 0);
        assert!(marker_for(&a, (0.0, 0.0), 0.0, VIEWPORT, FADE_TICKS).is_none());
        assert!(marker_for(&a, (0.0, 0.0), 0.0, VIEWPORT, FADE_TICKS + 50).is_none());
    }

    #[test]
    fn future_alert_is_none() {
        // tick < alert.tick (not yet live) yields None rather than a negative age.
        let a = alert(AlertKind::TakingFire, 5, 5, 100);
        assert!(marker_for(&a, (0.0, 0.0), 0.0, VIEWPORT, 50).is_none());
    }

    #[test]
    fn color_tracks_kind() {
        let p = (0.0, 0.0);
        let fire = marker_for(&alert(AlertKind::TakingFire, 1, 0, 0), p, 0.0, VIEWPORT, 0).unwrap();
        let base = marker_for(&alert(AlertKind::BaseUnderAttack, 1, 0, 0), p, 0.0, VIEWPORT, 0)
            .unwrap();
        assert_eq!([fire.r, fire.g, fire.b], alert_color(AlertKind::TakingFire));
        assert_eq!([base.r, base.g, base.b], alert_color(AlertKind::BaseUnderAttack));
        assert_ne!([fire.r, fire.g, fire.b], [base.r, base.g, base.b]);
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
