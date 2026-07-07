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
//! `grid_lines` fn so it is unit-testable without a GPU, exactly the `marquee_quads` / `layout_glyphs`
//! pattern. `TerrainRenderer::render` is the only GPU-touching code and is exercised by the
//! offscreen `viz-runner`, not the no-GPU CI matrix.

use gonedark_core::flow_field::GRID;
use gonedark_core::obstacles::{Obstacle, ObstacleKind};
use gonedark_core::terrain::{Cover, Terrain};
use std::f32::consts::FRAC_PI_4;
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
/// CPU data produced by [`grid_lines`]; converted to a `LineInstance` for upload.
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

/// Whether the line `i` cells out from the origin is a "major" line (every `MAJOR_EVERY` cells,
/// and the origin axis itself at `i == 0`). Pure — the testable classifier the layout shares.
#[inline]
pub fn is_major(i: i32) -> bool {
    i % MAJOR_EVERY == 0
}

/// Build the ground-grid lines for the command view: a lattice of vertical + horizontal lines from
/// `-half_extent` to `+half_extent` spaced `spacing` apart, the every-`MAJOR_EVERY`th heavier.
/// Pure (no GPU, no sim, no fog) — the testable layout seam. Returns vertical lines first, then
/// horizontal, each as a thin world rectangle ready to expand to a `LineInstance`.
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
/// intersection (the origin and every `MAJOR_EVERY`th line either way), giving the lattice precise,
/// surveyed nodes like a military map's coordinate ticks. Pure (no GPU, no sim, no fog) — a fixed
/// lattice keyed only off the world extent, identical every frame. Each cross is two short
/// perpendicular [`GridLine`] arms (horizontal then vertical) coloured `TICK_COLOR` so the junctions
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

/// CPU **reference implementation** of `terrain.wgsl`'s `elevation` — the smooth, alias-free
/// procedural relief field the command ground is shaded and contoured from, normalized to ~[-1, 1].
/// A few low-frequency, mutually-rotated sinusoids sum into broad rolling relief. The ground pass is
/// a pure function of world position through this field (no sim/fog input), so mirroring it here
/// lets its range + shape be unit-tested off-GPU — the `world::star_hash21` / `world::moon_glow`
/// pattern. `render` is the float boundary (invariant #1), so `f32` transcendentals are fair game.
/// Keep in lockstep with the shader (same constants, same operation order).
pub fn elevation(x: f32, y: f32) -> f32 {
    let h = (x * 0.045 + 0.7).sin() * (y * 0.039 - 0.3).cos()
        + 0.60 * (x * 0.021 - y * 0.018 + 1.7).sin()
        + 0.50 * (y * 0.030 + x * 0.013).cos();
    (h / 2.10).clamp(-1.0, 1.0)
}

/// CPU **reference implementation** of `terrain.wgsl`'s `hill` term: the cartographic hillshade
/// weight at world `(x, y)`. Finite-differences [`elevation`] into a surface normal and lights it
/// from a fixed NW key, returning a low-contrast multiplier in `[0.90, 1.14]` (rises catch the key,
/// hollows fall into shade) so the command floor reads as lit 3-D relief without ever competing with
/// the units. Pure — a function of world position ONLY (no sim/fog), so it is identical every frame
/// (invariant #6) and its range + response are unit-testable. Keep the constants in lockstep with
/// the shader.
pub fn hillshade(x: f32, y: f32) -> f32 {
    const E: f32 = 3.0;
    let hx = elevation(x + E, y) - elevation(x - E, y);
    let hy = elevation(x, y + E) - elevation(x, y - E);
    // Surface normal from the height gradient (z up), then a normalized NW key light.
    let (nx, ny, nz) = (-hx * 6.0, -hy * 6.0, 1.0);
    let nlen = (nx * nx + ny * ny + nz * nz).sqrt();
    let (kx, ky, kz) = (-0.55f32, 0.62, 0.56);
    let klen = (kx * kx + ky * ky + kz * kz).sqrt();
    let ndotl = (nx * kx + ny * ky + nz * kz) / (nlen * klen);
    let t = (ndotl * 0.5 + 0.5).clamp(0.0, 1.0);
    0.90 + t * (1.14 - 0.90)
}

