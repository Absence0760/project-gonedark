//! Animated 3D parallax **title backdrop** — the live, mood-setting background behind the desktop
//! title/landing screen (the "almost interactive 3D image" that subtly follows the cursor). It is a
//! fully **self-contained** renderer: it owns its pipelines, builds all of its geometry from cheap
//! procedural shapes (no external assets), and draws into a caller-supplied `TextureView`, clearing
//! it itself.
//!
//! ## The scene (a dark "Going Dark" diorama)
//! Drawn back-to-front in [`TitleBackdrop::render`] (see `title_backdrop.wgsl` for the shading):
//!  1. a fullscreen **sky** gradient (deep blue-black ink overhead → warmer dark horizon) — clears;
//!  2. a large **ground plane** with a grid receding to a vanishing point, dimming into horizon fog;
//!  3. a procedural **silhouette skyline** of ~13 extruded boxes (a distant camp/city), near-black
//!     against the sky, a few catching a faint amber fresnel rim — generated from a fixed seed;
//!  4. a sparse field of **drifting embers** (warm motes that rise + twinkle), additively blended;
//!  5. a **vignette** darkening the corners so the centred title text reads.
//!
//! The camera does a slow continuous drift (a function of `time`) plus a clamped cursor **parallax**
//! (it nudges opposite the pointer for depth). Subtle and slow — this is a background, not a
//! screensaver.
//!
//! ## Float boundary (invariant #1/#4)
//! `render` is the float side: floats are forbidden only in `core`/the sim, never here. The
//! camera/parallax/animation math is factored into the pure, GPU-free, unit-tested free functions
//! [`parallax_offset`] and [`backdrop_view_proj`] (plus the small scalar matrix helpers below) — no
//! `glam` dependency (D19): like `mesh::model_matrix`, the matrices are hand-rolled column-major
//! `[[f32; 4]; 4]` (the `glam Mat4::to_cols_array_2d` layout) so the crate stays `wgpu` + `bytemuck`.
//! Palette colours come from [`crate::theme`]; WGSL bakes the few it needs with a name pointing back.

use wgpu::util::DeviceExt;

use crate::mesh::{create_depth_view, DEPTH_FORMAT};
use crate::theme;

// ---- camera / animation tuning ----------------------------------------------------------------

/// Vertical field of view, radians (~52°).
const FOVY: f32 = 0.91;
/// Near/far clip planes (world metres). Far is generous so the distant skyline + ground never clip.
const NEAR: f32 = 0.5;
const FAR: f32 = 600.0;

/// Resting camera height and distance back from the scene (looking down −Z toward the skyline).
const EYE_Y: f32 = 4.0;
const EYE_Z: f32 = 14.0;
/// The fixed look target — far down the field, so parallax pivots around the distant horizon (near
/// geometry shifts more than far, the depth cue).
const TARGET: [f32; 3] = [0.0, 3.0, -45.0];

/// Slow automatic drift: a gentle sway in X and a slower bob in Y, both small.
const DRIFT_RATE_X: f32 = 0.06;
const DRIFT_AMP_X: f32 = 2.2;
const DRIFT_RATE_Y: f32 = 0.045;
const DRIFT_AMP_Y: f32 = 0.7;

/// Maximum camera shift (world metres per axis) the cursor parallax can induce — the clamp bound a
/// wild cursor can never exceed. Kept small so the background only *nudges*.
const PARALLAX_STRENGTH: f32 = 1.4;

// ---- pure math seam (unit-tested, no GPU) -----------------------------------------------------

/// Map a cursor position in normalized device coords (`[-1, 1]²`, x right / y up) to a clamped
/// camera offset in world metres. The cursor is first clamped into `[-1, 1]` per axis, so the
/// returned offset magnitude **never exceeds `strength`** on either axis no matter how wild the
/// input (a `[100, -100]` cursor still yields `[strength, -strength]`). A centred cursor `[0, 0]`
/// yields `[0, 0]`. Pure + GPU-free.
pub fn parallax_offset(cursor: [f32; 2], strength: f32) -> [f32; 2] {
    let cx = cursor[0].clamp(-1.0, 1.0);
    let cy = cursor[1].clamp(-1.0, 1.0);
    [cx * strength, cy * strength]
}

/// The camera eye world position for `(time, cursor)`: a slow automatic drift (function of `time`)
/// plus the clamped cursor [`parallax_offset`], applied **opposite** the cursor so the foreground
/// appears to swing the other way (depth). Pure + GPU-free; shared by [`backdrop_view_proj`] and the
/// renderer (which also needs the eye for the box rim fresnel).
fn camera_eye(time: f32, cursor: [f32; 2]) -> [f32; 3] {
    let par = parallax_offset(cursor, PARALLAX_STRENGTH);
    let dx = (time * DRIFT_RATE_X).sin() * DRIFT_AMP_X;
    let dy = (time * DRIFT_RATE_Y).cos() * DRIFT_AMP_Y;
    [dx - par[0], EYE_Y + dy + par[1] * 0.6, EYE_Z]
}

