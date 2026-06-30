//! Screen-space **icon** pass — the command-view glyph renderer that draws small tactical icons
//! beside the otherwise text-only command-bar / readout labels, so the RTS chrome reads as
//! *designed* rather than debug. It is the visual sibling of [`text`](crate::text): same fixed-cell
//! atlas-sampled instanced-quad pipeline, same lazy-upload-on-first-render shape, same pure layout
//! seam — only the atlas is a small set of **icons** (an RGBA8 texture) instead of font glyphs.
//!
//! Like [`text`](crate::text) this is a screen-space LOAD pass (it composites over the already-
//! rendered frame, never clears) and a **pure presentation derivation** — it reads only the icons
//! the host/other passes queue and emits NDC quads. It owns its own tiny pipeline + shader
//! (`icon.wgsl`) so it never contends with the unit/HUD/overlay/text passes for a shader source.
//! Float side of invariant #4: every number here is already an `f32` (the Q16.16 → f32 hop happened
//! in `core` callers, never in `core`). Icons carry NO world position (invariant #6: chrome, not
//! intel).
//!
//! ## A CC0 icon atlas (decisions.md D41/D46)
//!
//! The icons are baked by `tools/icons/gen_icons.py` from CLI-authored SVG sources via **Inkscape**
//! (SVG → PNG) + ImageMagick (montage → raw bytes), exactly the script-not-binary method D41 uses
//! for meshes and D74 for the font. The atlas ships as raw **straight-alpha RGBA8** bytes
//! (`assets/icons/icons_atlas.rgba`) `include_bytes!`d straight in — so the render crate stays
//! `wgpu` + `bytemuck` only (**no** png-decode / image crate). Each icon emits **one** quad that
//! samples its cell from the atlas; the white shape is tinted per draw (the theme palette), so a
//! resources icon can glow amber and a unit-type icon take the faction blue.
//!
//! The `ICON_*` consts below are the **contract with the generator** — they MUST match the `grid`
//! block in `assets/icons/manifest.json`, and [`IconKind`]'s order MUST match the `icons` list there
//! (each icon's atlas index is its position in that list). The [`atlas_matches_metrics`](tests) test
//! pins the `include_bytes!`d blob's length to `ATLAS_W * ATLAS_H * 4`, so a generator/metrics drift
//! fails `cargo test` rather than corrupting icons at runtime.
//!
//! ## The pure seam
//!
//! All layout math — atlas-UV lookup and the icon → NDC + aspect-corrected half-extent expansion —
//! lives in free fns ([`icon_uv`], [`half_extents`], [`expand`]) so it is unit-testable without a
//! GPU, exactly the `layout_glyphs` / `overlay_quads` pattern. [`IconRenderer::render`] is the only
//! GPU-touching code and is exercised by the offscreen `viz-runner`, not the no-GPU CI matrix.

use wgpu::util::DeviceExt;

/// One tactical icon in the atlas. The discriminant is the icon's **atlas index** (row-major over
/// `ICON_COLS`), so it MUST match the `icons` list order in `tools/icons/gen_icons.py` /
/// `assets/icons/manifest.json`. Adding an icon means: append it there, regenerate, and add the
/// variant here with the next index.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
#[repr(u32)]
pub enum IconKind {
    /// A foot-soldier token — the Rifleman / infantry train button.
    Infantry = 0,
    /// A tank token — the Heavy / armor train button.
    Armor = 1,
    /// A hammer — build / construct.
    Build = 2,
    /// A double chevron — upgrade a tier.
    Upgrade = 3,
    /// A credits crystal — banked resources.
    Resources = 4,
    /// A flag — mission objective / control point.
    Objective = 5,
    /// An arrow — the move order.
    Move = 6,
    /// A crosshair — the attack order.
    Attack = 7,
    /// A shield — the hold-position stance.
    Hold = 8,
}

impl IconKind {
    /// The icon's zero-based atlas index (its discriminant).
    pub fn index(self) -> u32 {
        self as u32
    }
}

