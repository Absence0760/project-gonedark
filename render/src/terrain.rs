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
//! [`grid_lines`] fn so it is unit-testable without a GPU, exactly the `marquee_quads` / `layout_glyphs`
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

/// Half-thickness (world units) of a normal (minor) grid line. Thin so the minor subdivisions read
/// as a faint whisper under the major blocks, not as the dominant lattice.
const MINOR_HALF: f32 = 0.055;

/// Half-thickness (world units) of a major grid line (every [`MAJOR_EVERY`] cells, and the axes).
/// Distinctly heavier than [`MINOR_HALF`] (~4x) so the eye chunks the field into clear blocks — the
/// major tier carries the structural read, the minor tier only subdivides it.
const MAJOR_HALF: f32 = 0.21;

/// Every Nth line (counting out from the origin) is drawn as a heavier "major" line.
const MAJOR_EVERY: i32 = 4;

/// Half-length (world units) of each arm of a registration cross drawn at a major×major
/// intersection — a small "+" survey mark, like the grid ticks on a military map. Short so the cross
/// reads as a deliberate node at the junction, not another full line.
const TICK_HALF_LEN: f32 = 1.15;

/// Half-thickness (world units) of a registration-cross arm. Between minor and major thickness.
const TICK_HALF_THICK: f32 = 0.12;

/// Registration-cross color — clearly brighter than [`MAJOR_COLOR`] (still cold, low-saturation,
/// well under the unit/selection brightness) so the surveyed junctions read as intentional marks.
const TICK_COLOR: [f32; 3] = [0.255, 0.305, 0.385];

/// A minor grid line color — a cold, low-saturation slate pulled *below* the theme [`HAIRLINE`] so
/// the subdivisions sit just above the ground fill and recede; the major tier, not this, structures
/// the board. (`HAIRLINE` ≈ 0.10/0.13/0.18; this is a touch dimmer.)
///
/// [`HAIRLINE`]: crate::theme::HAIRLINE
const MINOR_COLOR: [f32; 3] = [0.072, 0.092, 0.130];

/// A major grid line color — clearly brighter than minor (a cold blue-grey, blue leading, still low
/// saturation) so the larger blocks read as the map's structure without competing with unit bodies.
const MAJOR_COLOR: [f32; 3] = [0.205, 0.250, 0.335];

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

