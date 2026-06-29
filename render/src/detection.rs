//! The **"gone dark" detection tell** — a command-view marker over each hostile EMBODIED enemy the
//! local commander can currently sense (`core::detection`, Q2 → D33). When an opposing player goes
//! dark (possesses a unit), the commander earns a tell on that avatar: in `Subtle` it appears only
//! while an own unit holds range + line of sight and then **fades** through a linger window after
//! sight is lost; in `Marked` it is persistent. This module is the **draw** half — the seam that
//! turns [`core::detection::Tell`](gonedark_core::detection::Tell)s into markers lives in
//! `engine::detection_markers` (the pure, host-tested mapper, mirroring `engine::debug_overlay_lines`).
//!
//! Like [`debug`](crate::debug) it is a command-view, world-space **LINE** pass: a screen-composited
//! **LOAD** pass (never clears), no depth test (always reads on top), reusing the unit pass's camera
//! bind group (the top-down view-projection) so world points map to clip exactly as the units do.
//! The renderable geometry is built by the GPU-free [`detection_vertices`] seam (unit-tested without
//! a device); [`DetectionRenderer`] is the thin GPU glue.
//!
//! ## Fairness (invariant #6) is preserved structurally
//!
//! The tell is **"alerts, not intel" for the COMMANDER**: a directional marker on a unit the player
//! has already *earned* sight of (proximity + sightline, or the explicit `Marked` tuning), never a
//! reveal of the rest of the map. Two guards keep it inside the fairness boundary, both load-bearing:
//!
//! - The host only ever draws it in the **command view**, never the dark embodied frame.
//! - The pure `engine::detection_markers` seam **refuses to emit any marker while the local player is
//!   embodied** — so even a mis-wired caller cannot paint a tell over the avatar-only frame.
//!
//! Each marker carries no fog mask and no off-screen state; it is one world point + an alpha. A
//! `Subtle` linger marks where the avatar was **last seen**, not where it secretly went (the
//! `core::detection` memory holds the last-seen position) — so a loss reads as "I lingered too close,
//! too long," never "the game robbed me."

/// Segments approximating the marker's diamond ring (a rotated square reads as a "marked here"
/// reticle, distinct from the debug overlay's many-segment circular rings).
const DIAMOND_SEGS: usize = 4;
/// Half-extent of the diamond ring, world units.
const MARKER_RADIUS: f32 = 1.6;
/// How far above the marker center the downward caret's tip sits (world units) — a "▼ here" pointer
/// floating just over the sensed unit.
const CARET_TIP_Y: f32 = MARKER_RADIUS + 0.6;
/// The caret's arm spread (half-width / extra height above the tip), world units.
const CARET_SPREAD: f32 = 0.95;
const CARET_TOP_Y: f32 = MARKER_RADIUS + 1.7;

/// The tell tint — an alarm amber-red, distinct from the debug overlay's facet/cone/tracer palette
/// and from the faction body colors, so a marker reads as "an enemy has gone dark, here." The alpha
/// is supplied per-marker (the linger fade).
const COLOR_TELL: [f32; 3] = [1.0, 0.32, 0.20];

/// A sensed hostile-embodied unit to mark — already converted to f32 at the render boundary, with
/// the freshness `alpha` the `engine::detection_markers` seam derived from the tell's age. The
/// CPU-side handoff type the host (engine) builds and hands in, exactly like [`crate::debug::DebugUnit`].
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct DetectionMarker {
    /// World position of the tell (live while in sight; the last-seen point while a `Subtle` linger
    /// ages — never the unit's secret current position).
    pub x: f32,
    pub y: f32,
    /// Marker opacity in `[0, 1]`: `1.0` for a fresh / in-sight / `Marked` tell, fading toward the
    /// floor as a `Subtle` linger ages out.
    pub alpha: f32,
}

/// One world-space line endpoint + its RGBA color, the GPU-uploadable vertex. `repr(C)` + `Pod`; the
/// field order MUST match `detection.wgsl`'s vertex attributes and the `vertex_attr_array` below.
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, bytemuck::Pod, bytemuck::Zeroable)]
pub struct DetectionVertex {
    pub world: [f32; 2],
    pub color: [f32; 4],
}

