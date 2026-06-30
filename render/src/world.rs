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
//!    intel even in principle. This module owns that pass ([`WorldRenderer`]).
//! 2. **Weapon viewmodel** — the first-person gun. As of D44 this is the real `weapon_rifle`
//!    greybox **3D mesh** drawn through the shared [`crate::mesh::MeshPipeline`] (the
//!    [`crate::Renderer`] owns that pipeline + the mesh library + the depth buffer and drives the
//!    pass — see `Renderer::render_world_weapon`), anchored in *view space* by
//!    [`weapon_view_model`] so it stays glued to the lower-right of the screen regardless of camera
//!    yaw. A muzzle-flash term flares the gun for a few ticks after the player fires; this module
//!    still owns the flash *intensity* curve ([`muzzle_flash_intensity`]) and the placement math.
//!
//! The float boundary lives here (invariant #1/#4): every value is already `f32`, the renderer
//! never mutates sim state and never calls back into `core`. Like the rest of this crate it takes
//! **no `glam`/windowing dep** (D19) — the host (which owns glam) hands matrices in as plain
//! column-major `[[f32; 4]; 4]` arrays; this module only does scalar `f32` math.

/// How many ticks the muzzle flash stays lit after a shot before it has fully faded. At 60 Hz this
/// is a ~0.13 s flare — a snappy cue, gone before the next likely shot.
pub const MUZZLE_FLASH_TICKS: u64 = 8;

/// Edge length (px) of the square ground detail map (`assets/textures/ground.gray`). The contract
/// with `tools/textures/gen_textures.py` (`SIZE` there MUST match): the baked file is
/// `GROUND_TEX_SIZE * GROUND_TEX_SIZE` raw R8 bytes. The [`ground_tex_matches_metrics`](tests) test
/// pins the `include_bytes!`d blob length so a generator/metrics drift fails `cargo test`.
pub const GROUND_TEX_SIZE: u32 = 256;

/// The baked seamless ground detail map: raw `GROUND_TEX_SIZE²` R8 bytes (one luminance byte per
/// texel), `include_bytes!`d straight in so the render crate needs no image-decode dependency (it
/// stays `wgpu` + `bytemuck` only — the same rule as the D74 font atlas). Generated with ImageMagick
/// by `tools/textures/gen_textures.py`; render-only, carries no sim/intel (invariants #1/#4/#6).
const GROUND_TEX_BYTES: &[u8] = include_bytes!("../../assets/textures/ground.gray");

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

/// Build the column-major **view-space** model matrix that places the weapon viewmodel in the
/// avatar's hands — anchored to the lower-right of the screen and pointing into the world. Because
/// the host hands the mesh pipeline the *projection alone* as its camera matrix for this pass (not
/// `view * proj`), the gun lives in view space and stays put under camera yaw/pitch, exactly like a
/// real FPS viewmodel. Pure scalar `f32` (no `glam`, D19) so it is unit-testable.
///
/// View space is camera-at-origin looking down `-Z`, `+Y` up, `+X` right. The rifle mesh is modelled
/// Z-up with its barrel along local `+X`, so we re-base its axes: local `+X` (barrel) → view `-Z`
/// (forward, into the screen), local `+Z` (up) → view `+Y` (up). `flash` adds a small recoil kick
/// back toward the camera so firing reads as a jolt, not just a colour flare.
pub fn weapon_view_model(flash: f32) -> [[f32; 4]; 4] {
    let s = 0.42; // gun size in view units
                  // Lower-right anchor, a little in front of the near plane. Recoil kicks it back/up.
    let tx = 0.16;
    let ty = -0.20 + flash * 0.03;
    let tz = -0.62 + flash * 0.07;

    // Columns = images of the scaled local axes in view space, then the translation column.
    //   local +X (barrel) → view -Z;  local +Y → view -X;  local +Z (up) → view +Y.
    [
        [0.0, 0.0, -s, 0.0], // s * (0,0,-1)
        [-s, 0.0, 0.0, 0.0], // s * (-1,0,0)
        [0.0, s, 0.0, 0.0],  // s * (0,1,0)
        [tx, ty, tz, 1.0],
    ]
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

/// Sky + ground pass for the embodied (first-person) view. Owns the fullscreen sky/ground pipeline
/// (which CLEARS the frame). The weapon viewmodel is no longer drawn here — it is a 3D mesh drawn by
/// the [`crate::Renderer`] through the shared [`crate::mesh::MeshPipeline`] (D44).
pub struct WorldRenderer {
    /// Fullscreen sky/ground pipeline (clears the frame to the world).
    sky_pipeline: wgpu::RenderPipeline,
    /// The world uniform (inverse view-proj, eye, flash).
    uniform_buf: wgpu::Buffer,
    uniform_bind_group: wgpu::BindGroup,
    /// The ground detail-map texture, kept so the raw R8 bytes can be uploaded lazily on the first
    /// [`render_sky`](Self::render_sky) (the construction path has only a `device`, not a `queue` —
    /// the same lazy-upload pattern as `text::TextRenderer::ensure_atlas_uploaded`).
    ground_tex: wgpu::Texture,
    /// Whether [`ground_tex`](Self::ground_tex)'s bytes have been written yet.
    ground_uploaded: bool,
}

impl WorldRenderer {
    /// Build the sky/ground pipeline against the swapchain `surface_format`. The `device` is
    /// borrowed (D19).
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

        // The ground detail-map texture (R8 coverage); bytes written lazily on the first render_sky()
        // (the construction path has no queue — the `text` atlas pattern). A REPEAT sampler so the
        // shader can tile it across the world plane seamlessly.
        let ground_tex = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("gonedark.world_ground_tex"),
            size: wgpu::Extent3d {
                width: GROUND_TEX_SIZE,
                height: GROUND_TEX_SIZE,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::R8Unorm,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        let ground_view = ground_tex.create_view(&wgpu::TextureViewDescriptor::default());
        let ground_samp = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("gonedark.world_ground_sampler"),
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            address_mode_u: wgpu::AddressMode::Repeat,
            address_mode_v: wgpu::AddressMode::Repeat,
            ..Default::default()
        });

        let uniform_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("gonedark.world_uniform_layout"),
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

        let uniform_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("gonedark.world_uniform_bind_group"),
            layout: &uniform_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: uniform_buf.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::TextureView(&ground_view),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: wgpu::BindingResource::Sampler(&ground_samp),
                },
            ],
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

        WorldRenderer {
            sky_pipeline,
            uniform_buf,
            uniform_bind_group,
            ground_tex,
            ground_uploaded: false,
        }
    }

    /// Upload the baked R8 ground detail map into the texture, once. Called on the first
    /// [`render_sky`](Self::render_sky) (the construction path has no `queue`); a no-op thereafter.
    /// Mirrors `text::TextRenderer::ensure_atlas_uploaded`.
    fn ensure_ground_uploaded(&mut self, queue: &wgpu::Queue) {
        if self.ground_uploaded {
            return;
        }
        queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &self.ground_tex,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            GROUND_TEX_BYTES,
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(GROUND_TEX_SIZE),
                rows_per_image: Some(GROUND_TEX_SIZE),
            },
            wgpu::Extent3d {
                width: GROUND_TEX_SIZE,
                height: GROUND_TEX_SIZE,
                depth_or_array_layers: 1,
            },
        );
        self.ground_uploaded = true;
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
        self.ensure_ground_uploaded(queue);
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
}

