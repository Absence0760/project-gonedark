//! Embodied **sniper / zoom gun-sight** scope overlay (tank embodiment P9) — the screen-space scope
//! chrome drawn over the dark embodied frame while the local player aims down sight in a tank. It is
//! the render half of the zoom view; the FOV-narrowing + input→zoom-intent math is the engine's
//! [`gonedark_engine::scope`] seam. Five alpha-blended elements turn "the player is scoped at zoom
//! `t`" into a recognizable gun-sight:
//!
//! - **Vignette tunnel** — a full-screen darken everywhere *outside* the round aperture, so the
//!   periphery blacks out into the classic scope tunnel ([`scope_instances`], shape `VIGNETTE`).
//! - **Aperture ring** — the bright circular sight edge ([`scope_instances`], shape `RING`).
//! - **Crosshair bars** — a horizontal + vertical reticle line through center (shape `BAR`).
//! - **Center dot** — the aiming pip (shape `DOT`).
//!
//! The whole overlay **fades in with the zoom** ([`scope_fade`]) so it eases on with the FOV rather
//! than popping. Invariant #4: this is the **float side** — every number here is `f32` host-side
//! presentation, never `core` sim state, and the renderer only READS the [`ScopeState`] it is
//! handed. Invariant #6: it is avatar-only chrome with no world position — it reveals nothing about
//! unseen enemies and narrows (never widens) the visible frustum. Like [`tank_hud`](crate::tank_hud)
//! all the geometry math lives in pure free fns so it is unit-testable without a GPU; only
//! [`ScopeRenderer::render`] needs a device.

use wgpu::util::DeviceExt;

/// Shader `shape` ids — must match the `fs_main` branches in `scope.wgsl`.
const SHAPE_RING: f32 = 0.0;
const SHAPE_BAR: f32 = 1.0;
const SHAPE_DOT: f32 = 2.0;
const SHAPE_VIGNETTE: f32 = 3.0;

/// The clear aperture radius in NDC-y half-height units — the round hole you see the world through.
/// The ring sits here and the vignette darkens beyond it.
const APERTURE_R: f32 = 0.80;
/// The ring's quad is a touch larger than the aperture so the shader's edge band lands ON the
/// aperture radius (the band is at `~0.93..0.99` of local; `0.86 * 0.96 ≈ 0.825 ≈ APERTURE_R`).
const RING_QUAD_R: f32 = 0.86;
/// Crosshair line half-thickness (NDC-y). Kept thin; the vertical bar's NDC-x thickness is this
/// divided by aspect so both bars read the same on-screen width.
const BAR_THICK: f32 = 0.004;
/// Center aiming-dot radius (NDC-y).
const DOT_R: f32 = 0.012;

/// Below this zoom `t` the scope is invisible; above [`SCOPE_FADE_HI`] it is fully opaque. The fade
/// eases the whole overlay in with the FOV narrowing.
const SCOPE_FADE_LO: f32 = 0.15;
const SCOPE_FADE_HI: f32 = 0.85;

/// Element colors (RGB) + base alphas (pre-fade). The reticle is a pale gunsight green to match the
/// tank HUD; the vignette is near-black.
const RETICLE_COL: [f32; 3] = [0.80, 0.95, 0.84];
const RETICLE_ALPHA: f32 = 0.95;
const VIGNETTE_COL: [f32; 3] = [0.0, 0.0, 0.0];
const VIGNETTE_ALPHA: f32 = 0.92;

/// Everything the scope overlay needs to draw this frame — a pure, `Copy` presentation description
/// the host fills from the embodied zoom state. No `core` types cross this boundary (invariant #2).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ScopeState {
    /// Viewport aspect (width / height) — keeps the aperture circular and the crosshair square.
    pub aspect: f32,
    /// Current zoom interpolation `t` in `[0, 1]` (`0` hip, `1` full ADS). Drives the fade-in
    /// (`gonedark_engine::scope::step_zoom_t` produces it). `<= SCOPE_FADE_LO` draws nothing.
    pub zoom_t: f32,
}

impl Default for ScopeState {
    fn default() -> Self {
        ScopeState { aspect: 1.0, zoom_t: 0.0 }
    }
}

/// One scope element ready to upload. `repr(C)` + `Pod` so it streams into the per-instance vertex
/// buffer; the field order MUST match the instance attribute locations in `scope.wgsl` and the
/// `vertex_attr_array` in [`ScopeRenderer::new`].
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, bytemuck::Pod, bytemuck::Zeroable)]
pub struct ScopeInstance {
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
    /// Shape id: 0 ring, 1 bar, 2 dot, 3 vignette.
    pub shape: f32,
    /// Per-element params. Vignette: `(aspect, aperture_radius)` for the round-tunnel reconstruction;
    /// unused (0,0) otherwise.
    pub p0: f32,
    pub p1: f32,
}

