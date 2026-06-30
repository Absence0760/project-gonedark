//! On-screen FPS touch-control HUD renderer (the COD-style embodied controls) — a screen-space
//! overlay drawn over the dark embodied frame, **Android only**. The engine describes WHAT to draw
//! ([`TouchControlsHud`]: the floating move stick + the Fire/Crouch/Reload/Surface buttons, in
//! pixels) and this module turns it into alpha-blended quads with shader-drawn glyphs
//! (`touch_controls.wgsl`). No binary art assets — real icons are a later polish (D46 pipeline).
//!
//! Mirrors the [`hud`](crate::hud) pattern: its own pipeline + a unit-quad VBO + a per-instance
//! buffer, recorded as a LOAD pass so it composites over the frame. `render` is the float boundary
//! (invariant #1/#4): the pixel→NDC quad math is pure f32 and lives in the host-testable
//! [`build_quads`]; only the GPU plumbing needs a device.
//!
//! The engine owns the layout (`engine::touch_controls::TouchLayout`); to keep the layering clean
//! (`engine -> render`, never the reverse — invariant #2) the engine fills this crate's own
//! [`TouchControlsHud`] description, exactly as the engine fills the contextual
//! [`command_panel::CommandPanelView`](crate::command_panel::CommandPanelView).

use wgpu::util::DeviceExt;

/// Which procedural glyph a button draws (matches the `shape` ids in `touch_controls.wgsl`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TouchGlyph {
    Fire,
    Crouch,
    Reload,
    Surface,
    /// Aim-down-sight (ADS) — a sniper-scope reticle. Drawn only for a unit that has a gun-sight
    /// (the engine omits this button otherwise; the W2 turret/tank gate, wave-2 W6).
    Aim,
}

impl TouchGlyph {
    /// The shader `shape` id for this glyph (2..6; 0/1 are the stick ring/thumb).
    fn shape(self) -> f32 {
        match self {
            TouchGlyph::Fire => 2.0,
            TouchGlyph::Crouch => 3.0,
            TouchGlyph::Reload => 4.0,
            TouchGlyph::Surface => 5.0,
            TouchGlyph::Aim => 6.0,
        }
    }
}

/// One on-screen button to draw, in **pixels** (the engine passes its `TouchLayout` circles
/// straight through). `pressed` is a momentary touch-down flash; `active` is a sticky toggle-on
/// highlight (used by Crouch to show the avatar is currently crouched).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TouchButton {
    pub cx: f32,
    pub cy: f32,
    pub r: f32,
    pub glyph: TouchGlyph,
    pub pressed: bool,
    pub active: bool,
    /// Player-set draw opacity multiplier in `[0,1]` from the HUD layout editor (WS-D); `1.0` is the
    /// shipped look. Multiplies the resting/hot alpha — a pure presentation fade, never a hit-test
    /// change (the geometry the engine hit-tests is unaffected).
    pub opacity: f32,
}

/// The floating move stick, in pixels: the captured base center + radius and the current (clamped)
/// thumb position.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct StickView {
    pub base_x: f32,
    pub base_y: f32,
    pub radius: f32,
    pub thumb_x: f32,
    pub thumb_y: f32,
    /// Player-set draw opacity multiplier in `[0,1]` from the HUD layout editor (WS-D); `1.0` is the
    /// shipped look. Fades the ring + thumb together; never affects the input seam.
    pub opacity: f32,
}

/// What to draw this frame: the viewport (for pixel→NDC), the optional move stick, and the four
/// buttons. Built by the engine from its `TouchLayout` + the per-frame `TouchHud` + sim posture.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TouchControlsHud {
    pub viewport: (u32, u32),
    pub stick: Option<StickView>,
    pub fire: TouchButton,
    pub crouch: TouchButton,
    pub reload: TouchButton,
    pub surface: TouchButton,
    /// The aim-down-sight (ADS / zoom) button — `None` when the embodied unit has no gun-sight, so
    /// the button only appears for a scope-capable avatar (the engine mirrors W2's `has_scope` gate).
    pub aim: Option<TouchButton>,
}