/// An icon the host/other passes queued to be drawn this frame. `pos` is the cell **center** in NDC
/// ([-1,1], +y up); `size` is the cell height in NDC units (the icon is square in *pixels*, so its
/// NDC width is aspect-corrected at layout time). `tint` is straight RGB; `alpha` composites it over
/// the frame. Carries NO world position (invariant #6: icons are chrome, not intel).
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct IconItem {
    pub kind: IconKind,
    pub pos: [f32; 2],
    pub size: f32,
    pub tint: [f32; 3],
    pub alpha: f32,
}

// ---- Icon-atlas metrics — the contract with tools/icons/gen_icons.py ----------------------------
//
// A fixed-cell square grid: icon `index` lives at index `index`, laid out row-major across
// `ICON_COLS` columns. These MUST match the `grid` block in `assets/icons/manifest.json`.

/// Number of icons in the atlas (must match [`IconKind`]'s variant count).
pub const ICON_COUNT: u32 = 9;
/// Atlas grid columns / rows.
pub const ICON_COLS: u32 = 4;
pub const ICON_ROWS: u32 = 3; // ceil(9 / 4)
/// One cell's pixel size in the atlas (square).
pub const CELL: u32 = 64;
/// Full atlas pixel size.
pub const ATLAS_W: u32 = ICON_COLS * CELL; // 256
pub const ATLAS_H: u32 = ICON_ROWS * CELL; // 192

/// The baked straight-alpha RGBA8 atlas (raw `ATLAS_W * ATLAS_H * 4` bytes). Raw (not PNG) so the
/// render crate needs no image-decode dependency.
const ATLAS_BYTES: &[u8] = include_bytes!("../../assets/icons/icons_atlas.rgba");

/// The atlas UV rect (origin + size) of the icon at `index`. Pure — the cell grid math, unit-tested.
pub fn icon_uv(index: u32) -> ([f32; 2], [f32; 2]) {
    let col = index % ICON_COLS;
    let row = index / ICON_COLS;
    let du = 1.0 / ICON_COLS as f32;
    let dv = 1.0 / ICON_ROWS as f32;
    ([col as f32 * du, row as f32 * dv], [du, dv])
}

/// The NDC half-extents `(hw, hh)` of an icon drawn at height `size` on a viewport of the given
/// `aspect` (width / height). The icon is **square in pixels**, so the cell height is `size` and the
/// width is `size / aspect` — keeping its true 1:1 proportion on a wide window instead of stretching
/// (the same fix `text::cell_size` applies). Pure — no GPU. Returned as half-sizes (`size * 0.5`).
pub fn half_extents(size: f32, aspect: f32) -> (f32, f32) {
    let hh = size * 0.5;
    // Guard a degenerate aspect (zero-height surface mid-resize) so we never divide by ~0.
    let a = if aspect.abs() < 1e-6 { 1.0 } else { aspect };
    (hh / a, hh)
}

/// One icon, expanded to an NDC quad + its atlas-UV rect, ready to upload. `repr(C)` + `Pod` so it
/// streams straight into the per-instance vertex buffer; the field order MUST match `icon.wgsl`'s
/// instance attributes and the `vertex_attr_array` in [`IconRenderer::new`]. Identical layout to
/// `text::GlyphInstance` (the two pipelines are siblings).
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, bytemuck::Pod, bytemuck::Zeroable)]
pub struct IconInstance {
    /// Cell center in NDC.
    pub cx: f32,
    pub cy: f32,
    /// Cell half-extent in NDC.
    pub hw: f32,
    pub hh: f32,
    /// Atlas UV of the cell's top-left corner ([0,1]).
    pub u0: f32,
    pub v0: f32,
    /// Atlas UV size of one cell ([0,1]).
    pub du: f32,
    pub dv: f32,
    pub r: f32,
    pub g: f32,
    pub b: f32,
    pub alpha: f32,
}

