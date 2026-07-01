//! Dynamic-resolution intermediate render target + upscale blit (Phase 4 WS-C).
//!
//! **This is the wgpu wiring the WS-C policy was waiting on.** `render::tiers` /
//! `engine::tuning` already decide a `resolution_scale` (a pure, tested RENDERING choice —
//! invariant #1/#4, never a sim input); this module is where that scale finally bites the GPU. The
//! heavy 3D scene (the embodied sky/ground + world meshes, the command-view grid + unit tokens, the
//! avatar, the weapon viewmodel) is rendered into an **offscreen intermediate** texture sized by the
//! scale, then this pass **upscales** that intermediate across the full swapchain. Drawing fewer
//! fragments at a sub-native scale is the whole point of dyn-res: it holds the frame budget without
//! ever changing the sim tick (the per-tick checksum stream is byte-identical at every scale — the
//! `tier_choice_is_sim_independent` guard in `engine`).
//!
//! HUD / overlay / text chrome is drawn by the host AFTER [`SceneTarget::present`], straight onto the
//! swapchain, so it stays crisp at native resolution — only the world/scene scales.
//!
//! ## What is testable vs GPU-only
//! The pure *plumbing logic* — the intermediate's pixel size from a scale ([`scene_target_dims`]) and
//! the recreate-on-resize decision ([`needs_realloc`]) — is unit-tested below (host-side `f32`/`u32`
//! math, no device). The raw wgpu glue ([`SceneTarget::new`]/[`ensure`](SceneTarget::ensure)/
//! [`present`](SceneTarget::present): texture/sampler/bind-group/pipeline creation + the render pass)
//! needs a real `wgpu::Device`, which CI has no display for — exactly like [`crate::Renderer::new`]
//! and [`crate::world::WorldRenderer`], it is intentionally not unit-tested. The blit shader itself
//! IS validated offline by naga (`present_wgsl_parses_and_validates`), so a WGSL regression still
//! fails `cargo test` rather than only blowing up at pipeline creation on a user's GPU.

/// The intermediate render-target pixel size for a swapchain of `(swapchain_w, swapchain_h)` drawn at
/// dynamic-resolution `scale`. Each axis is `round(dim * scale)`, **clamped to `[1, dim]`**: never
/// zero-area (a degenerate pass) and never larger than the swapchain (a scale `> 1` would waste
/// fragments and overshoot the upscale — the tier ceiling is `<= 1.0` anyway, but clamp defensively).
/// A non-finite or non-positive scale falls back to native (`scale` treated as `1.0`). Pure +
/// device-free, so the sizing is unit-tested without a GPU.
pub fn scene_target_dims(swapchain_w: u32, swapchain_h: u32, scale: f32) -> (u32, u32) {
    let s = if scale.is_finite() && scale > 0.0 {
        scale
    } else {
        1.0
    };
    let axis = |dim: u32| -> u32 {
        let scaled = (dim as f32 * s).round() as i64;
        scaled.clamp(1, dim.max(1) as i64) as u32
    };
    (axis(swapchain_w), axis(swapchain_h))
}

/// Whether the intermediate texture must be (re)allocated to serve `wanted` pixels, given the size it
/// is `current`ly allocated for (`None` = nothing allocated yet). A pure decision so the
/// reallocate-only-on-change policy is unit-tested without a device (mirrors `Renderer::ensure_depth`
/// / the instance-buffer grow check — reallocating a GPU texture every frame would be wasteful).
pub fn needs_realloc(current: Option<(u32, u32)>, wanted: (u32, u32)) -> bool {
    current != Some(wanted)
}