// ---- Map overlay: cover tint + prop markers (command view only) ---------------------------------
//
// The grid + procedural ground above are MAP-AGNOSTIC — identical every match. These two layers make
// the command view show the *real* map: (1) a translucent per-cell wash keyed off the sim's actual
// [`Cover`] grid, so different battlefields look different and cover is tactically legible top-down,
// and (2) a top-down marker per static [`Obstacle`], so the props the embodied view draws as 3-D
// meshes also read on the strategic map. Both are drawn INSIDE the terrain pass (under the units), so
// they read as ground/terrain structure, and ONLY in the command view — the terrain pass never runs
// under the dark embodied frame (invariant #6). Pure render derivations (core → render, the allowed
// direction): they READ the sim's static map data and never mutate it, and carry no floats into
// `core`. `render` is the float boundary (invariant #1), so `f32` here is fair game.

/// World half-extent of the sim cover grid as `f32` — mirrors `debug::GRID_HALF` and
/// `core::flow_field::HALF_EXTENT` (`GRID/2`, with `CELL_SIZE == 1`). Cell `(cx,cy)` spans world
/// `[-COVER_GRID_HALF + cx, -COVER_GRID_HALF + cx + 1)`, so an overlay quad lands exactly on the sim
/// cell the flow field / line-of-sight read.
const COVER_GRID_HALF: f32 = (GRID / 2) as f32;

/// Cover-wash fill colours (RGBA). Hue mirrors the `debug` cover-outline palette (Light amber, Heavy
/// steel-blue, Impassable hot red-orange) so the two map-diagnostic views agree; alpha rises with the
/// tier so a movement-blocking `Impassable` cell reads strongest, `Light` concealment faintest — the
/// wash tints the ground+grid without ever hiding a unit token drawn on top.
const COVER_FILL_LIGHT: [f32; 4] = [0.85, 0.70, 0.25, 0.16];
const COVER_FILL_HEAVY: [f32; 4] = [0.42, 0.55, 0.72, 0.22];
const COVER_FILL_IMPASSABLE: [f32; 4] = [0.90, 0.35, 0.20, 0.30];

/// Opacity of a top-down prop marker — near-opaque so a discrete diamond reads as a placed object,
/// clearly distinct from the translucent per-cell cover wash beneath it.
const PROP_MARKER_ALPHA: f32 = 0.90;

/// One filled, world-space overlay quad (cover-wash cell or prop marker): centre + half-extents +
/// RGBA + a rotation (0 for the axis-aligned cover cells, `π/4` for the diamond prop markers). Pure
/// CPU data produced by [`cover_fill_quads`] / [`prop_markers`]; uploaded straight to the GPU as the
/// per-instance stream of the overlay pipeline. `repr(C)` + `Pod`; the field order MUST match
/// `terrain.wgsl`'s `vs_overlay` instance attributes and the `vertex_attr_array` below. The trailing
/// `_pad` keeps the stride a multiple of 8 bytes.
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, bytemuck::Pod, bytemuck::Zeroable)]
pub struct OverlayQuad {
    /// Quad centre in world space.
    pub cx: f32,
    pub cy: f32,
    /// Half-extent in world units on each local axis (before rotation).
    pub hw: f32,
    pub hh: f32,
    /// Fill colour, straight sRGB `[0,1]`, with a translucency alpha (the layer is alpha-blended).
    pub r: f32,
    pub g: f32,
    pub b: f32,
    pub a: f32,
    /// Rotation (radians) applied to the quad about its centre — `0` = axis-aligned square (cover
    /// cell), `π/4` = diamond (prop marker).
    pub rot: f32,
    _pad: f32,
}

