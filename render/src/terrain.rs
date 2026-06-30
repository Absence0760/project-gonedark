//! Command-view **ground grid** (W6 — command-view polish).
//!
//! The top-down view used to be flat dark slate with colored quads floating on it — position and
//! motion were hard to read because there was no fixed reference. This pass draws a subtle,
//! game-like ground grid *under* the units (the first thing in the command pass, after the clear
//! and before the unit/overlay passes) so a unit's place and movement read against a stable lattice.
//!
//! Unlike [`hud`](crate::hud) / [`marquee`](crate::marquee) / [`text`](crate::text), which are
//! screen-space NDC chrome, the grid is **world-space**: each line is an axis-aligned quad on the
//! ground plane (z = 0), transformed by the *same* top-down camera the unit pass uses (it shares the
//! camera bind group). That is what makes it a ground grid rather than a screen overlay — it pans
//! and frames with the world.
//!
//! ## Fairness (invariant #6)
//!
//! The grid is pure cosmetic terrain: it carries **no fog mask and no sim state**, so it is drawn
//! only in the command view (`!world_dark`) by [`crate::Renderer::render`] and never under the dark
//! embodied frame. It reveals nothing about units — it is a fixed lattice keyed only off the world
//! extent, identical every frame regardless of what is on the map.
//!
//! ## The pure seam
//!
//! All layout math — which lines exist and each line's world rectangle — lives in the free
//! [`grid_lines`] fn so it is unit-testable without a GPU, exactly the `marquee_quads` / `layout_cells`
//! pattern. [`TerrainRenderer::render`] is the only GPU-touching code and is exercised by the
//! offscreen `viz-runner`, not the no-GPU CI matrix.

use wgpu::util::DeviceExt;

/// How far (in world units) the grid extends from the origin on each axis. The top-down camera
/// frames `±TOPDOWN_HALF_EXTENT` (40) world units (`engine`'s `topdown_view_proj`); the grid is
/// drawn a touch wider so its edge lines never sit exactly on the viewport border.
pub const GRID_HALF_EXTENT: f32 = 44.0;

/// World-unit spacing between adjacent grid lines. A 8-unit cell at the ±40 framing gives ~10 cells
/// across the screen — dense enough to read motion, sparse enough not to clutter.
pub const GRID_SPACING: f32 = 8.0;

/// Half-thickness (world units) of a normal (minor) grid line. Thin so the lattice is subtle.
const MINOR_HALF: f32 = 0.06;

/// Half-thickness (world units) of a major grid line (every [`MAJOR_EVERY`] cells, and the axes).
/// Slightly heavier so the eye can chunk the field into larger blocks.
const MAJOR_HALF: f32 = 0.14;

/// Every Nth line (counting out from the origin) is drawn as a heavier "major" line.
const MAJOR_EVERY: i32 = 4;

/// A minor grid line color — a desaturated cool slate, just above the [`crate::CLEAR_LIT`] clear so
/// it reads as a faint lattice without competing with the unit bodies.
const MINOR_COLOR: [f32; 3] = crate::theme::HAIRLINE;

/// A major grid line color — a touch brighter than minor so the larger blocks read.
const MAJOR_COLOR: [f32; 3] = [0.16, 0.20, 0.27];

/// One ground-grid line as an axis-aligned world rectangle (center + half-extents + color). Pure
/// CPU data produced by [`grid_lines`]; converted to a [`LineInstance`] for upload.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct GridLine {
    /// Line center in world space.
    pub cx: f32,
    pub cy: f32,
    /// Half-extent in world units (one axis is long = the line length, the other is the thin
    /// half-thickness).
    pub hw: f32,
    pub hh: f32,
    pub r: f32,
    pub g: f32,
    pub b: f32,
    /// Whether this is a heavier "major" line (centralizes the test's structural assertions).
    pub major: bool,
}

impl GridLine {
    fn instance(&self) -> LineInstance {
        LineInstance {
            cx: self.cx,
            cy: self.cy,
            hw: self.hw,
            hh: self.hh,
            r: self.r,
            g: self.g,
            b: self.b,
            _pad: 0.0,
        }
    }
}