/// The full **view-projection** matrix for the backdrop camera at `(time, cursor, aspect)`, as a
/// column-major `[[f32; 4]; 4]` (the `glam Mat4::to_cols_array_2d()` layout the GPU expects). It
/// combines the slow automatic drift + cursor parallax of [`camera_eye`] with a right-handed
/// perspective projection (wgpu's `z ∈ [0, 1]` clip convention). A non-finite or near-zero `aspect`
/// falls back to `1.0` so the matrix is always finite. Pure + GPU-free (no `glam`, D19) and
/// unit-tested.
pub fn backdrop_view_proj(time: f32, cursor: [f32; 2], aspect: f32) -> [[f32; 4]; 4] {
    let (vp, _eye) = camera_matrix(time, cursor, aspect);
    vp
}

/// Both halves of the camera for a frame: the view-projection and the eye position (the renderer
/// needs the eye for the box rim fresnel; the matrix for the uniform). Internal so the two never
/// drift apart.
fn camera_matrix(time: f32, cursor: [f32; 2], aspect: f32) -> ([[f32; 4]; 4], [f32; 3]) {
    let eye = camera_eye(time, cursor);
    let aspect = if aspect.is_finite() && aspect > 1e-3 { aspect } else { 1.0 };
    let proj = perspective_rh_zo(FOVY, aspect, NEAR, FAR);
    let view = look_at_rh(eye, TARGET, [0.0, 1.0, 0.0]);
    (mat4_mul(proj, view), eye)
}

/// Right-handed perspective projection with a `z ∈ [0, 1]` clip range (wgpu/D3D/Metal/Vulkan),
/// column-major. Mirrors `glam::Mat4::perspective_rh`. Hand-rolled scalar `f32` (no `glam`, D19).
fn perspective_rh_zo(fovy: f32, aspect: f32, near: f32, far: f32) -> [[f32; 4]; 4] {
    let f = 1.0 / (fovy * 0.5).tan();
    let nf = 1.0 / (near - far);
    [
        [f / aspect, 0.0, 0.0, 0.0],
        [0.0, f, 0.0, 0.0],
        [0.0, 0.0, far * nf, -1.0],
        [0.0, 0.0, far * near * nf, 0.0],
    ]
}

/// Right-handed look-at view matrix (camera at `eye` looking toward `target`, `up` roughly up),
/// column-major. Mirrors `glam::Mat4::look_at_rh`. Hand-rolled scalar `f32` (no `glam`, D19).
fn look_at_rh(eye: [f32; 3], target: [f32; 3], up: [f32; 3]) -> [[f32; 4]; 4] {
    let f = normalize(sub(target, eye)); // forward (toward the target)
    let s = normalize(cross(f, up)); // right
    let uu = cross(s, f); // true up
    [
        [s[0], uu[0], -f[0], 0.0],
        [s[1], uu[1], -f[1], 0.0],
        [s[2], uu[2], -f[2], 0.0],
        [-dot(s, eye), -dot(uu, eye), dot(f, eye), 1.0],
    ]
}

/// Column-major 4×4 multiply: returns `a * b`.
fn mat4_mul(a: [[f32; 4]; 4], b: [[f32; 4]; 4]) -> [[f32; 4]; 4] {
    let mut m = [[0.0f32; 4]; 4];
    for c in 0..4 {
        for r in 0..4 {
            let mut s = 0.0;
            for k in 0..4 {
                s += a[k][r] * b[c][k];
            }
            m[c][r] = s;
        }
    }
    m
}

#[inline]
fn sub(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}
#[inline]
fn dot(a: [f32; 3], b: [f32; 3]) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}
#[inline]
fn cross(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}
#[inline]
fn normalize(v: [f32; 3]) -> [f32; 3] {
    let len = dot(v, v).sqrt();
    if len > 1e-8 {
        [v[0] / len, v[1] / len, v[2] / len]
    } else {
        [0.0, 0.0, 0.0]
    }
}

/// A column-major box placement: per-axis scale (half-extents, the unit cube spans `[-1, 1]³`) +
/// a yaw about the up (Y) axis + a translation. Used for the skyline silhouette boxes.
fn box_model(translation: [f32; 3], half: [f32; 3], yaw: f32) -> [[f32; 4]; 4] {
    let (sy, cy) = yaw.sin_cos();
    [
        [half[0] * cy, 0.0, -half[0] * sy, 0.0],
        [0.0, half[1], 0.0, 0.0],
        [half[2] * sy, 0.0, half[2] * cy, 0.0],
        [translation[0], translation[1], translation[2], 1.0],
    ]
}

