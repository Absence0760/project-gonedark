//! The **globe backdrop** (D103) — the campaign atlas earth behind the desktop Operations hub,
//! the Q28 presentation *increment* between "grouped list" and the still-open endstate (navigable
//! globe vs. 2.5D regional map). The sibling of [`crate::title_backdrop`] and built to its exact
//! idiom: a fully **self-contained** renderer (own pipelines, procedural geometry, draws into a
//! caller-supplied view, clears it itself) whose only data is one tiny embedded blob.
//!
//! ## The scene
//! A slowly turning stylized earth — land/sea from the equirectangular [`LANDMASK`] (Natural
//! Earth 1:110m, public domain; regenerable via `tools/earth/gen_landmask.py`, provenance in
//! `assets/earth/manifest.json`), a faint 30° graticule, an amber fresnel rim — with one glowing
//! **pin per conflict** ([`GlobePin`], authored `Conflict::lat_x10/lon_x10`). The globe settles
//! with the *focused* conflict facing the camera ([`globe_yaw`]) and sways gently; the cursor
//! parallax reuses [`title_backdrop::parallax_offset`] so the two backdrops feel like one family.
//!
//! ## Float boundary (invariant #1/#4)
//! `render` is the float side: the shells hand over integer tenth-degrees from `core::campaign`
//! and this module converts at its boundary. All camera/placement math is factored into pure,
//! GPU-free, unit-tested free functions ([`latlon_to_unit`], [`land_at`], [`globe_yaw`],
//! [`sphere_mesh`]) — hand-rolled matrices, no `glam` (D19), so the crate stays `wgpu` +
//! `bytemuck`. The WGSL mask lookup mirrors [`latlon_to_unit`]; the two must stay inverses.

use wgpu::util::DeviceExt;

use crate::mesh::{create_depth_view, DEPTH_FORMAT};
use crate::title_backdrop::{look_at_rh, mat4_mul, parallax_offset, perspective_rh_zo};

// ---- the embedded land mask (the contract with tools/earth/gen_landmask.py) --------------------

/// Mask width in texels (0.5°/texel longitude). MUST match the generator's `MASK_W`.
pub const MASK_W: usize = 720;
/// Mask height in texels. MUST match the generator's `MASK_H`.
pub const MASK_H: usize = 360;
/// The equirectangular R8 land mask (255 = land, 0 = sea; row 0 = lat +90°), embedded raw so the
/// crate needs no decode dependency — the `assets/fonts/hud_atlas.gray` contract.
pub static LANDMASK: &[u8] = include_bytes!("../../assets/earth/landmask.gray");

// ---- camera / placement tuning -----------------------------------------------------------------

/// Vertical FOV (radians) and clip planes for the globe camera.
const FOVY: f32 = 0.85;
const NEAR: f32 = 0.5;
const FAR: f32 = 40.0;
/// Camera eye/target: slightly above the equator line, looking gently down at the globe.
const EYE: [f32; 3] = [0.0, 0.55, 3.6];
const TARGET: [f32; 3] = [0.0, -0.25, 0.0];
/// Where the globe sits and how big it is — low in frame so its upper limb carries the hub card.
const GLOBE_CENTER: [f32; 3] = [0.0, -0.55, 0.0];
const GLOBE_RADIUS: f32 = 1.55;
/// The slow sway around the settled focus yaw (radians / rate), the diorama-drift analogue.
const SWAY_AMP: f32 = 0.10;
const SWAY_RATE: f32 = 0.05;
/// Cursor parallax strength (world metres) — smaller than the diorama's; the globe is close.
const PARALLAX_STRENGTH: f32 = 0.55;
/// UV-sphere tessellation — smooth enough for the rim at backdrop size, trivial to draw.
const SPHERE_RINGS: u32 = 48;
const SPHERE_SEGS: u32 = 96;

// ---- pure math seam (unit-tested, no GPU) ------------------------------------------------------

/// A conflict pin for the globe: authored anchor in **degrees** (the shell converts the campaign's
/// integer tenth-degrees at this boundary) plus whether it is the focused conflict (brighter,
/// pulsing, and the yaw target).
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct GlobePin {
    pub lat_deg: f32,
    pub lon_deg: f32,
    pub focused: bool,
}