/// The GPU-uploadable slice of a [`GridLine`] (drops the CPU-only `major`). `repr(C)` + `Pod`; the
/// field order MUST match `terrain.wgsl`'s instance attributes and the `vertex_attr_array` below.
/// `_pad` keeps the stride a multiple of 8 bytes (vec2 alignment) and the color a clean vec3.
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, bytemuck::Pod, bytemuck::Zeroable)]
struct LineInstance {
    cx: f32,
    cy: f32,
    hw: f32,
    hh: f32,
    r: f32,
    g: f32,
    b: f32,
    _pad: f32,
}

/// Whether the line `i` cells out from the origin is a "major" line (every [`MAJOR_EVERY`] cells,
/// and the origin axis itself at `i == 0`). Pure — the testable classifier the layout shares.
#[inline]
pub fn is_major(i: i32) -> bool {
    i % MAJOR_EVERY == 0
}

/// Build the ground-grid lines for the command view: a lattice of vertical + horizontal lines from
/// `-half_extent` to `+half_extent` spaced `spacing` apart, the every-[`MAJOR_EVERY`]th heavier.
/// Pure (no GPU, no sim, no fog) — the testable layout seam. Returns vertical lines first, then
/// horizontal, each as a thin world rectangle ready to expand to a [`LineInstance`].
///
/// `spacing` is clamped to a sane positive floor so a degenerate `0.0` can't loop forever or divide
/// by zero (a render-side guard; the host always passes the constant [`GRID_SPACING`]).
pub fn grid_lines(half_extent: f32, spacing: f32) -> Vec<GridLine> {
    let spacing = spacing.max(0.5);
    let half_extent = half_extent.max(spacing);
    // How many lines fit on one side of the origin (origin line is index 0).
    let count = (half_extent / spacing).floor() as i32;

    let mut out = Vec::with_capacity(((count * 2 + 1) * 2) as usize);
    for i in -count..=count {
        let pos = i as f32 * spacing;
        let major = is_major(i);
        let half_thick = if major { MAJOR_HALF } else { MINOR_HALF };
        let [r, g, b] = if major { MAJOR_COLOR } else { MINOR_COLOR };
        // Vertical line at world x = pos, spanning the full y extent.
        out.push(GridLine {
            cx: pos,
            cy: 0.0,
            hw: half_thick,
            hh: half_extent,
            r,
            g,
            b,
            major,
        });
    }
    for i in -count..=count {
        let pos = i as f32 * spacing;
        let major = is_major(i);
        let half_thick = if major { MAJOR_HALF } else { MINOR_HALF };
        let [r, g, b] = if major { MAJOR_COLOR } else { MINOR_COLOR };
        // Horizontal line at world y = pos, spanning the full x extent.
        out.push(GridLine {
            cx: 0.0,
            cy: pos,
            hw: half_extent,
            hh: half_thick,
            r,
            g,
            b,
            major,
        });
    }
    out
}

/// A unit-quad corner in [-1, 1]^2 (the shader scales it by the per-line half-size).
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

/// World-space ground-grid renderer. Unlike the screen-space chrome passes it does NOT own a
/// camera UBO — it borrows the unit pass's camera bind group (it must share the exact top-down
/// view-projection so the grid lines up with the units). Owns only its pipeline + buffers.
pub struct TerrainRenderer {
    pipeline: wgpu::RenderPipeline,
    quad_buf: wgpu::Buffer,
    instance_buf: wgpu::Buffer,
    instance_cap: usize,
    /// The CPU-side grid lines, built once (the lattice is fixed — it never depends on sim state).
    lines: Vec<LineInstance>,
}