/// Line segments drawn per marker: the diamond ring (`DIAMOND_SEGS`) + the two caret arms.
const SEGS_PER_MARKER: usize = DIAMOND_SEGS + 2;
/// Vertices drawn per marker (two endpoints per line segment).
pub const VERTS_PER_MARKER: usize = SEGS_PER_MARKER * 2;

/// Build the world-space line list for every detection marker: a diamond ring around the sensed unit
/// plus a downward caret floating just above it, tinted [`COLOR_TELL`] at the marker's `alpha`. Pure
/// (no GPU) — the testable geometry seam (the `hitbox_lines` / `interpolate_instances` pattern). The
/// fairness gate (no markers while the local player is embodied) lives upstream in
/// `engine::detection_markers`; by the time markers reach here they are already cleared to draw.
pub fn detection_vertices(markers: &[DetectionMarker]) -> Vec<DetectionVertex> {
    let mut v = Vec::with_capacity(markers.len() * VERTS_PER_MARKER);
    for m in markers {
        let color = [COLOR_TELL[0], COLOR_TELL[1], COLOR_TELL[2], m.alpha.clamp(0.0, 1.0)];
        let at = |x: f32, y: f32| DetectionVertex {
            world: [x, y],
            color,
        };
        // Diamond ring: N → E → S → W → N (points of a rotated square at MARKER_RADIUS).
        let pts = [
            (m.x, m.y + MARKER_RADIUS),
            (m.x + MARKER_RADIUS, m.y),
            (m.x, m.y - MARKER_RADIUS),
            (m.x - MARKER_RADIUS, m.y),
        ];
        for i in 0..DIAMOND_SEGS {
            let (ax, ay) = pts[i];
            let (bx, by) = pts[(i + 1) % DIAMOND_SEGS];
            v.push(at(ax, ay));
            v.push(at(bx, by));
        }
        // Downward caret "▼" pointing at the unit: two arms meeting at a tip just above the diamond.
        let tip = (m.x, m.y + CARET_TIP_Y);
        v.push(at(m.x - CARET_SPREAD, m.y + CARET_TOP_Y));
        v.push(at(tip.0, tip.1));
        v.push(at(tip.0, tip.1));
        v.push(at(m.x + CARET_SPREAD, m.y + CARET_TOP_Y));
    }
    v
}

/// World-space line renderer for the detection-tell overlay. Owns a `LineList` pipeline + a
/// grow-on-demand vertex buffer; reuses the caller's camera bind group (the command-view
/// view-projection). Mirrors [`crate::debug::DebugRenderer`], with an RGBA vertex so the linger fade
/// blends.
pub struct DetectionRenderer {
    pipeline: wgpu::RenderPipeline,
    vbuf: wgpu::Buffer,
    /// Capacity in vertices currently allocated in `vbuf`.
    cap: usize,
}