// ---- GPU data ---------------------------------------------------------------------------------

/// Per-frame uniform: the view-projection, the eye (+ time packed in `.w`), and the aspect.
/// `repr(C)` + `Pod`; field order/offsets MUST match `title_backdrop.wgsl`'s `Uniform`.
#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct BackdropUniform {
    view_proj: [[f32; 4]; 4],
    /// xyz = camera world position; w = time (seconds).
    eye: [f32; 4],
    /// x = aspect (w/h); yzw reserved padding (kept 0).
    misc: [f32; 4],
}

/// A ground-plane vertex (world position only); the grid is computed in the fragment shader.
#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct GridVertex {
    pos: [f32; 3],
}

/// A unit-cube vertex (position + face normal) for the silhouette boxes. Cube spans `[-1, 1]³`.
#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct CubeVertex {
    pos: [f32; 3],
    normal: [f32; 3],
}

/// One silhouette box instance: a column-major model matrix + a tint (`rgb` = near-black base,
/// `a` = amber rim amount). Layout matches `vs_box`'s instance attributes (loc 2..=6).
#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct BoxInstance {
    model: [[f32; 4]; 4],
    tint: [f32; 4],
}

/// A billboard corner (`[-1, 1]²`) for the ember quad.
#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct EmberCorner {
    corner: [f32; 2],
}

/// One ember instance: world anchor + phase, then size/speed/rise-range/twinkle params. Layout
/// matches `vs_ember`'s instance attributes (loc 1, 2).
#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct EmberInstance {
    /// xyz = world anchor; w = phase `[0, 1)`.
    anchor_phase: [f32; 4],
    /// x = size (NDC half-width); y = drift speed; z = rise range (metres); w = twinkle frequency.
    params: [f32; 4],
}

/// The ember billboard quad: two triangles in `[-1, 1]²`.
const EMBER_QUAD: [EmberCorner; 6] = [
    EmberCorner { corner: [-1.0, -1.0] },
    EmberCorner { corner: [1.0, -1.0] },
    EmberCorner { corner: [1.0, 1.0] },
    EmberCorner { corner: [-1.0, -1.0] },
    EmberCorner { corner: [1.0, 1.0] },
    EmberCorner { corner: [-1.0, 1.0] },
];

/// A tiny deterministic LCG yielding `f32` in `[0, 1)` — so the skyline + ember layouts are a fixed,
/// reproducible function of a seed (render-only `f32`; no sim/checksum surface).
struct Lcg(u32);
impl Lcg {
    fn next(&mut self) -> f32 {
        self.0 = self.0.wrapping_mul(1664525).wrapping_add(1013904223);
        ((self.0 >> 8) as f32) / ((1u32 << 24) as f32)
    }
}

/// Build the 36 vertices (12 triangles) of a unit cube spanning `[-1, 1]³`, each carrying its face
/// normal — the silhouette box mesh, instanced per skyline building.
fn unit_cube() -> Vec<CubeVertex> {
    // (normal, 4 CCW corners) per face.
    let faces: [([f32; 3], [[i32; 3]; 4]); 6] = [
        ([0.0, 0.0, 1.0], [[-1, -1, 1], [1, -1, 1], [1, 1, 1], [-1, 1, 1]]),
        ([0.0, 0.0, -1.0], [[1, -1, -1], [-1, -1, -1], [-1, 1, -1], [1, 1, -1]]),
        ([1.0, 0.0, 0.0], [[1, -1, 1], [1, -1, -1], [1, 1, -1], [1, 1, 1]]),
        ([-1.0, 0.0, 0.0], [[-1, -1, -1], [-1, -1, 1], [-1, 1, 1], [-1, 1, -1]]),
        ([0.0, 1.0, 0.0], [[-1, 1, 1], [1, 1, 1], [1, 1, -1], [-1, 1, -1]]),
        ([0.0, -1.0, 0.0], [[-1, -1, -1], [1, -1, -1], [1, -1, 1], [-1, -1, 1]]),
    ];
    let mut v = Vec::with_capacity(36);
    for (n, quad) in faces {
        for &i in &[0usize, 1, 2, 0, 2, 3] {
            let c = quad[i];
            v.push(CubeVertex {
                pos: [c[0] as f32, c[1] as f32, c[2] as f32],
                normal: n,
            });
        }
    }
    v
}

