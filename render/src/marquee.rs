//! The **band-select marquee** renderer — the selection rectangle drawn in the command view while
//! the player is dragging a band-select (`engine::selection`'s in-flight drag). Without it the band
//! gesture gives no feedback until release; this draws the box the player is sweeping.
//!
//! Like [`hud`](crate::hud) / [`overlay`](crate::overlay) / [`radial`](crate::radial) it is a
//! screen-space LOAD pass (composites over the command frame, never clears) and a **pure
//! presentation derivation** — it reads only the [`Marquee`] rect (already in NDC) the host hands
//! it. It owns its own tiny pipeline + shader (`marquee.wgsl`).
//!
//! ## Fairness (invariant #6)
//!
//! NDC chrome only — the rect carries no fog mask, and the host derives it by projecting the live
//! selection drag corners and only draws it in the command view (never the dark embodied frame).
//!
//! The testable layout (a translucent fill plus four border edges) lives in the free
//! [`marquee_quads`] so it is unit-testable without a GPU — the `overlay_quads` / `radial_quads`
//! pattern.

use wgpu::util::DeviceExt;

/// The band-select rectangle to draw this frame, as an axis-aligned NDC box. The render side never
/// owns the gesture state (that is `engine::selection`); it is handed the already-projected corners.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct Marquee {
    /// Lower-left corner in NDC ([-1,1], +y up).
    pub min: [f32; 2],
    /// Upper-right corner in NDC.
    pub max: [f32; 2],
}

/// A semantic role for a marquee quad (centralizes the color and lets tests assert structure).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum MarqueeRole {
    /// The translucent interior fill.
    Fill,
    /// One of the four bright border edges.
    Border,
}

/// One screen-space marquee quad in NDC. The `role` is CPU-side only (drives the color and lets
/// tests assert structure); it is dropped from the uploaded [`MarqueeInstance`].
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct MarqueeQuad {
    pub cx: f32,
    pub cy: f32,
    pub hw: f32,
    pub hh: f32,
    pub r: f32,
    pub g: f32,
    pub b: f32,
    pub alpha: f32,
    pub role: MarqueeRole,
}

/// The GPU-uploadable slice of a [`MarqueeQuad`] (drops the CPU-only `role`). `repr(C)` + `Pod`; the
/// field order MUST match `marquee.wgsl`'s instance attributes and the `vertex_attr_array` below.
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, bytemuck::Pod, bytemuck::Zeroable)]
struct MarqueeInstance {
    cx: f32,
    cy: f32,
    hw: f32,
    hh: f32,
    r: f32,
    g: f32,
    b: f32,
    alpha: f32,
}

impl MarqueeQuad {
    fn instance(&self) -> MarqueeInstance {
        MarqueeInstance {
            cx: self.cx,
            cy: self.cy,
            hw: self.hw,
            hh: self.hh,
            r: self.r,
            g: self.g,
            b: self.b,
            alpha: self.alpha,
        }
    }
}

// Layout constants (NDC).
/// Half-thickness of a border edge.
const BORDER_HALF: f32 = 0.004;
/// The interior fill is faint so the units under the band stay readable.
const FILL_ALPHA: f32 = 0.12;
/// The border is a crisp, opaque cool line (matches the unit selection-rim read).
const BORDER_ALPHA: f32 = 1.0;

/// The band-select colours are derived from the shared **player-blue** [`crate::theme::PLAYER`]
/// (lightened toward [`crate::theme::BONE`]) rather than hand-tuned literals, so the marquee's
/// "matches the selection rim" read is *structural*: the box wears the same faction-identity blue the
/// selected units' rims do (WS-C). The fill is a faint low-mix wash; the border a brighter high-mix
/// edge. (Flagged design change — these RGBs shifted slightly off the old literals; no golden test
/// pins the marquee's colour, only its geometry + relative alpha.)
fn color(role: MarqueeRole) -> [f32; 3] {
    match role {
        MarqueeRole::Fill => crate::theme::mix(crate::theme::PLAYER, crate::theme::BONE, 0.3),
        MarqueeRole::Border => crate::theme::mix(crate::theme::PLAYER, crate::theme::BONE, 0.65),
    }
}

fn quad(cx: f32, cy: f32, hw: f32, hh: f32, alpha: f32, role: MarqueeRole) -> MarqueeQuad {
    let [r, g, b] = color(role);
    MarqueeQuad {
        cx,
        cy,
        hw,
        hh,
        r,
        g,
        b,
        alpha,
        role,
    }
}

