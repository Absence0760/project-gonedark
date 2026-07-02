//! Screen-space **text** pass — the reusable in-match glyph renderer the other render passes
//! (radial menu, post-match summary, command-view readouts) call to draw a string at a screen
//! position with a size and color.
//!
//! Like [`hud`](crate::hud), [`overlay`](crate::overlay), and [`radial`](crate::radial) this is a
//! screen-space LOAD pass (it composites over the already-rendered frame, never clears) and a
//! **pure presentation derivation** — it reads only the strings the host/other passes queue and
//! emits NDC quads. It owns its own tiny pipeline + shader (`text.wgsl`) so it never contends with
//! the unit/HUD/overlay/radial passes for a shader source. Float side of invariant #4: every number
//! here is already an `f32` (the Q16.16 → f32 hop happened in `core` callers, never in `core`).
//!
//! ## An anti-aliased font atlas (decisions.md D74 — supersedes the 5×7 bitmap)
//!
//! The text was originally a 5×7 bit-packed bitmap, one solid quad per lit cell — coarse,
//! uppercase-only, no punctuation, and the single biggest "this looks like a prototype" tell in the
//! HUD. It is now a **fixed-cell monospace atlas** of the full printable ASCII range (0x20..0x7E,
//! lowercase + punctuation included), baked by `tools/fonts/gen_hud_font.py` from Liberation Mono
//! Bold via ImageMagick. The atlas ships as raw R8 coverage bytes (`assets/fonts/hud_atlas.gray`)
//! `include_bytes!`d straight in — so the render crate stays `wgpu` + `bytemuck` only (**no**
//! png-decode / font-rasterisation crate). Each glyph emits **one** quad that samples its cell from
//! the atlas; anti-aliased edges blend smoothly at any HUD size, and the dependency surface is
//! unchanged.
//!
//! The `FONT_*` consts below are the **contract with the generator** — they MUST match the `grid`
//! block in `assets/fonts/manifest.json`. The [`atlas_matches_metrics`](tests) test pins the
//! `include_bytes!`d blob's length to `ATLAS_W * ATLAS_H`, so a generator/metrics drift fails
//! `cargo test` rather than corrupting glyphs at runtime.
//!
//! ## The pure seam
//!
//! All layout/measure math — glyph advance, line width, anchor positioning, and the glyph → NDC +
//! atlas-UV expansion — lives in free fns ([`measure`], [`layout_glyphs`]) so it is unit-testable
//! without a GPU, exactly the `overlay_quads` / `marker_for` pattern. [`TextRenderer::render`] is the
//! only GPU-touching code and is exercised by the offscreen `viz-runner`, not the no-GPU CI matrix.

use wgpu::util::DeviceExt;

/// Where a queued string is anchored relative to its `pos` (in NDC). The renderer positions the
/// string's bounding box so that `pos` is the named point — so a host can ask for "centered on this
/// button" without doing the width math itself.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Anchor {
    /// `pos` is the top-left corner of the string box (text grows right and down).
    TopLeft,
    /// `pos` is the horizontal center of the top edge (text is centered, top-aligned).
    TopCenter,
    /// `pos` is the exact center of the string box.
    Center,
    /// `pos` is the horizontal center of the bottom edge (text is centered, bottom-aligned).
    BottomCenter,
}

/// A string the host/other passes queued to be drawn this frame. `pos` is in NDC ([-1,1], +y up);
/// `px_size` is the glyph cell height in NDC units (a "1.0" tall glyph would span half the screen,
/// so practical label sizes are ~0.03–0.08). `color` is straight RGB; `alpha` composites it over
/// the frame. This carries NO world position (invariant #6: text is chrome, not intel).
#[derive(Clone, PartialEq, Debug)]
pub struct TextItem {
    pub text: String,
    pub pos: [f32; 2],
    pub px_size: f32,
    pub anchor: Anchor,
    pub color: [f32; 3],
    pub alpha: f32,
}

// ---- Font-atlas metrics — the contract with tools/fonts/gen_hud_font.py ------------------------
//
// A fixed-cell monospace grid: glyph `cp` lives at index `cp - FIRST_CP`, laid out row-major across
// `ATLAS_COLS` columns. These MUST match the `grid` block in `assets/fonts/manifest.json`.