/// The procedural distant skyline: ~13 extruded boxes from a fixed seed, near-black against the sky,
/// a few flagged for an amber rim. Deterministic (ordinary `f32`, no fixed-point needed — render-only).
fn skyline_boxes() -> Vec<BoxInstance> {
    let mut rng = Lcg(0x1234_5678);
    let count = 13;
    let mut out = Vec::with_capacity(count);
    // Base silhouette colour: theme::INK, deepened so the towers read near-black against the sky.
    let ink = theme::INK;
    let base = [ink[0] * 0.5, ink[1] * 0.5, ink[2] * 0.6];
    for i in 0..count {
        let (r1, r2, r3, r4, r5, r6) = (
            rng.next(),
            rng.next(),
            rng.next(),
            rng.next(),
            rng.next(),
            rng.next(),
        );
        let x = -56.0 + i as f32 * 9.2 + (r1 - 0.5) * 5.5;
        let z = -34.0 - r2 * 42.0;
        let hw = 2.2 + r3 * 2.6;
        let hd = 2.2 + r4 * 2.6;
        let hh = 2.5 + r5 * 9.0;
        let yaw = (r6 - 0.5) * 0.5;
        // A couple of towers (every 4th, offset) catch the warm rim — "embers in the dark".
        let rim = if i % 4 == 1 { 0.9 } else { 0.12 };
        out.push(BoxInstance {
            model: box_model([x, hh, z], [hw, hh, hd], yaw),
            tint: [base[0], base[1], base[2], rim],
        });
    }
    out
}

/// A sparse field of drifting embers from a fixed seed. Each rises slowly and twinkles (the shader
/// animates them from `time` + the per-ember phase/params). Deterministic render-only `f32`.
fn embers() -> Vec<EmberInstance> {
    let mut rng = Lcg(0x9E37_79B9);
    let count = 80;
    let mut out = Vec::with_capacity(count);
    for _ in 0..count {
        let x = (rng.next() - 0.5) * 72.0;
        let y = 1.0 + rng.next() * 4.0;
        let z = -8.0 - rng.next() * 56.0;
        let phase = rng.next();
        let size = 0.012 + rng.next() * 0.020;
        let speed = 0.02 + rng.next() * 0.05;
        let range = 8.0 + rng.next() * 16.0;
        let twinkle = 1.5 + rng.next() * 4.0;
        out.push(EmberInstance {
            anchor_phase: [x, y, z, phase],
            params: [size, speed, range, twinkle],
        });
    }
    out
}

/// The large ground plane (two triangles on `y = 0`), big enough to fill the view to the horizon.
fn ground_plane() -> [GridVertex; 6] {
    let x = 320.0;
    let zf = -440.0; // far edge (toward the horizon)
    let zn = 40.0; // near edge (behind the camera-ish)
    [
        GridVertex { pos: [-x, 0.0, zn] },
        GridVertex { pos: [x, 0.0, zn] },
        GridVertex { pos: [x, 0.0, zf] },
        GridVertex { pos: [-x, 0.0, zn] },
        GridVertex { pos: [x, 0.0, zf] },
        GridVertex { pos: [-x, 0.0, zf] },
    ]
}

// ---- renderer ---------------------------------------------------------------------------------

/// The self-contained animated parallax title backdrop. Owns its pipelines, procedural geometry,
/// per-frame uniform, and a lazily-(re)allocated depth buffer; draws into a caller-supplied view,
/// clearing it. Build once with [`TitleBackdrop::new`]; call [`TitleBackdrop::render`] per frame.
pub struct TitleBackdrop {
    uniform_buf: wgpu::Buffer,
    uniform_bind_group: wgpu::BindGroup,
    sky_pipeline: wgpu::RenderPipeline,
    grid_pipeline: wgpu::RenderPipeline,
    box_pipeline: wgpu::RenderPipeline,
    ember_pipeline: wgpu::RenderPipeline,
    vignette_pipeline: wgpu::RenderPipeline,
    grid_vbuf: wgpu::Buffer,
    grid_vcount: u32,
    cube_vbuf: wgpu::Buffer,
    cube_vcount: u32,
    box_inst_buf: wgpu::Buffer,
    box_count: u32,
    ember_quad_buf: wgpu::Buffer,
    ember_inst_buf: wgpu::Buffer,
    ember_count: u32,
    depth_view: wgpu::TextureView,
    depth_size: (u32, u32),
}

