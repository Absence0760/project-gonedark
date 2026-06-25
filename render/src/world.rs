//! Embodied first-person world (W5) — the ground/sky/weapon the avatar stands in while the
//! strategic map is dark (invariant #6). This is **render-only**: it draws a believable
//! first-person *space* (a floor, a horizon, a held weapon) but reveals **no map intel** — no
//! enemy units, no enemy buildings, no control points. Those are filtered out upstream by
//! [`crate::fog::visible_instances`] (the avatar quad is the only world instance that survives the
//! dark frame); this module only ever draws the *environment*, which carries zero intel.
//!
//! ## What it draws, in order (all in the embodied pass, before the avatar + HUD)
//! 1. **Sky + ground** — a single fullscreen pass. The fragment shader reconstructs each pixel's
//!    view ray from the inverse view-projection and shades it: rays that point at/below the ground
//!    plane (`z = 0`) get a gridded floor (so motion + heading read); rays above the horizon get a
//!    sky gradient. This replaces the bare near-black `CLEAR_DARK` void with a real space while
//!    staying a pure function of the *camera* — it has no access to sim entities, so it cannot leak
//!    intel even in principle.
//! 2. **Weapon viewmodel** — a simple first-person gun shape anchored to the lower-centre of the
//!    screen (screen-space NDC, like `hud.rs`), with a muzzle-flash cue that flares for a few ticks
//!    after the player fires. The flash is driven by a host-fed "last fire tick", NOT by reaching
//!    into sim state.
//!
//! The float boundary lives here (invariant #1/#4): every value is already `f32`; the renderer
//! never mutates sim state and never calls back into `core`. Like the rest of this crate it takes
//! **no `glam`/windowing dep** (D19) — the host (which owns glam) hands matrices in as plain
//! column-major `[[f32; 4]; 4]` arrays; this module only does scalar `f32` math.

/// How many ticks the muzzle flash stays lit after a shot before it has fully faded. At 60 Hz this
/// is a ~0.13 s flare — a snappy cue, gone before the next likely shot.
pub const MUZZLE_FLASH_TICKS: u64 = 8;

/// Compute the muzzle-flash intensity in `[0, 1]` for the current `tick`, given the tick the
/// player last fired on (`None` if they have not fired). Fresh shot → `1.0`, then a linear ramp to
/// `0.0` over [`MUZZLE_FLASH_TICKS`]; a future-stamped or long-past fire is dark. Pure float math
/// (presentation boundary), so it is unit-testable without a GPU.
pub fn muzzle_flash_intensity(last_fire_tick: Option<u64>, tick: u64) -> f32 {
    let Some(fired) = last_fire_tick else {
        return 0.0;
    };
    if tick < fired {
        return 0.0; // future-stamped fire is not yet live
    }
    let age = tick - fired;
    if age >= MUZZLE_FLASH_TICKS {
        return 0.0;
    }
    let t = age as f32 / MUZZLE_FLASH_TICKS as f32; // 0 fresh → 1 at cutoff
    1.0 - t
}

/// Parameters for the embodied world pass, handed in by the host each frame. All `f32` — the
/// render-side float boundary. `inv_view_proj` is the inverse of the camera's view-projection
/// (column-major, the host's `Mat4::inverse().to_cols_array_2d()`), used by the shader to
/// reconstruct world rays for the sky/ground. `eye` is the camera world position (so the shader can
/// fade the floor grid with distance). `flash` is the current muzzle-flash intensity in `[0,1]`.
/// `repr(C)` + `Pod` so it uploads straight into the uniform buffer; field order/offsets MUST match
/// `world.wgsl`'s `World` uniform.
#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
pub struct WorldUniform {
    /// Inverse view-projection (column-major). The host computes the inverse (it owns glam).
    pub inv_view_proj: [[f32; 4]; 4],
    /// Camera eye in world space (xyz); w is unused padding (kept 0).
    pub eye: [f32; 4],
    /// Muzzle-flash intensity `[0,1]` in x; y/z/w are reserved padding (kept 0).
    pub flash: [f32; 4],
}