/// First codepoint baked into the atlas (ASCII space).
pub const FIRST_CP: u32 = 0x20;
/// Last codepoint baked into the atlas (ASCII tilde).
pub const LAST_CP: u32 = 0x7E;
/// Number of glyphs in the atlas (printable ASCII).
pub const GLYPH_COUNT: u32 = LAST_CP - FIRST_CP + 1; // 95
/// Atlas grid columns / rows.
pub const ATLAS_COLS: u32 = 16;
pub const ATLAS_ROWS: u32 = 6; // ceil(95 / 16)
/// One cell's pixel size in the atlas.
pub const CELL_W: u32 = 28;
pub const CELL_H: u32 = 44;
/// Full atlas pixel size.
pub const ATLAS_W: u32 = ATLAS_COLS * CELL_W; // 448
pub const ATLAS_H: u32 = ATLAS_ROWS * CELL_H; // 264

/// The baked R8 coverage atlas (raw `ATLAS_W * ATLAS_H` bytes, one luminance byte per texel). Raw
/// (not PNG) so the render crate needs no image-decode dependency.
const ATLAS_BYTES: &[u8] = include_bytes!("../../assets/fonts/hud_atlas.gray");

/// A glyph cell's width:height ratio — the monospace advance per character is this fraction of the
/// cell height (`px_size`). 28/44 ≈ 0.636, so strings are a touch tighter than the old 6/7 bitmap
/// advance (less panel overflow, never more).
const GLYPH_ASPECT: f32 = CELL_W as f32 / CELL_H as f32;

/// Clamp range for the physical UI scale (see [`TextRenderer::set_ui_scale`]). Below ~0.5 chrome
/// becomes illegible; above ~3.0 it swamps the frame. Matches `icon`'s clamp and the touch-layout
/// density clamp so every UI surface treats a bogus platform report the same way.
const UI_SCALE_MIN: f32 = 0.5;
const UI_SCALE_MAX: f32 = 3.0;

/// Map a character to its zero-based atlas glyph index, or `None` if it is not a drawable glyph
/// (out of the printable ASCII range — it still advances like a space, but emits no quad). ASCII
/// space itself maps to `None` here too: its atlas cell is blank, so skipping the quad is both
/// correct and cheaper.
pub fn glyph_index(c: char) -> Option<u32> {
    let cp = c as u32;
    if cp <= FIRST_CP || cp > LAST_CP {
        // `<= FIRST_CP` folds space (and any control char) into "advance only, no quad".
        None
    } else {
        Some(cp - FIRST_CP)
    }
}

/// The size in NDC of one glyph cell for a string drawn at `px_size` on a viewport of the given
/// `aspect` (width / height). The cell **height** is `px_size`; the **width** is `px_size *
/// GLYPH_ASPECT` divided by `aspect`, so a glyph keeps its true 28:44 proportion *in pixels* on a
/// wide window instead of stretching horizontally (the chief reason the old HUD read as amateurish).
/// Pure — no GPU. Returned as `(cell_w, cell_h)`.
pub fn cell_size(px_size: f32, aspect: f32) -> (f32, f32) {
    let cell_h = px_size;
    // Guard a degenerate aspect (zero-height surface mid-resize) so we never divide by ~0.
    let a = if aspect.abs() < 1e-6 { 1.0 } else { aspect };
    (px_size * GLYPH_ASPECT / a, cell_h)
}

/// [`cell_size`] after applying the physical `ui_scale` (logical-point-per-NDC correction; `1.0` =
/// legacy). Chrome sized to read at a constant *physical* size multiplies its NDC size by `ui_scale`,
/// so a denser display (higher PPI) draws the same label larger in NDC — hence the same physical
/// size — instead of shrinking it to a bare fraction of the raw framebuffer. This is the seam
/// [`TextRenderer::render`] applies per frame from the host-supplied scale (via
/// [`set_ui_scale`](TextRenderer::set_ui_scale)); `cell_size` is exactly the `ui_scale == 1.0` case.
/// Pure — no GPU.
pub fn cell_size_scaled(px_size: f32, aspect: f32, ui_scale: f32) -> (f32, f32) {
    cell_size(px_size * ui_scale, aspect)
}

/// The NDC bounding-box size `(width, height)` of `text` rendered at `px_size` on a viewport of the
/// given `aspect` (width / height). Monospace: width is `char_count` full cell advances (each cell
/// already carries its inter-glyph bearing), aspect-corrected (see [`cell_size`]); height is exactly
/// `px_size`. An empty string measures to `(0, 0)`. Pure (no GPU) — the testable measure seam.
pub fn measure(text: &str, px_size: f32, aspect: f32) -> (f32, f32) {
    let n = text.chars().count();
    if n == 0 {
        return (0.0, 0.0);
    }
    let (cell_w, _) = cell_size(px_size, aspect);
    (n as f32 * cell_w, px_size)
}