impl TitleBackdrop {
    /// Build the backdrop against the surface `device`/`format` (an sRGB surface format). The
    /// `device` is borrowed (D19); all geometry is pre-built here so the per-frame path only writes
    /// the uniform + (re)allocates the depth buffer on resize.
    pub fn new(device: &wgpu::Device, format: wgpu::TextureFormat) -> Self {
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("gonedark.title_backdrop_shader"),
            source: wgpu::ShaderSource::Wgsl(include_str!("title_backdrop.wgsl").into()),
        });

        let uniform_buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("gonedark.title_backdrop_uniform"),
            size: std::mem::size_of::<BackdropUniform>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let uniform_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("gonedark.title_backdrop_uniform_layout"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX | wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            }],
        });
        let uniform_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("gonedark.title_backdrop_uniform_bind_group"),
            layout: &uniform_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: uniform_buf.as_entire_binding(),
            }],
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("gonedark.title_backdrop_pipeline_layout"),
            bind_group_layouts: &[Some(&uniform_layout)],
            immediate_size: 0,
        });

        let color_target = |blend: Option<wgpu::BlendState>| {
            Some(wgpu::ColorTargetState {
                format,
                blend,
                write_mask: wgpu::ColorWrites::ALL,
            })
        };
        let depth_stencil = || {
            Some(wgpu::DepthStencilState {
                format: DEPTH_FORMAT,
                depth_write_enabled: Some(true),
                depth_compare: Some(wgpu::CompareFunction::Less),
                stencil: wgpu::StencilState::default(),
                bias: wgpu::DepthBiasState::default(),
            })
        };

        // Sky: fullscreen triangle, opaque, no depth (the clearing layer).
        let sky_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("gonedark.title_backdrop_sky"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_fs"),
                buffers: &[],
                compilation_options: wgpu::PipelineCompilationOptions::default(),
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs_sky"),
                targets: &[color_target(Some(wgpu::BlendState::REPLACE))],
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

        // Ground grid: a world-space plane, depth-tested.
        let grid_layout = wgpu::VertexBufferLayout {
            array_stride: std::mem::size_of::<GridVertex>() as u64,
            step_mode: wgpu::VertexStepMode::Vertex,
            attributes: &wgpu::vertex_attr_array![0 => Float32x3],
        };
        let grid_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("gonedark.title_backdrop_grid"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_grid"),
                buffers: &[grid_layout],
                compilation_options: wgpu::PipelineCompilationOptions::default(),
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs_grid"),
                targets: &[color_target(Some(wgpu::BlendState::REPLACE))],
                compilation_options: wgpu::PipelineCompilationOptions::default(),
            }),
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::TriangleList,
                ..Default::default()
            },
            depth_stencil: depth_stencil(),
            multisample: wgpu::MultisampleState::default(),
            multiview_mask: None,
            cache: None,
        });

        // Silhouette boxes: instanced cubes, depth-tested. No back-face cull (winding-agnostic).
        let cube_layout = wgpu::VertexBufferLayout {
            array_stride: std::mem::size_of::<CubeVertex>() as u64,
            step_mode: wgpu::VertexStepMode::Vertex,
            attributes: &wgpu::vertex_attr_array![0 => Float32x3, 1 => Float32x3],
        };
        let box_inst_layout = wgpu::VertexBufferLayout {
            array_stride: std::mem::size_of::<BoxInstance>() as u64,
            step_mode: wgpu::VertexStepMode::Instance,
            attributes: &wgpu::vertex_attr_array![
                2 => Float32x4, 3 => Float32x4, 4 => Float32x4, 5 => Float32x4, 6 => Float32x4
            ],
        };
        let box_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("gonedark.title_backdrop_box"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_box"),
                buffers: &[cube_layout, box_inst_layout],
                compilation_options: wgpu::PipelineCompilationOptions::default(),
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs_box"),
                targets: &[color_target(Some(wgpu::BlendState::REPLACE))],
                compilation_options: wgpu::PipelineCompilationOptions::default(),
            }),
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::TriangleList,
                ..Default::default()
            },
            depth_stencil: depth_stencil(),
            multisample: wgpu::MultisampleState::default(),
            multiview_mask: None,
            cache: None,
        });

        // Embers: instanced billboards, additive (premultiplied One/One), no depth.
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
        let ember_corner_layout = wgpu::VertexBufferLayout {
            array_stride: std::mem::size_of::<EmberCorner>() as u64,
            step_mode: wgpu::VertexStepMode::Vertex,
            attributes: &wgpu::vertex_attr_array![0 => Float32x2],
        };
        let ember_inst_layout = wgpu::VertexBufferLayout {
            array_stride: std::mem::size_of::<EmberInstance>() as u64,
            step_mode: wgpu::VertexStepMode::Instance,
            attributes: &wgpu::vertex_attr_array![1 => Float32x4, 2 => Float32x4],
        };
        let ember_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("gonedark.title_backdrop_ember"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_ember"),
                buffers: &[ember_corner_layout, ember_inst_layout],
                compilation_options: wgpu::PipelineCompilationOptions::default(),
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs_ember"),
                targets: &[color_target(Some(additive))],
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

        // Vignette: fullscreen triangle, alpha-blended corner darkening, no depth.
        let vignette_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("gonedark.title_backdrop_vignette"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_fs"),
                buffers: &[],
                compilation_options: wgpu::PipelineCompilationOptions::default(),
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs_vignette"),
                targets: &[color_target(Some(wgpu::BlendState::ALPHA_BLENDING))],
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

        // Pre-built geometry.
        let grid_verts = ground_plane();
        let grid_vbuf = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("gonedark.title_backdrop_grid_vbo"),
            contents: bytemuck::cast_slice(&grid_verts),
            usage: wgpu::BufferUsages::VERTEX,
        });
        let cube_verts = unit_cube();
        let cube_vbuf = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("gonedark.title_backdrop_cube_vbo"),
            contents: bytemuck::cast_slice(&cube_verts),
            usage: wgpu::BufferUsages::VERTEX,
        });
        let box_insts = skyline_boxes();
        let box_inst_buf = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("gonedark.title_backdrop_box_inst"),
            contents: bytemuck::cast_slice(&box_insts),
            usage: wgpu::BufferUsages::VERTEX,
        });
        let ember_quad_buf = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("gonedark.title_backdrop_ember_quad"),
            contents: bytemuck::cast_slice(&EMBER_QUAD),
            usage: wgpu::BufferUsages::VERTEX,
        });
        let ember_insts = embers();
        let ember_inst_buf = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("gonedark.title_backdrop_ember_inst"),
            contents: bytemuck::cast_slice(&ember_insts),
            usage: wgpu::BufferUsages::VERTEX,
        });

        let depth_view = create_depth_view(device, 1, 1);

        TitleBackdrop {
            uniform_buf,
            uniform_bind_group,
            sky_pipeline,
            grid_pipeline,
            box_pipeline,
            ember_pipeline,
            vignette_pipeline,
            grid_vbuf,
            grid_vcount: grid_verts.len() as u32,
            cube_vbuf,
            cube_vcount: cube_verts.len() as u32,
            box_inst_buf,
            box_count: box_insts.len() as u32,
            ember_quad_buf,
            ember_inst_buf,
            ember_count: ember_insts.len() as u32,
            depth_view,
            depth_size: (1, 1),
        }
    }

    /// Ensure the depth buffer matches `(width, height)`, recreating it only on a size change.
    fn ensure_depth(&mut self, device: &wgpu::Device, width: u32, height: u32) {
        let size = (width.max(1), height.max(1));
        if self.depth_size != size {
            self.depth_view = create_depth_view(device, size.0, size.1);
            self.depth_size = size;
        }
    }

    /// Draw the animated parallax backdrop into `view`, CLEARING it (the sky pass clears to the sky
    /// colour). `viewport` is the target's `(width, height)` in physical pixels (for aspect). `time`
    /// is seconds since app start (the monotonic animation clock; may be large). `cursor` is the
    /// pointer in NDC `[-1, 1]²` (x right / y up), or `None` when the pointer is absent (treated as
    /// centred `[0, 0]`). Submits its own command encoder.
    pub fn render(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        view: &wgpu::TextureView,
        viewport: (u32, u32),
        time: f32,
        cursor: Option<[f32; 2]>,
    ) {
        let (w, h) = (viewport.0.max(1), viewport.1.max(1));
        let aspect = w as f32 / h as f32;
        let cur = cursor.unwrap_or([0.0, 0.0]);

        let (view_proj, eye) = camera_matrix(time, cur, aspect);
        let uniform = BackdropUniform {
            view_proj,
            eye: [eye[0], eye[1], eye[2], time],
            misc: [aspect, 0.0, 0.0, 0.0],
        };
        queue.write_buffer(&self.uniform_buf, 0, bytemuck::bytes_of(&uniform));
        self.ensure_depth(device, w, h);

        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("gonedark.title_backdrop_encoder"),
        });

        // Pass 1 — sky: CLEAR the frame to the sky colour, then paint the gradient over it.
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("gonedark.title_backdrop_sky_pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color {
                            r: 0.018,
                            g: 0.026,
                            b: 0.045,
                            a: 1.0,
                        }),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                multiview_mask: None,
                timestamp_writes: None,
                occlusion_query_set: None,
            });
            pass.set_pipeline(&self.sky_pipeline);
            pass.set_bind_group(0, &self.uniform_bind_group, &[]);
            pass.draw(0..3, 0..1);
        }

        // Pass 2 — ground grid + silhouette boxes, depth-tested (LOAD the sky).
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("gonedark.title_backdrop_scene_pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Load,
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view: &self.depth_view,
                    depth_ops: Some(wgpu::Operations {
                        load: wgpu::LoadOp::Clear(1.0),
                        store: wgpu::StoreOp::Discard,
                    }),
                    stencil_ops: None,
                }),
                multiview_mask: None,
                timestamp_writes: None,
                occlusion_query_set: None,
            });
            pass.set_bind_group(0, &self.uniform_bind_group, &[]);
            // Ground grid.
            pass.set_pipeline(&self.grid_pipeline);
            pass.set_vertex_buffer(0, self.grid_vbuf.slice(..));
            pass.draw(0..self.grid_vcount, 0..1);
            // Silhouette boxes.
            if self.box_count > 0 {
                pass.set_pipeline(&self.box_pipeline);
                pass.set_vertex_buffer(0, self.cube_vbuf.slice(..));
                pass.set_vertex_buffer(1, self.box_inst_buf.slice(..));
                pass.draw(0..self.cube_vcount, 0..self.box_count);
            }
        }

        // Pass 3 — drifting embers, additive, no depth (LOAD).
        if self.ember_count > 0 {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("gonedark.title_backdrop_ember_pass"),
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
            pass.set_pipeline(&self.ember_pipeline);
            pass.set_bind_group(0, &self.uniform_bind_group, &[]);
            pass.set_vertex_buffer(0, self.ember_quad_buf.slice(..));
            pass.set_vertex_buffer(1, self.ember_inst_buf.slice(..));
            pass.draw(0..EMBER_QUAD.len() as u32, 0..self.ember_count);
        }

        // Pass 4 — vignette, darken the corners (LOAD, alpha blend).
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("gonedark.title_backdrop_vignette_pass"),
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
            pass.set_pipeline(&self.vignette_pipeline);
            pass.set_bind_group(0, &self.uniform_bind_group, &[]);
            pass.draw(0..3, 0..1);
        }

        queue.submit(std::iter::once(encoder.finish()));
    }
}