/// The scope's opacity fade for a given zoom `t`: `0` below [`SCOPE_FADE_LO`], easing (smoothstep) to
/// `1` at [`SCOPE_FADE_HI`]. So the overlay eases on with the FOV narrowing rather than popping in.
/// Pure, host-testable.
pub fn scope_fade(zoom_t: f32) -> f32 {
    smoothstep(SCOPE_FADE_LO, SCOPE_FADE_HI, zoom_t)
}

/// Local smoothstep (the WGSL builtin's CPU twin) — keeps the fade identical host-side and in-shader.
#[inline]
fn smoothstep(edge0: f32, edge1: f32, x: f32) -> f32 {
    if edge0 == edge1 {
        return if x < edge0 { 0.0 } else { 1.0 };
    }
    let t = ((x - edge0) / (edge1 - edge0)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// An NDC-y radius → per-axis half-size that keeps a circle round on a non-square viewport. NDC spans
/// 2.0 on each axis; an `r`-tall circle is `r/aspect` wide (`aspect = w/h`).
#[inline]
fn round_half(r: f32, aspect: f32) -> (f32, f32) {
    let a = if aspect.abs() < 1.0e-6 { 1.0 } else { aspect };
    (r / a, r)
}

/// Build the scope overlay's screen-space instances from `state`, in a stable back-to-front order
/// (vignette tunnel, aperture ring, horizontal bar, vertical bar, center dot). PURE float math —
/// host-testable without a GPU; [`ScopeRenderer::render`] just uploads + draws whatever this returns.
/// Returns **empty** when the zoom is too shallow to show ([`scope_fade`] `== 0`), so the renderer
/// no-ops at hip. Every element's alpha is scaled by the fade so the whole sight eases in with the
/// FOV. Round elements are aspect-corrected via [`round_half`]; the crosshair bars are sized so both
/// lines share one on-screen thickness regardless of aspect.
pub fn scope_instances(state: &ScopeState) -> Vec<ScopeInstance> {
    let fade = scope_fade(state.zoom_t);
    if fade <= 0.0 {
        return Vec::new();
    }
    let aspect = if state.aspect.abs() < 1.0e-6 { 1.0 } else { state.aspect };
    let mut out = Vec::with_capacity(5);

    let reticle = |ndc_x, ndc_y, half_x, half_y, shape, p0, p1| ScopeInstance {
        ndc_x,
        ndc_y,
        half_x,
        half_y,
        r: RETICLE_COL[0],
        g: RETICLE_COL[1],
        b: RETICLE_COL[2],
        a: RETICLE_ALPHA * fade,
        shape,
        p0,
        p1,
    };

    // 1. Vignette tunnel (drawn first, under the reticle): a full-screen quad. The fragment rebuilds
    //    a round aperture using (aspect, APERTURE_R) and darkens everything outside it.
    out.push(ScopeInstance {
        ndc_x: 0.0,
        ndc_y: 0.0,
        half_x: 1.0,
        half_y: 1.0,
        r: VIGNETTE_COL[0],
        g: VIGNETTE_COL[1],
        b: VIGNETTE_COL[2],
        a: VIGNETTE_ALPHA * fade,
        shape: SHAPE_VIGNETTE,
        p0: aspect,
        p1: APERTURE_R,
    });

    // 2. Aperture ring — the bright circular sight edge (round via per-axis half).
    let (rhx, rhy) = round_half(RING_QUAD_R, aspect);
    out.push(reticle(0.0, 0.0, rhx, rhy, SHAPE_RING, 0.0, 0.0));

    // 3. Horizontal crosshair bar — reaches the aperture (half_x = APERTURE_R in NDC-x = /aspect),
    //    thin in y.
    out.push(reticle(0.0, 0.0, APERTURE_R / aspect, BAR_THICK, SHAPE_BAR, 0.0, 0.0));

    // 4. Vertical crosshair bar — thin in x (BAR_THICK/aspect, so its on-screen width matches the
    //    horizontal bar's NDC-y thickness), reaches the aperture in y.
    out.push(reticle(0.0, 0.0, BAR_THICK / aspect, APERTURE_R, SHAPE_BAR, 0.0, 0.0));

    // 5. Center aiming dot (round via per-axis half).
    let (dhx, dhy) = round_half(DOT_R, aspect);
    out.push(reticle(0.0, 0.0, dhx, dhy, SHAPE_DOT, 0.0, 0.0));

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

const INITIAL_CAP: usize = 5;

/// Screen-space embodied scope overlay (its own pipeline + buffers, like [`tank_hud`](crate::tank_hud)
/// and [`hud`](crate::hud)). Recorded as a LOAD pass so it composites over the dark embodied frame,
/// never clearing it.
pub struct ScopeRenderer {
    pipeline: wgpu::RenderPipeline,
    quad_buf: wgpu::Buffer,
    instance_buf: wgpu::Buffer,
    instance_cap: usize,
}

impl ScopeRenderer {
    /// Build the pipeline against the swapchain `surface_format` (alpha-blended LOAD overlay).
    pub fn new(device: &wgpu::Device, surface_format: wgpu::TextureFormat) -> Self {
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("gonedark.scope_shader"),
            source: wgpu::ShaderSource::Wgsl(include_str!("scope.wgsl").into()),
        });

        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("gonedark.scope_pipeline_layout"),
            bind_group_layouts: &[],
            immediate_size: 0,
        });

        let quad_layout = wgpu::VertexBufferLayout {
            array_stride: std::mem::size_of::<QuadVertex>() as u64,
            step_mode: wgpu::VertexStepMode::Vertex,
            attributes: &wgpu::vertex_attr_array![0 => Float32x2],
        };
        let instance_layout = wgpu::VertexBufferLayout {
            array_stride: std::mem::size_of::<ScopeInstance>() as u64,
            step_mode: wgpu::VertexStepMode::Instance,
            // 1=center(vec2), 2=half(vec2), 3=color(vec4), 4=shape(f32), 5=params(vec2).
            attributes: &wgpu::vertex_attr_array![
                1 => Float32x2,
                2 => Float32x2,
                3 => Float32x4,
                4 => Float32,
                5 => Float32x2
            ],
        };

        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("gonedark.scope_pipeline"),
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
            label: Some("gonedark.scope_quad_vbo"),
            contents: bytemuck::cast_slice(&QUAD_VERTS),
            usage: wgpu::BufferUsages::VERTEX,
        });

        let instance_cap = INITIAL_CAP;
        let instance_buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("gonedark.scope_instance_vbo"),
            size: (instance_cap * std::mem::size_of::<ScopeInstance>()) as u64,
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        ScopeRenderer {
            pipeline,
            quad_buf,
            instance_buf,
            instance_cap,
        }
    }

    /// Draw the scope overlay for `state` over `view` (a LOAD pass — never clears). Builds the live
    /// instance set via [`scope_instances`], uploads it, and records one render pass. A no-op at hip
    /// (the builder returns empty below the fade threshold).
    pub fn render(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        view: &wgpu::TextureView,
        state: &ScopeState,
    ) {
        let instances = scope_instances(state);
        if instances.is_empty() {
            return;
        }
        if instances.len() > self.instance_cap {
            let new_cap = instances.len().next_power_of_two();
            self.instance_buf = device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("gonedark.scope_instance_vbo"),
                size: (new_cap * std::mem::size_of::<ScopeInstance>()) as u64,
                usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });
            self.instance_cap = new_cap;
        }
        queue.write_buffer(&self.instance_buf, 0, bytemuck::cast_slice(&instances));

        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("gonedark.scope_encoder"),
        });
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("gonedark.scope_pass"),
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
    //! epsilon compares are fair game. `ScopeRenderer::new` needs a real `wgpu::Device` (no display
    //! in CI), so the pipeline path is untested; the testable placement/sizing math is factored into
    //! the pure free fns below — exactly the `tank_hud_instances` / `marker_for` pattern.

    use super::*;

    fn state(aspect: f32, zoom_t: f32) -> ScopeState {
        ScopeState { aspect, zoom_t }
    }

    // ---- fade ----

    #[test]
    fn fade_is_zero_at_hip_and_one_when_fully_scoped() {
        assert_eq!(scope_fade(0.0), 0.0, "hip draws nothing");
        assert_eq!(scope_fade(SCOPE_FADE_LO - 0.01), 0.0, "below the threshold draws nothing");
        assert!((scope_fade(1.0) - 1.0).abs() < 1e-6, "full ADS is fully opaque");
    }

    #[test]
    fn fade_is_monotone_nondecreasing_in_zoom() {
        let mut prev = -1.0;
        for i in 0..=20 {
            let f = scope_fade(i as f32 / 20.0);
            assert!(f >= prev - 1e-9, "fade non-decreasing in t: {f} !>= {prev}");
            assert!((0.0..=1.0).contains(&f), "fade {f} out of [0,1]");
            prev = f;
        }
    }

    // ---- builder gating ----

    #[test]
    fn builder_is_empty_at_hip() {
        assert!(scope_instances(&state(1.0, 0.0)).is_empty(), "no scope at hip");
        assert!(
            scope_instances(&state(1.0, SCOPE_FADE_LO - 0.01)).is_empty(),
            "no scope below the fade threshold"
        );
    }

    #[test]
    fn builder_emits_the_five_elements_when_scoped() {
        let inst = scope_instances(&state(16.0 / 9.0, 1.0));
        assert_eq!(inst.len(), 5, "vignette + ring + 2 bars + dot");
        assert_eq!(inst[0].shape, SHAPE_VIGNETTE);
        assert_eq!(inst[1].shape, SHAPE_RING);
        assert_eq!(inst[2].shape, SHAPE_BAR);
        assert_eq!(inst[3].shape, SHAPE_BAR);
        assert_eq!(inst[4].shape, SHAPE_DOT);
    }

    // ---- alpha fades with the zoom ----

    #[test]
    fn element_alpha_scales_with_the_fade() {
        // Partway in (lower fade) → dimmer than fully scoped.
        let mid = scope_instances(&state(1.0, 0.5));
        let full = scope_instances(&state(1.0, 1.0));
        assert!(!mid.is_empty() && !full.is_empty());
        // Ring is element [1] in both.
        assert!(mid[1].a < full[1].a, "scope is dimmer partway in: {} < {}", mid[1].a, full[1].a);
        assert!((full[1].a - RETICLE_ALPHA).abs() < 1e-6, "full ADS is full base alpha");
    }

    // ---- the aspect / NDC gotcha: a 16:9 case must stay round, not stretch ----

    #[test]
    fn aperture_ring_and_dot_stay_circular_on_a_wide_window() {
        // aspect 16:9 (wider than tall) → the round elements' x half-size is the y half-size / aspect
        // so they render as circles, not ellipses. This is the fat-reticle-on-a-wide-window footgun.
        let aspect = 16.0 / 9.0;
        let inst = scope_instances(&state(aspect, 1.0));
        let ring = inst[1];
        let dot = inst[4];
        assert!((ring.half_x - ring.half_y / aspect).abs() < 1e-6, "ring round per-axis");
        assert!((dot.half_x - dot.half_y / aspect).abs() < 1e-6, "dot round per-axis");
    }

    #[test]
    fn crosshair_bars_share_one_on_screen_thickness_under_aspect() {
        // The horizontal bar's thickness is BAR_THICK in NDC-y; the vertical bar's thickness is in
        // NDC-x, which is BAR_THICK/aspect — so multiplied back by aspect they match on screen. A
        // naive equal-NDC thickness would render the vertical line `aspect`× too fat on a wide window.
        let aspect = 16.0 / 9.0;
        let inst = scope_instances(&state(aspect, 1.0));
        let h_bar = inst[2]; // thin in y
        let v_bar = inst[3]; // thin in x
        assert!((h_bar.half_y - BAR_THICK).abs() < 1e-6, "h-bar thickness is BAR_THICK in y");
        assert!(
            (v_bar.half_x * aspect - h_bar.half_y).abs() < 1e-6,
            "v-bar on-screen thickness ({}·aspect) matches the h-bar ({})",
            v_bar.half_x,
            h_bar.half_y
        );
        // And both bars reach the same on-screen aperture extent.
        assert!((h_bar.half_x * aspect - v_bar.half_y).abs() < 1e-6, "both bars reach the aperture");
    }

    #[test]
    fn vignette_carries_aspect_and_aperture_for_the_round_tunnel() {
        let aspect = 16.0 / 9.0;
        let v = scope_instances(&state(aspect, 1.0))[0];
        assert_eq!(v.shape, SHAPE_VIGNETTE);
        assert!((v.p0 - aspect).abs() < 1e-6, "vignette p0 is the aspect for the round-tunnel rebuild");
        assert!((v.p1 - APERTURE_R).abs() < 1e-6, "vignette p1 is the aperture radius");
        // The vignette is a full-screen quad.
        assert_eq!((v.half_x, v.half_y), (1.0, 1.0), "vignette covers the whole screen");
    }

    #[test]
    fn degenerate_aspect_does_not_divide_by_zero() {
        // A mid-resize zero-height surface (aspect ~0) falls back to 1.0 — no NaN/inf half-sizes.
        let inst = scope_instances(&state(0.0, 1.0));
        for i in &inst {
            assert!(i.half_x.is_finite() && i.half_y.is_finite(), "no inf half-size at aspect 0");
        }
    }

    /// Validate `scope.wgsl` offline with naga (the compiler wgpu uses), so a WGSL regression fails
    /// the suite instead of only blowing up at pipeline creation on a real GPU (mirrors `tank_hud`).
    #[test]
    fn scope_wgsl_parses_and_validates() {
        let src = include_str!("scope.wgsl");
        let module = naga::front::wgsl::parse_str(src).expect("scope.wgsl must parse");
        let mut validator = naga::valid::Validator::new(
            naga::valid::ValidationFlags::all(),
            naga::valid::Capabilities::all(),
        );
        validator.validate(&module).expect("scope.wgsl must validate");
    }
}