/// Build the translucent cover-wash quads for the command view: one filled cell-sized square per
/// non-open [`Cover`] cell of `terrain`, coloured by tier (`COVER_FILL_LIGHT`/`HEAVY`/`IMPASSABLE`).
/// `Cover::None` cells draw nothing (an open map ⇒ no quads). Each quad sits on the cell CENTRE with a
/// `0.5` half-extent, so it exactly fills the sim cell — the same mapping `core::terrain` uses. Pure
/// (no GPU) — the testable seam, mirroring [`crate::debug::covergrid_lines`].
pub fn cover_fill_quads(terrain: &Terrain) -> Vec<OverlayQuad> {
    let mut out = Vec::new();
    for cy in 0..GRID as i32 {
        for cx in 0..GRID as i32 {
            let [r, g, b, a] = match terrain.cover_at_cell(cx, cy) {
                Cover::None => continue,
                Cover::Light => COVER_FILL_LIGHT,
                Cover::Heavy => COVER_FILL_HEAVY,
                Cover::Impassable => COVER_FILL_IMPASSABLE,
            };
            out.push(OverlayQuad {
                cx: -COVER_GRID_HALF + cx as f32 + 0.5,
                cy: -COVER_GRID_HALF + cy as f32 + 0.5,
                hw: 0.5,
                hh: 0.5,
                r,
                g,
                b,
                a,
                rot: 0.0,
                _pad: 0.0,
            });
        }
    }
    out
}

/// The top-down marker colour for a prop kind — chosen to be distinct from the cover-wash hues
/// (amber/steel/red-orange) AND from each other: trees green, rocks grey, crates tan, barricades
/// khaki, and the two turret emplacements their owning faction's cool tone. Pure — the kind → colour
/// mapping, unit-tested off-GPU.
fn prop_marker_color(kind: ObstacleKind) -> [f32; 3] {
    match kind {
        ObstacleKind::Tree => [0.28, 0.55, 0.30],
        ObstacleKind::Rock => [0.56, 0.57, 0.61],
        ObstacleKind::Crate => [0.72, 0.52, 0.24],
        ObstacleKind::Barricade => [0.60, 0.58, 0.36],
        ObstacleKind::TurretUs => [0.30, 0.55, 0.86],
        ObstacleKind::TurretFr => [0.38, 0.72, 0.72],
    }
}