#[cfg(test)]
mod tests {
    //! `render` is the float boundary (invariant #1), so `f32` math is fair game. The pipelines need
    //! a real `wgpu::Device` (no display in CI); the testable logic is the pure camera/parallax math.

    use super::*;

    fn finite_mat(m: &[[f32; 4]; 4]) -> bool {
        m.iter().flatten().all(|c| c.is_finite())
    }

    // ---- parallax ----

    #[test]
    fn parallax_centered_cursor_is_zero() {
        let o = parallax_offset([0.0, 0.0], PARALLAX_STRENGTH);
        assert!(o[0].abs() < 1e-6 && o[1].abs() < 1e-6, "centred → ~0, got {o:?}");
    }

    #[test]
    fn parallax_clamps_extreme_cursor_to_strength() {
        // A wild cursor far outside the NDC box can never swing the camera past ±strength.
        for &strength in &[0.5, 1.4, 3.0] {
            for &cur in &[[100.0, -100.0], [-50.0, 50.0], [1e9, 1e9], [-7.0, 0.3]] {
                let o = parallax_offset(cur, strength);
                assert!(
                    o[0].abs() <= strength + 1e-6 && o[1].abs() <= strength + 1e-6,
                    "offset {o:?} exceeded clamp ±{strength} for cursor {cur:?}"
                );
            }
        }
    }

