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
//! ## Why a baked bitmap-font atlas (no new deps)
//!
//! The lightest viable glyph approach: a **5×7 bitmap font baked into the binary as a `const`
//! table** ([`glyph::ROWS`]). Each visible glyph emits one tiny alpha-blended quad **per lit cell**
//! (the shader paints solid cells, [`fog`](crate)-style). No texture atlas to upload, no sampler,
//! no image-decode, and crucially **no new crate** — `wgpu_text`/`glyphon`/`ab_glyph` would each
//! drag in font-rasterization + atlas-management dependencies for what the UI needs here, which is
//! short uppercase labels and integer counts (button names, kill/territory/resource numbers, radial
//! action names). A cell-quad font is coarse but perfectly legible at HUD sizes and keeps the
//! render crate's dependency surface exactly where it is (`wgpu` + `bytemuck`).
//!
//! If a future screen needs proportional/anti-aliased body text, swapping the cell emitter for an
//! atlas-sampling shader is a localized change behind the same [`TextRenderer::queue`] API.
//!
//! ## The pure seam
//!
//! All layout/measure math — glyph advance, line width, anchor positioning, and the cell → NDC
//! expansion — lives in free fns ([`measure`], [`layout_cells`]) so it is unit-testable without a
//! GPU, exactly the `overlay_quads` / `marker_for` pattern. [`TextRenderer::render`] is the only
//! GPU-touching code and is exercised by the offscreen `viz-runner`, not the no-GPU CI matrix.

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

/// The bitmap font: a 5-wide × 7-tall cell grid per glyph, bit-packed one `u8` per row (the low 5
/// bits, MSB = leftmost column). Only the glyphs the in-match UI needs are baked: A–Z, 0–9, and a
/// little punctuation (space, `:`, `/`, `-`, `.`, `%`, `+`). Lowercase maps to uppercase (the UI is
/// all-caps), and any unknown glyph renders as blank (advancing like a space) so a host can never
/// panic the renderer with an odd character.
pub mod glyph {
    /// Cell grid dimensions (columns × rows) of one glyph.
    pub const COLS: usize = 5;
    pub const ROWS_PER_GLYPH: usize = 7;

    /// One bit-packed glyph: 7 rows, low 5 bits each (MSB = leftmost column, bit set = lit cell).
    pub type Bitmap = [u8; ROWS_PER_GLYPH];

    /// A blank glyph (space / unknown) — advances but lights no cell.
    pub const BLANK: Bitmap = [0; ROWS_PER_GLYPH];