impl WorldUniform {
    /// Build the uniform from the host-computed inverse view-projection, the eye world position,
    /// and the muzzle-flash intensity (clamped to `[0,1]`). Pure + device-free, so it is
    /// unit-testable. The host owns the matrix inverse (it has glam; this crate must not — D19).
    pub fn new(inv_view_proj: [[f32; 4]; 4], eye: [f32; 3], flash: f32) -> Self {
        WorldUniform {
            inv_view_proj,
            eye: [eye[0], eye[1], eye[2], 0.0],
            flash: [flash.clamp(0.0, 1.0), 0.0, 0.0, 0.0],
        }
    }
}

/// One screen-space NDC vertex of the weapon viewmodel, carrying a per-vertex shade so the gun has
/// simple form (a darker grip, a lighter slide) without a texture. `repr(C)` so it uploads as the
/// viewmodel vertex stream; field order/offsets MUST match `world.wgsl`'s `vs_weapon` attributes.
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, bytemuck::Pod, bytemuck::Zeroable)]
pub struct ViewmodelVertex {
    /// Position in NDC ([-1,1], +y up).
    pub ndc: [f32; 2],
    /// Greyscale shade in `[0,1]` for this vertex (flat-ish gun metal).
    pub shade: f32,
    /// 0 = gun body, 1 = muzzle-flash quad (lit by the flash uniform).
    pub kind: f32,
}

/// Build the weapon-viewmodel geometry in screen-space NDC for the given viewport aspect. A simple
/// low-poly pistol: a grip block rooted at the lower-centre, a raised slide, and a small muzzle
/// quad at the barrel tip that the muzzle flash lights. The shape is **aspect-corrected** so it
/// keeps its proportions on wide vs tall surfaces — the horizontal extents are scaled by `1/aspect`
/// when wider than tall (so the gun doesn't stretch). Pure float math, returned as a flat triangle
/// list, so it is unit-testable without a GPU.
pub fn viewmodel_vertices(aspect: f32) -> Vec<ViewmodelVertex> {
    // Horizontal squash so the gun reads the same width on any aspect (>1 wide, <1 tall).
    let sx = if aspect >= 1.0 { 1.0 / aspect } else { 1.0 };

    // Anchor: lower-centre, offset slightly right (a right-handed hold). NDC y is up.
    let cx = 0.18 * sx;
    let floor = -1.0; // bottom of the screen

    let mut out: Vec<ViewmodelVertex> = Vec::new();
    let mut quad = |x0: f32, y0: f32, x1: f32, y1: f32, shade: f32, kind: f32| {
        let v = |x: f32, y: f32| ViewmodelVertex {
            ndc: [x, y],
            shade,
            kind,
        };
        out.push(v(x0, y0));
        out.push(v(x1, y0));
        out.push(v(x1, y1));
        out.push(v(x0, y0));
        out.push(v(x1, y1));
        out.push(v(x0, y1));
    };

    // Grip: a tall darker block rooted at the bottom edge.
    let grip_w = 0.16 * sx;
    quad(cx - grip_w, floor, cx + grip_w, floor + 0.42, 0.22, 0.0);
    // Body/slide: a lighter horizontal block extending left (toward screen centre — the barrel
    // points "into" the world ahead of the avatar).
    let body_left = cx - 0.46 * sx;
    quad(body_left, floor + 0.30, cx + grip_w, floor + 0.50, 0.40, 0.0);
    // Muzzle quad: a small block at the barrel tip; `kind = 1` so the shader can flare it.
    let muzzle_w = 0.07 * sx;
    quad(
        body_left - muzzle_w,
        floor + 0.32,
        body_left,
        floor + 0.48,
        0.30,
        1.0,
    );

    out
}

