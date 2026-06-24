//! Renderer — consumes a READ-ONLY core snapshot and draws it (invariant #4).
//!
//! This is the float boundary: Q16.16 sim positions become `f32` HERE, never in `core`.
//! The renderer only ever *reads* a [`Snapshot`]; it never mutates sim state and never
//! calls back into `core`. It talks to `wgpu` (→ Vulkan/D3D12/Metal per device) and to no
//! specific GPU API and no windowing crate — the RHI-over-many-APIs property holds (D19).
//!
//! ## Ownership of the GPU device (D19)
//! The `wgpu::Device`/`Queue` are owned by the concrete platform backend and handed *in*
//! by the `app` wiring layer: [`Renderer::new`] borrows a `&wgpu::Device` to build its
//! pipeline once, and [`Renderer::render`] borrows `&Device`/`&Queue` each frame to upload
//! and submit. The renderer never acquires or presents the surface — it records into the
//! `&TextureView` it is handed and submits; the caller owns acquire/present.
//!
//! ## What it draws
//! Each live unit is one instanced quad. A camera uniform (a column-major `view_proj`
//! built by `app` from glam) places it. Embodied units draw in a bright amber; ordinary
//! units in a neutral grey-blue (see `shader.wgsl`).
//!
//! ## "World goes dark" (invariant #6)
//! When `world_dark` is set (the local player is embodied), the frame clears to near-black
//! and **only embodied instances are uploaded** — the strategic map genuinely disappears,
//! leaving just the avatar(s). This is intel-free by construction: a unit that is not the
//! avatar contributes nothing to the dark frame, so it cannot leak position as a pixel.
//! Filtering happens at upload time in [`Renderer::render`]; [`Renderer::prepare`] still
//! interpolates the full set so a single un-embodied frame can light the whole map again.

use gonedark_core::fixed::Fixed;
use gonedark_core::snapshot::Snapshot;
use wgpu::util::DeviceExt;

/// Convert a Q16.16 fixed value to `f32` for the GPU. The ONLY sanctioned fixed→float hop.
#[inline]
pub fn fixed_to_f32(v: Fixed) -> f32 {
    v.to_bits() as f32 / Fixed::SCALE as f32
}

/// Column-major 4x4 view-projection matrix, built by `app` (glam `Mat4::to_cols_array_2d()`).
///
/// Uploaded verbatim into the camera uniform buffer each frame.
#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
pub struct Camera {
    pub view_proj: [[f32; 4]; 4],
}

/// One renderable unit instance in float space (render-only). `repr(C)` + `Pod` so it can
/// be uploaded straight into the per-instance vertex buffer; `embodied` is a `u32` (not a
/// `bool`) because `bool` is not `Pod` and has no defined GPU representation.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, bytemuck::Pod, bytemuck::Zeroable)]
pub struct UnitInstance {
    pub x: f32,
    pub y: f32,
    pub embodied: u32,
}

/// A unit-quad corner in local space. Two triangles cover `[-1, 1]^2` (the shader scales by
/// a half-extent). `repr(C)` so it uploads as the per-vertex stream.
#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct QuadVertex {
    corner: [f32; 2],
}

/// The two triangles of a unit quad, corners in `[-1, 1]^2`.
const QUAD_VERTS: [QuadVertex; 6] = [
    QuadVertex { corner: [-1.0, -1.0] },
    QuadVertex { corner: [1.0, -1.0] },
    QuadVertex { corner: [1.0, 1.0] },
    QuadVertex { corner: [-1.0, -1.0] },
    QuadVertex { corner: [1.0, 1.0] },
    QuadVertex { corner: [-1.0, 1.0] },
];

/// Lit-frame clear (command view): a dark slate the units read against.
const CLEAR_LIT: wgpu::Color = wgpu::Color {
    r: 0.02,
    g: 0.03,
    b: 0.05,
    a: 1.0,
};

/// Dark-frame clear (embodied "world goes dark"): near-black. The map is gone.
const CLEAR_DARK: wgpu::Color = wgpu::Color {
    r: 0.0,
    g: 0.0,
    b: 0.0,
    a: 1.0,
};

/// The renderer: an instanced pipeline plus its GPU buffers and camera uniform.
///
/// Built once for a given surface format; re-fed a fresh instance set every frame. The
/// instance buffer grows on demand and is otherwise reused frame to frame.
pub struct Renderer {
    pipeline: wgpu::RenderPipeline,
    camera_buf: wgpu::Buffer,
    camera_bind_group: wgpu::BindGroup,
    quad_buf: wgpu::Buffer,
    /// Per-instance GPU buffer; reallocated only when it must grow.
    instance_buf: wgpu::Buffer,
    /// Capacity (in instances) currently allocated in `instance_buf`.
    instance_cap: usize,
    /// CPU-side interpolated instances from the last [`Renderer::prepare`].
    instances: Vec<UnitInstance>,
}