    /// Look up the bitmap for a character. Lowercase folds to uppercase; unknown → [`BLANK`].
    pub fn bitmap(c: char) -> Bitmap {
        let c = c.to_ascii_uppercase();
        match c {
            ' ' => BLANK,
            'A' => [0b01110, 0b10001, 0b10001, 0b11111, 0b10001, 0b10001, 0b10001],
            'B' => [0b11110, 0b10001, 0b10001, 0b11110, 0b10001, 0b10001, 0b11110],
            'C' => [0b01110, 0b10001, 0b10000, 0b10000, 0b10000, 0b10001, 0b01110],
            'D' => [0b11100, 0b10010, 0b10001, 0b10001, 0b10001, 0b10010, 0b11100],
            'E' => [0b11111, 0b10000, 0b10000, 0b11110, 0b10000, 0b10000, 0b11111],
            'F' => [0b11111, 0b10000, 0b10000, 0b11110, 0b10000, 0b10000, 0b10000],
            'G' => [0b01110, 0b10001, 0b10000, 0b10111, 0b10001, 0b10001, 0b01111],
            'H' => [0b10001, 0b10001, 0b10001, 0b11111, 0b10001, 0b10001, 0b10001],
            'I' => [0b01110, 0b00100, 0b00100, 0b00100, 0b00100, 0b00100, 0b01110],
            'J' => [0b00111, 0b00010, 0b00010, 0b00010, 0b00010, 0b10010, 0b01100],
            'K' => [0b10001, 0b10010, 0b10100, 0b11000, 0b10100, 0b10010, 0b10001],
            'L' => [0b10000, 0b10000, 0b10000, 0b10000, 0b10000, 0b10000, 0b11111],
            'M' => [0b10001, 0b11011, 0b10101, 0b10101, 0b10001, 0b10001, 0b10001],
            'N' => [0b10001, 0b10001, 0b11001, 0b10101, 0b10011, 0b10001, 0b10001],
            'O' => [0b01110, 0b10001, 0b10001, 0b10001, 0b10001, 0b10001, 0b01110],
            'P' => [0b11110, 0b10001, 0b10001, 0b11110, 0b10000, 0b10000, 0b10000],
            'Q' => [0b01110, 0b10001, 0b10001, 0b10001, 0b10101, 0b10010, 0b01101],
            'R' => [0b11110, 0b10001, 0b10001, 0b11110, 0b10100, 0b10010, 0b10001],
            'S' => [0b01111, 0b10000, 0b10000, 0b01110, 0b00001, 0b00001, 0b11110],
            'T' => [0b11111, 0b00100, 0b00100, 0b00100, 0b00100, 0b00100, 0b00100],
            'U' => [0b10001, 0b10001, 0b10001, 0b10001, 0b10001, 0b10001, 0b01110],
            'V' => [0b10001, 0b10001, 0b10001, 0b10001, 0b10001, 0b01010, 0b00100],
            'W' => [0b10001, 0b10001, 0b10001, 0b10101, 0b10101, 0b11011, 0b10001],
            'X' => [0b10001, 0b10001, 0b01010, 0b00100, 0b01010, 0b10001, 0b10001],
            'Y' => [0b10001, 0b10001, 0b01010, 0b00100, 0b00100, 0b00100, 0b00100],
            'Z' => [0b11111, 0b00001, 0b00010, 0b00100, 0b01000, 0b10000, 0b11111],
            '0' => [0b01110, 0b10001, 0b10011, 0b10101, 0b11001, 0b10001, 0b01110],
            '1' => [0b00100, 0b01100, 0b00100, 0b00100, 0b00100, 0b00100, 0b01110],
            '2' => [0b01110, 0b10001, 0b00001, 0b00110, 0b01000, 0b10000, 0b11111],
            '3' => [0b11111, 0b00010, 0b00100, 0b00010, 0b00001, 0b10001, 0b01110],
            '4' => [0b00010, 0b00110, 0b01010, 0b10010, 0b11111, 0b00010, 0b00010],
            '5' => [0b11111, 0b10000, 0b11110, 0b00001, 0b00001, 0b10001, 0b01110],
            '6' => [0b00110, 0b01000, 0b10000, 0b11110, 0b10001, 0b10001, 0b01110],
            '7' => [0b11111, 0b00001, 0b00010, 0b00100, 0b01000, 0b01000, 0b01000],
            '8' => [0b01110, 0b10001, 0b10001, 0b01110, 0b10001, 0b10001, 0b01110],
            '9' => [0b01110, 0b10001, 0b10001, 0b01111, 0b00001, 0b00010, 0b01100],
            ':' => [0b00000, 0b00100, 0b00100, 0b00000, 0b00100, 0b00100, 0b00000],
            '/' => [0b00001, 0b00010, 0b00010, 0b00100, 0b01000, 0b01000, 0b10000],
            '-' => [0b00000, 0b00000, 0b00000, 0b11111, 0b00000, 0b00000, 0b00000],
            '.' => [0b00000, 0b00000, 0b00000, 0b00000, 0b00000, 0b00100, 0b00100],
            '%' => [0b11001, 0b11010, 0b00010, 0b00100, 0b01000, 0b01011, 0b10011],
            '+' => [0b00000, 0b00100, 0b00100, 0b11111, 0b00100, 0b00100, 0b00000],
            _ => BLANK,
        }
    }

    /// Whether `(col, row)` is a lit cell in `bitmap`. `col` is 0 (left) .. `COLS`, `row` is 0 (top)
    /// .. `ROWS_PER_GLYPH`.
    #[inline]
    pub fn is_lit(bitmap: &Bitmap, col: usize, row: usize) -> bool {
        if col >= COLS || row >= ROWS_PER_GLYPH {
            return false;
        }
        // MSB of the low 5 bits is the leftmost column.
        let mask = 1u8 << (COLS - 1 - col);
        bitmap[row] & mask != 0
    }
}

/// One blank column of spacing between adjacent glyphs, expressed in cells. The glyph is [`COLS`]
/// (5) cells wide, so each character advances 6 cells. Named so the measure math and the cell
/// layout agree by construction.
const GLYPH_SPACING_CELLS: usize = 1;

/// Total cell advance of one character (glyph width + inter-glyph gap).
const ADVANCE_CELLS: usize = glyph::COLS + GLYPH_SPACING_CELLS;