/// The top-left NDC corner of the string box, given its `pos`, measured size, and [`Anchor`]. Pure
/// math (no GPU). With +y up, "top" is the larger y; the box extends `+x` (right) and `-y` (down)
/// from this corner.
pub fn anchor_top_left(pos: [f32; 2], size: (f32, f32), anchor: Anchor) -> [f32; 2] {
    let (w, h) = size;
    match anchor {
        Anchor::TopLeft => pos,
        Anchor::TopCenter => [pos[0] - w * 0.5, pos[1]],
        Anchor::Center => [pos[0] - w * 0.5, pos[1] + h * 0.5],
        Anchor::BottomCenter => [pos[0] - w * 0.5, pos[1] + h],
    }
}

/// One glyph, expanded to an NDC quad + its atlas-UV rect, ready to upload. `repr(C)` + `Pod` so it
/// streams straight into the per-instance vertex buffer; the field order MUST match `text.wgsl`'s
/// instance attributes and the `vertex_attr_array` in [`TextRenderer::new`].
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, bytemuck::Pod, bytemuck::Zeroable)]
pub struct GlyphInstance {
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

/// The atlas UV rect (origin + size) of glyph `index`. Pure — the cell grid math, unit-tested.
fn glyph_uv(index: u32) -> ([f32; 2], [f32; 2]) {
    let col = index % ATLAS_COLS;
    let row = index / ATLAS_COLS;
    let du = 1.0 / ATLAS_COLS as f32;
    let dv = 1.0 / ATLAS_ROWS as f32;
    ([col as f32 * du, row as f32 * dv], [du, dv])
}

/// Expand one [`TextItem`] into its per-glyph NDC quads on a viewport of the given `aspect`
/// (width / height). Pure (no GPU, no sim) — the testable layout seam. Glyphs lay out left-to-right
/// from the anchored top-left corner; each character advances one aspect-corrected cell (see
/// [`cell_size`]); each drawable glyph becomes one [`GlyphInstance`] carrying its atlas-UV rect.
/// Spaces and out-of-range characters advance but emit no quad, so spacing is stable. An empty
/// string yields no glyphs.
pub fn layout_glyphs(item: &TextItem, aspect: f32) -> Vec<GlyphInstance> {
    let size = measure(&item.text, item.px_size, aspect);
    if size.0 <= 0.0 {
        return Vec::new();
    }
    let (cell_w, cell_h) = cell_size(item.px_size, aspect);
    let [ox, oy] = anchor_top_left(item.pos, size, item.anchor);
    let [r, g, b] = item.color;

    let mut out = Vec::new();
    for (gi, ch) in item.text.chars().enumerate() {
        let Some(index) = glyph_index(ch) else {
            continue; // space / unknown: advance only
        };
        let ([u0, v0], [du, dv]) = glyph_uv(index);
        // Cell center: column steps +x from the anchored left edge; the row is one line tall, so the
        // center sits half a cell below the box top (+y up → top is larger y).
        let cx = ox + (gi as f32 + 0.5) * cell_w;
        let cy = oy - cell_h * 0.5;
        out.push(GlyphInstance {
            cx,
            cy,
            hw: cell_w * 0.5,
            hh: cell_h * 0.5,
            u0,
            v0,
            du,
            dv,
            r,
            g,
            b,
            alpha: item.alpha,
        });
    }
    out
}

/// A unit-quad corner in [-1, 1]^2 (the shader scales it by the per-glyph half-size).
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

const INITIAL_CAP: usize = 256;

/// Screen-space text renderer. Owns its own pipeline + buffers + font-atlas texture (separate from
/// the unit/HUD/overlay/radial passes so they never contend for a shader). Alpha-blended LOAD pass:
/// composites over the already-rendered frame.
///
/// Usage: [`queue`](TextRenderer::queue) one or more strings during a frame, then call
/// [`render`](TextRenderer::render) once to flush them all in a single LOAD pass. The queue is
/// drained by `render`, so the next frame starts empty.
pub struct TextRenderer {
    pipeline: wgpu::RenderPipeline,
    quad_buf: wgpu::Buffer,
    instance_buf: wgpu::Buffer,
    instance_cap: usize,
    /// The font-atlas texture + sampler bind group (group 0 of the pipeline).
    atlas_bind_group: wgpu::BindGroup,
    /// The atlas texture, kept so the raw coverage bytes can be uploaded lazily on the first
    /// [`render`](TextRenderer::render) (the construction path has only a `device`, not a `queue`).
    atlas_tex: wgpu::Texture,
    /// Whether the atlas coverage bytes have been written to [`atlas_tex`](Self::atlas_tex) yet.
    atlas_uploaded: bool,
    /// Strings queued this frame (drained by [`render`](TextRenderer::render)).
    queued: Vec<TextItem>,
    /// Viewport aspect (width / height) used to keep glyphs square in pixels. Set once per frame by
    /// the host via [`set_aspect`](TextRenderer::set_aspect); defaults to `1.0` (square) so a caller
    /// that never sets it gets the old square-NDC behaviour.
    aspect: f32,
    /// Physical UI scale (logical-point-per-NDC correction). Set once per frame by the host via
    /// [`set_ui_scale`](TextRenderer::set_ui_scale) so chrome reads at a constant *physical* size
    /// across displays of differing density; defaults to `1.0` (legacy) so a caller that never sets
    /// it gets the old bare-fraction behaviour.
    ui_scale: f32,
}

impl TextRenderer {
    /// Build the text pipeline against the swapchain `surface_format`, allocating the font-atlas R8
    /// texture. The `device` is borrowed (D19); the atlas coverage bytes are uploaded lazily on the
    /// first [`render`](TextRenderer::render) (the only path that has a `queue`). Alpha blending so
    /// glyphs composite over the frame.
    pub fn new(device: &wgpu::Device, surface_format: wgpu::TextureFormat) -> Self {
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("gonedark.text_shader"),
            source: wgpu::ShaderSource::Wgsl(include_str!("text.wgsl").into()),
        });