impl Renderer {
    /// Build the instanced pipeline, camera UBO, unit-quad vertex buffer, and a small
    /// initial instance buffer for `surface_format`. The `device` is borrowed (D19) — the
    /// caller (a concrete platform backend via `app`) owns it.
    pub fn new(device: &wgpu::Device, surface_format: wgpu::TextureFormat) -> Self {
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("gonedark.unit_shader"),
            source: wgpu::ShaderSource::Wgsl(include_str!("shader.wgsl").into()),
        });

        // Camera uniform: one column-major view_proj.
        let camera_buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("gonedark.camera_ubo"),
            size: std::mem::size_of::<Camera>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        let camera_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("gonedark.camera_layout"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            }],
        });

        let camera_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("gonedark.camera_bind_group"),
            layout: &camera_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: camera_buf.as_entire_binding(),
            }],
        });

        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("gonedark.pipeline_layout"),
            bind_group_layouts: &[Some(&camera_layout)],
            immediate_size: 0,
        });

        // Vertex buffer layouts: slot 0 = per-vertex quad corner, slot 1 = per-instance unit.
        let quad_layout = wgpu::VertexBufferLayout {
            array_stride: std::mem::size_of::<QuadVertex>() as u64,
            step_mode: wgpu::VertexStepMode::Vertex,
            attributes: &wgpu::vertex_attr_array![0 => Float32x2],
        };
        let instance_layout = wgpu::VertexBufferLayout {
            array_stride: std::mem::size_of::<UnitInstance>() as u64,
            step_mode: wgpu::VertexStepMode::Instance,
            // location 1 = pos (Float32x2), location 2 = embodied (Uint32).
            attributes: &wgpu::vertex_attr_array![1 => Float32x2, 2 => Uint32],
        };

        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("gonedark.unit_pipeline"),
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
            label: Some("gonedark.quad_vbo"),
            contents: bytemuck::cast_slice(&QUAD_VERTS),
            usage: wgpu::BufferUsages::VERTEX,
        });

        // Start the instance buffer with room for a handful of units; it grows on demand.
        let instance_cap = 64;
        let instance_buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("gonedark.instance_vbo"),
            size: (instance_cap * std::mem::size_of::<UnitInstance>()) as u64,
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        Renderer {
            pipeline,
            camera_buf,
            camera_bind_group,
            quad_buf,
            instance_buf,
            instance_cap,
            instances: Vec::new(),
        }
    }

    /// Build render instances by interpolating between the previous and current sim
    /// snapshots by `alpha` in `[0,1]` (invariant #4 — interpolation lives here, not in
    /// the sim). Units are matched by index; this assumes a stable unit set. Produces CPU
    /// data only; the GPU upload happens in [`Renderer::render`].
    pub fn prepare(&mut self, prev: &Snapshot, curr: &Snapshot, alpha: f32) {
        self.instances.clear();
        let n = prev.units.len().min(curr.units.len());
        for i in 0..n {
            let a = &prev.units[i];
            let b = &curr.units[i];
            let (ax, ay) = (fixed_to_f32(a.pos.x), fixed_to_f32(a.pos.y));
            let (bx, by) = (fixed_to_f32(b.pos.x), fixed_to_f32(b.pos.y));
            self.instances.push(UnitInstance {
                x: ax + (bx - ax) * alpha,
                y: ay + (by - ay) * alpha,
                embodied: u32::from(b.embodied),
            });
        }
    }

    /// The CPU-side interpolated instances from the last [`Renderer::prepare`].
    pub fn instances(&self) -> &[UnitInstance] {
        &self.instances
    }

    /// Upload instances + camera, clear, record one render pass into `view`, and submit.
    ///
    /// `world_dark` is the embodied "world goes dark" state: when set, the frame clears to
    /// near-black and only embodied instances are drawn (invariant #6 — the map disappears
    /// and non-avatar units leak no pixels). When clear, the full command view is drawn.
    pub fn render(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        view: &wgpu::TextureView,
        camera: &Camera,
        world_dark: bool,
    ) {
        queue.write_buffer(&self.camera_buf, 0, bytemuck::bytes_of(camera));

        // Pick the draw set: dark frames show only the embodied avatar(s).
        let draw_set: Vec<UnitInstance> = if world_dark {
            self.instances
                .iter()
                .copied()
                .filter(|u| u.embodied != 0)
                .collect()
        } else {
            self.instances.clone()
        };

        // Grow the instance buffer if this frame needs more room than we have.
        if draw_set.len() > self.instance_cap {
            let new_cap = draw_set.len().next_power_of_two();
            self.instance_buf = device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("gonedark.instance_vbo"),
                size: (new_cap * std::mem::size_of::<UnitInstance>()) as u64,
                usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });
            self.instance_cap = new_cap;
        }
        if !draw_set.is_empty() {
            queue.write_buffer(&self.instance_buf, 0, bytemuck::cast_slice(&draw_set));
        }

        let clear = if world_dark { CLEAR_DARK } else { CLEAR_LIT };

        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("gonedark.frame_encoder"),
        });
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("gonedark.unit_pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(clear),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                multiview_mask: None,
                timestamp_writes: None,
                occlusion_query_set: None,
            });

            // Even an empty draw set still clears the frame (the pass above) — only the
            // instanced draw is skipped when there is nothing to show.
            if !draw_set.is_empty() {
                pass.set_pipeline(&self.pipeline);
                pass.set_bind_group(0, &self.camera_bind_group, &[]);
                pass.set_vertex_buffer(0, self.quad_buf.slice(..));
                pass.set_vertex_buffer(1, self.instance_buf.slice(..));
                pass.draw(0..QUAD_VERTS.len() as u32, 0..draw_set.len() as u32);
            }
        }

        queue.submit(std::iter::once(encoder.finish()));
    }
}