/// Sky + ground + weapon-viewmodel pass for the embodied (first-person) view. Owns two pipelines:
/// a fullscreen sky/ground pass (which CLEARS the frame) and a screen-space weapon-viewmodel pass
/// (a LOAD pass drawn after the avatar). Separate pipelines/shaders so neither contends with the
/// unit or HUD passes.
pub struct WorldRenderer {
    /// Fullscreen sky/ground pipeline (clears the frame to the world).
    sky_pipeline: wgpu::RenderPipeline,
    /// The world uniform (inverse view-proj, eye, flash).
    uniform_buf: wgpu::Buffer,
    uniform_bind_group: wgpu::BindGroup,
    /// Weapon-viewmodel pipeline (screen-space LOAD pass).
    weapon_pipeline: wgpu::RenderPipeline,
    /// Per-vertex GPU buffer for the viewmodel; reallocated only when it must grow.
    weapon_buf: wgpu::Buffer,
    /// Capacity (in vertices) currently allocated in `weapon_buf`.
    weapon_cap: usize,
    /// Bind group for the weapon pass (reuses `uniform_buf` for the flash intensity).
    weapon_bind_group: wgpu::BindGroup,
}

impl WorldRenderer {
    /// Build the sky/ground + weapon pipelines against the swapchain `surface_format`. The `device`
    /// is borrowed (D19).
    pub fn new(device: &wgpu::Device, surface_format: wgpu::TextureFormat) -> Self {
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("gonedark.world_shader"),
            source: wgpu::ShaderSource::Wgsl(include_str!("world.wgsl").into()),
        });

        let uniform_buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("gonedark.world_uniform"),
            size: std::mem::size_of::<WorldUniform>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        let uniform_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("gonedark.world_uniform_layout"),
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
            label: Some("gonedark.world_uniform_bind_group"),
            layout: &uniform_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: uniform_buf.as_entire_binding(),
            }],
        });
        let weapon_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("gonedark.world_weapon_bind_group"),
            layout: &uniform_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: uniform_buf.as_entire_binding(),
            }],
        });

        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("gonedark.world_pipeline_layout"),
            bind_group_layouts: &[Some(&uniform_layout)],
            immediate_size: 0,
        });

        // Sky/ground: a fullscreen triangle generated in the vertex shader (no vertex buffer).
        let sky_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("gonedark.world_sky_pipeline"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_sky"),
                buffers: &[],
                compilation_options: wgpu::PipelineCompilationOptions::default(),
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs_sky"),
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

        let weapon_layout = wgpu::VertexBufferLayout {
            array_stride: std::mem::size_of::<ViewmodelVertex>() as u64,
            step_mode: wgpu::VertexStepMode::Vertex,
            // 0=ndc(vec2), 1=shade(f32), 2=kind(f32).
            attributes: &wgpu::vertex_attr_array![0 => Float32x2, 1 => Float32, 2 => Float32],
        };

        let weapon_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("gonedark.world_weapon_pipeline"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_weapon"),
                buffers: &[weapon_layout],
                compilation_options: wgpu::PipelineCompilationOptions::default(),
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs_weapon"),
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

        let weapon_cap = 64;
        let weapon_buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("gonedark.world_weapon_vbo"),
            size: (weapon_cap * std::mem::size_of::<ViewmodelVertex>()) as u64,
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        WorldRenderer {
            sky_pipeline,
            uniform_buf,
            uniform_bind_group,
            weapon_pipeline,
            weapon_buf,
            weapon_cap,
            weapon_bind_group,
        }
    }

    /// Draw the sky + ground for the embodied frame. This is the CLEARING pass for the embodied
    /// view: it replaces the bare `CLEAR_DARK` void with a real first-person space (a sky gradient
    /// above the horizon, a gridded floor below). It reveals **no** map intel — it is a pure
    /// function of the camera, with no access to sim entities. The host calls this FIRST in the
    /// embodied branch, before the avatar + weapon + HUD passes.
    pub fn render_sky(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        view: &wgpu::TextureView,
        uniform: &WorldUniform,
    ) {
        queue.write_buffer(&self.uniform_buf, 0, bytemuck::bytes_of(uniform));

        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("gonedark.world_sky_encoder"),
        });
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("gonedark.world_sky_pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        // CLEAR — this is the embodied frame's clearing pass (replaces CLEAR_DARK).
                        load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
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
            // Fullscreen triangle: 3 vertices, no vertex buffer.
            pass.draw(0..3, 0..1);
        }
        queue.submit(std::iter::once(encoder.finish()));
    }

    /// Draw the weapon viewmodel on top of the embodied frame (a LOAD pass — never clears),
    /// flaring the muzzle by the `flash` already in the uniform (uploaded by [`Self::render_sky`]).
    /// The host calls this AFTER the avatar pass and before the HUD. `aspect` is `width / height`
    /// for proportion correction. Screen-space chrome with no world position — reveals no intel.
    pub fn render_weapon(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        view: &wgpu::TextureView,
        aspect: f32,
    ) {
        let verts = viewmodel_vertices(aspect);
        if verts.len() > self.weapon_cap {
            let new_cap = verts.len().next_power_of_two();
            self.weapon_buf = device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("gonedark.world_weapon_vbo"),
                size: (new_cap * std::mem::size_of::<ViewmodelVertex>()) as u64,
                usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });
            self.weapon_cap = new_cap;
        }
        queue.write_buffer(&self.weapon_buf, 0, bytemuck::cast_slice(&verts));

        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("gonedark.world_weapon_encoder"),
        });
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("gonedark.world_weapon_pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Load, // draw over the world + avatar
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                multiview_mask: None,
                timestamp_writes: None,
                occlusion_query_set: None,
            });
            pass.set_pipeline(&self.weapon_pipeline);
            pass.set_bind_group(0, &self.weapon_bind_group, &[]);
            pass.set_vertex_buffer(0, self.weapon_buf.slice(..));
            pass.draw(0..verts.len() as u32, 0..1);
        }
        queue.submit(std::iter::once(encoder.finish()));
    }
}