        // --- font atlas texture (R8 coverage); bytes written lazily on first render() ---
        let atlas_tex = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("gonedark.text_atlas"),
            size: wgpu::Extent3d {
                width: ATLAS_W,
                height: ATLAS_H,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::R8Unorm,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        let atlas_view = atlas_tex.create_view(&wgpu::TextureViewDescriptor::default());
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("gonedark.text_sampler"),
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            ..Default::default()
        });
        let bind_group_layout =
            device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some("gonedark.text_atlas_bgl"),
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
            label: Some("gonedark.text_atlas_bg"),
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
            label: Some("gonedark.text_pipeline_layout"),
            bind_group_layouts: &[Some(&bind_group_layout)],
            immediate_size: 0,
        });

        let quad_layout = wgpu::VertexBufferLayout {
            array_stride: std::mem::size_of::<QuadVertex>() as u64,
            step_mode: wgpu::VertexStepMode::Vertex,
            attributes: &wgpu::vertex_attr_array![0 => Float32x2],
        };
        let instance_layout = wgpu::VertexBufferLayout {
            array_stride: std::mem::size_of::<GlyphInstance>() as u64,
            step_mode: wgpu::VertexStepMode::Instance,
            // 1=center(vec2), 2=half(vec2), 3=uv0(vec2), 4=uv_size(vec2), 5=color(vec3), 6=alpha(f32).
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
            label: Some("gonedark.text_pipeline"),
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
            label: Some("gonedark.text_quad_vbo"),
            contents: bytemuck::cast_slice(&QUAD_VERTS),
            usage: wgpu::BufferUsages::VERTEX,
        });

        let instance_cap = INITIAL_CAP;
        let instance_buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("gonedark.text_instance_vbo"),
            size: (instance_cap * std::mem::size_of::<GlyphInstance>()) as u64,
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        TextRenderer {
            pipeline,
            quad_buf,
            instance_buf,
            instance_cap,
            atlas_bind_group,
            atlas_tex,
            atlas_uploaded: false,
            queued: Vec::new(),
            aspect: 1.0,
            ui_scale: 1.0,
        }
    }

    /// Upload the baked R8 coverage atlas into the texture, once. Called on the first
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
                bytes_per_row: Some(ATLAS_W),
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

    /// Set the viewport aspect (width / height) for this frame's glyph layout so text stays square in
    /// pixels instead of stretching horizontally on a wide window. Cheap; the host calls it once per
    /// frame before queuing/flushing. A non-finite or ~zero value is ignored (keeps the last good
    /// aspect) so a mid-resize zero-height surface never collapses the text.
    pub fn set_aspect(&mut self, aspect: f32) {
        if aspect.is_finite() && aspect.abs() > 1e-6 {
            self.aspect = aspect;
        }
    }

    /// Set the physical UI scale (logical-point-per-NDC correction) for this frame's glyph layout, so
    /// chrome reads at a constant *physical* size across displays of differing density/PPI instead of
    /// a bare fraction of the raw framebuffer. `1.0` is the legacy behaviour; the host sources it from
    /// the platform (`winit` `Window::scale_factor()` on desktop, `densityDpi / DENSITY_DEFAULT` on
    /// Android) and calls this once per frame before queuing/flushing, mirroring
    /// [`set_aspect`](Self::set_aspect). It is applied internally in [`render`](Self::render) by
    /// scaling each queued label's NDC size (equivalently, [`cell_size_scaled`] with this scale), so
    /// no pure layout signature changes. Clamped to `[0.5, 3.0]`; a non-finite or non-positive value
    /// is ignored (keeps the last good scale) so a bogus platform report never collapses the chrome.
    pub fn set_ui_scale(&mut self, ui_scale: f32) {
        if ui_scale.is_finite() && ui_scale > 0.0 {
            self.ui_scale = ui_scale.clamp(UI_SCALE_MIN, UI_SCALE_MAX);
        }
    }

    /// Queue a string to draw at `pos` (NDC) with `px_size` (glyph cell height in NDC), `anchor`,
    /// `color` (RGB), and `alpha`. Accumulates until the next [`render`](TextRenderer::render),
    /// which draws them all in one LOAD pass and clears the queue. The clean API other render
    /// passes / the host call.
    pub fn queue(
        &mut self,
        text: impl Into<String>,
        pos: [f32; 2],
        px_size: f32,
        anchor: Anchor,
        color: [f32; 3],
        alpha: f32,
    ) {
        self.queued.push(TextItem {
            text: text.into(),
            pos,
            px_size,
            anchor,
            color,
            alpha,
        });
    }

    /// The number of strings queued but not yet flushed (a host/test read; see also the unit tests).
    pub fn queued_len(&self) -> usize {
        self.queued.len()
    }

    /// Flush all queued strings: expand each to its glyph quads, upload, and record one LOAD render
    /// pass so the text composites over the already-rendered frame. Drains the queue (so the next
    /// frame starts empty) even when nothing is drawn. No-op (beyond draining) if no glyphs result.
    pub fn render(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        view: &wgpu::TextureView,
    ) {
        let items = std::mem::take(&mut self.queued);
        let aspect = self.aspect;
        let ui_scale = self.ui_scale;
        // Apply the physical UI scale to every label's NDC size, then lay out as usual — this is the
        // constant-physical-size correction (equivalent to `cell_size_scaled(px, aspect, ui_scale)`).
        // At the default `ui_scale == 1.0` this is a no-op, so the legacy geometry is unchanged.
        let glyphs: Vec<GlyphInstance> = items
            .iter()
            .flat_map(|it| {
                let scaled = TextItem {
                    px_size: it.px_size * ui_scale,
                    ..it.clone()
                };
                layout_glyphs(&scaled, aspect)
            })
            .collect();
        if glyphs.is_empty() {
            return;
        }
        self.ensure_atlas_uploaded(queue);

        if glyphs.len() > self.instance_cap {
            let new_cap = glyphs.len().next_power_of_two();
            self.instance_buf = device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("gonedark.text_instance_vbo"),
                size: (new_cap * std::mem::size_of::<GlyphInstance>()) as u64,
                usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });
            self.instance_cap = new_cap;
        }
        queue.write_buffer(&self.instance_buf, 0, bytemuck::cast_slice(&glyphs));

        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("gonedark.text_encoder"),
        });
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("gonedark.text_pass"),
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
            pass.draw(0..QUAD_VERTS.len() as u32, 0..glyphs.len() as u32);
        }
        queue.submit(std::iter::once(encoder.finish()));
    }
}