/// One overlay quad ready to upload. `repr(C)` + `Pod` so it streams into the per-instance vertex
/// buffer; the field order MUST match the instance attribute locations in `touch_controls.wgsl` and
/// the `vertex_attr_array` in [`TouchControlsRenderer::new`].
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, bytemuck::Pod, bytemuck::Zeroable)]
pub struct TouchQuad {
    /// Center in NDC ([-1,1], +y up).
    pub ndc_x: f32,
    pub ndc_y: f32,
    /// Per-axis NDC half-size (pixel radius converted per axis, so discs stay round under aspect).
    pub half_x: f32,
    pub half_y: f32,
    pub r: f32,
    pub g: f32,
    pub b: f32,
    pub a: f32,
    /// Glyph id: 0 ring, 1 disc, 2 fire, 3 crouch, 4 reload, 5 surface.
    pub shape: f32,
}

// Base colors (RGB). Buttons read by glyph + position; color is a secondary cue.
const STICK_BASE_COL: [f32; 3] = [0.80, 0.82, 0.86];
const STICK_THUMB_COL: [f32; 3] = [0.92, 0.94, 0.98];
const FIRE_COL: [f32; 3] = [0.95, 0.32, 0.28];
const CROUCH_COL: [f32; 3] = [0.45, 0.78, 0.80];
const RELOAD_COL: [f32; 3] = [0.96, 0.74, 0.32];
const SURFACE_COL: [f32; 3] = [0.86, 0.88, 0.92];
const AIM_COL: [f32; 3] = [0.70, 0.86, 0.58];

/// Resting / pressed / toggle-active alpha for a button (pressed or active reads brighter).
const IDLE_ALPHA: f32 = 0.42;
const HOT_ALPHA: f32 = 0.85;

const STICK_BASE_ALPHA: f32 = 0.34;
const STICK_THUMB_ALPHA: f32 = 0.70;

/// Pixel point → NDC ([-1,1], +y up). Pixel origin is top-left (+y down), so y flips.
#[inline]
fn to_ndc(px: f32, py: f32, w: f32, h: f32) -> (f32, f32) {
    (px / w * 2.0 - 1.0, 1.0 - py / h * 2.0)
}

/// A pixel radius → per-axis NDC half-size. NDC spans 2.0 across each axis, so `r` px is `r/dim*2`.
#[inline]
fn half_ndc(r: f32, w: f32, h: f32) -> (f32, f32) {
    (r / w * 2.0, r / h * 2.0)
}

/// Build one button's quad in NDC. `active` (toggle) reads as hot as `pressed`.
fn button_quad(b: &TouchButton, w: f32, h: f32) -> TouchQuad {
    let (ndc_x, ndc_y) = to_ndc(b.cx, b.cy, w, h);
    let (half_x, half_y) = half_ndc(b.r, w, h);
    let [r, g, bl] = match b.glyph {
        TouchGlyph::Fire => FIRE_COL,
        TouchGlyph::Crouch => CROUCH_COL,
        TouchGlyph::Reload => RELOAD_COL,
        TouchGlyph::Surface => SURFACE_COL,
        TouchGlyph::Aim => AIM_COL,
    };
    let base_a = if b.pressed || b.active {
        HOT_ALPHA
    } else {
        IDLE_ALPHA
    };
    let a = base_a * b.opacity.clamp(0.0, 1.0);
    TouchQuad {
        ndc_x,
        ndc_y,
        half_x,
        half_y,
        r,
        g,
        b: bl,
        a,
        shape: b.glyph.shape(),
    }
}

