//! Embodied **bullet-impact VFX** (WS-A, CP-2 game-feel bar) — a short spark/dust burst at the point
//! the avatar's OWN shot landed, the muzzle-flash's downrange twin. It is the render half of the hit
//! feedback (the engine derives the hit point + the fade clock from the avatar-source
//! `SimEvent::Damaged` stream); this module turns "a burst of intensity `i` at NDC `(x, y)`" into an
//! **additive** screen-space flash so it reads as light. Three elements per burst: a hot near-white
//! **flash core** that snaps out on a punchy `i²` curve, a ring of crisp **spark embers** flung
//! radially outward, and a soft **dust puff** that expands as it ages — together a sharp, readable
//! strike against the dark earthy ground rather than a single soft blob.
//!
//! Invariant #4: this is the **float side** — every number is `f32` host-side presentation, never
//! `core` sim state, and the renderer only READS the burst params it is handed. Invariant #6: the
//! burst sits at a point the player *just shot at* (their own action), not intel about an unseen
//! enemy — feedback, never a reveal. Like [`scope`](crate::scope)/[`hud`](crate::hud) all the
//! geometry math lives in pure free fns so it is unit-testable without a GPU; only
//! [`ImpactRenderer::render`] needs a device.

use wgpu::util::DeviceExt;

/// Shader `shape` ids — must match the `fs_main` branches in `impact.wgsl`.
const SHAPE_CORE: f32 = 0.0; // hot flash core
const SHAPE_RING: f32 = 1.0; // expanding dust puff
const SHAPE_SPARK: f32 = 2.0; // crisp flying ember

/// How many sim ticks an impact burst stays alive after the shot landed, fading linearly to nothing.
/// At 60 Hz this is a ~0.15 s spark — long enough to register the strike, short enough to feel crisp
/// and not smear across sustained fire (matched to the muzzle-flash window's order of magnitude).
pub const IMPACT_TICKS: u64 = 9;

/// Hot-flash radius (NDC half-height) at full intensity — the bright strike point. It shrinks toward
/// a hot pinpoint (never fully to zero) as the burst fades, so the strike never wholly disappears
/// before the alpha snaps out.
const FLASH_R: f32 = 0.052;

/// Expanding dust-puff radius (NDC half-height): grows from `RING_R0` (fresh) to `RING_R1` (fully
/// aged) so the dust visibly kicks outward from the strike.
const RING_R0: f32 = 0.030;
const RING_R1: f32 = 0.105;

/// Spark-ember burst: `SPARK_COUNT` crisp dots flung radially out of the strike, each riding from
/// `SPARK_R0` (fresh, at the impact) to `SPARK_R1` (aged, flung out) along a `sqrt` ease so they
/// snap outward early then settle. An odd count + a golden-ratio jitter keeps the burst from reading
/// as a tidy arcade ring. `SPARK_DOT_R` is the head-dot half-size (the tail is smaller/dimmer).
const SPARK_COUNT: usize = 7;
const SPARK_R0: f32 = 0.012;
const SPARK_R1: f32 = 0.120;
const SPARK_DOT_R: f32 = 0.011;

/// Warm impact palette — kept in the muzzle-flash warm family (white-yellow heat against the cold
/// blue-grey FPS world), so the strike reads as hot light, not intel (invariant #6 legibility).
/// Flash: near-white heat. Spark: orange ember. Dust: muted warm earth catching the flash light.
const FLASH_COLOR: [f32; 3] = [1.0, 0.93, 0.78];
const SPARK_COLOR: [f32; 3] = [1.0, 0.76, 0.40];
const DUST_COLOR: [f32; 3] = [0.66, 0.52, 0.40];

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

/// A center offset (in NDC) for a point `radius` out from the strike at `angle`, aspect-corrected on
/// x so the spark ring stays circular on a wide window (same rule as [`round_half`]). Pure helper.
#[inline]
fn radial_offset(angle: f32, radius: f32, aspect: f32) -> (f32, f32) {
    let a = if aspect.abs() < 1.0e-6 { 1.0 } else { aspect };
    (radius * angle.cos() / a, radius * angle.sin())
}