/// The offscreen intermediate scene target + the fullscreen upscale-blit pipeline. Built once
/// against the swapchain format; the texture is (re)created lazily to match the requested dyn-res
/// size ([`SceneTarget::ensure`]). Render-only — depth/colour here never touch the sim (invariant
/// #1/#4).
pub struct SceneTarget {
    /// The swapchain colour format the intermediate texture + present pipeline are built for.
    format: wgpu::TextureFormat,
    /// Linear sampler used by the blit so a sub-native scene upscales smoothly.
    sampler: wgpu::Sampler,
    /// The WS-E present uniform (the "going dark" amount). Created once; written each
    /// [`present`](Self::present) and read by `present.wgsl`'s dark branch.
    present_uniform_buf: wgpu::Buffer,
    /// Bind-group layout (intermediate texture + sampler + present uniform) the present pipeline reads.
    bind_group_layout: wgpu::BindGroupLayout,
    /// The fullscreen upscale-blit pipeline (samples the intermediate, writes the swapchain).
    present_pipeline: wgpu::RenderPipeline,
    /// The current intermediate colour texture, `None` until the first [`ensure`](Self::ensure).
    texture: Option<wgpu::Texture>,
    /// A view onto [`texture`](Self::texture) — the host renders the scene into this and the present
    /// pipeline samples it.
    view: Option<wgpu::TextureView>,
    /// The present bind group wrapping the current [`view`](Self::view) + sampler. Recreated with the
    /// texture, so it always points at the live intermediate.
    bind_group: Option<wgpu::BindGroup>,
    /// The pixel size the current texture is allocated for (drives [`needs_realloc`]).
    size: Option<(u32, u32)>,
}