/// Map latitude/longitude (degrees) to the earth-fixed **unit sphere** position the mesh, pins,
/// and WGSL mask lookup all share: `+Y` = north pole, longitude `0` faces `+Z`, east positive
/// toward `+X`. The WGSL inverse is `lat = asin(y), lon = atan2(x, z)` — keep them in lock-step.
/// Pure + GPU-free.
pub fn latlon_to_unit(lat_deg: f32, lon_deg: f32) -> [f32; 3] {
    let (lat, lon) = (lat_deg.to_radians(), lon_deg.to_radians());
    [lat.cos() * lon.sin(), lat.sin(), lat.cos() * lon.cos()]
}

/// Sample the embedded [`LANDMASK`] at a latitude/longitude (degrees): `true` = land. The CPU twin
/// of the shader's texture lookup (nearest-texel), used by the tests to pin the mask's orientation
/// — a flipped or transposed regeneration fails loudly here, not as a silently mirrored earth.
/// Out-of-range latitudes clamp to the poles; longitude wraps. Pure + GPU-free.
pub fn land_at(lat_deg: f32, lon_deg: f32) -> bool {
    let u = (lon_deg + 180.0).rem_euclid(360.0) / 360.0;
    let v = ((90.0 - lat_deg) / 180.0).clamp(0.0, 1.0);
    let x = ((u * MASK_W as f32) as usize).min(MASK_W - 1);
    let y = ((v * MASK_H as f32) as usize).min(MASK_H - 1);
    LANDMASK[y * MASK_W + x] > 127
}

/// The globe's yaw (radians) at `time`, settled so the **focused** conflict's longitude faces the
/// camera (`+Z`, where the eye sits) with a slow sway on top. Under [`latlon_to_unit`] a point at
/// `lon` needs a rotation of `-lon` about `+Y` to land on `+Z`, so the settled yaw is
/// `-focus_lon`. Pure + GPU-free — the "which way is the earth turned" decision, unit-tested.
pub fn globe_yaw(focus_lon_deg: f32, time: f32) -> f32 {
    -focus_lon_deg.to_radians() + (time * SWAY_RATE).sin() * SWAY_AMP
}

/// Rotate a point about `+Y` by `yaw` radians. Pure — the CPU twin of the model matrix's rotation,
/// used by the yaw test to prove the focused pin lands facing the camera.
pub fn rotate_y(p: [f32; 3], yaw: f32) -> [f32; 3] {
    let (s, c) = yaw.sin_cos();
    [p[0] * c + p[2] * s, p[1], -p[0] * s + p[2] * c]
}

/// The globe's model matrix for a yaw: rotate about `+Y`, scale to [`GLOBE_RADIUS`], translate to
/// [`GLOBE_CENTER`] — column-major, matching the hand-rolled `title_backdrop` matrix layout. Pure.
fn globe_model(yaw: f32) -> [[f32; 4]; 4] {
    let (s, c) = yaw.sin_cos();
    let r = GLOBE_RADIUS;
    // Column-major: columns are the rotated+scaled basis vectors, then the translation.
    [
        [c * r, 0.0, -s * r, 0.0],
        [0.0, r, 0.0, 0.0],
        [s * r, 0.0, c * r, 0.0],
        [GLOBE_CENTER[0], GLOBE_CENTER[1], GLOBE_CENTER[2], 1.0],
    ]
}

/// Generate a UV-sphere as `(positions, indices)` — positions are unit vectors (the mesh IS its
/// normals), `rings × segs` quads split into triangles, poles welded per ring column. Deterministic
/// and pure; unit-tested for count/normalization/index-bounds so a tessellation edit can't ship a
/// degenerate globe.
pub fn sphere_mesh(rings: u32, segs: u32) -> (Vec<[f32; 3]>, Vec<u32>) {
    let mut pos = Vec::with_capacity(((rings + 1) * (segs + 1)) as usize);
    for ring in 0..=rings {
        let lat_deg = 90.0 - 180.0 * ring as f32 / rings as f32;
        for seg in 0..=segs {
            let lon_deg = -180.0 + 360.0 * seg as f32 / segs as f32;
            pos.push(latlon_to_unit(lat_deg, lon_deg));
        }
    }
    let stride = segs + 1;
    let mut idx = Vec::with_capacity((rings * segs * 6) as usize);
    for ring in 0..rings {
        for seg in 0..segs {
            let a = ring * stride + seg;
            let b = a + 1;
            let c = a + stride;
            let d = c + 1;
            idx.extend_from_slice(&[a, c, b, b, c, d]);
        }
    }
    (pos, idx)
}