#[cfg(test)]
mod tests {
    //! `render` is the float boundary, so f32 layout math is fair game. `TextRenderer::new` needs a
    //! real `wgpu::Device` (no display in CI), so the pipeline path is untested; the testable
    //! layout/measure math is factored into [`measure`], [`anchor_top_left`], and [`layout_glyphs`].

    use super::*;

    const EPS: f32 = 1e-5;

    fn item(text: &str, pos: [f32; 2], px: f32, anchor: Anchor) -> TextItem {
        TextItem {
            text: text.into(),
            pos,
            px_size: px,
            anchor,
            color: [1.0, 1.0, 1.0],
            alpha: 1.0,
        }
    }

    // ---- atlas / metrics contract ----

    #[test]
    fn atlas_matches_metrics() {
        // The baked atlas blob length MUST equal the grid metrics — a guard against the generator
        // and these consts drifting (which would shear every glyph's UV).
        assert_eq!(GLYPH_COUNT, LAST_CP - FIRST_CP + 1);
        assert!(ATLAS_COLS * ATLAS_ROWS >= GLYPH_COUNT, "grid holds every glyph");
        assert_eq!(ATLAS_W, ATLAS_COLS * CELL_W);
        assert_eq!(ATLAS_H, ATLAS_ROWS * CELL_H);
        assert_eq!(
            ATLAS_BYTES.len(),
            (ATLAS_W * ATLAS_H) as usize,
            "raw R8 atlas size must match ATLAS_W*ATLAS_H — regenerate with `pnpm assets:font`"
        );
    }