/// Expand one [`IconItem`] into its NDC quad on a viewport of the given `aspect` (width / height).
/// Pure (no GPU, no sim) — the testable layout seam. A non-positive `size` yields `None` (nothing to
/// draw); otherwise the icon becomes one [`IconInstance`] centred on `pos`, aspect-corrected to stay
/// square in pixels, carrying its atlas-UV rect + tint + alpha.
pub fn expand(item: &IconItem, aspect: f32) -> Option<IconInstance> {
    // Skip anything with no positive height to draw — this also rejects a NaN size (which would
    // otherwise slip past a bare `<= 0.0`).
    if !item.size.is_finite() || item.size <= 0.0 {
        return None;
    }
    let (hw, hh) = half_extents(item.size, aspect);
    let ([u0, v0], [du, dv]) = icon_uv(item.kind.index());
    let [r, g, b] = item.tint;
    Some(IconInstance {
        cx: item.pos[0],
        cy: item.pos[1],
        hw,
        hh,
        u0,
        v0,
        du,
        dv,
        r,
        g,
        b,
        alpha: item.alpha,
    })
}

/// A unit-quad corner in [-1, 1]^2 (the shader scales it by the per-icon half-size).
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

const INITIAL_CAP: usize = 64;

/// Screen-space icon renderer. Owns its own pipeline + buffers + icon-atlas texture (separate from
/// the text/overlay passes so they never contend for a shader). Alpha-blended LOAD pass: composites
/// over the already-rendered frame.
///
/// Usage: [`queue`](IconRenderer::queue) one or more icons during a frame, then call
/// [`render`](IconRenderer::render) once to flush them all in a single LOAD pass. The queue is
/// drained by `render`, so the next frame starts empty.
pub struct IconRenderer {
    pipeline: wgpu::RenderPipeline,
    quad_buf: wgpu::Buffer,
    instance_buf: wgpu::Buffer,
    instance_cap: usize,
    /// The icon-atlas texture + sampler bind group (group 0 of the pipeline).
    atlas_bind_group: wgpu::BindGroup,
    /// The atlas texture, kept so the raw RGBA bytes can be uploaded lazily on the first
    /// [`render`](IconRenderer::render) (the construction path has only a `device`, not a `queue`).
    atlas_tex: wgpu::Texture,
    /// Whether the atlas bytes have been written to [`atlas_tex`](Self::atlas_tex) yet.
    atlas_uploaded: bool,
    /// Icons queued this frame (drained by [`render`](IconRenderer::render)).
    queued: Vec<IconItem>,
    /// Viewport aspect (width / height) used to keep icons square in pixels. Set once per frame by
    /// the host via [`set_aspect`](IconRenderer::set_aspect); defaults to `1.0` (square).
    aspect: f32,
}