/// The size in NDC of a single bitmap cell for a string drawn at `px_size` (cell height = the glyph
/// height divided across [`ROWS_PER_GLYPH`] rows). Pure — no GPU. Returned as `(cell_w, cell_h)`;
/// cells are square so the two are equal, but both are returned for call-site clarity.
pub fn cell_size(px_size: f32) -> (f32, f32) {
    let cell = px_size / glyph::ROWS_PER_GLYPH as f32;
    (cell, cell)
}

/// The NDC bounding-box size `(width, height)` of `text` rendered at `px_size`. Width counts a full
/// [`ADVANCE_CELLS`] per character but trims the trailing inter-glyph gap so the box hugs the last
/// glyph; height is exactly `px_size`. An empty string measures to `(0, 0)`. Pure (no GPU) — the
/// testable measure seam.
pub fn measure(text: &str, px_size: f32) -> (f32, f32) {
    let n = text.chars().count();
    if n == 0 {
        return (0.0, 0.0);
    }
    let (cell_w, _) = cell_size(px_size);
    // n glyphs each ADVANCE_CELLS wide, minus the trailing gap (no glyph follows the last).
    let width_cells = n * ADVANCE_CELLS - GLYPH_SPACING_CELLS;
    (width_cells as f32 * cell_w, px_size)
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

/// One lit cell, expanded to an NDC quad ready to upload. `repr(C)` + `Pod` so it streams straight
/// into the per-instance vertex buffer; the field order MUST match `text.wgsl`'s instance attributes
/// and the `vertex_attr_array` in [`TextRenderer::new`].
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, bytemuck::Pod, bytemuck::Zeroable)]
pub struct CellInstance {
    /// Cell center in NDC.
    pub cx: f32,
    pub cy: f32,
    /// Cell half-extent in NDC (square cells).
    pub hw: f32,
    pub hh: f32,
    pub r: f32,
    pub g: f32,
    pub b: f32,
    pub alpha: f32,
}

/// Expand one [`TextItem`] into its lit-cell NDC quads. Pure (no GPU, no sim) — the testable layout
/// seam. Glyphs lay out left-to-right from the anchored top-left corner; each character advances
/// [`ADVANCE_CELLS`] cells; each lit bitmap cell becomes one [`CellInstance`]. Unknown characters
/// and spaces light no cell but still advance, so spacing is stable. An empty string yields no
/// cells.
pub fn layout_cells(item: &TextItem) -> Vec<CellInstance> {
    let size = measure(&item.text, item.px_size);
    if size.0 <= 0.0 {
        return Vec::new();
    }
    let (cell_w, cell_h) = cell_size(item.px_size);
    let [ox, oy] = anchor_top_left(item.pos, size, item.anchor);
    let [r, g, b] = item.color;

    let mut out = Vec::new();
    for (gi, ch) in item.text.chars().enumerate() {
        let bitmap = glyph::bitmap(ch);
        // Left edge of this glyph's cell box, in NDC (+x right).
        let glyph_x = ox + (gi * ADVANCE_CELLS) as f32 * cell_w;
        for row in 0..glyph::ROWS_PER_GLYPH {
            for col in 0..glyph::COLS {
                if !glyph::is_lit(&bitmap, col, row) {
                    continue;
                }
                // Cell center: column steps +x from the glyph's left edge; row steps -y from the
                // box top (row 0 is the top of the glyph).
                let cx = glyph_x + (col as f32 + 0.5) * cell_w;
                let cy = oy - (row as f32 + 0.5) * cell_h;
                out.push(CellInstance {
                    cx,
                    cy,
                    hw: cell_w * 0.5,
                    hh: cell_h * 0.5,
                    r,
                    g,
                    b,
                    alpha: item.alpha,
                });
            }
        }
    }
    out
}

/// A unit-quad corner in [-1, 1]^2 (the shader scales it by the per-cell half-size).
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

/// Screen-space text renderer. Owns its own pipeline + buffers (separate from the unit/HUD/overlay/
/// radial passes so they never contend for a shader). Alpha-blended LOAD pass: composites over the
/// already-rendered frame.
///
/// Usage: [`queue`](TextRenderer::queue) one or more strings during a frame, then call
/// [`render`](TextRenderer::render) once to flush them all in a single LOAD pass. The queue is
/// drained by `render`, so the next frame starts empty.
pub struct TextRenderer {
    pipeline: wgpu::RenderPipeline,
    quad_buf: wgpu::Buffer,
    instance_buf: wgpu::Buffer,
    instance_cap: usize,
    /// Strings queued this frame (drained by [`render`](TextRenderer::render)).
    queued: Vec<TextItem>,
}