/// The fixed (deterministic) angle + radial-length jitter for spark `k`. Evenly spaced with a phase
/// offset (so the burst isn't axis-aligned) plus a golden-ratio per-spark wobble on both angle and
/// reach — enough irregularity to read as scattered debris, not a tidy arcade ring. Pure: no RNG, so
/// the burst is stable frame-to-frame. Returns `(angle_radians, length_factor)`.
fn spark_layout(k: usize) -> (f32, f32) {
    use std::f32::consts::TAU;
    const PHASE: f32 = 0.55;
    const GOLDEN: f32 = 0.618_034;
    let frac = (k as f32 * GOLDEN).fract(); // 0..1, well-spread across sparks
    let angle = PHASE + k as f32 * TAU / SPARK_COUNT as f32 + (frac - 0.5) * 0.30;
    let length_factor = 0.78 + 0.44 * frac;
    (angle, length_factor)
}

/// Build the impact burst's screen-space instances at NDC `(ndc_x, ndc_y)` for `intensity` in
/// `[0, 1]` and viewport `aspect`. Returns **empty** when `intensity <= 0` (so the renderer no-ops
/// once the burst has faded). Three elements, all aspect-corrected and fading with the burst:
/// element `[0]` is the hot **flash core** (shrinks toward a pinpoint, alpha snaps out on `i²`),
/// element `[1]` is the **dust puff** (grows with age `age_t = 1 - i`, low warm alpha), and the rest
/// are crisp **spark embers** flung outward (head + dimmer tail per spark, riding a `sqrt(age_t)`
/// ease so they snap out early). Pure float math — host-testable without a GPU.
pub fn impact_instances(ndc_x: f32, ndc_y: f32, intensity: f32, aspect: f32) -> Vec<ImpactInstance> {
    let i = intensity.clamp(0.0, 1.0);
    if i <= 0.0 {
        return Vec::new();
    }
    let age_t = 1.0 - i; // 0 fresh → 1 fully aged
    let fling = age_t.sqrt(); // snap-out ease: fast early travel, settle late

    let mut out = Vec::with_capacity(2 + 2 * SPARK_COUNT);

    // [0] Flash core: hot near-white, tight at the strike, shrinking toward a pinpoint. Alpha snaps
    // out on i² so the heat reads as an instant punch that's gone fast, not a lingering smear.
    let [fr, fg, fb] = FLASH_COLOR;
    let (chx, chy) = round_half(FLASH_R * (0.45 + 0.55 * i), aspect);
    out.push(ImpactInstance {
        ndc_x,
        ndc_y,
        half_x: chx,
        half_y: chy,
        r: fr,
        g: fg,
        b: fb,
        a: i * i,
        shape: SHAPE_CORE,
    });

    // [1] Dust puff: warm muted earth catching the flash, expanding as it ages, kept low so it
    // grounds the strike without washing the dark field.
    let [dr, dg, db] = DUST_COLOR;
    let ring_r = RING_R0 + (RING_R1 - RING_R0) * age_t;
    let (rhx, rhy) = round_half(ring_r, aspect);
    out.push(ImpactInstance {
        ndc_x,
        ndc_y,
        half_x: rhx,
        half_y: rhy,
        r: dr,
        g: dg,
        b: db,
        a: i * 0.32,
        shape: SHAPE_RING,
    });

    // [2..] Spark embers: crisp orange dots flung radially outward (head + dimmer inner tail), each
    // fading with the burst so they wink out at the rim.
    let [sr, sg, sb] = SPARK_COLOR;
    let (hx_dot, hy_dot) = round_half(SPARK_DOT_R, aspect);
    let (tx_dot, ty_dot) = round_half(SPARK_DOT_R * 0.7, aspect);
    for k in 0..SPARK_COUNT {
        let (angle, len_f) = spark_layout(k);
        let reach = SPARK_R0 + (SPARK_R1 * len_f - SPARK_R0) * fling;
        // Head (bright, at the leading edge).
        let (ox, oy) = radial_offset(angle, reach, aspect);
        out.push(ImpactInstance {
            ndc_x: ndc_x + ox,
            ndc_y: ndc_y + oy,
            half_x: hx_dot,
            half_y: hy_dot,
            r: sr,
            g: sg,
            b: sb,
            a: i,
            shape: SHAPE_SPARK,
        });
        // Tail (dimmer, trailing closer to the strike — a short streak under additive blend).
        let (tox, toy) = radial_offset(angle, reach * 0.55, aspect);
        out.push(ImpactInstance {
            ndc_x: ndc_x + tox,
            ndc_y: ndc_y + toy,
            half_x: tx_dot,
            half_y: ty_dot,
            r: sr,
            g: sg,
            b: sb,
            a: i * 0.5,
            shape: SHAPE_SPARK,
        });
    }

    out
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
    #[allow(clippy::too_many_arguments)]
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
    fn live_burst_emits_flash_dust_and_sparks() {
        let inst = impact_instances(0.3, -0.2, 1.0, 1.0);
        assert_eq!(inst.len(), 2 + 2 * SPARK_COUNT, "flash + dust + head/tail per spark");
        assert_eq!(inst[0].shape, SHAPE_CORE, "[0] is the flash core");
        assert_eq!(inst[1].shape, SHAPE_RING, "[1] is the dust puff");
        // The flash + dust sit exactly on the hit point; sparks are flung off it.
        for e in &inst[..2] {
            assert!((e.ndc_x - 0.3).abs() < EPS && (e.ndc_y + 0.2).abs() < EPS, "centered on the hit");
        }
        assert!(
            inst[2..].iter().all(|e| e.shape == SHAPE_SPARK),
            "every remaining element is a spark ember",
        );
    }

    #[test]
    fn flash_shrinks_and_dust_grows_as_the_burst_ages() {
        let fresh = impact_instances(0.0, 0.0, 1.0, 1.0);
        let aged = impact_instances(0.0, 0.0, 0.2, 1.0);
        // Flash (element 0) shrinks with intensity.
        assert!(aged[0].half_y < fresh[0].half_y, "flash shrinks as it fades");
        // Dust (element 1) grows as it ages.
        assert!(aged[1].half_y > fresh[1].half_y, "dust puff expands outward");
        // Flash alpha fades with intensity (and on an i² curve, so faster than linear).
        assert!(aged[0].a < fresh[0].a, "flash alpha snaps out");
        assert!((fresh[0].a - 1.0).abs() < EPS, "fresh flash is full bright");
    }

    #[test]
    fn flash_alpha_uses_a_snappy_quadratic_fade() {
        // i² is below the linear i for every interior intensity → a punchier, faster snap-out.
        for &i in &[0.25_f32, 0.5, 0.75] {
            let flash = impact_instances(0.0, 0.0, i, 1.0)[0];
            assert!(flash.a < i - EPS, "flash fades faster than linear at i={i}");
            assert!((flash.a - i * i).abs() < EPS, "flash alpha is i²");
        }
    }

    #[test]
    fn sparks_fling_outward_and_dim_as_the_burst_ages() {
        let fresh = impact_instances(0.0, 0.0, 1.0, 1.0);
        let aged = impact_instances(0.0, 0.0, 0.2, 1.0);
        // Compare the same spark (first head, element [2]) fresh vs aged.
        let reach = |e: &ImpactInstance| (e.ndc_x * e.ndc_x + e.ndc_y * e.ndc_y).sqrt();
        assert!(reach(&aged[2]) > reach(&fresh[2]), "sparks travel out from the strike with age");
        assert!(reach(&fresh[2]) < 0.05, "fresh sparks start at the impact point");
        assert!(aged[2].a < fresh[2].a, "spark embers dim as they fly out");
    }

    #[test]
    fn spark_heads_outshine_their_tails() {
        let inst = impact_instances(0.0, 0.0, 1.0, 1.0);
        // Heads are even-indexed (2, 4, …), tails the odd one right after.
        for k in 0..SPARK_COUNT {
            let head = &inst[2 + 2 * k];
            let tail = &inst[2 + 2 * k + 1];
            assert!(head.a > tail.a, "head brighter than tail (spark {k})");
            assert!(head.half_y > tail.half_y, "head larger than tail (spark {k})");
        }
    }

    #[test]
    fn spark_layout_is_deterministic_and_spread() {
        // Pure, RNG-free: identical every call (stable burst), and the angles are not all the same.
        let a = (0..SPARK_COUNT).map(spark_layout).collect::<Vec<_>>();
        let b = (0..SPARK_COUNT).map(spark_layout).collect::<Vec<_>>();
        assert_eq!(a, b, "spark layout is deterministic");
        let spread = a.iter().any(|&(ang, _)| (ang - a[0].0).abs() > 0.5);
        assert!(spread, "sparks fan out across distinct angles");
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
