//! The **radial command menu** renderer — the on-screen wedge ring a held long-press opens over the
//! command vocabulary (`engine::command_ui`'s radial preview / commit, invariant #3: depth lives in
//! the vocabulary, never in unit AI). The host fills [`gonedark_engine::Game::radial_menu`] on a
//! long-press and hands the renderer a flat [`RadialMenu`] description; this draws the wedges.
//!
//! Like [`hud`](crate::hud) and [`overlay`](crate::overlay) this is a screen-space LOAD pass (it
//! composites over the already-rendered frame, never clears) and a **pure presentation derivation**
//! — it reads only the small [`RadialMenu`] the host hands it and emits NDC quads. It owns its own
//! tiny pipeline + shader (`radial.wgsl`) so it never contends with the unit/HUD/overlay passes.
//!
//! ## Fairness (invariant #6) holds by construction
//!
//! Every quad is in NDC ([`RadialQuad`] carries no world position, no fog mask), and the host only
//! ever draws this in the **command view** — the menu is empty while embodied and the host gates the
//! pass on `!embodied`, so it never paints over the dark frame. The ring is *chrome*, not intel.
//!
//! ## What it draws (and does not)
//!
//! A dim backdrop, a center hub at the anchor, and one wedge per available action laid out clockwise
//! from the top. There is **no glyph/text system in this engine yet**, so the wedges are unlabeled —
//! the slot *count* and the (optional) highlighted slot are the affordance. Labels live in the
//! host's `radial_menu` for when a text/icon pass lands; they are deliberately not plumbed here.
//!
//! The testable layout math (how many quads, where each wedge sits) lives in the free
//! [`radial_quads`] so it is unit-testable without a GPU — the `overlay_quads` / `marker_for`
//! pattern.

use std::f32::consts::{FRAC_PI_2, TAU};
use wgpu::util::DeviceExt;

/// A flat, presentation-only description of the radial menu to draw this frame. The render side
/// never owns the command-vocabulary state machine (that is `engine`); it is handed exactly what to
/// draw. No world position — `center` is already in NDC (invariant #6).
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct RadialMenu {
    /// The menu anchor in NDC ([-1,1], +y up) — where the long-press opened (the pointer), or the
    /// screen center when no pointer is known.
    pub center: [f32; 2],
    /// How many wedge slots to draw — one per action the host's `radial_menu` offers this frame.
    pub slots: usize,
    /// The slot currently highlighted (under the pointer / about to commit), if any. Drawn with the
    /// [`WedgeHighlight`](RadialRole::WedgeHighlight) role; `None` highlights nothing.
    pub highlight: Option<usize>,
}

/// A semantic role for a radial-menu quad, so the color is centralized and a test can assert *what*
/// was drawn without pixel-matching (mirrors `overlay::QuadRole`).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum RadialRole {
    /// A dim square behind the ring so the wedges read over the command frame.
    Backdrop,
    /// The center hub marking the menu anchor (where the long-press landed).
    Hub,
    /// A neutral action wedge.
    Wedge,
    /// The highlighted action wedge (the slot under the pointer / about to commit).
    WedgeHighlight,
}

/// One screen-space radial-menu quad in NDC, ready to upload. The `role` is CPU-side only (drives
/// the color and lets tests assert structure); it is dropped from the uploaded [`RadialInstance`].
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct RadialQuad {
    /// Center in NDC ([-1,1], +y up).
    pub cx: f32,
    pub cy: f32,
    /// Half-width / half-height in NDC.
    pub hw: f32,
    pub hh: f32,
    pub r: f32,
    pub g: f32,
    pub b: f32,
    pub alpha: f32,
    /// Semantic role (CPU-side; drives the color above and lets tests assert structure).
    pub role: RadialRole,
}

/// The GPU-uploadable slice of a [`RadialQuad`] (drops the CPU-only `role`). `repr(C)` + `Pod` so it
/// streams straight into the instance buffer; the field order MUST match `radial.wgsl`'s instance
/// attributes and the `vertex_attr_array` in [`RadialRenderer::new`].
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, bytemuck::Pod, bytemuck::Zeroable)]
struct RadialInstance {
    cx: f32,
    cy: f32,
    hw: f32,
    hh: f32,
    r: f32,
    g: f32,
    b: f32,
    alpha: f32,
}

