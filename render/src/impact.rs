//! Embodied **bullet-impact VFX** (WS-A, CP-2 game-feel bar) — a short spark/dust burst at the point
//! the avatar's OWN shot landed, the muzzle-flash's downrange twin. It is the render half of the hit
//! feedback (the engine derives the hit point + the fade clock from the avatar-source
//! `SimEvent::Damaged` stream); this module turns "a burst of intensity `i` at NDC `(x, y)`" into an
//! **additive** screen-space flash so it reads as light. Two elements per burst: a hot radial **core**
//! flash that shrinks as it fades, and an **expanding ring** of dust that grows as it ages.
//!
//! Invariant #4: this is the **float side** — every number is `f32` host-side presentation, never
//! `core` sim state, and the renderer only READS the burst params it is handed. Invariant #6: the
//! burst sits at a point the player *just shot at* (their own action), not intel about an unseen
//! enemy — feedback, never a reveal. Like [`scope`](crate::scope)/[`hud`](crate::hud) all the
//! geometry math lives in pure free fns so it is unit-testable without a GPU; only
//! [`ImpactRenderer::render`] needs a device.

use wgpu::util::DeviceExt;

/// Shader `shape` ids — must match the `fs_main` branches in `impact.wgsl`.
const SHAPE_CORE: f32 = 0.0;
const SHAPE_RING: f32 = 1.0;

/// How many sim ticks an impact burst stays alive after the shot landed, fading linearly to nothing.
/// At 60 Hz this is a ~0.15 s spark — long enough to register the strike, short enough to feel crisp
/// and not smear across sustained fire (matched to the muzzle-flash window's order of magnitude).
pub const IMPACT_TICKS: u64 = 9;

/// Core-flash radius (NDC half-height) at full intensity — the bright strike point. It shrinks with
/// intensity as the burst fades.
const CORE_R: f32 = 0.040;

/// Expanding dust-ring radius (NDC half-height): grows from `RING_R0` (fresh) to `RING_R1` (fully
/// aged) so the dust visibly puffs outward.
const RING_R0: f32 = 0.025;
const RING_R1: f32 = 0.085;

/// Warm spark color (orange-white) — distinct from the cool blue-grey FPS world and the pale-green
/// reticle, so the strike reads as a hot impact (invariant #6 legibility, not intel).
const IMPACT_COLOR: [f32; 3] = [1.0, 0.80, 0.45];

/// The impact burst intensity in `[0, 1]` for the current `tick`, given the tick the avatar's shot
/// last landed on (`None` → no burst). Fresh strike → `1.0`, linear ramp to `0.0` over
/// [`IMPACT_TICKS`]; a future-stamped or long-past hit is dark. Pure float math (presentation
/// boundary), so it is unit-testable without a GPU. The fade twin of `world::muzzle_flash_intensity`.
pub fn impact_intensity(last_impact_tick: Option<u64>, tick: u64) -> f32 {
    let Some(landed) = last_impact_tick else {
        return 0.0;
    };
    if tick < landed {
        return 0.0; // future-stamped hit is not yet live
    }
    let age = tick - landed;
    if age >= IMPACT_TICKS {
        return 0.0;
    }
    1.0 - age as f32 / IMPACT_TICKS as f32
}

/// One impact-burst element ready to upload. `repr(C)` + `Pod` so it streams into the per-instance
/// vertex buffer; field order MUST match the instance attribute locations in `impact.wgsl` and the
/// `vertex_attr_array` in [`ImpactRenderer::new`].
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, bytemuck::Pod, bytemuck::Zeroable)]
pub struct ImpactInstance {
    /// Center in NDC ([-1,1], +y up).
    pub ndc_x: f32,
    pub ndc_y: f32,
    /// Per-axis NDC half-size (round elements stay circular under aspect).
    pub half_x: f32,
    pub half_y: f32,
    pub r: f32,
    pub g: f32,
    pub b: f32,
    pub a: f32,
    /// Shape id: 0 core flash, 1 expanding ring.
    pub shape: f32,
}

/// An NDC-y radius → per-axis half-size that keeps a circle round on a non-square viewport (the same
/// rule as the scope overlay): NDC spans 2.0 per axis, so an `r`-tall circle is `r/aspect` wide.
#[inline]
fn round_half(r: f32, aspect: f32) -> (f32, f32) {
    let a = if aspect.abs() < 1.0e-6 { 1.0 } else { aspect };
    (r / a, r)
}