impl IconRenderer {
    /// Build the icon pipeline against the swapchain `surface_format`, allocating the icon-atlas
    /// RGBA8 texture. The `device` is borrowed (D19); the atlas bytes are uploaded lazily on the
    /// first [`render`](IconRenderer::render) (the only path that has a `queue`). Alpha blending so
    /// icons composite over the frame.
    pub fn new(device: &wgpu::Device, surface_format: wgpu::TextureFormat) -> Self {
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("gonedark.icon_shader"),
            source: wgpu::ShaderSource::Wgsl(include_str!("icon.wgsl").into()),
        });

        // --- icon atlas texture (RGBA8); bytes written lazily on first render() ---
        let atlas_tex = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("gonedark.icon_atlas"),
            size: wgpu::Extent3d {
                width: ATLAS_W,
                height: ATLAS_H,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        let atlas_view = atlas_tex.create_view(&wgpu::TextureViewDescriptor::default());
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("gonedark.icon_sampler"),
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            ..Default::default()
        });
        let bind_group_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("gonedark.icon_atlas_bgl"),
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
            ],
        });
        let atlas_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("gonedark.icon_atlas_bg"),
            layout: &bind_group_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(&atlas_view),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::Sampler(&sampler),
                },
            ],
        });

        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("gonedark.icon_pipeline_layout"),
            bind_group_layouts: &[Some(&bind_group_layout)],
            immediate_size: 0,
        });

        let quad_layout = wgpu::VertexBufferLayout {
            array_stride: std::mem::size_of::<QuadVertex>() as u64,
            step_mode: wgpu::VertexStepMode::Vertex,
            attributes: &wgpu::vertex_attr_array![0 => Float32x2],
        };
        let instance_layout = wgpu::VertexBufferLayout {
            array_stride: std::mem::size_of::<IconInstance>() as u64,
            step_mode: wgpu::VertexStepMode::Instance,
            // 1=center(vec2), 2=half(vec2), 3=uv0(vec2), 4=uv_size(vec2), 5=tint(vec3), 6=alpha(f32).
            attributes: &wgpu::vertex_attr_array![
                1 => Float32x2,
                2 => Float32x2,
                3 => Float32x2,
                4 => Float32x2,
                5 => Float32x3,
                6 => Float32
            ],
        };

        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("gonedark.icon_pipeline"),
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
            label: Some("gonedark.icon_quad_vbo"),
            contents: bytemuck::cast_slice(&QUAD_VERTS),
            usage: wgpu::BufferUsages::VERTEX,
        });

        let instance_cap = INITIAL_CAP;
        let instance_buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("gonedark.icon_instance_vbo"),
            size: (instance_cap * std::mem::size_of::<IconInstance>()) as u64,
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        IconRenderer {
            pipeline,
            quad_buf,
            instance_buf,
            instance_cap,
            atlas_bind_group,
            atlas_tex,
            atlas_uploaded: false,
            queued: Vec::new(),
            aspect: 1.0,
        }
    }

    /// Upload the baked RGBA8 atlas into the texture, once. Called on the first
    /// [`render`](Self::render) (the construction path has no `queue`); a no-op thereafter.
    fn ensure_atlas_uploaded(&mut self, queue: &wgpu::Queue) {
        if self.atlas_uploaded {
            return;
        }
        queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &self.atlas_tex,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            ATLAS_BYTES,
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(ATLAS_W * 4),
                rows_per_image: Some(ATLAS_H),
            },
            wgpu::Extent3d {
                width: ATLAS_W,
                height: ATLAS_H,
                depth_or_array_layers: 1,
            },
        );
        self.atlas_uploaded = true;
    }

    /// Set the viewport aspect (width / height) for this frame's icon layout so icons stay square in
    /// pixels instead of stretching horizontally on a wide window. Cheap; the host calls it once per
    /// frame before queuing/flushing. A non-finite or ~zero value is ignored (keeps the last good
    /// aspect) so a mid-resize zero-height surface never collapses the icons.
    pub fn set_aspect(&mut self, aspect: f32) {
        if aspect.is_finite() && aspect.abs() > 1e-6 {
            self.aspect = aspect;
        }
    }

    /// Queue an icon to draw centred at `pos` (NDC) with `size` (cell height in NDC), `tint` (RGB),
    /// and `alpha`. Accumulates until the next [`render`](IconRenderer::render), which draws them all
    /// in one LOAD pass and clears the queue. The clean API other render passes / the host call.
    pub fn queue(&mut self, kind: IconKind, pos: [f32; 2], size: f32, tint: [f32; 3], alpha: f32) {
        self.queued.push(IconItem {
            kind,
            pos,
            size,
            tint,
            alpha,
        });
    }

    /// Queue a prepared [`IconItem`] directly (the form a HUD module's `*_icons` seam produces).
    pub fn queue_item(&mut self, item: IconItem) {
        self.queued.push(item);
    }

    /// The number of icons queued but not yet flushed (a host/test read; see also the unit tests).
    pub fn queued_len(&self) -> usize {
        self.queued.len()
    }

    /// Flush all queued icons: expand each to its NDC quad, upload, and record one LOAD render pass
    /// so the icons composite over the already-rendered frame. Drains the queue (so the next frame
    /// starts empty) even when nothing is drawn. No-op (beyond draining) if no quads result.
    pub fn render(&mut self, device: &wgpu::Device, queue: &wgpu::Queue, view: &wgpu::TextureView) {
        let items = std::mem::take(&mut self.queued);
        let aspect = self.aspect;
        let quads: Vec<IconInstance> = items.iter().filter_map(|it| expand(it, aspect)).collect();
        if quads.is_empty() {
            return;
        }
        self.ensure_atlas_uploaded(queue);

        if quads.len() > self.instance_cap {
            let new_cap = quads.len().next_power_of_two();
            self.instance_buf = device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("gonedark.icon_instance_vbo"),
                size: (new_cap * std::mem::size_of::<IconInstance>()) as u64,
                usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });
            self.instance_cap = new_cap;
        }
        queue.write_buffer(&self.instance_buf, 0, bytemuck::cast_slice(&quads));

        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("gonedark.icon_encoder"),
        });
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("gonedark.icon_pass"),
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
            pass.set_bind_group(0, &self.atlas_bind_group, &[]);
            pass.set_vertex_buffer(0, self.quad_buf.slice(..));
            pass.set_vertex_buffer(1, self.instance_buf.slice(..));
            pass.draw(0..QUAD_VERTS.len() as u32, 0..quads.len() as u32);
        }
        queue.submit(std::iter::once(encoder.finish()));
    }
}