    #[test]
    fn parallax_is_linear_inside_the_box_and_signed() {
        // Inside [-1,1] the offset tracks the cursor proportionally and keeps its sign (the camera
        // later applies it opposite, but the offset itself follows the cursor).
        let o = parallax_offset([0.5, -0.25], 1.0);
        assert!((o[0] - 0.5).abs() < 1e-6 && (o[1] + 0.25).abs() < 1e-6, "got {o:?}");
    }

    // ---- view-projection ----

    #[test]
    fn view_proj_is_finite_for_representative_inputs() {
        let cases = [
            (0.0f32, [0.0f32, 0.0], 1.0f32),
            (3.0, [0.6, -0.4], 16.0 / 9.0),
            (1234.5, [-1.0, 1.0], 21.0 / 9.0),
            (1.0e6, [100.0, -100.0], 0.5),
            (-50.0, [0.0, 0.0], 4.0 / 3.0),
        ];
        for (t, c, a) in cases {
            let m = backdrop_view_proj(t, c, a);
            assert!(finite_mat(&m), "non-finite matrix for (t={t}, c={c:?}, a={a})");
        }
    }

    #[test]
    fn view_proj_handles_degenerate_aspect() {
        // A zero / non-finite aspect must not produce NaNs/Inf (it falls back to 1.0).
        for a in [0.0f32, -1.0, f32::NAN, f32::INFINITY] {
            let m = backdrop_view_proj(2.0, [0.2, 0.3], a);
            assert!(finite_mat(&m), "non-finite matrix for degenerate aspect {a}");
        }
    }

    #[test]
    fn aspect_changes_the_projection() {
        // Aspect feeds the perspective X scale, so the matrix MUST differ between aspects (a guard
        // against the wide-window stretching bug the project memo warns about).
        let wide = backdrop_view_proj(0.0, [0.0, 0.0], 21.0 / 9.0);
        let square = backdrop_view_proj(0.0, [0.0, 0.0], 1.0);
        // The first row carries the aspect-dependent X scale; it must change.
        let changed = (0..4).any(|c| (wide[c][0] - square[c][0]).abs() > 1e-4);
        assert!(changed, "aspect did not change the projection (stretch bug)");
    }