/// Build the screen-space marquee quads for `m`: a translucent fill, then the four border edges
/// (top/bottom/left/right). Pure (no GPU) — the testable layout seam. The corners are normalized so
/// a drag in any direction yields the same box. Returned fill-first so the border reads over it.
pub fn marquee_quads(m: &Marquee) -> Vec<MarqueeQuad> {
    let x0 = m.min[0].min(m.max[0]);
    let x1 = m.min[0].max(m.max[0]);
    let y0 = m.min[1].min(m.max[1]);
    let y1 = m.min[1].max(m.max[1]);
    let cx = (x0 + x1) * 0.5;
    let cy = (y0 + y1) * 0.5;
    let hw = (x1 - x0) * 0.5;
    let hh = (y1 - y0) * 0.5;
    let t = BORDER_HALF;
    vec![
        // Interior fill.
        quad(cx, cy, hw, hh, FILL_ALPHA, MarqueeRole::Fill),
        // Border edges (top/bottom span the width; left/right span the height).
        quad(cx, y1, hw, t, BORDER_ALPHA, MarqueeRole::Border),
        quad(cx, y0, hw, t, BORDER_ALPHA, MarqueeRole::Border),
        quad(x0, cy, t, hh, BORDER_ALPHA, MarqueeRole::Border),
        quad(x1, cy, t, hh, BORDER_ALPHA, MarqueeRole::Border),
    ]
}

/// A unit-quad corner in [-1, 1]^2 (the shader scales it by the per-quad half-size).
#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct QuadVertex {
    corner: [f32; 2],
}

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

/// A marquee is always exactly 5 quads (fill + 4 edges), so the instance buffer never grows.
const CAP: usize = 5;

/// Screen-space band-select marquee renderer. Owns its own pipeline + buffers (separate from the
/// unit/HUD/overlay/radial passes). Alpha-blended LOAD pass over the command frame.
pub struct MarqueeRenderer {
    pipeline: wgpu::RenderPipeline,
    quad_buf: wgpu::Buffer,
    instance_buf: wgpu::Buffer,
}