impl TerrainRenderer {
    /// Build the ground-grid pipeline against the swapchain `surface_format`, sharing the unit
    /// pass's `camera_layout` (so the grid uses the same view-projection). The `device` is borrowed
    /// (D19). The grid geometry is built once here — it is a fixed lattice, not per-frame data.
    pub fn new(
        device: &wgpu::Device,
        surface_format: wgpu::TextureFormat,
        camera_layout: &wgpu::BindGroupLayout,
    ) -> Self {
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("gonedark.terrain_shader"),
            source: wgpu::ShaderSource::Wgsl(include_str!("terrain.wgsl").into()),
        });

        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("gonedark.terrain_pipeline_layout"),
            bind_group_layouts: &[Some(camera_layout)],
            immediate_size: 0,
        });

        let quad_layout = wgpu::VertexBufferLayout {
            array_stride: std::mem::size_of::<QuadVertex>() as u64,
            step_mode: wgpu::VertexStepMode::Vertex,
            attributes: &wgpu::vertex_attr_array![0 => Float32x2],
        };
        let instance_layout = wgpu::VertexBufferLayout {
            array_stride: std::mem::size_of::<LineInstance>() as u64,
            step_mode: wgpu::VertexStepMode::Instance,
            // 1=center(vec2), 2=half(vec2), 3=color(vec3). The trailing `_pad` f32 is not bound.
            attributes: &wgpu::vertex_attr_array![
                1 => Float32x2,
                2 => Float32x2,
                3 => Float32x3
            ],
        };

        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("gonedark.terrain_pipeline"),
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
                    // Opaque — the grid IS the ground, drawn first; nothing reads behind it.
                    blend: Some(wgpu::BlendState::REPLACE),
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
            label: Some("gonedark.terrain_quad_vbo"),
            contents: bytemuck::cast_slice(&QUAD_VERTS),
            usage: wgpu::BufferUsages::VERTEX,
        });

        let lines: Vec<LineInstance> = grid_lines(GRID_HALF_EXTENT, GRID_SPACING)
            .iter()
            .map(|l| l.instance())
            .collect();
        let instance_cap = lines.len().max(1);
        let instance_buf = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("gonedark.terrain_instance_vbo"),
            contents: bytemuck::cast_slice(&lines),
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
        });

        TerrainRenderer {
            pipeline,
            quad_buf,
            instance_buf,
            instance_cap,
            lines,
        }
    }

    /// Draw the ground grid into the existing command-view render pass (the caller owns the pass so
    /// the grid composites into the same clear/store as the units, drawn first under them). Borrows
    /// the unit pass's `camera_bind_group` so the grid shares the world frame. World-space, no fog —
    /// the host calls this only in the command view (`!world_dark`).
    pub fn draw<'a>(
        &'a self,
        pass: &mut wgpu::RenderPass<'a>,
        camera_bind_group: &'a wgpu::BindGroup,
    ) {
        if self.lines.is_empty() {
            return;
        }
        debug_assert!(self.lines.len() <= self.instance_cap);
        pass.set_pipeline(&self.pipeline);
        pass.set_bind_group(0, camera_bind_group, &[]);
        pass.set_vertex_buffer(0, self.quad_buf.slice(..));
        pass.set_vertex_buffer(1, self.instance_buf.slice(..));
        pass.draw(0..QUAD_VERTS.len() as u32, 0..self.lines.len() as u32);
    }
}

#[cfg(test)]
mod tests {
    //! `render` is the float boundary, so f32 layout math is fair game. `TerrainRenderer::new` needs
    //! a real `wgpu::Device` (no display in CI), so the pipeline path is untested; the testable
    //! layout math is factored into [`grid_lines`] / [`is_major`].

    use super::*;

    const EPS: f32 = 1e-5;

    #[test]
    fn is_major_marks_origin_and_every_nth() {
        assert!(is_major(0), "origin axis is major");
        assert!(is_major(MAJOR_EVERY), "Nth line is major");
        assert!(is_major(-MAJOR_EVERY), "Nth line either side is major");
        assert!(!is_major(1), "adjacent line is minor");
        assert!(!is_major(MAJOR_EVERY - 1), "off-grid line is minor");
    }

    #[test]
    fn grid_has_vertical_and_horizontal_lines_in_equal_count() {
        let lines = grid_lines(40.0, 8.0);
        // 40/8 = 5 lines each side + the origin = 11 per axis, two axes = 22.
        let count = (40.0f32 / 8.0).floor() as i32;
        let per_axis = (count * 2 + 1) as usize;
        assert_eq!(lines.len(), per_axis * 2);
        // First half are vertical (long in y, thin in x); second half horizontal (long in x).
        let (vert, horiz) = lines.split_at(per_axis);
        assert!(vert.iter().all(|l| l.hh > l.hw), "vertical lines are tall+thin");
        assert!(horiz.iter().all(|l| l.hw > l.hh), "horizontal lines are wide+thin");
    }