#[cfg(test)]
mod tests {
    //! `render` is the float boundary, so f32 layout math is fair game. `IconRenderer::new` needs a
    //! real `wgpu::Device` (no display in CI), so the pipeline path is untested; the testable
    //! layout math is factored into [`icon_uv`], [`half_extents`], and [`expand`].

    use super::*;

    const EPS: f32 = 1e-5;

    fn item(kind: IconKind, pos: [f32; 2], size: f32) -> IconItem {
        IconItem {
            kind,
            pos,
            size,
            tint: [1.0, 1.0, 1.0],
            alpha: 1.0,
        }
    }

    // ---- atlas / metrics contract ----

    #[test]
    fn atlas_matches_metrics() {
        // The baked atlas blob length MUST equal the grid metrics × 4 (RGBA) — a guard against the
        // generator and these consts drifting (which would shear every icon's UV).
        assert!(ICON_COLS * ICON_ROWS >= ICON_COUNT, "grid holds every icon");
        assert_eq!(ATLAS_W, ICON_COLS * CELL);
        assert_eq!(ATLAS_H, ICON_ROWS * CELL);
        assert_eq!(
            ATLAS_BYTES.len(),
            (ATLAS_W * ATLAS_H * 4) as usize,
            "raw RGBA8 atlas size must match ATLAS_W*ATLAS_H*4 — regenerate with `pnpm assets:icons`"
        );
    }

    #[test]
    fn icon_kind_indices_are_dense_and_match_count() {
        // The discriminants must be 0..ICON_COUNT with no gaps (each is an atlas cell).
        let all = [
            IconKind::Infantry,
            IconKind::Armor,
            IconKind::Build,
            IconKind::Upgrade,
            IconKind::Resources,
            IconKind::Objective,
            IconKind::Move,
            IconKind::Attack,
            IconKind::Hold,
        ];
        assert_eq!(all.len() as u32, ICON_COUNT);
        for (i, k) in all.iter().enumerate() {
            assert_eq!(k.index(), i as u32, "{k:?} index must equal its slot");
        }
    }

    #[test]
    fn every_icon_uv_lies_inside_the_atlas() {
        for idx in 0..ICON_COUNT {
            let ([u0, v0], [du, dv]) = icon_uv(idx);
            assert!(
                u0 >= 0.0 && u0 + du <= 1.0 + EPS,
                "u in [0,1] for icon {idx}"
            );
            assert!(
                v0 >= 0.0 && v0 + dv <= 1.0 + EPS,
                "v in [0,1] for icon {idx}"
            );
        }
    }

    #[test]
    fn distinct_icons_get_distinct_uv_origins() {
        let a = icon_uv(IconKind::Infantry.index()).0;
        let b = icon_uv(IconKind::Armor.index()).0;
        let c = icon_uv(IconKind::Resources.index()).0; // second row
        assert_ne!(a, b);
        assert_ne!(a, c);
        assert_ne!(b, c);
    }

    #[test]
    fn second_row_icon_has_a_nonzero_v() {
        // Resources is index 4 → row 1 (zero-based), so its v origin is one cell down.
        let (_, v0_first) = (icon_uv(0).0, icon_uv(0).0[1]);
        let v0_second = icon_uv(IconKind::Resources.index()).0[1];
        let _ = v0_first;
        assert!(v0_second > 0.0, "row-1 icon sits below the top row");
        assert!((v0_second - 1.0 / ICON_ROWS as f32).abs() < EPS);
    }

    // ---- half_extents / aspect correction ----