    #[test]
    fn glyph_index_covers_printable_ascii_and_folds_space() {
        assert_eq!(glyph_index(' '), None, "space advances but emits no quad");
        assert_eq!(glyph_index('!'), Some(1)); // 0x21 - 0x20
        assert_eq!(glyph_index('A'), Some('A' as u32 - FIRST_CP));
        assert_eq!(glyph_index('a'), Some('a' as u32 - FIRST_CP));
        assert_eq!(glyph_index('~'), Some(GLYPH_COUNT - 1));
        assert_eq!(glyph_index('\u{1F600}'), None, "out-of-range emits no quad");
        assert_eq!(glyph_index('\n'), None, "control char emits no quad");
    }

    #[test]
    fn every_glyph_uv_lies_inside_the_atlas() {
        for idx in 0..GLYPH_COUNT {
            let ([u0, v0], [du, dv]) = glyph_uv(idx);
            assert!(u0 >= 0.0 && u0 + du <= 1.0 + EPS, "u in [0,1] for glyph {idx}");
            assert!(v0 >= 0.0 && v0 + dv <= 1.0 + EPS, "v in [0,1] for glyph {idx}");
        }
    }

    #[test]
    fn distinct_glyphs_get_distinct_uv_origins() {
        let a = glyph_uv(glyph_index('A').unwrap()).0;
        let b = glyph_uv(glyph_index('B').unwrap()).0;
        assert_ne!(a, b);
    }

    // ---- measure ----

    #[test]
    fn empty_string_measures_zero() {
        assert_eq!(measure("", 0.07, 1.0), (0.0, 0.0));
    }

    #[test]
    fn height_equals_px_size() {
        let (_, h) = measure("HELLO", 0.07, 1.0);
        assert!((h - 0.07).abs() < EPS);
    }

    #[test]
    fn single_glyph_width_is_one_cell() {
        let (cell_w, _) = cell_size(0.07, 1.0);
        let (w, _) = measure("A", 0.07, 1.0);
        assert!((w - cell_w).abs() < EPS);
    }

    #[test]
    fn width_grows_by_one_cell_per_extra_glyph() {
        let (cell_w, _) = cell_size(0.07, 1.0);
        let (w1, _) = measure("A", 0.07, 1.0);
        let (w2, _) = measure("AB", 0.07, 1.0);
        assert!((w2 - w1 - cell_w).abs() < EPS, "monospace advance is one cell");
    }

    #[test]
    fn measure_scales_linearly_with_px_size() {
        let (w1, h1) = measure("SCORE", 0.04, 1.0);
        let (w2, h2) = measure("SCORE", 0.08, 1.0);
        assert!((w2 - 2.0 * w1).abs() < EPS, "double size → double width");
        assert!((h2 - 2.0 * h1).abs() < EPS, "double size → double height");
    }

    #[test]
    fn space_counts_toward_width_but_not_glyphs() {
        // Measure counts every char (monospace), but layout emits no quad for the space.
        let (w_ab, _) = measure("AB", 0.07, 1.0);
        let (w_a_b, _) = measure("A B", 0.07, 1.0);
        let (cell_w, _) = cell_size(0.07, 1.0);
        assert!((w_a_b - w_ab - cell_w).abs() < EPS, "the space is one cell of advance");
    }

    // ---- aspect correction (the fat-text-on-a-wide-window fix) ----

    #[test]
    fn glyph_cells_keep_native_proportion_in_pixels_under_aspect() {
        // On a wide viewport the cell's NDC width is smaller than px_size*GLYPH_ASPECT by exactly
        // `aspect`, so cell_w·aspect == px_size·GLYPH_ASPECT — i.e. the glyph keeps its 28:44 shape
        // *in pixels*, never stretched.
        for aspect in [1.0_f32, 16.0 / 9.0, 21.0 / 9.0, 0.5] {
            let (cw, ch) = cell_size(0.07, aspect);
            assert!((cw * aspect - ch * GLYPH_ASPECT).abs() < EPS);
        }
    }

    #[test]
    fn measure_width_shrinks_with_aspect_but_height_holds() {
        let (w_sq, h_sq) = measure("STANCE", 0.04, 1.0);
        let (w_wide, h_wide) = measure("STANCE", 0.04, 16.0 / 9.0);
        assert!(w_wide < w_sq, "string is narrower in NDC on a wide screen");
        assert!((w_wide - w_sq * 9.0 / 16.0).abs() < EPS, "width scales by 1/aspect");
        assert!((h_sq - h_wide).abs() < EPS, "height is aspect-independent");
    }