impl DetectionRenderer {
    /// Build the line pipeline against `surface_format`, using `camera_layout` (the unit pass's
    /// camera bind group layout) so its bind group can be reused at draw time. `device` borrowed (D19).
    pub fn new(
        device: &wgpu::Device,
        surface_format: wgpu::TextureFormat,
        camera_layout: &wgpu::BindGroupLayout,
    ) -> Self {
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("gonedark.detection_shader"),
            source: wgpu::ShaderSource::Wgsl(include_str!("detection.wgsl").into()),
        });

        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("gonedark.detection_pipeline_layout"),
            bind_group_layouts: &[Some(camera_layout)],
            immediate_size: 0,
        });

        let vertex_layout = wgpu::VertexBufferLayout {
            array_stride: std::mem::size_of::<DetectionVertex>() as u64,
            step_mode: wgpu::VertexStepMode::Vertex,
            // 0=world(vec2), 1=color(vec4) — matching the `repr(C)` `DetectionVertex`.
            attributes: &wgpu::vertex_attr_array![0 => Float32x2, 1 => Float32x4],
        };

        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("gonedark.detection_pipeline"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_main"),
                buffers: &[vertex_layout],
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
                topology: wgpu::PrimitiveTopology::LineList,
                ..Default::default()
            },
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            multiview_mask: None,
            cache: None,
        });

        let cap = VERTS_PER_MARKER; // one marker's worth; grows on demand
        let vbuf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("gonedark.detection_vbo"),
            size: (cap * std::mem::size_of::<DetectionVertex>()) as u64,
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        DetectionRenderer {
            pipeline,
            vbuf,
            cap,
        }
    }

    /// Draw the pre-composed world-space line list `verts` over `view` (a LOAD pass — never clears),
    /// using `camera_bind_group` (the command-view view-projection the host just uploaded). The host
    /// builds `verts` from [`detection_vertices`], so this stays the thin GPU glue. (Re)allocates the
    /// vertex buffer if it must grow; a no-op when `verts` is empty.
    pub fn render(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        view: &wgpu::TextureView,
        camera_bind_group: &wgpu::BindGroup,
        verts: &[DetectionVertex],
    ) {
        if verts.is_empty() {
            return;
        }

        if verts.len() > self.cap {
            self.cap = verts.len().next_power_of_two();
            self.vbuf = device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("gonedark.detection_vbo"),
                size: (self.cap * std::mem::size_of::<DetectionVertex>()) as u64,
                usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });
        }
        queue.write_buffer(&self.vbuf, 0, bytemuck::cast_slice(verts));

        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("gonedark.detection_encoder"),
        });
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("gonedark.detection_pass"),
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
            pass.set_bind_group(0, camera_bind_group, &[]);
            pass.set_vertex_buffer(0, self.vbuf.slice(..));
            pass.draw(0..verts.len() as u32, 0..1);
        }
        queue.submit(Some(encoder.finish()));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_markers_produce_no_geometry() {
        assert!(detection_vertices(&[]).is_empty());
    }

    #[test]
    fn one_marker_emits_the_fixed_vertex_count() {
        let v = detection_vertices(&[DetectionMarker {
            x: 3.0,
            y: -2.0,
            alpha: 1.0,
        }]);
        assert_eq!(v.len(), VERTS_PER_MARKER);
        // Every vertex carries the tell tint and the marker's alpha.
        for vert in &v {
            assert_eq!([vert.color[0], vert.color[1], vert.color[2]], COLOR_TELL);
            assert_eq!(vert.color[3], 1.0);
        }
    }

    #[test]
    fn vertex_count_scales_with_marker_count() {
        let markers = [
            DetectionMarker { x: 0.0, y: 0.0, alpha: 1.0 },
            DetectionMarker { x: 5.0, y: 5.0, alpha: 0.5 },
            DetectionMarker { x: -5.0, y: 2.0, alpha: 0.25 },
        ];
        assert_eq!(detection_vertices(&markers).len(), 3 * VERTS_PER_MARKER);
    }

    #[test]
    fn marker_geometry_is_centered_on_the_unit() {
        let (cx, cy) = (10.0_f32, -4.0_f32);
        let v = detection_vertices(&[DetectionMarker {
            x: cx,
            y: cy,
            alpha: 1.0,
        }]);
        // Every vertex sits within the marker's bounding extent around (cx, cy): the diamond reaches
        // MARKER_RADIUS, the caret reaches CARET_TOP_Y above and CARET_SPREAD aside.
        let max_dx = MARKER_RADIUS.max(CARET_SPREAD) + 0.001;
        for vert in &v {
            assert!((vert.world[0] - cx).abs() <= max_dx, "x within marker extent");
            assert!(vert.world[1] >= cy - MARKER_RADIUS - 0.001, "no vertex below the diamond");
            assert!(vert.world[1] <= cy + CARET_TOP_Y + 0.001, "no vertex above the caret top");
        }
    }

    #[test]
    fn alpha_is_propagated_and_clamped() {
        let v = detection_vertices(&[DetectionMarker {
            x: 0.0,
            y: 0.0,
            alpha: 0.4,
        }]);
        assert!(v.iter().all(|vert| (vert.color[3] - 0.4).abs() < 1e-6));
        // Out-of-range alpha is clamped into [0,1] (defensive — the seam already produces in-range).
        let hi = detection_vertices(&[DetectionMarker { x: 0.0, y: 0.0, alpha: 2.0 }]);
        assert!(hi.iter().all(|vert| vert.color[3] == 1.0));
        let lo = detection_vertices(&[DetectionMarker { x: 0.0, y: 0.0, alpha: -1.0 }]);
        assert!(lo.iter().all(|vert| vert.color[3] == 0.0));
    }
}