/// Build the top-down prop markers for the command view: one diamond per static [`Obstacle`], centred
/// on the prop's world position, sized to its sim collision footprint
/// ([`ObstacleKind::footprint_radius`]) so a wide barricade reads bigger than a slim tree, and tinted
/// per kind (`prop_marker_color`). The embodied view already draws these as 3-D meshes
/// (`crate::prop_draw_plan`); this is the strategic-map read of the SAME sim list (core → render).
/// Pure (no GPU) — the testable seam. `_pad`/`rot` mark the quad a rotated diamond so it reads as a
/// placed object over the axis-aligned cover wash.
pub fn prop_markers(obstacles: &[Obstacle]) -> Vec<OverlayQuad> {
    obstacles
        .iter()
        .map(|o| {
            let [r, g, b] = prop_marker_color(o.kind);
            let footprint = crate::fixed_to_f32(o.kind.footprint_radius());
            OverlayQuad {
                cx: crate::fixed_to_f32(o.pos.x),
                cy: crate::fixed_to_f32(o.pos.y),
                hw: footprint,
                hh: footprint,
                r,
                g,
                b,
                a: PROP_MARKER_ALPHA,
                rot: FRAC_PI_4,
                _pad: 0.0,
            }
        })
        .collect()
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
    GroundVertex {
        world: [-GROUND_FILL_HALF, -GROUND_FILL_HALF],
    },
    GroundVertex {
        world: [GROUND_FILL_HALF, -GROUND_FILL_HALF],
    },
    GroundVertex {
        world: [GROUND_FILL_HALF, GROUND_FILL_HALF],
    },
    GroundVertex {
        world: [-GROUND_FILL_HALF, -GROUND_FILL_HALF],
    },
    GroundVertex {
        world: [GROUND_FILL_HALF, GROUND_FILL_HALF],
    },
    GroundVertex {
        world: [-GROUND_FILL_HALF, GROUND_FILL_HALF],
    },
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
    /// Map-overlay pass (cover wash + prop markers): an ALPHA-BLENDED instanced pipeline
    /// (`vs_overlay`/`fs_overlay`) drawn AFTER the grid, so the translucent cover tint + the prop
    /// diamonds composite over the ground+grid but still sit UNDER the units (invariant #6: command
    /// view only — this whole pass never runs under the dark embodied frame). The instances are the
    /// real map (`cover_fill_quads` + `prop_markers`), uploaded once via [`Self::set_map_overlay`];
    /// `overlay_count == 0` (an open, prop-less map) draws nothing.
    overlay_pipeline: wgpu::RenderPipeline,
    overlay_buf: wgpu::Buffer,
    overlay_count: usize,
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

        // Map-overlay pipeline: same camera layout (group 0), the shared unit-quad corner stream
        // (vertex buffer 0) + a per-instance `OverlayQuad`, drawn with ALPHA_BLENDING so the cover
        // wash is translucent (the ground/grid read through) and prop diamonds layer cleanly.
        // A fresh corner-vertex layout (the earlier `quad_layout` was moved into the grid pipeline).
        let overlay_quad_layout = wgpu::VertexBufferLayout {
            array_stride: std::mem::size_of::<QuadVertex>() as u64,
            step_mode: wgpu::VertexStepMode::Vertex,
            attributes: &wgpu::vertex_attr_array![0 => Float32x2],
        };
        let overlay_instance_layout = wgpu::VertexBufferLayout {
            array_stride: std::mem::size_of::<OverlayQuad>() as u64,
            step_mode: wgpu::VertexStepMode::Instance,
            // 1=center(vec2), 2=half(vec2), 3=color(vec4), 4=rot(f32). Trailing `_pad` is not bound.
            attributes: &wgpu::vertex_attr_array![
                1 => Float32x2,
                2 => Float32x2,
                3 => Float32x4,
                4 => Float32
            ],
        };
        let overlay_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("gonedark.terrain_overlay_pipeline"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_overlay"),
                buffers: &[overlay_quad_layout, overlay_instance_layout],
                compilation_options: wgpu::PipelineCompilationOptions::default(),
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs_overlay"),
                targets: &[Some(wgpu::ColorTargetState {
                    format: surface_format,
                    // Translucent tint over the opaque ground+grid drawn first this pass.
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

        // A 1-instance placeholder overlay buffer (a zeroed, never-drawn quad) so the field is always
        // a valid non-empty buffer; `set_map_overlay` replaces it with the real map data and bumps
        // `overlay_count`. `overlay_count == 0` until then, so the placeholder is never drawn.
        let overlay_buf = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("gonedark.terrain_overlay_vbo"),
            contents: bytemuck::cast_slice(&[<OverlayQuad as bytemuck::Zeroable>::zeroed()]),
            usage: wgpu::BufferUsages::VERTEX,
        });

        TerrainRenderer {
            pipeline,
            quad_buf,
            instance_buf,
            instance_cap,
            lines,
            ground_pipeline,
            ground_buf,
            overlay_pipeline,
            overlay_buf,
            overlay_count: 0,
        }
    }

    /// Upload the command-view **map overlay** — the translucent cover wash ([`cover_fill_quads`])
    /// plus the top-down prop markers ([`prop_markers`]) — built from the sim's static map data. The
    /// cover grid + obstacle list are static (placed at scenario build, never mutated per tick), so
    /// this is called ONCE at match boot rather than per frame. Reads `core` map data and writes only
    /// GPU render state (core → render; never the reverse — invariant #4). A no-op-to-draw when the
    /// map is open with no props (`overlay_count` stays 0). `create_buffer_init` uploads through the
    /// `device` alone, so this needs no `&Queue`.
    pub fn set_map_overlay(
        &mut self,
        device: &wgpu::Device,
        terrain: &Terrain,
        obstacles: &[Obstacle],
    ) {
        let mut quads = cover_fill_quads(terrain);
        quads.extend(prop_markers(obstacles));
        self.overlay_count = quads.len();
        if quads.is_empty() {
            return; // keep the placeholder buffer; nothing to draw
        }
        self.overlay_buf = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("gonedark.terrain_overlay_vbo"),
            contents: bytemuck::cast_slice(&quads),
            usage: wgpu::BufferUsages::VERTEX,
        });
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
        if !self.lines.is_empty() {
            debug_assert!(self.lines.len() <= self.instance_cap);
            pass.set_pipeline(&self.pipeline);
            pass.set_vertex_buffer(0, self.quad_buf.slice(..));
            pass.set_vertex_buffer(1, self.instance_buf.slice(..));
            pass.draw(0..QUAD_VERTS.len() as u32, 0..self.lines.len() as u32);
        }

        // Map overlay LAST within the pass (still under the units, drawn in a later pass): the
        // translucent cover wash + prop-marker diamonds, alpha-blended over the ground+grid so the
        // real map reads top-down. `set_map_overlay` (called once at boot) fills the buffer; an open,
        // prop-less map leaves `overlay_count == 0` and this is skipped.
        if self.overlay_count > 0 {
            pass.set_pipeline(&self.overlay_pipeline);
            pass.set_vertex_buffer(0, self.quad_buf.slice(..));
            pass.set_vertex_buffer(1, self.overlay_buf.slice(..));
            pass.draw(0..QUAD_VERTS.len() as u32, 0..self.overlay_count as u32);
        }
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
        assert!(
            vert.iter().all(|l| l.hh > l.hw),
            "vertical lines are tall+thin"
        );
        assert!(
            horiz.iter().all(|l| l.hw > l.hh),
            "horizontal lines are wide+thin"
        );
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
        let origin_vert = lines
            .iter()
            .find(|l| l.cx.abs() < EPS && l.hh > l.hw)
            .unwrap();
        let origin_horiz = lines
            .iter()
            .find(|l| l.cy.abs() < EPS && l.hw > l.hh)
            .unwrap();
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
            assert!(
                l.r > 0.02 && l.g > 0.03 && l.b > 0.05,
                "grid reads above the clear"
            );
            assert!(
                l.r < 0.4 && l.g < 0.4 && l.b < 0.4,
                "grid stays subtle, under the units"
            );
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
        let majors_per_axis = (-((40.0f32 / spacing).floor() as i32)
            ..=((40.0f32 / spacing).floor() as i32))
            .filter(|&i| is_major(i))
            .count();
        let marks = tick_marks(40.0, spacing);
        assert_eq!(
            marks.len(),
            majors_per_axis * majors_per_axis * 2,
            "two arms per node"
        );
        // Each arm is short (well under a major cell) and one axis is the long arm, the other thin.
        for m in &marks {
            assert!(
                m.hw.max(m.hh) <= TICK_HALF_LEN + EPS,
                "arm is short, not a full line"
            );
            assert!(m.hw.max(m.hh) < step, "arm shorter than a major cell");
            assert!(m.hw.min(m.hh) <= TICK_HALF_THICK + EPS, "arm is thin");
            assert!(
                (m.hw - m.hh).abs() > EPS,
                "arm is a line (long on one axis)"
            );
        }
        // Per node there is exactly one horizontal arm (hw>hh) and one vertical (hh>hw).
        assert_eq!(
            marks.iter().filter(|m| m.hw > m.hh).count(),
            majors_per_axis * majors_per_axis
        );
        assert_eq!(
            marks.iter().filter(|m| m.hh > m.hw).count(),
            majors_per_axis * majors_per_axis
        );
    }

    #[test]
    fn tick_marks_are_brighter_than_major_lines_but_stay_subtle() {
        // The survey nodes read above the major lattice, yet stay cold/low-sat and under the unit
        // brightness so units/selection rims keep popping.
        let marks = tick_marks(40.0, 8.0);
        let tick_lum: f32 = TICK_COLOR.iter().sum();
        let major_lum: f32 = MAJOR_COLOR.iter().sum();
        assert!(
            tick_lum > major_lum,
            "registration marks brighter than major lines"
        );
        for m in &marks {
            assert_eq!([m.r, m.g, m.b], TICK_COLOR, "marks carry the tick colour");
            assert!(
                m.r < 0.4 && m.g < 0.4 && m.b < 0.45,
                "marks stay subtle, under the units"
            );
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
                centers
                    .iter()
                    .any(|&(ox, oy)| (ox + x).abs() < EPS && (oy - y).abs() < EPS),
                "every node has an x-mirror"
            );
        }
    }

    #[test]
    fn ground_fill_quad_covers_the_camera_framing() {
        // The ground-fill quad must fully cover the ±40 top-down camera framing (and the ±44 grid)
        // with margin, so no flat slate sliver shows at the frame edges.
        // (`const` asserts: pure const comparisons, enforced at compile time on any build.)
        const _: () = assert!(
            GROUND_FILL_HALF > GRID_HALF_EXTENT,
            "ground covers the grid"
        );
        const _: () = assert!(
            GROUND_FILL_HALF >= 60.0,
            "ground covers the ±40 framing's corners with margin"
        );
        // It is two triangles (6 verts) and every vertex sits on a ±GROUND_FILL_HALF corner.
        assert_eq!(GROUND_VERTS.len(), 6);
        for v in &GROUND_VERTS {
            assert!((v.world[0].abs() - GROUND_FILL_HALF).abs() < EPS);
            assert!((v.world[1].abs() - GROUND_FILL_HALF).abs() < EPS);
        }
    }

    // ---- elevation field + hillshade (command-ground relief mirror) ----

    #[test]
    fn elevation_stays_in_normalized_range() {
        // The field is the single source for both shading and contours; it MUST stay in [-1,1] (the
        // shader clamps to it) so the tonal + contour math downstream is sound.
        for i in 0..64 {
            for j in 0..64 {
                let x = i as f32 * 2.3 - 70.0;
                let y = j as f32 * 2.9 - 90.0;
                let e = elevation(x, y);
                assert!(
                    (-1.0..=1.0).contains(&e),
                    "elevation {e} out of range at ({x},{y})"
                );
            }
        }
    }

    #[test]
    fn elevation_is_deterministic_and_not_constant() {
        // A fixed cosmetic reference (invariant #6): the same world point always gives the same
        // relief (identical every frame) — but the field is not flat, so it carries real relief.
        let mut min = f32::INFINITY;
        let mut max = f32::NEG_INFINITY;
        for i in 0..40 {
            let x = i as f32 * 3.1 - 40.0;
            let y = i as f32 * -1.7 + 12.0;
            assert_eq!(
                elevation(x, y),
                elevation(x, y),
                "relief is stable at ({x},{y})"
            );
            let e = elevation(x, y);
            min = min.min(e);
            max = max.max(e);
        }
        assert!(
            max - min > 0.4,
            "the field must carry relief, spread {}",
            max - min
        );
    }

    #[test]
    fn hillshade_stays_in_the_low_contrast_band() {
        // The relief lighting is a gentle multiplier the map recedes under — never a hard shadow
        // that could hide or reveal anything. It must stay inside [0.90, 1.14].
        for i in 0..80 {
            for j in 0..80 {
                let x = i as f32 * 1.9 - 76.0;
                let y = j as f32 * 2.1 - 84.0;
                let h = hillshade(x, y);
                assert!(
                    (0.90 - EPS..=1.14 + EPS).contains(&h),
                    "hillshade {h} out of band"
                );
            }
        }
    }

    #[test]
    fn hillshade_is_deterministic_and_shades_the_relief() {
        // Pure function of position (identical every frame), and it actually lights the slopes — a
        // sunlit rise reads brighter than a shaded hollow, so the terrain looks 3-D, not flat.
        let mut min = f32::INFINITY;
        let mut max = f32::NEG_INFINITY;
        for i in 0..60 {
            let x = i as f32 * 2.7 - 60.0;
            let y = i as f32 * 1.3 - 30.0;
            assert_eq!(
                hillshade(x, y),
                hillshade(x, y),
                "hillshade stable at ({x},{y})"
            );
            let h = hillshade(x, y);
            min = min.min(h);
            max = max.max(h);
        }
        assert!(
            max - min > 0.05,
            "hillshade must vary with the slope, spread {}",
            max - min
        );
    }

    // ---- map overlay: cover wash + prop markers ----

    use gonedark_core::components::Vec2;
    use gonedark_core::fixed::Fixed;

    #[test]
    fn cover_fill_open_field_draws_nothing() {
        assert!(
            cover_fill_quads(&Terrain::open()).is_empty(),
            "an open map washes nothing"
        );
    }

    #[test]
    fn cover_fill_one_cell_lands_on_the_right_world_square() {
        let mut t = Terrain::open();
        t.set_cover(0, 0, Cover::Heavy); // south-west corner cell
        let q = cover_fill_quads(&t);
        assert_eq!(q.len(), 1, "one covered cell = one quad");
        let c = q[0];
        // Cell (0,0) centre is (-COVER_GRID_HALF + 0.5) on each axis, filled with a 0.5 half-extent.
        assert!((c.cx - (-COVER_GRID_HALF + 0.5)).abs() < EPS);
        assert!((c.cy - (-COVER_GRID_HALF + 0.5)).abs() < EPS);
        assert!(
            (c.hw - 0.5).abs() < EPS && (c.hh - 0.5).abs() < EPS,
            "quad exactly fills the cell"
        );
        assert_eq!(c.rot, 0.0, "cover cells are axis-aligned, not diamonds");
        assert_eq!([c.r, c.g, c.b, c.a], COVER_FILL_HEAVY, "heavy tier tint");
    }

    #[test]
    fn cover_fill_colours_by_tier_and_skips_open() {
        let mut t = Terrain::open();
        t.set_cover(3, 4, Cover::Light);
        t.set_cover(5, 6, Cover::Heavy);
        t.set_cover(7, 8, Cover::Impassable);
        let q = cover_fill_quads(&t);
        assert_eq!(q.len(), 3, "three covered cells (open cells skipped)");
        let rgba = |x: &OverlayQuad| [x.r, x.g, x.b, x.a];
        assert_eq!(q.iter().filter(|x| rgba(x) == COVER_FILL_LIGHT).count(), 1);
        assert_eq!(q.iter().filter(|x| rgba(x) == COVER_FILL_HEAVY).count(), 1);
        assert_eq!(
            q.iter()
                .filter(|x| rgba(x) == COVER_FILL_IMPASSABLE)
                .count(),
            1
        );
        // Alpha rises with the tier so a blocking cell reads strongest, light concealment faintest.
        // (`const` asserts: pure const comparisons, enforced at compile time on any build.)
        const _: () = assert!(COVER_FILL_LIGHT[3] < COVER_FILL_HEAVY[3]);
        const _: () = assert!(COVER_FILL_HEAVY[3] < COVER_FILL_IMPASSABLE[3]);
        // The wash stays translucent (never hides a unit token drawn on top).
        for x in &q {
            assert!(
                (0.0..1.0).contains(&x.a),
                "cover wash is translucent, alpha {}",
                x.a
            );
        }
    }

    #[test]
    fn cover_fill_count_matches_a_baked_map() {
        // A real baked map yields one quad per non-open cell — the overlay tracks the real cover grid.
        let t = Terrain::from_map_id(Terrain::POINTE_DU_HOC_MAP_ID).unwrap();
        let covered = (0..GRID as i32)
            .flat_map(|cy| (0..GRID as i32).map(move |cx| (cx, cy)))
            .filter(|&(cx, cy)| t.cover_at_cell(cx, cy) != Cover::None)
            .count();
        assert_eq!(cover_fill_quads(&t).len(), covered);
        assert!(covered > 0, "a baked map has cover to wash");
    }

    #[test]
    fn prop_markers_map_position_kind_and_footprint() {
        // One obstacle of each kind at a known world position.
        let obstacles: Vec<Obstacle> = [
            (ObstacleKind::Tree, 3, -4),
            (ObstacleKind::Rock, -7, 2),
            (ObstacleKind::Crate, 5, 5),
            (ObstacleKind::Barricade, -2, -6),
            (ObstacleKind::TurretUs, 8, 1),
            (ObstacleKind::TurretFr, -8, 1),
        ]
        .iter()
        .map(|&(kind, x, y)| Obstacle {
            kind,
            pos: Vec2::new(Fixed::from_int(x), Fixed::from_int(y)),
        })
        .collect();

        let m = prop_markers(&obstacles);
        assert_eq!(m.len(), obstacles.len(), "one marker per obstacle");
        for (o, q) in obstacles.iter().zip(&m) {
            // Position mirrors the obstacle's world centre through the render float boundary.
            assert!((q.cx - crate::fixed_to_f32(o.pos.x)).abs() < EPS);
            assert!((q.cy - crate::fixed_to_f32(o.pos.y)).abs() < EPS);
            // Kind tint + a diamond (rotated) marker sized to the sim footprint.
            assert_eq!([q.r, q.g, q.b], prop_marker_color(o.kind));
            assert_eq!(q.a, PROP_MARKER_ALPHA);
            assert_eq!(q.rot, FRAC_PI_4, "prop markers are diamonds");
            let fr = crate::fixed_to_f32(o.kind.footprint_radius());
            assert!((q.hw - fr).abs() < EPS && (q.hh - fr).abs() < EPS);
        }
    }

    #[test]
    fn prop_markers_barricade_is_bigger_than_a_tree() {
        // The marker size tracks the sim collision footprint — a wide berm reads bigger than a tree.
        let tree = [Obstacle {
            kind: ObstacleKind::Tree,
            pos: Vec2::new(Fixed::ZERO, Fixed::ZERO),
        }];
        let berm = [Obstacle {
            kind: ObstacleKind::Barricade,
            pos: Vec2::new(Fixed::ZERO, Fixed::ZERO),
        }];
        assert!(prop_markers(&berm)[0].hw > prop_markers(&tree)[0].hw);
    }

    #[test]
    fn prop_marker_colours_are_distinct_per_kind() {
        // Each prop kind gets a distinguishable top-down tint (so composition reads at a glance).
        let kinds = [
            ObstacleKind::Tree,
            ObstacleKind::Rock,
            ObstacleKind::Crate,
            ObstacleKind::Barricade,
            ObstacleKind::TurretUs,
            ObstacleKind::TurretFr,
        ];
        for i in 0..kinds.len() {
            for j in (i + 1)..kinds.len() {
                let a = prop_marker_color(kinds[i]);
                let b = prop_marker_color(kinds[j]);
                let d2: f32 = (0..3).map(|k| (a[k] - b[k]).powi(2)).sum();
                assert!(d2 > 0.01, "{:?} vs {:?} too close", kinds[i], kinds[j]);
            }
        }
        // All colour channels stay in range (an out-of-range channel clips on the way to the swapchain).
        for k in kinds {
            for ch in prop_marker_color(k) {
                assert!((0.0..=1.0).contains(&ch));
            }
        }
    }

    #[test]
    fn prop_markers_empty_when_no_obstacles() {
        assert!(
            prop_markers(&[]).is_empty(),
            "a prop-less map draws no markers"
        );
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