impl SceneTarget {
    /// Build the upscale-blit pipeline + sampler + bind-group layout for `surface_format`. The
    /// intermediate texture is allocated lazily on the first [`ensure`](Self::ensure). The `device`
    /// is borrowed (D19).
    pub fn new(device: &wgpu::Device, surface_format: wgpu::TextureFormat) -> Self {
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("gonedark.scene_present_shader"),
            source: wgpu::ShaderSource::Wgsl(include_str!("present.wgsl").into()),
        });

        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("gonedark.scene_present_sampler"),
            // Linear min/mag so a sub-native intermediate upscales smoothly; clamp so edge taps never
            // wrap (the blit covers exactly [0,1] UVs).
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            mipmap_filter: wgpu::MipmapFilterMode::Nearest,
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            address_mode_w: wgpu::AddressMode::ClampToEdge,
            ..Default::default()
        });

        let present_uniform_buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("gonedark.scene_present_uniform"),
            size: std::mem::size_of::<crate::present::PresentUniform>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        let bind_group_layout =
            device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some("gonedark.scene_present_layout"),
                entries: &[
                    wgpu::BindGroupLayoutEntry {
                        binding: 0,
                        visibility: wgpu::ShaderStages::FRAGMENT,
                        ty: wgpu::BindingType::Texture {
                            sample_type: wgpu::TextureSampleType::Float { filterable: true },
                            view_dimension: wgpu::TextureViewDimension::D2,
                            multisampled: false,
                        },
                        count: None,
                    },
                    wgpu::BindGroupLayoutEntry {
                        binding: 1,
                        visibility: wgpu::ShaderStages::FRAGMENT,
                        ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                        count: None,
                    },
                    // binding 2: the WS-E "going dark" present uniform.
                    wgpu::BindGroupLayoutEntry {
                        binding: 2,
                        visibility: wgpu::ShaderStages::FRAGMENT,
                        ty: wgpu::BindingType::Buffer {
                            ty: wgpu::BufferBindingType::Uniform,
                            has_dynamic_offset: false,
                            min_binding_size: None,
                        },
                        count: None,
                    },
                ],
            });

        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("gonedark.scene_present_pipeline_layout"),
            bind_group_layouts: &[Some(&bind_group_layout)],
            immediate_size: 0,
        });

        let present_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("gonedark.scene_present_pipeline"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_present"),
                buffers: &[],
                compilation_options: wgpu::PipelineCompilationOptions::default(),
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs_present"),
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

        SceneTarget {
            format: surface_format,
            sampler,
            present_uniform_buf,
            bind_group_layout,
            present_pipeline,
            texture: None,
            view: None,
            bind_group: None,
            size: None,
        }
    }

    /// Ensure the intermediate texture matches `scene_target_dims(swapchain_w, swapchain_h, scale)`,
    /// (re)creating the texture + view + present bind group only when the size changes
    /// ([`needs_realloc`]). Returns the intermediate's pixel size so the caller can size the matching
    /// depth buffer and the scene passes' viewports. Cheap on an unchanged size (the common case
    /// while the dyn-res scale holds steady — the controller eases, so the rounded pixel size only
    /// flips occasionally).
    pub fn ensure(
        &mut self,
        device: &wgpu::Device,
        swapchain_w: u32,
        swapchain_h: u32,
        scale: f32,
    ) -> (u32, u32) {
        let wanted = scene_target_dims(swapchain_w, swapchain_h, scale);
        if needs_realloc(self.size, wanted) {
            let texture = device.create_texture(&wgpu::TextureDescriptor {
                label: Some("gonedark.scene_target"),
                size: wgpu::Extent3d {
                    width: wanted.0,
                    height: wanted.1,
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: self.format,
                // Scene passes render INTO it; the present pass SAMPLES it.
                usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
                view_formats: &[],
            });
            let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
            let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("gonedark.scene_present_bind_group"),
                layout: &self.bind_group_layout,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: wgpu::BindingResource::TextureView(&view),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: wgpu::BindingResource::Sampler(&self.sampler),
                    },
                    wgpu::BindGroupEntry {
                        binding: 2,
                        resource: self.present_uniform_buf.as_entire_binding(),
                    },
                ],
            });
            self.texture = Some(texture);
            self.view = Some(view);
            self.bind_group = Some(bind_group);
            self.size = Some(wanted);
        }
        wanted
    }

    /// A view onto the current intermediate texture — the host renders the scene passes into this
    /// instead of the swapchain. The returned [`wgpu::TextureView`] is a cheap (Arc) clone, so the
    /// host can hold it across the `&mut self` scene-pass calls without borrowing the renderer.
    ///
    /// # Panics
    /// Panics if [`ensure`](Self::ensure) has not been called yet (no intermediate allocated). Hosts
    /// always call `ensure` first each frame, so this is a programming-error guard, not a runtime path.
    pub fn view(&self) -> wgpu::TextureView {
        self.view
            .clone()
            .expect("SceneTarget::ensure must run before view()")
    }

    /// The pixel size the intermediate is currently allocated for, or `None` before the first
    /// [`ensure`](Self::ensure).
    pub fn size(&self) -> Option<(u32, u32)> {
        self.size
    }

    /// Upscale the intermediate scene onto the swapchain `swapchain_view`: a fullscreen-triangle blit
    /// that CLEARS the swapchain and stretches the intermediate across it with the linear sampler
    /// (identity at scale 1.0). The host calls this once, AFTER every scene pass and BEFORE any chrome
    /// pass (so the native-resolution HUD/overlay/text LOADs on top of the upscaled scene). A no-op if
    /// no intermediate is allocated yet (defensive — `ensure` always runs first in practice).
    ///
    /// `dark` is the WS-E "world goes dark" amount ([`crate::present::dark_amount`]): `0` in command
    /// view, `1` while embodied. It drives the present shader's embodied dark intensification (a
    /// tunnel vignette + shadow crush) — presentation only, invariant #1/#4/#6.
    pub fn present(
        &self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        swapchain_view: &wgpu::TextureView,
        dark: f32,
    ) {
        let Some(bind_group) = self.bind_group.as_ref() else {
            return;
        };
        queue.write_buffer(
            &self.present_uniform_buf,
            0,
            bytemuck::bytes_of(&crate::present::PresentUniform::new(dark)),
        );
        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("gonedark.scene_present_encoder"),
        });
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("gonedark.scene_present_pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: swapchain_view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        // The blit covers the whole swapchain; clear is just defensive against any
                        // uncovered fringe from rounding.
                        load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                multiview_mask: None,
                timestamp_writes: None,
                occlusion_query_set: None,
            });
            pass.set_pipeline(&self.present_pipeline);
            pass.set_bind_group(0, bind_group, &[]);
            // Fullscreen triangle: 3 vertices, no vertex buffer.
            pass.draw(0..3, 0..1);
        }
        queue.submit(std::iter::once(encoder.finish()));
    }
}