impl TextRenderer {
    /// Build the text pipeline against the swapchain `surface_format`. The `device` is borrowed
    /// (D19). Alpha blending so glyph cells composite over the frame.
    pub fn new(device: &wgpu::Device, surface_format: wgpu::TextureFormat) -> Self {
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("gonedark.text_shader"),
            source: wgpu::ShaderSource::Wgsl(include_str!("text.wgsl").into()),
        });

        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("gonedark.text_pipeline_layout"),
            bind_group_layouts: &[],
            immediate_size: 0,
        });

        let quad_layout = wgpu::VertexBufferLayout {
            array_stride: std::mem::size_of::<QuadVertex>() as u64,
            step_mode: wgpu::VertexStepMode::Vertex,
            attributes: &wgpu::vertex_attr_array![0 => Float32x2],
        };
        let instance_layout = wgpu::VertexBufferLayout {
            array_stride: std::mem::size_of::<CellInstance>() as u64,
            step_mode: wgpu::VertexStepMode::Instance,
            // 1=center(vec2), 2=half(vec2), 3=color(vec3), 4=alpha(f32).
            attributes: &wgpu::vertex_attr_array![
                1 => Float32x2,
                2 => Float32x2,
                3 => Float32x3,
                4 => Float32
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
            size: (instance_cap * std::mem::size_of::<CellInstance>()) as u64,
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        TextRenderer {
            pipeline,
            quad_buf,
            instance_buf,
            instance_cap,
            queued: Vec::new(),
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

    /// Flush all queued strings: expand each to its lit cells, upload, and record one LOAD render
    /// pass so the text composites over the already-rendered frame. Drains the queue (so the next
    /// frame starts empty) even when nothing is drawn. No-op (beyond draining) if no cells result.
    pub fn render(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        view: &wgpu::TextureView,
    ) {
        let items = std::mem::take(&mut self.queued);
        let cells: Vec<CellInstance> = items.iter().flat_map(layout_cells).collect();
        if cells.is_empty() {
            return;
        }

        if cells.len() > self.instance_cap {
            let new_cap = cells.len().next_power_of_two();
            self.instance_buf = device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("gonedark.text_instance_vbo"),
                size: (new_cap * std::mem::size_of::<CellInstance>()) as u64,
                usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });
            self.instance_cap = new_cap;
        }
        queue.write_buffer(&self.instance_buf, 0, bytemuck::cast_slice(&cells));

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
            pass.set_vertex_buffer(0, self.quad_buf.slice(..));
            pass.set_vertex_buffer(1, self.instance_buf.slice(..));
            pass.draw(0..QUAD_VERTS.len() as u32, 0..cells.len() as u32);
        }
        queue.submit(std::iter::once(encoder.finish()));
    }
}

#[cfg(test)]
mod tests {
    //! `render` is the float boundary, so f32 layout math is fair game. `TextRenderer::new` needs a
    //! real `wgpu::Device` (no display in CI), so the pipeline path is untested; the testable
    //! layout/measure math is factored into [`measure`], [`anchor_top_left`], and [`layout_cells`].

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

    // ---- glyph table ----

    #[test]
    fn lowercase_folds_to_uppercase() {
        assert_eq!(glyph::bitmap('a'), glyph::bitmap('A'));
        assert_eq!(glyph::bitmap('z'), glyph::bitmap('Z'));
    }

    #[test]
    fn space_and_unknown_are_blank() {
        assert_eq!(glyph::bitmap(' '), glyph::BLANK);
        assert_eq!(glyph::bitmap('~'), glyph::BLANK);
        assert_eq!(glyph::bitmap('\u{1F600}'), glyph::BLANK);
    }

    #[test]
    fn known_glyphs_light_some_cells() {
        // Every printable label glyph must have at least one lit cell (else it's invisible).
        for c in "ABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789:/-.%+".chars() {
            let bm = glyph::bitmap(c);
            let lit: usize = (0..glyph::ROWS_PER_GLYPH)
                .flat_map(|row| (0..glyph::COLS).map(move |col| (col, row)))
                .filter(|&(col, row)| glyph::is_lit(&bm, col, row))
                .count();
            assert!(lit > 0, "glyph {c:?} lights no cells");
        }
    }