/// Turn the HUD description into the overlay quads to draw, in stable order (stick base, stick
/// thumb, then the four buttons). PURE float math — host-testable without a GPU (the wgpu pipeline
/// in [`TouchControlsRenderer::render`] just uploads + draws whatever this returns).
pub fn build_quads(hud: &TouchControlsHud) -> Vec<TouchQuad> {
    let (wi, hi) = hud.viewport;
    let w = wi.max(1) as f32;
    let h = hi.max(1) as f32;
    let mut quads = Vec::with_capacity(7);

    if let Some(s) = hud.stick {
        let op = s.opacity.clamp(0.0, 1.0);
        // Base ring at the captured origin.
        let (bx, by) = to_ndc(s.base_x, s.base_y, w, h);
        let (bhx, bhy) = half_ndc(s.radius, w, h);
        quads.push(TouchQuad {
            ndc_x: bx,
            ndc_y: by,
            half_x: bhx,
            half_y: bhy,
            r: STICK_BASE_COL[0],
            g: STICK_BASE_COL[1],
            b: STICK_BASE_COL[2],
            a: STICK_BASE_ALPHA * op,
            shape: 0.0,
        });
        // Thumb disc (~40% of the base radius) at the clamped finger position.
        let (tx, ty) = to_ndc(s.thumb_x, s.thumb_y, w, h);
        let (thx, thy) = half_ndc(s.radius * 0.42, w, h);
        quads.push(TouchQuad {
            ndc_x: tx,
            ndc_y: ty,
            half_x: thx,
            half_y: thy,
            r: STICK_THUMB_COL[0],
            g: STICK_THUMB_COL[1],
            b: STICK_THUMB_COL[2],
            a: STICK_THUMB_ALPHA * op,
            shape: 1.0,
        });
    }

    quads.push(button_quad(&hud.fire, w, h));
    quads.push(button_quad(&hud.crouch, w, h));
    quads.push(button_quad(&hud.reload, w, h));
    quads.push(button_quad(&hud.surface, w, h));
    // The ADS button only exists for a scope-capable avatar (gated host-side); skip it otherwise.
    if let Some(aim) = hud.aim {
        quads.push(button_quad(&aim, w, h));
    }
    quads
}

/// A unit-quad corner in [-1, 1]^2 (the shader scales it by the per-instance half-size).
#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct QuadVertex {
    corner: [f32; 2],
}

const QUAD_VERTS: [QuadVertex; 6] = [
    QuadVertex { corner: [-1.0, -1.0] },
    QuadVertex { corner: [1.0, -1.0] },
    QuadVertex { corner: [1.0, 1.0] },
    QuadVertex { corner: [-1.0, -1.0] },
    QuadVertex { corner: [1.0, 1.0] },
    QuadVertex { corner: [-1.0, 1.0] },
];

const INITIAL_CAP: usize = 8;

/// Screen-space FPS-controls overlay (its own pipeline + buffers, like [`hud`](crate::hud)).
pub struct TouchControlsRenderer {
    pipeline: wgpu::RenderPipeline,
    quad_buf: wgpu::Buffer,
    instance_buf: wgpu::Buffer,
    instance_cap: usize,
}