#[cfg(test)]
mod tests {
    //! `render` is the float boundary (invariant #1: floats live only in rendering), so `f32` math
    //! is fair game here. The pipelines need a real `wgpu::Device` (no display in CI), so the GPU
    //! path is untested; the testable math is factored into [`muzzle_flash_intensity`],
    //! [`WorldUniform::new`], and [`weapon_view_model`].

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

    // ---- weapon viewmodel placement (view space) ----

    /// Apply a column-major model matrix to a point (w = 1).
    fn xform(m: &[[f32; 4]; 4], p: [f32; 3]) -> [f32; 3] {
        let mut out = [m[3][0], m[3][1], m[3][2]];
        for j in 0..3 {
            for r in 0..3 {
                out[r] += m[j][r] * p[j];
            }
        }
        out
    }

    #[test]
    fn weapon_sits_in_front_lower_right() {
        // The gun's local origin lands in front of the camera (view -Z), to the right (+X) and
        // below centre (-Y) — a right-handed lower-screen hold.
        let m = weapon_view_model(0.0);
        let o = xform(&m, [0.0, 0.0, 0.0]);
        assert!(o[2] < 0.0, "in front of the camera (−Z), got {o:?}");
        assert!(o[0] > 0.0, "to the right");
        assert!(o[1] < 0.0, "below centre");
        assert_eq!(m[3], [0.16, -0.20, -0.62, 1.0], "affine translation column");
    }

    #[test]
    fn weapon_barrel_points_into_the_screen() {
        // The barrel tip (local +X) projects further from the camera (more negative view Z) than
        // the stock (local −X): the gun points forward, into the world.
        let m = weapon_view_model(0.0);
        let tip = xform(&m, [0.6, 0.0, 0.0]);
        let stock = xform(&m, [-0.3, 0.0, 0.0]);
        assert!(tip[2] < stock[2], "barrel tip is deeper into the scene");
        // Local up (+Z) maps to view up (+Y).
        let up = xform(&m, [0.0, 0.0, 1.0]);
        let base = xform(&m, [0.0, 0.0, 0.0]);
        assert!(up[1] > base[1], "the sights point up the screen");
    }

    #[test]
    fn weapon_recoils_on_fire() {
        // A live flash kicks the gun back toward the camera (less negative Z) and up vs the rest
        // pose, so firing reads as a jolt.
        let rest = weapon_view_model(0.0);
        let fired = weapon_view_model(1.0);
        assert!(fired[3][2] > rest[3][2], "recoils back toward the camera");
        assert!(fired[3][1] > rest[3][1], "and kicks up");
    }

    // ---- ground detail-map metrics contract ----

    #[test]
    fn ground_tex_matches_metrics() {
        // The baked ground blob length MUST equal GROUND_TEX_SIZE² — a guard against the generator
        // and this const drifting (which would shear / misalign the sampled detail at runtime).
        assert_eq!(
            GROUND_TEX_BYTES.len(),
            (GROUND_TEX_SIZE * GROUND_TEX_SIZE) as usize,
            "raw R8 ground size must match GROUND_TEX_SIZE² — regenerate with `pnpm assets:textures`"
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