/// Build the registration cross-marks: a small "+" survey mark at every major×major grid
/// intersection (the origin and every [`MAJOR_EVERY`]th line either way), giving the lattice precise,
/// surveyed nodes like a military map's coordinate ticks. Pure (no GPU, no sim, no fog) — a fixed
/// lattice keyed only off the world extent, identical every frame. Each cross is two short
/// perpendicular [`GridLine`] arms (horizontal then vertical) coloured [`TICK_COLOR`] so the junctions
/// read above the major lines they sit on. Drawn AFTER [`grid_lines`] (opaque REPLACE) so the marks
/// win at the intersection. Marks are flagged `major` (they belong to the structural tier).
pub fn tick_marks(half_extent: f32, spacing: f32) -> Vec<GridLine> {
    let spacing = spacing.max(0.5);
    let half_extent = half_extent.max(spacing);
    let count = (half_extent / spacing).floor() as i32;

    // The major line indices within the extent (origin + every MAJOR_EVERY-th, both directions).
    let majors: Vec<f32> = (-count..=count)
        .filter(|&i| is_major(i))
        .map(|i| i as f32 * spacing)
        .collect();

    let [r, g, b] = TICK_COLOR;
    let mut out = Vec::with_capacity(majors.len() * majors.len() * 2);
    for &cy in &majors {
        for &cx in &majors {
            // Horizontal arm.
            out.push(GridLine {
                cx,
                cy,
                hw: TICK_HALF_LEN,
                hh: TICK_HALF_THICK,
                r,
                g,
                b,
                major: true,
            });
            // Vertical arm.
            out.push(GridLine {
                cx,
                cy,
                hw: TICK_HALF_THICK,
                hh: TICK_HALF_LEN,
                r,
                g,
                b,
                major: true,
            });
        }
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

/// Half-extent (world units) of the procedural ground-fill quad drawn under the grid. Generously
/// larger than [`GRID_HALF_EXTENT`] / the ±40 camera framing so the textured floor fully covers the
/// frame (including its corners) with no slate sliver at the edges.
const GROUND_FILL_HALF: f32 = 120.0;

/// A single world-space XY vertex of the ground-fill quad (`terrain.wgsl` `vs_ground`).
#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct GroundVertex {
    world: [f32; 2],
}

/// The two triangles of the big ground quad spanning ±[`GROUND_FILL_HALF`] on the z = 0 plane.
const GROUND_VERTS: [GroundVertex; 6] = [
    GroundVertex { world: [-GROUND_FILL_HALF, -GROUND_FILL_HALF] },
    GroundVertex { world: [GROUND_FILL_HALF, -GROUND_FILL_HALF] },
    GroundVertex { world: [GROUND_FILL_HALF, GROUND_FILL_HALF] },
    GroundVertex { world: [-GROUND_FILL_HALF, -GROUND_FILL_HALF] },
    GroundVertex { world: [GROUND_FILL_HALF, GROUND_FILL_HALF] },
    GroundVertex { world: [-GROUND_FILL_HALF, GROUND_FILL_HALF] },
];

/// World-space ground-grid renderer. Unlike the screen-space chrome passes it does NOT own a
/// camera UBO — it borrows the unit pass's camera bind group (it must share the exact top-down
/// view-projection so the grid lines up with the units). Owns only its pipelines + buffers (a
/// procedural ground-fill quad drawn first, then the grid lines on top).
pub struct TerrainRenderer {
    pipeline: wgpu::RenderPipeline,
    quad_buf: wgpu::Buffer,
    instance_buf: wgpu::Buffer,
    instance_cap: usize,
    /// The CPU-side grid lines, built once (the lattice is fixed — it never depends on sim state).
    lines: Vec<LineInstance>,
    /// Procedural ground-fill quad: its own pipeline (the `vs_ground`/`fs_ground` entries in
    /// `terrain.wgsl`) + a 6-vertex world-space quad, drawn FIRST so the floor reads as grounded
    /// terrain under the grid. Shares the unit pass's camera bind group (group 0), like the lines.
    ground_pipeline: wgpu::RenderPipeline,
    ground_buf: wgpu::Buffer,
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

        // Ground-fill pipeline: same camera layout (group 0), a single world-space XY vertex stream,
        // the procedural `vs_ground`/`fs_ground` entries. Opaque REPLACE — drawn first, nothing behind.
        let ground_vertex_layout = wgpu::VertexBufferLayout {
            array_stride: std::mem::size_of::<GroundVertex>() as u64,
            step_mode: wgpu::VertexStepMode::Vertex,
            attributes: &wgpu::vertex_attr_array![0 => Float32x2],
        };
        let ground_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("gonedark.terrain_ground_pipeline"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_ground"),
                buffers: &[ground_vertex_layout],
                compilation_options: wgpu::PipelineCompilationOptions::default(),
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs_ground"),
                targets: &[Some(wgpu::ColorTargetState {
                    format: surface_format,
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
        let ground_buf = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("gonedark.terrain_ground_vbo"),
            contents: bytemuck::cast_slice(&GROUND_VERTS),
            usage: wgpu::BufferUsages::VERTEX,
        });

        // The grid lattice, then the registration cross-marks on top (one instanced draw, opaque
        // REPLACE — the appended marks win at the major junctions they sit on).
        let mut grid = grid_lines(GRID_HALF_EXTENT, GRID_SPACING);
        grid.extend(tick_marks(GRID_HALF_EXTENT, GRID_SPACING));
        let lines: Vec<LineInstance> = grid.iter().map(|l| l.instance()).collect();
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
            ground_pipeline,
            ground_buf,
        }
    }

    /// Draw the ground (procedural fill, then the grid lines on top) into the existing command-view
    /// render pass (the caller owns the pass so the ground composites into the same clear/store as
    /// the units, drawn first under them). Borrows the unit pass's `camera_bind_group` so the ground
    /// shares the world frame. World-space, no fog — the host calls this only in the command view
    /// (`!world_dark`).
    pub fn draw<'a>(
        &'a self,
        pass: &mut wgpu::RenderPass<'a>,
        camera_bind_group: &'a wgpu::BindGroup,
    ) {
        pass.set_bind_group(0, camera_bind_group, &[]);
        // Ground fill first (under everything): procedural tonal variation + vignette, so the floor
        // is grounded terrain rather than a flat slate clear.
        pass.set_pipeline(&self.ground_pipeline);
        pass.set_vertex_buffer(0, self.ground_buf.slice(..));
        pass.draw(0..GROUND_VERTS.len() as u32, 0..1);

        // Grid lines on top of the fill.
        if self.lines.is_empty() {
            return;
        }
        debug_assert!(self.lines.len() <= self.instance_cap);
        pass.set_pipeline(&self.pipeline);
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
    fn tick_marks_sit_only_on_major_intersections() {
        // Every cross arm centers on a major×major junction: both cx and cy must be a multiple of the
        // MAJOR_EVERY*spacing step (and within the extent).
        let half = 40.0;
        let spacing = 8.0;
        let step = MAJOR_EVERY as f32 * spacing;
        let marks = tick_marks(half, spacing);
        assert!(!marks.is_empty(), "registration marks are produced");
        for m in &marks {
            for c in [m.cx, m.cy] {
                let k = (c / step).round();
                assert!((c - k * step).abs() < EPS, "mark sits on a major step");
                assert!(c.abs() <= half + EPS, "mark within the extent");
            }
        }
    }

    #[test]
    fn tick_marks_are_two_short_perpendicular_arms_per_node() {
        // Majors within ±40 at step 32: indices -1,0,1 -> 3 per axis -> 9 nodes -> 18 arms (2 each).
        let spacing = 8.0;
        let step = MAJOR_EVERY as f32 * spacing;
        let majors_per_axis = (-((40.0f32 / spacing).floor() as i32)..=((40.0f32 / spacing).floor() as i32))
            .filter(|&i| is_major(i))
            .count();
        let marks = tick_marks(40.0, spacing);
        assert_eq!(marks.len(), majors_per_axis * majors_per_axis * 2, "two arms per node");
        // Each arm is short (well under a major cell) and one axis is the long arm, the other thin.
        for m in &marks {
            assert!(m.hw.max(m.hh) <= TICK_HALF_LEN + EPS, "arm is short, not a full line");
            assert!(m.hw.max(m.hh) < step, "arm shorter than a major cell");
            assert!(m.hw.min(m.hh) <= TICK_HALF_THICK + EPS, "arm is thin");
            assert!((m.hw - m.hh).abs() > EPS, "arm is a line (long on one axis)");
        }
        // Per node there is exactly one horizontal arm (hw>hh) and one vertical (hh>hw).
        assert_eq!(marks.iter().filter(|m| m.hw > m.hh).count(), majors_per_axis * majors_per_axis);
        assert_eq!(marks.iter().filter(|m| m.hh > m.hw).count(), majors_per_axis * majors_per_axis);
    }

    #[test]
    fn tick_marks_are_brighter_than_major_lines_but_stay_subtle() {
        // The survey nodes read above the major lattice, yet stay cold/low-sat and under the unit
        // brightness so units/selection rims keep popping.
        let marks = tick_marks(40.0, 8.0);
        let tick_lum: f32 = TICK_COLOR.iter().sum();
        let major_lum: f32 = MAJOR_COLOR.iter().sum();
        assert!(tick_lum > major_lum, "registration marks brighter than major lines");
        for m in &marks {
            assert_eq!([m.r, m.g, m.b], TICK_COLOR, "marks carry the tick colour");
            assert!(m.r < 0.4 && m.g < 0.4 && m.b < 0.45, "marks stay subtle, under the units");
            // Cold + low-saturation: blue leads, red trails.
            assert!(m.b > m.g && m.g > m.r, "marks stay cold (blue-leading)");
        }
    }

    #[test]
    fn tick_marks_are_symmetric_about_the_origin() {
        // Fixed, world-symmetric reference (no sim/fog input): a node at (+x,+y) has mirrors.
        let marks = tick_marks(40.0, 8.0);
        let centers: Vec<(f32, f32)> = marks.iter().map(|m| (m.cx, m.cy)).collect();
        for &(x, y) in &centers {
            assert!(
                centers.iter().any(|&(ox, oy)| (ox + x).abs() < EPS && (oy - y).abs() < EPS),
                "every node has an x-mirror"
            );
        }
    }

    #[test]
    fn ground_fill_quad_covers_the_camera_framing() {
        // The ground-fill quad must fully cover the ±40 top-down camera framing (and the ±44 grid)
        // with margin, so no flat slate sliver shows at the frame edges.
        assert!(GROUND_FILL_HALF > GRID_HALF_EXTENT, "ground covers the grid");
        assert!(GROUND_FILL_HALF >= 60.0, "ground covers the ±40 framing's corners with margin");
        // It is two triangles (6 verts) and every vertex sits on a ±GROUND_FILL_HALF corner.
        assert_eq!(GROUND_VERTS.len(), 6);
        for v in &GROUND_VERTS {
            assert!((v.world[0].abs() - GROUND_FILL_HALF).abs() < EPS);
            assert!((v.world[1].abs() - GROUND_FILL_HALF).abs() < EPS);
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