#[cfg(test)]
mod tests {
    //! `render` is the float boundary (invariant #1: floats live only in rendering), so `f32` math
    //! is fair game here. The pipelines need a real `wgpu::Device` (no display in CI), so the GPU
    //! path is untested; the testable math is factored into [`muzzle_flash_intensity`],
    //! [`WorldUniform::new`], and [`viewmodel_vertices`].

    use super::*;

    const EPS: f32 = 1e-4;

    // ---- muzzle flash fade ----

    #[test]
    fn no_fire_means_no_flash() {
        assert_eq!(muzzle_flash_intensity(None, 100), 0.0);
    }

    #[test]
    fn fresh_fire_is_full_flash() {
        assert!((muzzle_flash_intensity(Some(50), 50) - 1.0).abs() < EPS);
    }

    #[test]
    fn flash_decays_monotonically_to_zero() {
        let young = muzzle_flash_intensity(Some(0), 1);
        let mid = muzzle_flash_intensity(Some(0), MUZZLE_FLASH_TICKS / 2);
        let old = muzzle_flash_intensity(Some(0), MUZZLE_FLASH_TICKS - 1);
        assert!(young > mid, "flash should decrease with age");
        assert!(mid > old, "flash should keep decreasing");
        assert!(old > 0.0, "still lit just before the cutoff");
    }

    #[test]
    fn flash_is_dark_after_window() {
        assert_eq!(muzzle_flash_intensity(Some(0), MUZZLE_FLASH_TICKS), 0.0);
        assert_eq!(muzzle_flash_intensity(Some(0), MUZZLE_FLASH_TICKS + 100), 0.0);
    }

    #[test]
    fn future_fire_is_dark() {
        // A fire stamped in the future (tick < fired) is not yet live.
        assert_eq!(muzzle_flash_intensity(Some(100), 50), 0.0);
    }

    // ---- world uniform ----