    #[test]
    fn square_aspect_gives_equal_half_extents() {
        let (hw, hh) = half_extents(0.1, 1.0);
        assert!(
            (hw - hh).abs() < EPS,
            "square viewport → square icon in NDC"
        );
        assert!((hh - 0.05).abs() < EPS, "half-height is size/2");
    }

    #[test]
    fn icons_keep_square_proportion_in_pixels_under_aspect() {
        // On a wide viewport the NDC width is smaller than the height by exactly `aspect`, so
        // hw·aspect == hh — i.e. the icon keeps its 1:1 shape *in pixels*, never stretched.
        for aspect in [1.0_f32, 16.0 / 9.0, 21.0 / 9.0, 0.5] {
            let (hw, hh) = half_extents(0.1, aspect);
            assert!((hw * aspect - hh).abs() < EPS);
        }
    }

    #[test]
    fn width_shrinks_with_aspect_but_height_holds() {
        let (hw_sq, hh_sq) = half_extents(0.1, 1.0);
        let (hw_wide, hh_wide) = half_extents(0.1, 16.0 / 9.0);
        assert!(hw_wide < hw_sq, "icon is narrower in NDC on a wide screen");
        assert!(
            (hw_wide - hw_sq * 9.0 / 16.0).abs() < EPS,
            "width scales by 1/aspect"
        );
        assert!(
            (hh_sq - hh_wide).abs() < EPS,
            "height is aspect-independent"
        );
    }

    #[test]
    fn degenerate_aspect_falls_back_to_square() {
        let (hw, _hh) = half_extents(0.1, 0.0);
        assert!(hw.is_finite(), "zero aspect must not divide by ~0");
        assert!(hw > 0.0);
    }

    // ---- expand ----

    #[test]
    fn nonpositive_size_yields_no_quad() {
        assert!(expand(&item(IconKind::Build, [0.0, 0.0], 0.0), 1.0).is_none());
        assert!(expand(&item(IconKind::Build, [0.0, 0.0], -0.1), 1.0).is_none());
    }

    #[test]
    fn expand_centers_on_pos_and_carries_uv() {
        let inst = expand(&item(IconKind::Upgrade, [0.3, -0.4], 0.1), 1.0).unwrap();
        assert!((inst.cx - 0.3).abs() < EPS);
        assert!((inst.cy - (-0.4)).abs() < EPS);
        let ([u0, v0], [du, dv]) = icon_uv(IconKind::Upgrade.index());
        assert!((inst.u0 - u0).abs() < EPS);
        assert!((inst.v0 - v0).abs() < EPS);
        assert!((inst.du - du).abs() < EPS);
        assert!((inst.dv - dv).abs() < EPS);
    }

    #[test]
    fn expand_carries_tint_and_alpha() {
        let mut it = item(IconKind::Resources, [0.0, 0.0], 0.08);
        it.tint = [0.2, 0.4, 0.6];
        it.alpha = 0.5;
        let inst = expand(&it, 1.0).unwrap();
        assert_eq!([inst.r, inst.g, inst.b], [0.2, 0.4, 0.6]);
        assert!((inst.alpha - 0.5).abs() < EPS);
    }

    #[test]
    fn icons_are_screen_space_only() {
        // Fairness guard (invariant #6): icon quads are NDC chrome, never world positions.
        let it = item(IconKind::Attack, [0.0, 0.0], 0.06);
        let inst = expand(&it, 1.0).unwrap();
        assert!(inst.cx.is_finite() && inst.cy.is_finite());
        assert!(inst.cx >= -1.5 && inst.cx <= 1.5, "cx in NDC range");
        assert!(inst.cy >= -1.5 && inst.cy <= 1.5, "cy in NDC range");
    }

    #[test]
    fn icon_wgsl_parses_and_validates() {
        let src = include_str!("icon.wgsl");
        let module = naga::front::wgsl::parse_str(src).expect("icon.wgsl must parse");
        let mut validator = naga::valid::Validator::new(
            naga::valid::ValidationFlags::all(),
            naga::valid::Capabilities::all(),
        );
        validator
            .validate(&module)
            .expect("icon.wgsl must validate");
    }
}