// ---- GPU types ----------------------------------------------------------------------------------

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct GlobeUniform {
    view_proj: [[f32; 4]; 4],
    model: [[f32; 4]; 4],
    /// xyz = eye world position; w = time (seconds).
    eye: [f32; 4],
    /// x = aspect; yzw reserved (0).
    misc: [f32; 4],
}

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct SphereVertex {
    pos: [f32; 3],
}

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct PinInstance {
    unit: [f32; 3],
    focused: f32,
}

/// The self-contained globe renderer — same lifecycle as [`crate::title_backdrop::TitleBackdrop`]:
/// build once, call [`render`](Self::render) per frame with the pin list; it clears the view.
pub struct GlobeBackdrop {
    uniform_buf: wgpu::Buffer,
    bind_group: wgpu::BindGroup,
    sky_pipeline: wgpu::RenderPipeline,
    globe_pipeline: wgpu::RenderPipeline,
    pin_pipeline: wgpu::RenderPipeline,
    vignette_pipeline: wgpu::RenderPipeline,
    sphere_vbuf: wgpu::Buffer,
    sphere_ibuf: wgpu::Buffer,
    sphere_index_count: u32,
    /// Re-uploaded per frame (the pin set is tiny and may change focus between frames).
    pin_buf: wgpu::Buffer,
    pin_capacity: u32,
    /// The land-mask texture + a first-frame upload latch: `new` has no queue (matching
    /// `TitleBackdrop::new`'s signature), so the embedded bytes upload on the first `render`.
    mask_tex: wgpu::Texture,
    mask_uploaded: bool,
    depth_view: wgpu::TextureView,
    depth_size: (u32, u32),
}

/// The most pins the per-frame instance buffer holds — far above any plausible conflict count for
/// years; excess pins are dropped (never a reallocation-per-frame path).
const PIN_CAPACITY: u32 = 64;