    #[test]
    fn time_drives_camera_drift() {
        // Two distinct times (same cursor/aspect) must yield distinct matrices — the backdrop is
        // genuinely animated, not static.
        let a = backdrop_view_proj(0.0, [0.0, 0.0], 1.0);
        let b = backdrop_view_proj(5.0, [0.0, 0.0], 1.0);
        let changed = (0..4).any(|c| (0..4).any(|r| (a[c][r] - b[c][r]).abs() > 1e-4));
        assert!(changed, "the camera did not drift over time");
    }

    #[test]
    fn cursor_parallax_moves_the_camera() {
        // Same time/aspect, different cursor → different view (the parallax actually applies).
        let centered = backdrop_view_proj(0.0, [0.0, 0.0], 1.0);
        let offset = backdrop_view_proj(0.0, [0.8, -0.6], 1.0);
        let changed = (0..4).any(|c| (0..4).any(|r| (centered[c][r] - offset[c][r]).abs() > 1e-4));
        assert!(changed, "cursor parallax did not move the camera");
    }

    // ---- matrix helpers ----

    #[test]
    fn perspective_is_finite_and_scales_with_aspect() {
        let p = perspective_rh_zo(FOVY, 1.6, NEAR, FAR);
        assert!(finite_mat(&p));
        // X scale = f/aspect, Y scale = f → wider aspect shrinks the X term below Y.
        assert!(p[0][0] < p[1][1], "x scale must be f/aspect < f for aspect>1");
    }

    #[test]
    fn look_at_places_eye_on_negative_view_z() {
        // The look target must land in front of the camera: in view space (eye at origin looking
        // down −Z) the target's view-space Z is negative.
        let eye = camera_eye(0.0, [0.0, 0.0]);
        let view = look_at_rh(eye, TARGET, [0.0, 1.0, 0.0]);
        // Transform the target by the view matrix (column-major, w=1).
        let mut tv = [view[3][0], view[3][1], view[3][2]];
        for j in 0..3 {
            for r in 0..3 {
                tv[r] += view[j][r] * TARGET[j];
            }
        }
        assert!(tv[2] < 0.0, "target should be in front of the camera (−Z), got {tv:?}");
    }

    // ---- geometry sanity ----

    #[test]
    fn cube_has_36_finite_unit_verts() {
        let v = unit_cube();
        assert_eq!(v.len(), 36, "12 triangles");
        for cv in &v {
            assert!(cv.pos.iter().chain(&cv.normal).all(|c| c.is_finite()));
            assert!(cv.pos.iter().all(|c| c.abs() <= 1.0 + 1e-6), "cube spans [-1,1]");
            let n = (cv.normal[0].powi(2) + cv.normal[1].powi(2) + cv.normal[2].powi(2)).sqrt();
            assert!((n - 1.0).abs() < 1e-6, "unit normal");
        }
    }

    #[test]
    fn skyline_and_embers_are_deterministic_and_sane() {
        // Deterministic from the fixed seed (same layout every run) and all finite.
        let a = skyline_boxes();
        let b = skyline_boxes();
        assert_eq!(a.len(), b.len());
        assert!(!a.is_empty());
        for (x, y) in a.iter().zip(&b) {
            assert_eq!(bytemuck::bytes_of(x), bytemuck::bytes_of(y), "skyline must be deterministic");
            assert!(finite_mat(&x.model) && x.tint.iter().all(|c| c.is_finite()));
            // Box base sits on the ground (translation y == half-height).
            assert!(x.model[3][1] > 0.0, "tower stands above the ground plane");
        }
        let e1 = embers();
        let e2 = embers();
        assert_eq!(e1.len(), e2.len());
        assert!(!e1.is_empty());
        for (x, y) in e1.iter().zip(&e2) {
            assert_eq!(bytemuck::bytes_of(x), bytemuck::bytes_of(y), "embers must be deterministic");
            assert!(x.anchor_phase.iter().chain(&x.params).all(|c| c.is_finite()));
            assert!(x.params[0] > 0.0 && x.params[2] > 0.0, "positive size + rise range");
        }
    }

    /// Validate `title_backdrop.wgsl` offline with naga (the compiler wgpu uses), so a shader
    /// regression fails the suite instead of only blowing up at pipeline creation on a real GPU.
    #[test]
    fn title_backdrop_wgsl_parses_and_validates() {
        let src = include_str!("title_backdrop.wgsl");
        let module = naga::front::wgsl::parse_str(src).expect("title_backdrop.wgsl must parse");
        let mut validator = naga::valid::Validator::new(
            naga::valid::ValidationFlags::all(),
            naga::valid::Capabilities::all(),
        );
        validator
            .validate(&module)
            .expect("title_backdrop.wgsl must validate");
    }
}