/// Build the impact burst's screen-space instances at NDC `(ndc_x, ndc_y)` for `intensity` in
/// `[0, 1]` and viewport `aspect`. Returns **empty** when `intensity <= 0` (so the renderer no-ops
/// once the burst has faded). The **core** flash shrinks with intensity; the **ring** grows as the
/// burst ages (`age_t = 1 - intensity`), both fading their alpha with intensity. Round elements are
/// aspect-corrected via [`round_half`]. Pure float math — host-testable without a GPU.
pub fn impact_instances(ndc_x: f32, ndc_y: f32, intensity: f32, aspect: f32) -> Vec<ImpactInstance> {
    let i = intensity.clamp(0.0, 1.0);
    if i <= 0.0 {
        return Vec::new();
    }
    let age_t = 1.0 - i; // 0 fresh → 1 fully aged
    let [r, g, b] = IMPACT_COLOR;

    // Core: bright + tight at the strike, shrinking + dimming as it fades.
    let (chx, chy) = round_half(CORE_R * i, aspect);
    let core = ImpactInstance {
        ndc_x,
        ndc_y,
        half_x: chx,
        half_y: chy,
        r,
        g,
        b,
        a: i,
        shape: SHAPE_CORE,
    };

    // Ring: dust puffing outward — radius grows with age, alpha fades a touch faster than the core.
    let ring_r = RING_R0 + (RING_R1 - RING_R0) * age_t;
    let (rhx, rhy) = round_half(ring_r, aspect);
    let ring = ImpactInstance {
        ndc_x,
        ndc_y,
        half_x: rhx,
        half_y: rhy,
        r,
        g,
        b,
        a: i * 0.6,
        shape: SHAPE_RING,
    };

    vec![core, ring]
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

/// Screen-space embodied impact-VFX overlay (its own pipeline + buffers, like [`scope`](crate::scope)
/// and [`hud`](crate::hud)). Recorded as an ADDITIVE LOAD pass so the burst adds light over the dark
/// embodied frame, never clearing it.
pub struct ImpactRenderer {
    pipeline: wgpu::RenderPipeline,
    quad_buf: wgpu::Buffer,
    instance_buf: wgpu::Buffer,
    instance_cap: usize,
}

impl ImpactRenderer {
    /// Build the pipeline against the swapchain `surface_format` (additive LOAD overlay).
    pub fn new(device: &wgpu::Device, surface_format: wgpu::TextureFormat) -> Self {
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("gonedark.impact_shader"),
            source: wgpu::ShaderSource::Wgsl(include_str!("impact.wgsl").into()),
        });

        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("gonedark.impact_pipeline_layout"),
            bind_group_layouts: &[],
            immediate_size: 0,
        });

        let quad_layout = wgpu::VertexBufferLayout {
            array_stride: std::mem::size_of::<QuadVertex>() as u64,
            step_mode: wgpu::VertexStepMode::Vertex,
            attributes: &wgpu::vertex_attr_array![0 => Float32x2],
        };
        let instance_layout = wgpu::VertexBufferLayout {
            array_stride: std::mem::size_of::<ImpactInstance>() as u64,
            step_mode: wgpu::VertexStepMode::Instance,
            // 1=center(vec2), 2=half(vec2), 3=color(vec4), 4=shape(f32).
            attributes: &wgpu::vertex_attr_array![
                1 => Float32x2,
                2 => Float32x2,
                3 => Float32x4,
                4 => Float32
            ],
        };

        // Additive blend: src is premultiplied in the shader, so add it straight onto the frame.
        let additive = wgpu::BlendState {
            color: wgpu::BlendComponent {
                src_factor: wgpu::BlendFactor::One,
                dst_factor: wgpu::BlendFactor::One,
                operation: wgpu::BlendOperation::Add,
            },
            alpha: wgpu::BlendComponent {
                src_factor: wgpu::BlendFactor::One,
                dst_factor: wgpu::BlendFactor::One,
                operation: wgpu::BlendOperation::Add,
            },
        };

        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("gonedark.impact_pipeline"),
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
                    blend: Some(additive),
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
            label: Some("gonedark.impact_quad_vbo"),
            contents: bytemuck::cast_slice(&QUAD_VERTS),
            usage: wgpu::BufferUsages::VERTEX,
        });

        let instance_cap = INITIAL_CAP;
        let instance_buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("gonedark.impact_instance_vbo"),
            size: (instance_cap * std::mem::size_of::<ImpactInstance>()) as u64,
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        ImpactRenderer {
            pipeline,
            quad_buf,
            instance_buf,
            instance_cap,
        }
    }

    /// Draw the impact burst at NDC `(ndc_x, ndc_y)` for `intensity` over `view` (an additive LOAD
    /// pass — never clears). Builds the live instance set via [`impact_instances`], uploads it, and
    /// records one render pass. A no-op once the burst has faded (`intensity <= 0` → empty builder).
    pub fn render(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        view: &wgpu::TextureView,
        ndc_x: f32,
        ndc_y: f32,
        intensity: f32,
        aspect: f32,
    ) {
        let instances = impact_instances(ndc_x, ndc_y, intensity, aspect);
        if instances.is_empty() {
            return;
        }
        if instances.len() > self.instance_cap {
            let new_cap = instances.len().next_power_of_two();
            self.instance_buf = device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("gonedark.impact_instance_vbo"),
                size: (new_cap * std::mem::size_of::<ImpactInstance>()) as u64,
                usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });
            self.instance_cap = new_cap;
        }
        queue.write_buffer(&self.instance_buf, 0, bytemuck::cast_slice(&instances));

        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("gonedark.impact_encoder"),
        });
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("gonedark.impact_pass"),
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
    //! `render` is the float boundary (invariant #1), so f32 math is fair game. `ImpactRenderer::new`
    //! needs a real `wgpu::Device` (no display in CI), so the pipeline path is untested; the testable
    //! placement/fade math is the pure free fns below.

    use super::*;

    const EPS: f32 = 1e-4;

    // ---- fade ----

    #[test]
    fn no_hit_means_no_burst() {
        assert_eq!(impact_intensity(None, 100), 0.0);
    }

    #[test]
    fn fresh_hit_is_full_intensity() {
        assert!((impact_intensity(Some(50), 50) - 1.0).abs() < EPS);
    }

    #[test]
    fn intensity_decays_monotonically_then_vanishes() {
        let young = impact_intensity(Some(0), 1);
        let mid = impact_intensity(Some(0), IMPACT_TICKS / 2);
        let old = impact_intensity(Some(0), IMPACT_TICKS - 1);
        assert!(young > mid && mid > old, "fades with age");
        assert!(old > 0.0, "still lit just before the cutoff");
        assert_eq!(impact_intensity(Some(0), IMPACT_TICKS), 0.0, "gone at the cutoff");
        assert_eq!(impact_intensity(Some(0), IMPACT_TICKS + 100), 0.0);
    }

    #[test]
    fn future_stamped_hit_is_dark() {
        assert_eq!(impact_intensity(Some(100), 50), 0.0);
    }

    // ---- builder ----

    #[test]
    fn faded_burst_builds_nothing() {
        assert!(impact_instances(0.0, 0.0, 0.0, 1.0).is_empty());
        assert!(impact_instances(0.0, 0.0, -1.0, 1.0).is_empty());
    }

    #[test]
    fn live_burst_emits_core_and_ring_at_the_hit_point() {
        let inst = impact_instances(0.3, -0.2, 1.0, 1.0);
        assert_eq!(inst.len(), 2, "core + ring");
        assert_eq!(inst[0].shape, SHAPE_CORE);
        assert_eq!(inst[1].shape, SHAPE_RING);
        for e in &inst {
            assert!((e.ndc_x - 0.3).abs() < EPS && (e.ndc_y + 0.2).abs() < EPS, "centered on the hit");
        }
    }

    #[test]
    fn core_shrinks_and_ring_grows_as_the_burst_ages() {
        let fresh = impact_instances(0.0, 0.0, 1.0, 1.0);
        let aged = impact_instances(0.0, 0.0, 0.2, 1.0);
        // Core (element 0) shrinks with intensity.
        assert!(aged[0].half_y < fresh[0].half_y, "core shrinks as it fades");
        // Ring (element 1) grows as it ages.
        assert!(aged[1].half_y > fresh[1].half_y, "dust ring puffs outward");
        // Alpha fades with intensity.
        assert!(aged[0].a < fresh[0].a, "alpha fades with intensity");
    }

    #[test]
    fn round_elements_stay_circular_on_a_wide_window() {
        let aspect = 16.0 / 9.0;
        let inst = impact_instances(0.0, 0.0, 1.0, aspect);
        for e in &inst {
            assert!((e.half_x - e.half_y / aspect).abs() < EPS, "round per-axis on a wide window");
        }
    }

    #[test]
    fn degenerate_aspect_does_not_divide_by_zero() {
        let inst = impact_instances(0.0, 0.0, 1.0, 0.0);
        for e in &inst {
            assert!(e.half_x.is_finite() && e.half_y.is_finite(), "no inf half-size at aspect 0");
        }
    }

    /// Validate `impact.wgsl` offline with naga (the compiler wgpu uses), so a WGSL regression fails
    /// the suite instead of only blowing up at pipeline creation on a real GPU.
    #[test]
    fn impact_wgsl_parses_and_validates() {
        let src = include_str!("impact.wgsl");
        let module = naga::front::wgsl::parse_str(src).expect("impact.wgsl must parse");
        let mut validator = naga::valid::Validator::new(
            naga::valid::ValidationFlags::all(),
            naga::valid::Capabilities::all(),
        );
        validator.validate(&module).expect("impact.wgsl must validate");
    }
}