impl GlobeBackdrop {
    pub fn new(device: &wgpu::Device, format: wgpu::TextureFormat) -> Self {
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("gonedark.globe_backdrop_shader"),
            source: wgpu::ShaderSource::Wgsl(include_str!("globe_backdrop.wgsl").into()),
        });

        let uniform_buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("gonedark.globe_backdrop_uniform"),
            size: std::mem::size_of::<GlobeUniform>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        // The land-mask texture: R8, linear-filtered, wrapping in longitude (the ±180° seam) and
        // clamping at the poles — uploaded once from the embedded bytes.
        let mask_tex = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("gonedark.globe_landmask"),
            size: wgpu::Extent3d {
                width: MASK_W as u32,
                height: MASK_H as u32,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::R8Unorm,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        let mask_view = mask_tex.create_view(&wgpu::TextureViewDescriptor::default());
        let mask_samp = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("gonedark.globe_landmask_sampler"),
            address_mode_u: wgpu::AddressMode::Repeat,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });

        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("gonedark.globe_backdrop_layout"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::VERTEX | wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
            ],
        });
        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("gonedark.globe_backdrop_bind_group"),
            layout: &layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: uniform_buf.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::TextureView(&mask_view),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: wgpu::BindingResource::Sampler(&mask_samp),
                },
            ],
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("gonedark.globe_backdrop_pipeline_layout"),
            bind_group_layouts: &[Some(&layout)],
            immediate_size: 0,
        });

        let color_target = |blend: Option<wgpu::BlendState>| {
            Some(wgpu::ColorTargetState {
                format,
                blend,
                write_mask: wgpu::ColorWrites::ALL,
            })
        };
        let fullscreen = |label: &'static str, fs_entry: &'static str, blend: wgpu::BlendState| {
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some(label),
                layout: Some(&pipeline_layout),
                vertex: wgpu::VertexState {
                    module: &shader,
                    entry_point: Some("vs_fs"),
                    buffers: &[],
                    compilation_options: wgpu::PipelineCompilationOptions::default(),
                },
                fragment: Some(wgpu::FragmentState {
                    module: &shader,
                    entry_point: Some(fs_entry),
                    targets: &[color_target(Some(blend))],
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
            })
        };
        let sky_pipeline =
            fullscreen("gonedark.globe_backdrop_sky", "fs_sky", wgpu::BlendState::REPLACE);
        let vignette_pipeline = fullscreen(
            "gonedark.globe_backdrop_vignette",
            "fs_vignette",
            wgpu::BlendState::ALPHA_BLENDING,
        );

        let sphere_layout = wgpu::VertexBufferLayout {
            array_stride: std::mem::size_of::<SphereVertex>() as u64,
            step_mode: wgpu::VertexStepMode::Vertex,
            attributes: &wgpu::vertex_attr_array![0 => Float32x3],
        };
        let globe_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("gonedark.globe_backdrop_globe"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_globe"),
                buffers: &[sphere_layout],
                compilation_options: wgpu::PipelineCompilationOptions::default(),
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs_globe"),
                targets: &[color_target(Some(wgpu::BlendState::REPLACE))],
                compilation_options: wgpu::PipelineCompilationOptions::default(),
            }),
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::TriangleList,
                cull_mode: Some(wgpu::Face::Back),
                ..Default::default()
            },
            depth_stencil: Some(wgpu::DepthStencilState {
                format: DEPTH_FORMAT,
                depth_write_enabled: Some(true),
                depth_compare: Some(wgpu::CompareFunction::Less),
                stencil: wgpu::StencilState::default(),
                bias: wgpu::DepthBiasState::default(),
            }),
            multisample: wgpu::MultisampleState::default(),
            multiview_mask: None,
            cache: None,
        });

        let pin_layout = wgpu::VertexBufferLayout {
            array_stride: std::mem::size_of::<PinInstance>() as u64,
            step_mode: wgpu::VertexStepMode::Instance,
            attributes: &wgpu::vertex_attr_array![0 => Float32x3, 1 => Float32],
        };
        let pin_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("gonedark.globe_backdrop_pin"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_pin"),
                buffers: &[pin_layout],
                compilation_options: wgpu::PipelineCompilationOptions::default(),
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs_pin"),
                // Additive: the pins glow over the globe like the diorama's embers.
                targets: &[color_target(Some(wgpu::BlendState {
                    color: wgpu::BlendComponent {
                        src_factor: wgpu::BlendFactor::One,
                        dst_factor: wgpu::BlendFactor::One,
                        operation: wgpu::BlendOperation::Add,
                    },
                    alpha: wgpu::BlendComponent::OVER,
                }))],
                compilation_options: wgpu::PipelineCompilationOptions::default(),
            }),
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::TriangleList,
                ..Default::default()
            },
            // Depth-tested against the globe (a far-side pin hides) but never written.
            depth_stencil: Some(wgpu::DepthStencilState {
                format: DEPTH_FORMAT,
                depth_write_enabled: Some(false),
                depth_compare: Some(wgpu::CompareFunction::LessEqual),
                stencil: wgpu::StencilState::default(),
                bias: wgpu::DepthBiasState::default(),
            }),
            multisample: wgpu::MultisampleState::default(),
            multiview_mask: None,
            cache: None,
        });

        let (positions, indices) = sphere_mesh(SPHERE_RINGS, SPHERE_SEGS);
        let verts: Vec<SphereVertex> = positions.into_iter().map(|pos| SphereVertex { pos }).collect();
        let sphere_vbuf = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("gonedark.globe_backdrop_sphere_vbuf"),
            contents: bytemuck::cast_slice(&verts),
            usage: wgpu::BufferUsages::VERTEX,
        });
        let sphere_ibuf = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("gonedark.globe_backdrop_sphere_ibuf"),
            contents: bytemuck::cast_slice(&indices),
            usage: wgpu::BufferUsages::INDEX,
        });
        let pin_buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("gonedark.globe_backdrop_pins"),
            size: (PIN_CAPACITY as usize * std::mem::size_of::<PinInstance>()) as u64,
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        let depth_view = create_depth_view(device, 1, 1);

        GlobeBackdrop {
            uniform_buf,
            bind_group,
            sky_pipeline,
            globe_pipeline,
            pin_pipeline,
            vignette_pipeline,
            sphere_vbuf,
            sphere_ibuf,
            sphere_index_count: indices.len() as u32,
            pin_buf,
            pin_capacity: PIN_CAPACITY,
            mask_tex,
            mask_uploaded: false,
            depth_view,
            depth_size: (1, 1),
        }
    }

    fn ensure_depth(&mut self, device: &wgpu::Device, width: u32, height: u32) {
        let size = (width.max(1), height.max(1));
        if self.depth_size != size {
            self.depth_view = create_depth_view(device, size.0, size.1);
            self.depth_size = size;
        }
    }

    /// Draw the globe backdrop into `view`, CLEARING it. `pins` are the conflicts (degrees; see
    /// [`GlobePin`]); the focused pin steers [`globe_yaw`]. `time`/`cursor` as in the diorama's
    /// `render`. Submits its own encoder.
    #[allow(clippy::too_many_arguments)]
    pub fn render(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        view: &wgpu::TextureView,
        viewport: (u32, u32),
        time: f32,
        cursor: Option<[f32; 2]>,
        pins: &[GlobePin],
    ) {
        // One-time land-mask upload (the embedded bytes are tightly packed R8 rows).
        if !self.mask_uploaded {
            queue.write_texture(
                wgpu::TexelCopyTextureInfo {
                    texture: &self.mask_tex,
                    mip_level: 0,
                    origin: wgpu::Origin3d::ZERO,
                    aspect: wgpu::TextureAspect::All,
                },
                LANDMASK,
                wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(MASK_W as u32),
                    rows_per_image: Some(MASK_H as u32),
                },
                wgpu::Extent3d {
                    width: MASK_W as u32,
                    height: MASK_H as u32,
                    depth_or_array_layers: 1,
                },
            );
            self.mask_uploaded = true;
        }

        let (w, h) = (viewport.0.max(1), viewport.1.max(1));
        let aspect = w as f32 / h as f32;
        let cur = cursor.unwrap_or([0.0, 0.0]);
        let par = parallax_offset(cur, PARALLAX_STRENGTH);
        let eye = [EYE[0] - par[0], EYE[1] + par[1] * 0.6, EYE[2]];

        let focus_lon = pins
            .iter()
            .find(|p| p.focused)
            .or_else(|| pins.first())
            .map_or(0.0, |p| p.lon_deg);
        let proj = perspective_rh_zo(FOVY, if aspect > 1e-3 { aspect } else { 1.0 }, NEAR, FAR);
        let view_mat = look_at_rh(eye, TARGET, [0.0, 1.0, 0.0]);
        let uniform = GlobeUniform {
            view_proj: mat4_mul(proj, view_mat),
            model: globe_model(globe_yaw(focus_lon, time)),
            eye: [eye[0], eye[1], eye[2], time],
            misc: [aspect, 0.0, 0.0, 0.0],
        };
        queue.write_buffer(&self.uniform_buf, 0, bytemuck::bytes_of(&uniform));

        let instances: Vec<PinInstance> = pins
            .iter()
            .take(self.pin_capacity as usize)
            .map(|p| PinInstance {
                unit: latlon_to_unit(p.lat_deg, p.lon_deg),
                focused: if p.focused { 1.0 } else { 0.0 },
            })
            .collect();
        if !instances.is_empty() {
            queue.write_buffer(&self.pin_buf, 0, bytemuck::cast_slice(&instances));
        }

        self.ensure_depth(device, w, h);

        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("gonedark.globe_backdrop_encoder"),
        });

        // Pass 1 — sky: CLEAR to the ink void, paint the gradient + star noise.
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("gonedark.globe_backdrop_sky_pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color {
                            r: 0.012,
                            g: 0.018,
                            b: 0.034,
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
            pass.set_bind_group(0, &self.bind_group, &[]);
            pass.draw(0..3, 0..1);
        }

        // Pass 2 — the earth + its pins, depth-tested (LOAD the sky).
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("gonedark.globe_backdrop_scene_pass"),
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
                        store: wgpu::StoreOp::Store,
                    }),
                    stencil_ops: None,
                }),
                multiview_mask: None,
                timestamp_writes: None,
                occlusion_query_set: None,
            });
            pass.set_bind_group(0, &self.bind_group, &[]);
            pass.set_pipeline(&self.globe_pipeline);
            pass.set_vertex_buffer(0, self.sphere_vbuf.slice(..));
            pass.set_index_buffer(self.sphere_ibuf.slice(..), wgpu::IndexFormat::Uint32);
            pass.draw_indexed(0..self.sphere_index_count, 0, 0..1);
            if !instances.is_empty() {
                pass.set_pipeline(&self.pin_pipeline);
                pass.set_vertex_buffer(0, self.pin_buf.slice(..));
                pass.draw(0..6, 0..instances.len() as u32);
            }
        }

        // Pass 3 — vignette so the centred hub card reads.
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("gonedark.globe_backdrop_vignette_pass"),
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
            pass.set_bind_group(0, &self.bind_group, &[]);
            pass.draw(0..3, 0..1);
        }

        queue.submit(Some(encoder.finish()));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn latlon_maps_the_cardinal_points() {
        let close = |a: [f32; 3], b: [f32; 3]| a.iter().zip(b).all(|(x, y)| (x - y).abs() < 1e-5);
        assert!(close(latlon_to_unit(90.0, 0.0), [0.0, 1.0, 0.0]), "north pole is +Y");
        assert!(close(latlon_to_unit(-90.0, 0.0), [0.0, -1.0, 0.0]), "south pole is -Y");
        assert!(close(latlon_to_unit(0.0, 0.0), [0.0, 0.0, 1.0]), "lon 0 faces +Z");
        assert!(close(latlon_to_unit(0.0, 90.0), [1.0, 0.0, 0.0]), "east is +X");
        // Every output is unit-length (the mesh IS its normals).
        for (lat, lon) in [(37.0, -122.0), (-45.0, 170.0), (89.0, 13.0)] {
            let p = latlon_to_unit(lat, lon);
            let len = (p[0] * p[0] + p[1] * p[1] + p[2] * p[2]).sqrt();
            assert!((len - 1.0).abs() < 1e-5);
        }
    }

    #[test]
    fn the_landmask_is_oriented_like_the_earth() {
        // Pins the embedded mask's orientation end-to-end: a flipped/transposed regeneration
        // (tools/earth/gen_landmask.py) fails here loudly, not as a silently mirrored planet.
        assert_eq!(LANDMASK.len(), MASK_W * MASK_H);
        assert!(land_at(48.8, 2.3), "Paris is land");
        assert!(land_at(39.0, -98.0), "Kansas is land");
        assert!(land_at(-25.0, 134.0), "central Australia is land");
        assert!(land_at(-80.0, 0.0), "Antarctica is land");
        assert!(!land_at(30.0, -40.0), "the mid-Atlantic is sea");
        assert!(!land_at(0.0, -160.0), "the mid-Pacific is sea");
        // The shipped campaign's pin (the Channel Crisis, ~50.0N 1.5W) sits in the water between
        // two coasts — the mask is fine enough to resolve the Channel itself.
        assert!(!land_at(50.0, -1.5), "the mid-Channel pin is at sea");
        assert!(land_at(49.4, -1.0), "the Cotentin coast beside it is land");
    }

    #[test]
    fn the_focused_conflict_faces_the_camera() {
        // At any longitude, the settled yaw must rotate the focus point onto max +Z (toward the
        // eye). Evaluate at sway-neutral times (sin(t*rate)=0 → t=0).
        for lon in [-122.0_f32, -1.5, 0.0, 13.4, 139.7] {
            let p = rotate_y(latlon_to_unit(50.0, lon), globe_yaw(lon, 0.0));
            assert!(p[0].abs() < 1e-4, "lon {lon}: x should vanish, got {}", p[0]);
            assert!(p[2] > 0.6, "lon {lon}: focus should face +Z, got z={}", p[2]);
        }
    }

    #[test]
    fn sphere_mesh_is_well_formed() {
        let (pos, idx) = sphere_mesh(SPHERE_RINGS, SPHERE_SEGS);
        assert_eq!(pos.len(), ((SPHERE_RINGS + 1) * (SPHERE_SEGS + 1)) as usize);
        assert_eq!(idx.len(), (SPHERE_RINGS * SPHERE_SEGS * 6) as usize);
        assert!(idx.iter().all(|&i| (i as usize) < pos.len()), "indices in bounds");
        for p in &pos {
            let len = (p[0] * p[0] + p[1] * p[1] + p[2] * p[2]).sqrt();
            assert!((len - 1.0).abs() < 1e-4, "every vertex is a unit normal");
        }
    }
}