#[cfg(test)]
mod tests {
    //! Pure plumbing logic only — the dyn-res pixel sizing + the recreate-on-resize decision. The
    //! GPU glue (`SceneTarget::new`/`ensure`/`present`) needs a real `wgpu::Device` (no display in
    //! CI), so it is exempt, exactly like `Renderer::new`; `present.wgsl` is still validated offline
    //! by naga below.

    use super::*;

    #[test]
    fn dims_identity_at_full_scale() {
        // Scale 1.0 → the intermediate is exactly the swapchain (a 1:1, identity blit).
        assert_eq!(scene_target_dims(1920, 1080, 1.0), (1920, 1080));
    }

    #[test]
    fn dims_halve_at_half_scale() {
        // round(1920*0.5)=960, round(1080*0.5)=540.
        assert_eq!(scene_target_dims(1920, 1080, 0.5), (960, 540));
    }

    #[test]
    fn dims_round_to_nearest() {
        // round(101 * 0.65) = round(65.65) = 66.
        assert_eq!(scene_target_dims(101, 101, 0.65), (66, 66));
    }

    #[test]
    fn dims_never_zero_area() {
        // A tiny swapchain / tiny scale still yields at least 1×1 (no degenerate render pass).
        assert_eq!(scene_target_dims(1, 1, 0.5), (1, 1));
        assert_eq!(scene_target_dims(10, 10, 0.01), (1, 1));
        assert_eq!(scene_target_dims(0, 0, 1.0), (1, 1));
    }

    #[test]
    fn dims_clamp_to_swapchain_and_handle_bad_scale() {
        // A scale > 1 never overshoots the swapchain (defensive; the tier ceiling is <= 1.0).
        assert_eq!(scene_target_dims(800, 600, 2.0), (800, 600));
        // Non-finite / non-positive scale falls back to native.
        assert_eq!(scene_target_dims(800, 600, f32::NAN), (800, 600));
        assert_eq!(scene_target_dims(800, 600, 0.0), (800, 600));
        assert_eq!(scene_target_dims(800, 600, -0.5), (800, 600));
    }

    #[test]
    fn realloc_only_on_change() {
        // Nothing allocated yet → must allocate.
        assert!(needs_realloc(None, (960, 540)));
        // Same size → reuse (no churn).
        assert!(!needs_realloc(Some((960, 540)), (960, 540)));
        // Size changed (resize or a dyn-res step that flips the rounded size) → recreate.
        assert!(needs_realloc(Some((960, 540)), (1280, 720)));
        assert!(needs_realloc(Some((960, 540)), (961, 540)));
    }

    /// Validate `present.wgsl` offline with naga (the compiler wgpu uses), so a blit-shader regression
    /// fails the test suite instead of only blowing up at pipeline creation on a real GPU. Mirrors the
    /// `shader.wgsl` / `world.wgsl` offline-validation tests.
    #[test]
    fn present_wgsl_parses_and_validates() {
        let src = include_str!("present.wgsl");
        let module = naga::front::wgsl::parse_str(src).expect("present.wgsl must parse");
        let mut validator = naga::valid::Validator::new(
            naga::valid::ValidationFlags::all(),
            naga::valid::Capabilities::all(),
        );
        validator
            .validate(&module)
            .expect("present.wgsl must validate");
    }
}