    #[test]
    fn is_lit_reads_left_to_right_msb_first() {
        // Row pattern 0b10000 lights only the leftmost column (col 0), nothing else.
        let bm: glyph::Bitmap = [0b10000, 0, 0, 0, 0, 0, 0];
        assert!(glyph::is_lit(&bm, 0, 0), "col 0 (leftmost) is lit");
        for col in 1..glyph::COLS {
            assert!(!glyph::is_lit(&bm, col, 0), "col {col} is dark");
        }
        // 0b00001 lights only the rightmost column (col 4).
        let bm2: glyph::Bitmap = [0b00001, 0, 0, 0, 0, 0, 0];
        assert!(glyph::is_lit(&bm2, glyph::COLS - 1, 0), "rightmost lit");
        assert!(!glyph::is_lit(&bm2, 0, 0), "leftmost dark");
    }

    #[test]
    fn is_lit_out_of_range_is_false() {
        let bm: glyph::Bitmap = [0b11111; 7];
        assert!(!glyph::is_lit(&bm, glyph::COLS, 0), "col past width");
        assert!(!glyph::is_lit(&bm, 0, glyph::ROWS_PER_GLYPH), "row past height");
    }

    // ---- measure ----

    #[test]
    fn empty_string_measures_zero() {
        assert_eq!(measure("", 0.07), (0.0, 0.0));
    }

    #[test]
    fn height_equals_px_size() {
        let (_, h) = measure("HELLO", 0.07);
        assert!((h - 0.07).abs() < EPS);
    }

    #[test]
    fn single_glyph_width_is_glyph_cols() {
        // One glyph: COLS cells wide (no trailing inter-glyph gap).
        let (cell_w, _) = cell_size(0.07);
        let (w, _) = measure("A", 0.07);
        assert!((w - glyph::COLS as f32 * cell_w).abs() < EPS);
    }

    #[test]
    fn width_grows_by_advance_per_extra_glyph() {
        let (cell_w, _) = cell_size(0.07);
        let (w1, _) = measure("A", 0.07);
        let (w2, _) = measure("AB", 0.07);
        // Adding one glyph adds exactly ADVANCE_CELLS cells of width.
        assert!((w2 - w1 - ADVANCE_CELLS as f32 * cell_w).abs() < EPS);
    }

    #[test]
    fn measure_scales_linearly_with_px_size() {
        let (w1, h1) = measure("SCORE", 0.04);
        let (w2, h2) = measure("SCORE", 0.08);
        assert!((w2 - 2.0 * w1).abs() < EPS, "double size → double width");
        assert!((h2 - 2.0 * h1).abs() < EPS, "double size → double height");
    }

    // ---- anchoring ----

    #[test]
    fn top_left_anchor_is_identity() {
        let size = measure("HI", 0.07);
        assert_eq!(anchor_top_left([0.2, 0.3], size, Anchor::TopLeft), [0.2, 0.3]);
    }

    #[test]
    fn top_center_centers_horizontally_keeps_top() {
        let size = measure("HI", 0.07);
        let tl = anchor_top_left([0.0, 0.5], size, Anchor::TopCenter);
        assert!((tl[0] + size.0 * 0.5).abs() < EPS, "left edge is -w/2 from center");
        assert!((tl[1] - 0.5).abs() < EPS, "top y unchanged");
    }

    #[test]
    fn center_anchor_box_straddles_pos() {
        let size = measure("HI", 0.07);
        let tl = anchor_top_left([0.0, 0.0], size, Anchor::Center);
        // Top-left is up-and-left of center by half the box.
        assert!((tl[0] + size.0 * 0.5).abs() < EPS);
        assert!((tl[1] - size.1 * 0.5).abs() < EPS);
        // The box center is exactly pos: top-left x + w/2 == 0, top-left y - h/2 == 0.
        assert!((tl[0] + size.0 * 0.5).abs() < EPS);
        assert!((tl[1] - size.1 * 0.5).abs() < EPS);
    }

    #[test]
    fn bottom_center_puts_pos_at_baseline_center() {
        let size = measure("HI", 0.07);
        let tl = anchor_top_left([0.1, -0.4], size, Anchor::BottomCenter);
        // Bottom edge (top y - h) is at pos.y, centered horizontally.
        assert!((tl[0] + size.0 * 0.5 - 0.1).abs() < EPS);
        assert!((tl[1] - size.1 - (-0.4)).abs() < EPS, "bottom edge at pos.y");
    }

    // ---- layout_cells ----

    #[test]
    fn empty_string_lays_out_no_cells() {
        assert!(layout_cells(&item("", [0.0, 0.0], 0.07, Anchor::TopLeft)).is_empty());
    }