    #[test]
    fn lines_span_the_full_extent() {
        let half = 40.0;
        let lines = grid_lines(half, 8.0);
        // Every line's long half-extent reaches the grid edge.
        for l in &lines {
            let long = l.hw.max(l.hh);
            assert!((long - half).abs() < EPS, "line spans the full extent");
        }
    }

    #[test]
    fn lines_sit_on_spacing_multiples_within_extent() {
        let lines = grid_lines(40.0, 8.0);
        let per_axis = lines.len() / 2;
        let (vert, _) = lines.split_at(per_axis);
        for l in vert {
            // Vertical line x is an exact multiple of the spacing.
            let k = (l.cx / 8.0).round();
            assert!((l.cx - k * 8.0).abs() < EPS, "on a spacing multiple");
            assert!(l.cx.abs() <= 40.0 + EPS, "within the extent");
        }
    }

    #[test]
    fn origin_axes_are_major_lines() {
        let lines = grid_lines(40.0, 8.0);
        // The vertical line at x=0 and the horizontal at y=0 are major (origin index 0).
        let origin_vert = lines.iter().find(|l| l.cx.abs() < EPS && l.hh > l.hw).unwrap();
        let origin_horiz = lines.iter().find(|l| l.cy.abs() < EPS && l.hw > l.hh).unwrap();
        assert!(origin_vert.major, "x=0 axis is major");
        assert!(origin_horiz.major, "y=0 axis is major");
    }

    #[test]
    fn major_lines_are_thicker_and_brighter_than_minor() {
        let lines = grid_lines(40.0, 8.0);
        let major = lines.iter().find(|l| l.major).unwrap();
        let minor = lines.iter().find(|l| !l.major).unwrap();
        let major_thick = major.hw.min(major.hh);
        let minor_thick = minor.hw.min(minor.hh);
        assert!(major_thick > minor_thick, "major lines are thicker");
        // Brighter: at least one channel is higher (the major palette is lighter slate).
        let major_lum = major.r + major.g + major.b;
        let minor_lum = minor.r + minor.g + minor.b;
        assert!(major_lum > minor_lum, "major lines are brighter");
    }

    #[test]
    fn grid_is_above_the_clear_so_it_reads() {
        // Every grid line is brighter than the lit clear (~0.02,0.03,0.05) so it shows, but stays
        // dark enough to sit under the unit bodies — a subtle lattice, not a wall of lines.
        let lines = grid_lines(40.0, 8.0);
        for l in &lines {
            assert!(l.r > 0.02 && l.g > 0.03 && l.b > 0.05, "grid reads above the clear");
            assert!(l.r < 0.4 && l.g < 0.4 && l.b < 0.4, "grid stays subtle, under the units");
        }
    }

    #[test]
    fn degenerate_spacing_is_clamped_not_looping() {
        // A zero/negative spacing must not divide-by-zero or loop forever; it clamps to a floor and
        // still produces a finite, non-empty grid.
        let lines = grid_lines(40.0, 0.0);
        assert!(!lines.is_empty());
        assert!(lines.iter().all(|l| l.cx.is_finite() && l.cy.is_finite()));
    }

    #[test]
    fn grid_is_symmetric_about_the_origin() {
        // The lattice is a fixed, world-symmetric reference (no sim/fog input): a line at +pos has a
        // mirror at -pos.
        let lines = grid_lines(40.0, 8.0);
        let per_axis = lines.len() / 2;
        let (vert, _) = lines.split_at(per_axis);
        let xs: Vec<f32> = vert.iter().map(|l| l.cx).collect();
        for x in &xs {
            assert!(
                xs.iter().any(|o| (o + x).abs() < EPS),
                "every line has an origin mirror"
            );
        }
    }

    #[test]
    fn terrain_wgsl_parses_and_validates() {
        let src = include_str!("terrain.wgsl");
        let module = naga::front::wgsl::parse_str(src).expect("terrain.wgsl must parse");
        let mut validator = naga::valid::Validator::new(
            naga::valid::ValidationFlags::all(),
            naga::valid::Capabilities::all(),
        );
        validator
            .validate(&module)
            .expect("terrain.wgsl must validate");
    }
}