    #[test]
    fn layout_is_narrower_but_same_glyph_count_under_aspect() {
        let sq = layout_glyphs(&item("ABC", [0.0, 0.0], 0.07, Anchor::TopLeft), 1.0);
        let wide = layout_glyphs(&item("ABC", [0.0, 0.0], 0.07, Anchor::TopLeft), 16.0 / 9.0);
        assert_eq!(sq.len(), wide.len(), "same glyphs regardless of aspect");
        assert!(wide[0].hw < sq[0].hw, "glyphs are narrower on a wide screen");
        assert!((wide[0].hh - sq[0].hh).abs() < EPS, "glyph height is aspect-independent");
    }

    #[test]
    fn degenerate_aspect_falls_back_to_square() {
        let (cw, _ch) = cell_size(0.07, 0.0);
        assert!(cw.is_finite(), "zero aspect must not divide by ~0");
        assert!(cw > 0.0);
    }

    // ---- ui_scale (constant-physical-size correction) ----

    #[test]
    fn cell_size_scales_linearly_with_ui_scale() {
        // The seam `TextRenderer::render` applies: doubling ui_scale doubles the cell in both axes
        // (mirrors `measure_scales_linearly_with_px_size`, just via the physical-scale knob).
        let (w1, h1) = cell_size_scaled(0.07, 1.0, 1.0);
        let (w2, h2) = cell_size_scaled(0.07, 1.0, 2.0);
        assert!((w2 - 2.0 * w1).abs() < EPS, "2× ui_scale → 2× cell width");
        assert!((h2 - 2.0 * h1).abs() < EPS, "2× ui_scale → 2× cell height");
    }

    #[test]
    fn ui_scale_one_is_the_legacy_cell_size() {
        // The default scale must reproduce the pre-ui_scale geometry exactly, at any aspect.
        for aspect in [1.0_f32, 16.0 / 9.0, 0.5] {
            assert_eq!(cell_size_scaled(0.07, aspect, 1.0), cell_size(0.07, aspect));
        }
    }

    #[test]
    fn ui_scale_grows_every_glyph_uniformly_in_layout() {
        // The render path pre-scales each item's px_size by ui_scale; a 2× scale doubles every
        // glyph's half-extents while preserving the glyph count and NDC anchoring.
        let it = item("SCORE", [0.0, 0.0], 0.05, Anchor::TopLeft);
        let base = layout_glyphs(&it, 1.0);
        let scaled_item = TextItem {
            px_size: it.px_size * 2.0,
            ..it.clone()
        };
        let scaled = layout_glyphs(&scaled_item, 1.0);
        assert_eq!(base.len(), scaled.len(), "same glyphs regardless of ui_scale");
        for (b, s) in base.iter().zip(scaled.iter()) {
            assert!((s.hw - 2.0 * b.hw).abs() < EPS, "glyph width doubles with ui_scale");
            assert!((s.hh - 2.0 * b.hh).abs() < EPS, "glyph height doubles with ui_scale");
        }
    }

    // ---- anchoring ----

    #[test]
    fn top_left_anchor_is_identity() {
        let size = measure("HI", 0.07, 1.0);
        assert_eq!(anchor_top_left([0.2, 0.3], size, Anchor::TopLeft), [0.2, 0.3]);
    }

    #[test]
    fn top_center_centers_horizontally_keeps_top() {
        let size = measure("HI", 0.07, 1.0);
        let tl = anchor_top_left([0.0, 0.5], size, Anchor::TopCenter);
        assert!((tl[0] + size.0 * 0.5).abs() < EPS, "left edge is -w/2 from center");
        assert!((tl[1] - 0.5).abs() < EPS, "top y unchanged");
    }

    #[test]
    fn center_anchor_box_straddles_pos() {
        let size = measure("HI", 0.07, 1.0);
        let tl = anchor_top_left([0.0, 0.0], size, Anchor::Center);
        assert!((tl[0] + size.0 * 0.5).abs() < EPS);
        assert!((tl[1] - size.1 * 0.5).abs() < EPS);
    }

    #[test]
    fn bottom_center_puts_pos_at_baseline_center() {
        let size = measure("HI", 0.07, 1.0);
        let tl = anchor_top_left([0.1, -0.4], size, Anchor::BottomCenter);
        assert!((tl[0] + size.0 * 0.5 - 0.1).abs() < EPS);
        assert!((tl[1] - size.1 - (-0.4)).abs() < EPS, "bottom edge at pos.y");
    }

    // ---- layout_glyphs ----