    #[test]
    fn whitespace_only_lays_out_no_cells() {
        assert!(layout_cells(&item("   ", [0.0, 0.0], 0.07, Anchor::TopLeft)).is_empty());
    }

    #[test]
    fn cell_count_matches_lit_bitmap_cells() {
        // "1" lights a known number of cells; the layout emits exactly that many instances.
        let bm = glyph::bitmap('1');
        let expected: usize = (0..glyph::ROWS_PER_GLYPH)
            .flat_map(|row| (0..glyph::COLS).map(move |col| (col, row)))
            .filter(|&(col, row)| glyph::is_lit(&bm, col, row))
            .count();
        let cells = layout_cells(&item("1", [0.0, 0.0], 0.07, Anchor::TopLeft));
        assert_eq!(cells.len(), expected);
    }

    #[test]
    fn space_in_middle_advances_without_cells() {
        // "A A" emits the same cells as "AA" would for the two A's, but the second A is shifted one
        // extra ADVANCE further right (the space). So total cell count == 2 * cells('A').
        let a_cells = layout_cells(&item("A", [0.0, 0.0], 0.07, Anchor::TopLeft)).len();
        let two = layout_cells(&item("A A", [0.0, 0.0], 0.07, Anchor::TopLeft));
        assert_eq!(two.len(), 2 * a_cells, "space lights nothing but advances");
    }

    #[test]
    fn cells_carry_item_color_and_alpha() {
        let mut it = item("8", [0.0, 0.0], 0.07, Anchor::TopLeft);
        it.color = [0.2, 0.4, 0.6];
        it.alpha = 0.5;
        let cells = layout_cells(&it);
        assert!(!cells.is_empty());
        for c in &cells {
            assert_eq!([c.r, c.g, c.b], [0.2, 0.4, 0.6]);
            assert!((c.alpha - 0.5).abs() < EPS);
        }
    }

    #[test]
    fn cells_stay_within_the_measured_box() {
        // Every lit cell's quad lies inside the string's anchored bounding box (a layout sanity
        // bound the host relies on when it anchors text to a button rect).
        let it = item("SCORE", [0.0, 0.0], 0.07, Anchor::Center);
        let size = measure(&it.text, it.px_size);
        let [ox, oy] = anchor_top_left(it.pos, size, it.anchor);
        let cells = layout_cells(&it);
        assert!(!cells.is_empty());
        for c in &cells {
            // The box spans [ox, ox+w] in x and [oy-h, oy] in y. Each cell's extent must fit.
            assert!(c.cx - c.hw >= ox - EPS, "cell left within box");
            assert!(c.cx + c.hw <= ox + size.0 + EPS, "cell right within box");
            assert!(c.cy + c.hh <= oy + EPS, "cell top within box");
            assert!(c.cy - c.hh >= oy - size.1 - EPS, "cell bottom within box");
        }
    }

    #[test]
    fn cells_are_screen_space_only() {
        // Fairness guard (invariant #6): text quads are NDC chrome, never world positions.
        let it = item("KILLS: 42", [0.0, 0.0], 0.06, Anchor::Center);
        for c in layout_cells(&it) {
            assert!(c.cx.is_finite() && c.cy.is_finite());
            assert!(c.cx >= -1.5 && c.cx <= 1.5, "cx in NDC range");
            assert!(c.cy >= -1.5 && c.cy <= 1.5, "cy in NDC range");
        }
    }

    #[test]
    fn first_glyph_top_left_cell_sits_at_anchor_corner() {
        // The leftmost-topmost lit cell of the first glyph aligns to the anchored top-left corner.
        let it = item("E", [0.0, 0.0], 0.07, Anchor::TopLeft); // 'E' lights its top-left cell
        let (cell_w, cell_h) = cell_size(it.px_size);
        let cells = layout_cells(&it);
        // 'E' row 0 col 0 is lit → first cell center is half a cell in from the top-left corner.
        let top_left = cells
            .iter()
            .min_by(|a, b| {
                // smallest cy first (topmost), then smallest cx (leftmost)
                b.cy.partial_cmp(&a.cy)
                    .unwrap()
                    .then(a.cx.partial_cmp(&b.cx).unwrap())
            })
            .unwrap();
        assert!((top_left.cx - cell_w * 0.5).abs() < EPS, "first cell half-cell from left");
        assert!((top_left.cy - (-cell_h * 0.5)).abs() < EPS, "first cell half-cell down from top");
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