impl MarqueeRenderer {
    /// Build the marquee pipeline against the swapchain `surface_format`. The `device` is borrowed
    /// (D19). Alpha blending so the fill dims (not hides) the units beneath.
    pub fn new(device: &wgpu::Device, surface_format: wgpu::TextureFormat) -> Self {
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("gonedark.marquee_shader"),
            source: wgpu::ShaderSource::Wgsl(include_str!("marquee.wgsl").into()),
        });

        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("gonedark.marquee_pipeline_layout"),
            bind_group_layouts: &[],
            immediate_size: 0,
        });

        let quad_layout = wgpu::VertexBufferLayout {
            array_stride: std::mem::size_of::<QuadVertex>() as u64,
            step_mode: wgpu::VertexStepMode::Vertex,
            attributes: &wgpu::vertex_attr_array![0 => Float32x2],
        };
        let instance_layout = wgpu::VertexBufferLayout {
            array_stride: std::mem::size_of::<MarqueeInstance>() as u64,
            step_mode: wgpu::VertexStepMode::Instance,
            // 1=center(vec2), 2=half(vec2), 3=color(vec3), 4=alpha(f32).
            attributes: &wgpu::vertex_attr_array![
                1 => Float32x2,
                2 => Float32x2,
                3 => Float32x3,
                4 => Float32
            ],
        };

        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("gonedark.marquee_pipeline"),
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
            label: Some("gonedark.marquee_quad_vbo"),
            contents: bytemuck::cast_slice(&QUAD_VERTS),
            usage: wgpu::BufferUsages::VERTEX,
        });

        let instance_buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("gonedark.marquee_instance_vbo"),
            size: (CAP * std::mem::size_of::<MarqueeInstance>()) as u64,
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        MarqueeRenderer {
            pipeline,
            quad_buf,
            instance_buf,
        }
    }

    /// Draw the band-select marquee on top of `view` (a LOAD pass — never clears). Builds the quad
    /// set via [`marquee_quads`], uploads it, and records one LOAD render pass. The host calls this
    /// only while a command-view band-drag is in flight.
    pub fn render(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        view: &wgpu::TextureView,
        marquee: &Marquee,
    ) {
        let quads = marquee_quads(marquee);
        let instances: Vec<MarqueeInstance> = quads.iter().map(|q| q.instance()).collect();
        queue.write_buffer(&self.instance_buf, 0, bytemuck::cast_slice(&instances));

        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("gonedark.marquee_encoder"),
        });
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("gonedark.marquee_pass"),
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
    //! `render` is the float boundary, so f32 layout math is fair game. `MarqueeRenderer::new` needs
    //! a real `wgpu::Device` (no display in CI), so the pipeline path is untested; the testable
    //! layout math is factored into [`marquee_quads`].

    use super::*;

    fn roles(quads: &[MarqueeQuad]) -> Vec<MarqueeRole> {
        quads.iter().map(|q| q.role).collect()
    }

    #[test]
    fn fill_first_then_four_borders() {
        let q = marquee_quads(&Marquee {
            min: [-0.5, -0.3],
            max: [0.4, 0.6],
        });
        assert_eq!(q.len(), 5, "fill + 4 edges");
        assert_eq!(
            q[0].role,
            MarqueeRole::Fill,
            "fill is drawn first (under the border)"
        );
        for r in &roles(&q)[1..] {
            assert_eq!(*r, MarqueeRole::Border);
        }
    }

    #[test]
    fn fill_covers_the_rect_and_is_faint() {
        let (min, max) = ([-0.5, -0.3], [0.4, 0.6]);
        let q = marquee_quads(&Marquee { min, max });
        let fill = q[0];
        assert!((fill.cx - (-0.05)).abs() < 1e-6, "centered in x");
        assert!((fill.cy - 0.15).abs() < 1e-6, "centered in y");
        assert!((fill.hw - 0.45).abs() < 1e-6, "half the width");
        assert!((fill.hh - 0.45).abs() < 1e-6, "half the height");
        assert!(fill.alpha < BORDER_ALPHA, "fill is fainter than the border");
    }

    #[test]
    fn borders_hug_the_four_edges() {
        let q = marquee_quads(&Marquee {
            min: [-0.5, -0.3],
            max: [0.4, 0.6],
        });
        let borders = &q[1..];
        // Two horizontal edges (thin in y) at the top/bottom, two vertical edges (thin in x).
        let horiz: Vec<&MarqueeQuad> = borders.iter().filter(|b| b.hh < b.hw).collect();
        let vert: Vec<&MarqueeQuad> = borders.iter().filter(|b| b.hw < b.hh).collect();
        assert_eq!(horiz.len(), 2, "top + bottom");
        assert_eq!(vert.len(), 2, "left + right");
        // The horizontal edges sit at y = max and y = min.
        let mut ys: Vec<f32> = horiz.iter().map(|b| b.cy).collect();
        ys.sort_by(|a, b| a.partial_cmp(b).unwrap());
        assert!((ys[0] - (-0.3)).abs() < 1e-6 && (ys[1] - 0.6).abs() < 1e-6);
        // The vertical edges sit at x = min and x = max.
        let mut xs: Vec<f32> = vert.iter().map(|b| b.cx).collect();
        xs.sort_by(|a, b| a.partial_cmp(b).unwrap());
        assert!((xs[0] - (-0.5)).abs() < 1e-6 && (xs[1] - 0.4).abs() < 1e-6);
    }

    #[test]
    fn corners_are_normalized_regardless_of_drag_direction() {
        // A drag up-left (max below-left of min) yields the same box as down-right.
        let a = marquee_quads(&Marquee {
            min: [-0.5, -0.3],
            max: [0.4, 0.6],
        });
        let b = marquee_quads(&Marquee {
            min: [0.4, 0.6],
            max: [-0.5, -0.3],
        });
        assert_eq!(a, b, "the box is direction-independent");
    }

    /// Fairness guard (invariant #6): every marquee quad is NDC chrome with no world position, and a
    /// box inside the screen stays inside the screen.
    #[test]
    fn marquee_quads_are_screen_space_only() {
        let q = marquee_quads(&Marquee {
            min: [-0.9, -0.9],
            max: [0.9, 0.9],
        });
        for quad in &q {
            assert!(quad.cx.is_finite() && quad.cy.is_finite());
            assert!(
                quad.cx - quad.hw >= -1.0001 && quad.cx + quad.hw <= 1.0001,
                "in NDC x"
            );
            assert!(
                quad.cy - quad.hh >= -1.0001 && quad.cy + quad.hh <= 1.0001,
                "in NDC y"
            );
        }
    }

    #[test]
    fn colors_are_derived_from_the_player_blue_selection_identity() {
        // WS-C: the marquee wears the shared player-blue (lightened toward bone), so its "matches the
        // selection rim" claim is structural, not a coincidence of hand-tuned literals. Both roles
        // read cool (blue channel dominant) and the border is brighter than the fill.
        let q = marquee_quads(&Marquee {
            min: [-0.5, -0.3],
            max: [0.4, 0.6],
        });
        let fill = q[0];
        let border = q[1];
        for c in [fill, border] {
            assert!(c.b > c.r && c.b > c.g, "marquee colour reads cool (player-blue lineage)");
        }
        // Border is the higher mix toward bone → lighter than the fill on every channel.
        assert!(border.r > fill.r && border.g > fill.g, "border is the brighter edge");
        // Exact derivation: the fill/border are mixes of PLAYER toward BONE (one source of truth).
        assert_eq!(
            [fill.r, fill.g, fill.b],
            crate::theme::mix(crate::theme::PLAYER, crate::theme::BONE, 0.3)
        );
        assert_eq!(
            [border.r, border.g, border.b],
            crate::theme::mix(crate::theme::PLAYER, crate::theme::BONE, 0.65)
        );
    }

    #[test]
    fn marquee_wgsl_parses_and_validates() {
        let src = include_str!("marquee.wgsl");
        let module = naga::front::wgsl::parse_str(src).expect("marquee.wgsl must parse");
        let mut validator = naga::valid::Validator::new(
            naga::valid::ValidationFlags::all(),
            naga::valid::Capabilities::all(),
        );
        validator
            .validate(&module)
            .expect("marquee.wgsl must validate");
    }
}