    #[test]
    fn uniform_carries_inverse_eye_and_flash() {
        // The uniform must thread the host-computed inverse matrix and eye through verbatim, and
        // clamp flash into [0,1] (so the shader can trust it). We don't invert here (no glam dep);
        // a sentinel matrix proves the columns survive in column-major order.
        let inv = [
            [1.0, 2.0, 3.0, 4.0],
            [5.0, 6.0, 7.0, 8.0],
            [9.0, 10.0, 11.0, 12.0],
            [13.0, 14.0, 15.0, 16.0],
        ];
        let u = WorldUniform::new(inv, [1.5, -2.5, 1.5], 0.5);
        assert_eq!(u.inv_view_proj, inv, "matrix threads through verbatim");
        assert_eq!(u.eye, [1.5, -2.5, 1.5, 0.0], "eye padded to vec4");
        assert!((u.flash[0] - 0.5).abs() < EPS);
        assert_eq!([u.flash[1], u.flash[2], u.flash[3]], [0.0, 0.0, 0.0]);
    }

    #[test]
    fn uniform_clamps_flash() {
        let m = [[0.0; 4]; 4];
        assert_eq!(
            WorldUniform::new(m, [0.0; 3], 5.0).flash[0],
            1.0,
            "over-range flash clamps to 1"
        );
        assert_eq!(
            WorldUniform::new(m, [0.0; 3], -2.0).flash[0],
            0.0,
            "under-range flash clamps to 0"
        );
    }

    // ---- weapon viewmodel placement ----

    #[test]
    fn viewmodel_is_a_nonempty_triangle_list() {
        let v = viewmodel_vertices(1.0);
        assert!(!v.is_empty(), "the gun draws some geometry");
        assert_eq!(v.len() % 3, 0, "a flat triangle list");
    }

    #[test]
    fn viewmodel_anchors_to_the_lower_screen() {
        // Every vertex sits in the lower half of the screen (NDC y < 0 region) and inside the NDC
        // box — the gun is a lower-screen overlay, never floating in the sky or off-screen.
        let v = viewmodel_vertices(1.6);
        assert!(
            v.iter().all(|p| p.ndc[1] < 0.0),
            "the whole gun stays in the lower half of the screen"
        );
        assert!(
            v.iter()
                .all(|p| p.ndc[0].abs() <= 1.0 && p.ndc[1].abs() <= 1.0),
            "the gun stays inside the NDC viewport"
        );
    }

    #[test]
    fn viewmodel_has_a_muzzle_quad() {
        // Exactly one of the three quads is the muzzle (kind == 1) — the flash-lit barrel tip.
        let v = viewmodel_vertices(1.0);
        let muzzle = v.iter().filter(|p| p.kind == 1.0).count();
        assert_eq!(muzzle, 6, "the muzzle is one quad (6 verts) tagged kind=1");
        let body = v.iter().filter(|p| p.kind == 0.0).count();
        assert_eq!(body, 12, "two body quads (grip + slide) tagged kind=0");
    }

    #[test]
    fn viewmodel_is_aspect_corrected() {
        // On a wide surface the gun is squashed horizontally (sx = 1/aspect) so it keeps its
        // proportions — the max |x| at aspect 2.0 is half the max |x| at aspect 1.0.
        let wide = viewmodel_vertices(2.0);
        let square = viewmodel_vertices(1.0);
        let max_x =
            |vs: &[ViewmodelVertex]| vs.iter().map(|p| p.ndc[0].abs()).fold(0.0_f32, f32::max);
        let (mw, ms) = (max_x(&wide), max_x(&square));
        assert!(
            (mw - ms * 0.5).abs() < 1e-3,
            "wide-aspect gun is half the NDC width of the square one"
        );
    }

    /// Validate `world.wgsl` offline with naga (the compiler wgpu uses), so a WGSL regression fails
    /// the test suite instead of only blowing up at pipeline creation on a real GPU.
    #[test]
    fn world_wgsl_parses_and_validates() {
        let src = include_str!("world.wgsl");
        let module = naga::front::wgsl::parse_str(src).expect("world.wgsl must parse");
        let mut validator = naga::valid::Validator::new(
            naga::valid::ValidationFlags::all(),
            naga::valid::Capabilities::all(),
        );
        validator
            .validate(&module)
            .expect("world.wgsl must validate");
    }
}