    #[test]
    fn empty_string_lays_out_no_glyphs() {
        assert!(layout_glyphs(&item("", [0.0, 0.0], 0.07, Anchor::TopLeft), 1.0).is_empty());
    }

    #[test]
    fn whitespace_only_lays_out_no_glyphs() {
        assert!(layout_glyphs(&item("   ", [0.0, 0.0], 0.07, Anchor::TopLeft), 1.0).is_empty());
    }

    #[test]
    fn glyph_count_matches_drawable_chars() {
        // "A B" has 3 chars but only 2 drawable glyphs (the space emits nothing).
        let g = layout_glyphs(&item("A B", [0.0, 0.0], 0.07, Anchor::TopLeft), 1.0);
        assert_eq!(g.len(), 2);
    }

    #[test]
    fn space_advances_without_a_glyph() {
        // "A A" emits two glyphs, the second shifted two cells right (its own + the space).
        let g = layout_glyphs(&item("A A", [0.0, 0.0], 0.07, Anchor::TopLeft), 1.0);
        assert_eq!(g.len(), 2);
        let (cell_w, _) = cell_size(0.07, 1.0);
        assert!((g[1].cx - g[0].cx - 2.0 * cell_w).abs() < EPS, "second A is two cells over");
    }

    #[test]
    fn glyphs_carry_item_color_and_alpha() {
        let mut it = item("8", [0.0, 0.0], 0.07, Anchor::TopLeft);
        it.color = [0.2, 0.4, 0.6];
        it.alpha = 0.5;
        let g = layout_glyphs(&it, 1.0);
        assert!(!g.is_empty());
        for q in &g {
            assert_eq!([q.r, q.g, q.b], [0.2, 0.4, 0.6]);
            assert!((q.alpha - 0.5).abs() < EPS);
        }
    }

    #[test]
    fn glyphs_carry_their_atlas_uv() {
        let g = layout_glyphs(&item("A", [0.0, 0.0], 0.07, Anchor::TopLeft), 1.0);
        let ([u0, v0], [du, dv]) = glyph_uv(glyph_index('A').unwrap());
        assert!((g[0].u0 - u0).abs() < EPS);
        assert!((g[0].v0 - v0).abs() < EPS);
        assert!((g[0].du - du).abs() < EPS);
        assert!((g[0].dv - dv).abs() < EPS);
    }

    #[test]
    fn glyphs_stay_within_the_measured_box() {
        let it = item("SCORE", [0.0, 0.0], 0.07, Anchor::Center);
        let size = measure(&it.text, it.px_size, 1.0);
        let [ox, oy] = anchor_top_left(it.pos, size, it.anchor);
        let g = layout_glyphs(&it, 1.0);
        assert!(!g.is_empty());
        for q in &g {
            assert!(q.cx - q.hw >= ox - EPS, "glyph left within box");
            assert!(q.cx + q.hw <= ox + size.0 + EPS, "glyph right within box");
            assert!(q.cy + q.hh <= oy + EPS, "glyph top within box");
            assert!(q.cy - q.hh >= oy - size.1 - EPS, "glyph bottom within box");
        }
    }

    #[test]
    fn glyphs_are_screen_space_only() {
        // Fairness guard (invariant #6): text quads are NDC chrome, never world positions.
        let it = item("KILLS: 42", [0.0, 0.0], 0.06, Anchor::Center);
        for q in layout_glyphs(&it, 1.0) {
            assert!(q.cx.is_finite() && q.cy.is_finite());
            assert!(q.cx >= -1.5 && q.cx <= 1.5, "cx in NDC range");
            assert!(q.cy >= -1.5 && q.cy <= 1.5, "cy in NDC range");
        }
    }

    #[test]
    fn first_glyph_sits_at_anchor_corner() {
        let it = item("E", [0.0, 0.0], 0.07, Anchor::TopLeft);
        let (cell_w, cell_h) = cell_size(it.px_size, 1.0);
        let g = layout_glyphs(&it, 1.0);
        assert_eq!(g.len(), 1);
        assert!((g[0].cx - cell_w * 0.5).abs() < EPS, "first glyph half-cell from left");
        assert!((g[0].cy - (-cell_h * 0.5)).abs() < EPS, "first glyph half-cell down from top");
    }

    #[test]
    fn text_wgsl_parses_and_validates() {
        let src = include_str!("text.wgsl");
        let module = naga::front::wgsl::parse_str(src).expect("text.wgsl must parse");
        let mut validator = naga::valid::Validator::new(
            naga::valid::ValidationFlags::all(),
            naga::valid::Capabilities::all(),
        );
        validator.validate(&module).expect("text.wgsl must validate");
    }
}