impl RadialQuad {
    fn instance(&self) -> RadialInstance {
        RadialInstance {
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

// Layout constants (NDC). The ring is laid out in raw NDC, so on a non-square viewport it reads as a
// slight ellipse — consistent with the HUD ring and the overlay, which also work in raw NDC.
/// Radius of the wedge ring from the anchor.
const RING_RADIUS: f32 = 0.30;
/// Half-extent of a wedge (action) slot.
const WEDGE_HALF: f32 = 0.055;
/// Half-extent of the center hub.
const HUB_HALF: f32 = 0.022;
/// Half-extent of the dim backdrop square — covers the ring plus a small margin.
const BACKDROP_HALF: f32 = RING_RADIUS + WEDGE_HALF + 0.04;
/// The backdrop is dim so the command frame still reads faintly beneath the menu.
const BACKDROP_ALPHA: f32 = 0.5;
/// Wedges are near-opaque chrome.
const WEDGE_ALPHA: f32 = 0.95;

fn color(role: RadialRole) -> [f32; 3] {
    match role {
        RadialRole::Backdrop => [0.0, 0.0, 0.0],
        RadialRole::Hub => [0.20, 0.22, 0.28], // a faint anchor dot
        RadialRole::Wedge => [0.22, 0.25, 0.32], // a neutral choice slot (matches overlay's Button)
        RadialRole::WedgeHighlight => [0.30, 0.45, 0.70], // the affirmative/hovered slot
    }
}

fn quad(cx: f32, cy: f32, hw: f32, hh: f32, alpha: f32, role: RadialRole) -> RadialQuad {
    let [r, g, b] = color(role);
    RadialQuad {
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

/// Build the screen-space radial-menu quads for `menu`. Pure (no GPU, no sim) — the testable layout
/// seam. Returns an empty vec when there are no slots. Quads are returned back-to-front (backdrop,
/// then hub, then the wedges) so an alpha-blended LOAD pass composites correctly.
///
/// Wedges are laid out clockwise starting at the top (12 o'clock): slot 0 sits directly above the
/// anchor, the rest fan around the ring evenly. The optional `highlight` slot is drawn with the
/// [`WedgeHighlight`](RadialRole::WedgeHighlight) role.
pub fn radial_quads(menu: &RadialMenu) -> Vec<RadialQuad> {
    if menu.slots == 0 {
        return Vec::new();
    }
    let (cx, cy) = (menu.center[0], menu.center[1]);
    let mut out = Vec::with_capacity(menu.slots + 2);
    // Backdrop first (back-to-front): a dim square so the wedges read over the command frame.
    out.push(quad(
        cx,
        cy,
        BACKDROP_HALF,
        BACKDROP_HALF,
        BACKDROP_ALPHA,
        RadialRole::Backdrop,
    ));
    // The hub marks the anchor the menu opened at.
    out.push(quad(cx, cy, HUB_HALF, HUB_HALF, 1.0, RadialRole::Hub));
    // Wedges around the ring: slot 0 at the top, clockwise (NDC +y is up, so subtract the angle).
    let n = menu.slots as f32;
    for i in 0..menu.slots {
        let angle = FRAC_PI_2 - (i as f32) * TAU / n;
        let wx = cx + RING_RADIUS * angle.cos();
        let wy = cy + RING_RADIUS * angle.sin();
        let role = if menu.highlight == Some(i) {
            RadialRole::WedgeHighlight
        } else {
            RadialRole::Wedge
        };
        out.push(quad(wx, wy, WEDGE_HALF, WEDGE_HALF, WEDGE_ALPHA, role));
    }
    out
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

const INITIAL_CAP: usize = 16;

/// Screen-space radial command-menu renderer. Owns its own pipeline + buffers (separate from the
/// unit/HUD/overlay passes so the four never contend for a shader). Alpha-blended LOAD pass:
/// composites over the already-rendered command frame.
pub struct RadialRenderer {
    pipeline: wgpu::RenderPipeline,
    quad_buf: wgpu::Buffer,
    instance_buf: wgpu::Buffer,
    instance_cap: usize,
}

impl RadialRenderer {
    /// Build the radial pipeline against the swapchain `surface_format`. The `device` is borrowed
    /// (D19). Alpha blending so the backdrop dims the frame beneath.
    pub fn new(device: &wgpu::Device, surface_format: wgpu::TextureFormat) -> Self {
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("gonedark.radial_shader"),
            source: wgpu::ShaderSource::Wgsl(include_str!("radial.wgsl").into()),
        });

        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("gonedark.radial_pipeline_layout"),
            bind_group_layouts: &[],
            immediate_size: 0,
        });

        let quad_layout = wgpu::VertexBufferLayout {
            array_stride: std::mem::size_of::<QuadVertex>() as u64,
            step_mode: wgpu::VertexStepMode::Vertex,
            attributes: &wgpu::vertex_attr_array![0 => Float32x2],
        };
        let instance_layout = wgpu::VertexBufferLayout {
            array_stride: std::mem::size_of::<RadialInstance>() as u64,
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
            label: Some("gonedark.radial_pipeline"),
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
            label: Some("gonedark.radial_quad_vbo"),
            contents: bytemuck::cast_slice(&QUAD_VERTS),
            usage: wgpu::BufferUsages::VERTEX,
        });

        let instance_cap = INITIAL_CAP;
        let instance_buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("gonedark.radial_instance_vbo"),
            size: (instance_cap * std::mem::size_of::<RadialInstance>()) as u64,
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        RadialRenderer {
            pipeline,
            quad_buf,
            instance_buf,
            instance_cap,
        }
    }

    /// Draw the radial menu on top of `view` (a LOAD pass — never clears). Builds the quad set via
    /// [`radial_quads`], uploads it, and records one LOAD render pass so the menu composites over the
    /// command frame. No-op when the menu has no slots.
    pub fn render(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        view: &wgpu::TextureView,
        menu: &RadialMenu,
    ) {
        let quads = radial_quads(menu);
        if quads.is_empty() {
            return;
        }
        let instances: Vec<RadialInstance> = quads.iter().map(|q| q.instance()).collect();

        if instances.len() > self.instance_cap {
            let new_cap = instances.len().next_power_of_two();
            self.instance_buf = device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("gonedark.radial_instance_vbo"),
                size: (new_cap * std::mem::size_of::<RadialInstance>()) as u64,
                usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });
            self.instance_cap = new_cap;
        }
        queue.write_buffer(&self.instance_buf, 0, bytemuck::cast_slice(&instances));

        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("gonedark.radial_encoder"),
        });
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("gonedark.radial_pass"),
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
    //! `render` is the float boundary, so f32 layout math is fair game. `RadialRenderer::new` needs a
    //! real `wgpu::Device` (no display in CI), so the pipeline path is untested; the testable layout
    //! math is factored into [`radial_quads`].

    use super::*;

    fn menu(center: [f32; 2], slots: usize, highlight: Option<usize>) -> RadialMenu {
        RadialMenu {
            center,
            slots,
            highlight,
        }
    }

    fn roles(quads: &[RadialQuad]) -> Vec<RadialRole> {
        quads.iter().map(|q| q.role).collect()
    }

    fn wedges(quads: &[RadialQuad]) -> Vec<&RadialQuad> {
        quads
            .iter()
            .filter(|q| matches!(q.role, RadialRole::Wedge | RadialRole::WedgeHighlight))
            .collect()
    }

    #[test]
    fn empty_menu_draws_nothing() {
        assert!(radial_quads(&menu([0.0, 0.0], 0, None)).is_empty());
    }

    #[test]
    fn backdrop_then_hub_then_wedges() {
        let q = radial_quads(&menu([0.0, 0.0], 4, None));
        // Back-to-front: a dim backdrop, the hub, then one wedge per slot.
        assert_eq!(q[0].role, RadialRole::Backdrop);
        assert_eq!(q[1].role, RadialRole::Hub);
        assert_eq!(q.len(), 2 + 4, "backdrop + hub + 4 wedges");
        for r in &roles(&q)[2..] {
            assert_eq!(*r, RadialRole::Wedge);
        }
        // The backdrop spans the ring; the hub is the smallest element.
        assert!(q[0].hw > RING_RADIUS, "backdrop covers the ring");
        assert!(q[1].hw < q[0].hw, "hub is smaller than the backdrop");
    }

    #[test]
    fn slot_count_drives_wedge_count() {
        for n in 1..=10usize {
            let q = radial_quads(&menu([0.0, 0.0], n, None));
            assert_eq!(wedges(&q).len(), n, "one wedge per slot for n={n}");
        }
    }

    #[test]
    fn slot_zero_sits_at_top_above_the_anchor() {
        // Slot 0 is at 12 o'clock: same x as the center, directly above it (cy + RING_RADIUS).
        let c = [0.1, -0.2];
        let q = radial_quads(&menu(c, 4, None));
        let w0 = wedges(&q)[0];
        assert!((w0.cx - c[0]).abs() < 1e-5, "slot 0 shares the anchor's x");
        assert!(
            (w0.cy - (c[1] + RING_RADIUS)).abs() < 1e-5,
            "slot 0 sits one ring-radius above the anchor"
        );
    }

    #[test]
    fn wedges_lie_on_the_ring_around_the_center() {
        let c = [-0.3, 0.25];
        let q = radial_quads(&menu(c, 6, None));
        for w in wedges(&q) {
            let d = ((w.cx - c[0]).powi(2) + (w.cy - c[1]).powi(2)).sqrt();
            assert!(
                (d - RING_RADIUS).abs() < 1e-5,
                "every wedge is one ring-radius from the anchor (got {d})"
            );
        }
    }

    #[test]
    fn second_slot_is_clockwise_of_the_first() {
        // Clockwise from the top → slot 1 swings to the anchor's right (+x) and below the top.
        let q = radial_quads(&menu([0.0, 0.0], 4, None));
        let w = wedges(&q);
        assert!(
            w[1].cx > w[0].cx,
            "slot 1 is to the right of slot 0 (clockwise)"
        );
        assert!(w[1].cy < w[0].cy, "slot 1 is below the top slot");
    }

    #[test]
    fn highlight_marks_exactly_one_wedge() {
        let q = radial_quads(&menu([0.0, 0.0], 5, Some(2)));
        let w = wedges(&q);
        for (i, wedge) in w.iter().enumerate() {
            if i == 2 {
                assert_eq!(wedge.role, RadialRole::WedgeHighlight, "slot 2 highlighted");
            } else {
                assert_eq!(wedge.role, RadialRole::Wedge, "slot {i} not highlighted");
            }
        }
        assert_eq!(
            w.iter()
                .filter(|q| q.role == RadialRole::WedgeHighlight)
                .count(),
            1,
            "exactly one highlighted wedge"
        );
    }

    #[test]
    fn out_of_range_highlight_highlights_nothing() {
        // A highlight index past the slot count simply highlights no wedge (no panic).
        let q = radial_quads(&menu([0.0, 0.0], 3, Some(9)));
        assert!(!roles(&q).contains(&RadialRole::WedgeHighlight));
    }

    #[test]
    fn center_offset_translates_every_quad() {
        // Shifting the anchor shifts the whole menu rigidly: each quad moves by the same delta.
        let base = radial_quads(&menu([0.0, 0.0], 4, None));
        let moved = radial_quads(&menu([0.2, -0.1], 4, None));
        assert_eq!(base.len(), moved.len());
        for (b, m) in base.iter().zip(moved.iter()) {
            assert!((m.cx - b.cx - 0.2).abs() < 1e-5 && (m.cy - b.cy + 0.1).abs() < 1e-5);
            assert_eq!(b.role, m.role);
            assert_eq!((b.hw, b.hh), (m.hw, m.hh), "sizes are anchor-independent");
        }
    }

    /// Fairness guard (invariant #6): every radial quad is NDC chrome with no world position. With a
    /// centered anchor the whole menu stays inside the screen; nothing carries spatial sim data.
    #[test]
    fn radial_quads_are_screen_space_only() {
        let q = radial_quads(&menu([0.0, 0.0], 10, Some(0)));
        for quad in &q {
            assert!(quad.cx.is_finite() && quad.cy.is_finite());
            assert!(
                quad.cx - quad.hw >= -1.0 && quad.cx + quad.hw <= 1.0,
                "in NDC x"
            );
            assert!(
                quad.cy - quad.hh >= -1.0 && quad.cy + quad.hh <= 1.0,
                "in NDC y"
            );
        }
    }

    #[test]
    fn radial_wgsl_parses_and_validates() {
        let src = include_str!("radial.wgsl");
        let module = naga::front::wgsl::parse_str(src).expect("radial.wgsl must parse");
        let mut validator = naga::valid::Validator::new(
            naga::valid::ValidationFlags::all(),
            naga::valid::Capabilities::all(),
        );
        validator
            .validate(&module)
            .expect("radial.wgsl must validate");
    }
}