impl TouchControlsRenderer {
    /// Build the pipeline against the swapchain `surface_format` (alpha-blended LOAD overlay).
    pub fn new(device: &wgpu::Device, surface_format: wgpu::TextureFormat) -> Self {
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("gonedark.touch_controls_shader"),
            source: wgpu::ShaderSource::Wgsl(include_str!("touch_controls.wgsl").into()),
        });

        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("gonedark.touch_controls_pipeline_layout"),
            bind_group_layouts: &[],
            immediate_size: 0,
        });

        let quad_layout = wgpu::VertexBufferLayout {
            array_stride: std::mem::size_of::<QuadVertex>() as u64,
            step_mode: wgpu::VertexStepMode::Vertex,
            attributes: &wgpu::vertex_attr_array![0 => Float32x2],
        };
        let instance_layout = wgpu::VertexBufferLayout {
            array_stride: std::mem::size_of::<TouchQuad>() as u64,
            step_mode: wgpu::VertexStepMode::Instance,
            // 1=center(vec2), 2=half(vec2), 3=color(vec4), 4=shape(f32).
            attributes: &wgpu::vertex_attr_array![
                1 => Float32x2,
                2 => Float32x2,
                3 => Float32x4,
                4 => Float32
            ],
        };

        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("gonedark.touch_controls_pipeline"),
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
            label: Some("gonedark.touch_controls_quad_vbo"),
            contents: bytemuck::cast_slice(&QUAD_VERTS),
            usage: wgpu::BufferUsages::VERTEX,
        });

        let instance_cap = INITIAL_CAP;
        let instance_buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("gonedark.touch_controls_instance_vbo"),
            size: (instance_cap * std::mem::size_of::<TouchQuad>()) as u64,
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        TouchControlsRenderer {
            pipeline,
            quad_buf,
            instance_buf,
            instance_cap,
        }
    }

    /// Draw the on-screen FPS controls over `view` (a LOAD pass — never clears). No-op if there is
    /// nothing to draw.
    pub fn render(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        view: &wgpu::TextureView,
        hud: &TouchControlsHud,
    ) {
        let quads = build_quads(hud);
        if quads.is_empty() {
            return;
        }
        if quads.len() > self.instance_cap {
            let new_cap = quads.len().next_power_of_two();
            self.instance_buf = device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("gonedark.touch_controls_instance_vbo"),
                size: (new_cap * std::mem::size_of::<TouchQuad>()) as u64,
                usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });
            self.instance_cap = new_cap;
        }
        queue.write_buffer(&self.instance_buf, 0, bytemuck::cast_slice(&quads));

        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("gonedark.touch_controls_encoder"),
        });
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("gonedark.touch_controls_pass"),
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
            pass.draw(0..QUAD_VERTS.len() as u32, 0..quads.len() as u32);
        }
        queue.submit(std::iter::once(encoder.finish()));
    }
}

#[cfg(test)]
mod tests {
    //! `render` is the float boundary (invariant #1), so f32 math + epsilon compares are fair. The
    //! wgpu pipeline needs a real device (none in CI), so only the pure [`build_quads`] math is
    //! tested here.

    use super::*;

    fn btn(cx: f32, cy: f32, glyph: TouchGlyph) -> TouchButton {
        TouchButton {
            cx,
            cy,
            r: 60.0,
            glyph,
            pressed: false,
            active: false,
            opacity: 1.0,
        }
    }

    fn hud() -> TouchControlsHud {
        TouchControlsHud {
            viewport: (1000, 500),
            stick: None,
            fire: btn(840.0, 370.0, TouchGlyph::Fire),
            crouch: btn(700.0, 430.0, TouchGlyph::Crouch),
            reload: btn(950.0, 250.0, TouchGlyph::Reload),
            surface: btn(940.0, 40.0, TouchGlyph::Surface),
            // A scope-less default avatar: no ADS button (the engine sets `Some` only for a tank).
            aim: None,
        }
    }

    #[test]
    fn four_buttons_without_a_stick_or_scope_yields_four_quads() {
        let q = build_quads(&hud());
        assert_eq!(q.len(), 4, "no stick and no scope → just the four core buttons");
        // Glyph ids in order: fire, crouch, reload, surface.
        assert_eq!(q[0].shape, 2.0);
        assert_eq!(q[1].shape, 3.0);
        assert_eq!(q[2].shape, 4.0);
        assert_eq!(q[3].shape, 5.0);
    }

    #[test]
    fn a_scope_capable_avatar_adds_the_ads_button_last() {
        // For a tank (has a gun-sight) the engine fills `aim` → a fifth button, shape id 6, drawn
        // after the core four. A scope-less unit leaves it `None` (covered above), so it never shows.
        let mut h = hud();
        h.aim = Some(btn(700.0, 310.0, TouchGlyph::Aim));
        let q = build_quads(&h);
        assert_eq!(q.len(), 5, "the ADS button adds one quad");
        assert_eq!(q[4].shape, 6.0, "ADS reticle is shape id 6, drawn last");
        // It carries its own color (a secondary cue), not Fire's.
        assert_eq!([q[4].r, q[4].g, q[4].b], AIM_COL);
    }

    #[test]
    fn a_pressed_ads_button_reads_hot() {
        let mut h = hud();
        h.aim = Some(btn(700.0, 310.0, TouchGlyph::Aim));
        // Holding ADS lights it, exactly like a held Fire.
        if let Some(a) = h.aim.as_mut() {
            a.pressed = true;
        }
        let q = build_quads(&h);
        assert_eq!(q[4].a, HOT_ALPHA, "held ADS reads at the hot alpha");
    }

    #[test]
    fn a_stick_adds_base_ring_and_thumb_disc_first() {
        let mut h = hud();
        h.stick = Some(StickView {
            base_x: 150.0,
            base_y: 400.0,
            radius: 100.0,
            thumb_x: 180.0,
            thumb_y: 360.0,
            opacity: 1.0,
        });
        let q = build_quads(&h);
        assert_eq!(q.len(), 6, "stick base + thumb + four buttons");
        assert_eq!(q[0].shape, 0.0, "base ring first");
        assert_eq!(q[1].shape, 1.0, "thumb disc second");
        // Thumb disc is smaller than the base.
        assert!(q[1].half_x < q[0].half_x);
    }

    #[test]
    fn pixel_center_maps_to_ndc_origin_and_y_flips() {
        let mut h = hud();
        // A button dead-center of a 1000x500 viewport → NDC (0,0).
        h.fire = btn(500.0, 250.0, TouchGlyph::Fire);
        let q = build_quads(&h);
        assert!((q[0].ndc_x).abs() < 1e-5);
        assert!((q[0].ndc_y).abs() < 1e-5);
        // Top of screen (py=0) is NDC +1; bottom is -1.
        h.fire = btn(500.0, 0.0, TouchGlyph::Fire);
        assert!((build_quads(&h)[0].ndc_y - 1.0).abs() < 1e-5);
    }

    #[test]
    fn discs_stay_round_under_aspect_via_per_axis_half_size() {
        // Same pixel radius → different NDC half-size per axis on a non-square viewport.
        let q = build_quads(&hud());
        let f = q[0];
        // r=60 px on 1000 wide → 0.12 NDC; on 500 tall → 0.24 NDC.
        assert!((f.half_x - 0.12).abs() < 1e-5);
        assert!((f.half_y - 0.24).abs() < 1e-5);
    }

    #[test]
    fn editor_opacity_fades_a_buttons_alpha() {
        // The HUD layout editor's per-control opacity multiplies the base alpha (WS-D) — a pure
        // presentation fade. It must NOT change the hit shape (that's the engine's geometry).
        let mut h = hud();
        h.fire.opacity = 0.5;
        let q = build_quads(&h);
        assert!((q[0].a - IDLE_ALPHA * 0.5).abs() < 1e-6, "idle fire faded to half alpha");
        // A pressed-but-faded button scales the HOT alpha, not the idle one.
        h.fire.pressed = true;
        assert!((build_quads(&h)[0].a - HOT_ALPHA * 0.5).abs() < 1e-6);
        // Geometry is untouched by opacity.
        assert!((build_quads(&h)[0].half_x - q[0].half_x).abs() < 1e-9);
    }

    #[test]
    fn stick_opacity_fades_ring_and_thumb_together() {
        let mut h = hud();
        h.stick = Some(StickView {
            base_x: 150.0,
            base_y: 400.0,
            radius: 100.0,
            thumb_x: 180.0,
            thumb_y: 360.0,
            opacity: 0.25,
        });
        let q = build_quads(&h);
        assert!((q[0].a - STICK_BASE_ALPHA * 0.25).abs() < 1e-6, "ring faded");
        assert!((q[1].a - STICK_THUMB_ALPHA * 0.25).abs() < 1e-6, "thumb faded");
    }

    #[test]
    fn pressed_or_active_button_reads_brighter() {
        let mut h = hud();
        h.fire.pressed = true;
        h.crouch.active = true; // crouch toggle highlight
        let q = build_quads(&h);
        assert_eq!(q[0].a, HOT_ALPHA, "pressed fire is hot");
        assert_eq!(q[1].a, HOT_ALPHA, "active (crouched) crouch is hot");
        assert_eq!(q[2].a, IDLE_ALPHA, "idle reload stays dim");
    }
}
