//! The **"gone dark" detection tell** — a command-view marker over each hostile EMBODIED enemy the
//! local commander can currently sense (`core::detection`, Q2 → D33). When an opposing player goes
//! dark (possesses a unit), the commander earns a tell on that avatar: in `Subtle` it appears only
//! while an own unit holds range + line of sight and then **fades** through a linger window after
//! sight is lost; in `Marked` it is persistent. This module is the **draw** half — the seam that
//! turns [`core::detection::Tell`](gonedark_core::detection::Tell)s into markers lives in
//! `engine::detection_markers` (the pure, host-tested mapper, mirroring `engine::debug_overlay_lines`).
//!
//! ## The marker is a designed threat reticle
//!
//! Each tell draws a **diamond ring + a double downward chevron** pointing at the sensed contact —
//! a "marked here, threat" reticle, distinct from the debug overlay's many-segment circular rings
//! and from the faction body colors. Unlike a 1px line overlay, every stroke is a thin world-space
//! **ribbon** (a quad) carrying a cross-ribbon `edge` coordinate, so `detection.wgsl` gives it a
//! crisp **analytic anti-aliased** edge at any zoom (no MSAA needed).
//!
//! The reticle reads **urgency** straight off the tell's freshness `alpha` (no new information —
//! purely a restyle of the one value the seam already derived): a fresh / in-sight / `Marked` tell
//! is **larger, thicker, and a warm alert amber** at full opacity; an aging `Subtle` linger shrinks,
//! thins, **cools toward deep red, and fades** as it ages out of its window. So a nearer-in-time
//! (fresher) threat reads *stronger* without ever revealing more than the single sensed point.
//!
//! Like [`debug`](crate::debug) it is a command-view, world-space pass: a screen-composited **LOAD**
//! pass (never clears), no depth test (always reads on top), reusing the unit pass's camera bind
//! group (the top-down view-projection) so world points map to clip exactly as the units do. The
//! renderable geometry is built by the GPU-free [`detection_vertices`] seam (unit-tested without a
//! device); [`DetectionRenderer`] is the thin GPU glue.
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
//! Each marker carries no fog mask and no off-screen state; it is one world point + an alpha. The
//! urgency restyle is a function of that alpha alone — it adds no count, identity, or position intel.
//! A `Subtle` linger marks where the avatar was **last seen**, not where it secretly went (the
//! `core::detection` memory holds the last-seen position) — so a loss reads as "I lingered too close,
//! too long," never "the game robbed me."

/// Segments of the marker's diamond ring (a rotated square reads as a "marked here" reticle,
/// distinct from the debug overlay's many-segment circular rings).
const DIAMOND_SEGS: usize = 4;
/// Half-extent of the diamond ring at full urgency, world units (scaled down as a tell ages).
const DIAMOND_RADIUS: f32 = 1.6;

/// The double downward chevron "⌄⌄" floating above the diamond, pointing at the sensed unit. Two
/// stacked V's: the inner one nearer the diamond, the outer one above it — together a strong "threat,
/// here" arrow. All offsets are world units at full urgency and scale down as a tell ages.
const CHEV_INNER_TIP_Y: f32 = DIAMOND_RADIUS + 0.55;
const CHEV_INNER_TOP_Y: f32 = DIAMOND_RADIUS + 1.45;
const CHEV_INNER_HALF: f32 = 0.85;
const CHEV_OUTER_TIP_Y: f32 = DIAMOND_RADIUS + 1.55;
const CHEV_OUTER_TOP_Y: f32 = DIAMOND_RADIUS + 2.55;
const CHEV_OUTER_HALF: f32 = 1.15;

/// Half-width of a marker stroke (the ribbon), world units at full urgency. Thins as a tell ages.
const STROKE_HALF_WIDTH: f32 = 0.13;

/// Urgency → geometric scale. A fresh tell (`alpha == 1`) draws at `1.0`; the floor keeps an aged
/// linger legible rather than vanishing to a dot. `lerp(LOW, HIGH, alpha)`.
const SCALE_AGED: f32 = 0.82;
const SCALE_FRESH: f32 = 1.12;
/// Urgency → stroke thickness multiplier, same shape (fresh threats read bolder).
const WIDTH_AGED: f32 = 0.72;
const WIDTH_FRESH: f32 = 1.25;

/// The fresh-tell tint — a warm alarm amber: "an enemy has gone dark, sensed *now*, here."
const COLOR_FRESH: [f32; 3] = [1.0, 0.60, 0.16];
/// The aged-linger tint — a deeper, cooler alert red: the contact is stale, sight was lost.
const COLOR_AGED: [f32; 3] = [0.86, 0.18, 0.12];

/// A sensed hostile-embodied unit to mark — already converted to f32 at the render boundary, with
/// the freshness `alpha` the `engine::detection_markers` seam derived from the tell's age. The
/// CPU-side handoff type the host (engine) builds and hands in, exactly like [`crate::debug::DebugUnit`].
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct DetectionMarker {
    /// World position of the tell (live while in sight; the last-seen point while a `Subtle` linger
    /// ages — never the unit's secret current position).
    pub x: f32,
    pub y: f32,
    /// Marker urgency/opacity in `[0, 1]`: `1.0` for a fresh / in-sight / `Marked` tell, fading
    /// toward the floor as a `Subtle` linger ages out. Drives opacity, size, thickness, and warmth.
    pub alpha: f32,
}

/// One world-space ribbon corner: position + RGBA color + the cross-ribbon `edge` coordinate the
/// fragment shader turns into analytic anti-aliasing. `repr(C)` + `Pod`; the field order MUST match
/// `detection.wgsl`'s vertex attributes and the `vertex_attr_array` below.
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, bytemuck::Pod, bytemuck::Zeroable)]
pub struct DetectionVertex {
    pub world: [f32; 2],
    pub color: [f32; 4],
    /// `-1..=1` across the ribbon's width (`0` on its spine). The shader feathers `|edge| -> 1`.
    pub edge: f32,
}

/// Strokes (ribbons) drawn per marker: the diamond ring (`DIAMOND_SEGS`) + two chevrons × two arms.
const STROKES_PER_MARKER: usize = DIAMOND_SEGS + 4;
/// Vertices per stroke: a ribbon quad is two triangles.
const VERTS_PER_STROKE: usize = 6;
/// Vertices drawn per marker.
pub const VERTS_PER_MARKER: usize = STROKES_PER_MARKER * VERTS_PER_STROKE;

#[inline]
fn lerp(a: f32, b: f32, t: f32) -> f32 {
    a + (b - a) * t
}

/// Push one anti-aliased ribbon stroke from `(ax, ay)` to `(bx, by)` with half-width `hw` and `color`
/// (RGBA). Emits two triangles (six vertices); the `edge` attribute runs `+1` along one side of the
/// spine to `-1` along the other so the fragment shader can feather the boundary.
fn push_stroke(
    v: &mut Vec<DetectionVertex>,
    ax: f32,
    ay: f32,
    bx: f32,
    by: f32,
    hw: f32,
    color: [f32; 4],
) {
    let dx = bx - ax;
    let dy = by - ay;
    let len = (dx * dx + dy * dy).sqrt().max(1e-6);
    // Unit normal to the spine, scaled to the half-width.
    let nx = -dy / len * hw;
    let ny = dx / len * hw;
    let corner = |x: f32, y: f32, edge: f32| DetectionVertex {
        world: [x, y],
        color,
        edge,
    };
    let a_pos = corner(ax + nx, ay + ny, 1.0);
    let a_neg = corner(ax - nx, ay - ny, -1.0);
    let b_pos = corner(bx + nx, by + ny, 1.0);
    let b_neg = corner(bx - nx, by - ny, -1.0);
    // Two triangles covering the ribbon quad.
    v.push(a_pos);
    v.push(a_neg);
    v.push(b_pos);
    v.push(a_neg);
    v.push(b_neg);
    v.push(b_pos);
}

/// Build the world-space ribbon list for every detection marker: a diamond ring around the sensed
/// unit plus a double downward chevron floating above it, styled by the marker's `alpha` (urgency —
/// fresh tells draw larger, thicker, warmer amber and opaque; aged lingers shrink, thin, cool to deep
/// red, and fade). Pure (no GPU) — the testable geometry seam (the `hitbox_lines` /
/// `interpolate_instances` pattern). The fairness gate (no markers while the local player is
/// embodied) lives upstream in `engine::detection_markers`; by the time markers reach here they are
/// already cleared to draw, and this only *restyles* the single `alpha` they carry (no new intel).
pub fn detection_vertices(markers: &[DetectionMarker]) -> Vec<DetectionVertex> {
    let mut v = Vec::with_capacity(markers.len() * VERTS_PER_MARKER);
    for m in markers {
        // Urgency: the freshness alpha drives opacity, size, thickness, and warmth together.
        let u = m.alpha.clamp(0.0, 1.0);
        let scale = lerp(SCALE_AGED, SCALE_FRESH, u);
        let hw = STROKE_HALF_WIDTH * lerp(WIDTH_AGED, WIDTH_FRESH, u);
        let rgb = [
            lerp(COLOR_AGED[0], COLOR_FRESH[0], u),
            lerp(COLOR_AGED[1], COLOR_FRESH[1], u),
            lerp(COLOR_AGED[2], COLOR_FRESH[2], u),
        ];
        let color = [rgb[0], rgb[1], rgb[2], u];

        // Diamond ring: N → E → S → W → N (points of a rotated square at the scaled radius).
        let r = DIAMOND_RADIUS * scale;
        let pts = [
            (m.x, m.y + r),
            (m.x + r, m.y),
            (m.x, m.y - r),
            (m.x - r, m.y),
        ];
        for i in 0..DIAMOND_SEGS {
            let (ax, ay) = pts[i];
            let (bx, by) = pts[(i + 1) % DIAMOND_SEGS];
            push_stroke(&mut v, ax, ay, bx, by, hw, color);
        }

        // Double downward chevron "⌄⌄" pointing at the unit: each chevron is two arms meeting at a
        // tip below its top, the whole stack floating just above the diamond.
        for &(tip_y, top_y, half) in &[
            (CHEV_INNER_TIP_Y, CHEV_INNER_TOP_Y, CHEV_INNER_HALF),
            (CHEV_OUTER_TIP_Y, CHEV_OUTER_TOP_Y, CHEV_OUTER_HALF),
        ] {
            let tip = (m.x, m.y + tip_y * scale);
            let half = half * scale;
            let top = top_y * scale;
            // Left arm and right arm of the V, meeting at the tip.
            push_stroke(&mut v, m.x - half, m.y + top, tip.0, tip.1, hw, color);
            push_stroke(&mut v, tip.0, tip.1, m.x + half, m.y + top, hw, color);
        }
    }
    v
}

/// World-space ribbon renderer for the detection-tell overlay. Owns a `TriangleList` pipeline + a
/// grow-on-demand vertex buffer; reuses the caller's camera bind group (the command-view
/// view-projection). Mirrors [`crate::debug::DebugRenderer`], with an RGBA + `edge` vertex so the
/// urgency fade blends and the ribbon edges anti-alias.
pub struct DetectionRenderer {
    pipeline: wgpu::RenderPipeline,
    vbuf: wgpu::Buffer,
    /// Capacity in vertices currently allocated in `vbuf`.
    cap: usize,
}

impl DetectionRenderer {
    /// Build the triangle pipeline against `surface_format`, using `camera_layout` (the unit pass's
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
            // 0=world(vec2), 1=color(vec4), 2=edge(f32) — matching the `repr(C)` `DetectionVertex`.
            attributes: &wgpu::vertex_attr_array![0 => Float32x2, 1 => Float32x4, 2 => Float32],
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
                topology: wgpu::PrimitiveTopology::TriangleList,
                // No cull: ribbon winding flips with stroke direction, and the marker is flat.
                cull_mode: None,
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

    /// Draw the pre-composed world-space ribbon list `verts` over `view` (a LOAD pass — never clears),
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

    /// Recover the per-marker urgency factor the way `detection_vertices` derives it.
    fn urgency(alpha: f32) -> f32 {
        alpha.clamp(0.0, 1.0)
    }

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
        // A fresh tell (alpha == 1) is the warm amber at full opacity, uniformly across the marker.
        for vert in &v {
            assert_eq!([vert.color[0], vert.color[1], vert.color[2]], COLOR_FRESH);
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
    fn every_stroke_is_a_two_triangle_ribbon_with_signed_edges() {
        // Each stroke contributes VERTS_PER_STROKE vertices whose `edge` attribute is exactly the
        // ribbon pattern (+1,-1,+1, -1,-1,+1): two corners on each side of the spine. This is what
        // the fragment shader feathers, so it must hold for every stroke.
        let v = detection_vertices(&[DetectionMarker { x: 1.0, y: 2.0, alpha: 0.7 }]);
        assert_eq!(v.len() % VERTS_PER_STROKE, 0);
        let want = [1.0, -1.0, 1.0, -1.0, -1.0, 1.0];
        for stroke in v.chunks(VERTS_PER_STROKE) {
            for (vert, &e) in stroke.iter().zip(want.iter()) {
                assert_eq!(vert.edge, e);
            }
        }
        // Every emitted edge is one of the two ribbon sides — never an un-fed (e.g. 0) value.
        assert!(v.iter().all(|vert| vert.edge == 1.0 || vert.edge == -1.0));
    }

    #[test]
    fn marker_geometry_is_centered_on_the_unit() {
        let (cx, cy) = (10.0_f32, -4.0_f32);
        let alpha = 1.0_f32;
        let v = detection_vertices(&[DetectionMarker {
            x: cx,
            y: cy,
            alpha,
        }]);
        // Bound the marker's extent for this urgency: diamond + chevrons scale by `scale`, and the
        // ribbon half-width pushes corners out a touch further.
        let scale = lerp(SCALE_AGED, SCALE_FRESH, urgency(alpha));
        let hw = STROKE_HALF_WIDTH * lerp(WIDTH_AGED, WIDTH_FRESH, urgency(alpha));
        let max_dx = DIAMOND_RADIUS.max(CHEV_OUTER_HALF) * scale + hw + 1e-3;
        let min_y = cy - DIAMOND_RADIUS * scale - hw - 1e-3;
        let max_y = cy + CHEV_OUTER_TOP_Y * scale + hw + 1e-3;
        for vert in &v {
            assert!((vert.world[0] - cx).abs() <= max_dx, "x within marker extent");
            assert!(vert.world[1] >= min_y, "no vertex below the diamond");
            assert!(vert.world[1] <= max_y, "no vertex above the chevron stack");
        }
    }

    #[test]
    fn urgency_alpha_is_propagated_and_clamped() {
        // The per-vertex alpha is exactly the (clamped) marker urgency, uniformly across the marker.
        let v = detection_vertices(&[DetectionMarker { x: 0.0, y: 0.0, alpha: 0.4 }]);
        assert!(v.iter().all(|vert| (vert.color[3] - 0.4).abs() < 1e-6));
        // Out-of-range alpha is clamped into [0,1] (defensive — the seam already produces in-range).
        let hi = detection_vertices(&[DetectionMarker { x: 0.0, y: 0.0, alpha: 2.0 }]);
        assert!(hi.iter().all(|vert| vert.color[3] == 1.0));
        let lo = detection_vertices(&[DetectionMarker { x: 0.0, y: 0.0, alpha: -1.0 }]);
        assert!(lo.iter().all(|vert| vert.color[3] == 0.0));
    }

    #[test]
    fn fresher_tells_read_stronger_than_aged_lingers() {
        let center = (0.0_f32, 0.0_f32);
        let fresh = detection_vertices(&[DetectionMarker { x: center.0, y: center.1, alpha: 1.0 }]);
        let aged = detection_vertices(&[DetectionMarker { x: center.0, y: center.1, alpha: 0.2 }]);
        assert_eq!(fresh.len(), aged.len());

        // Opacity: a fresh tell is more opaque than an aged linger.
        assert!(fresh[0].color[3] > aged[0].color[3]);

        // Warmth: fresh trends toward warm amber (more green/blend than the deep aged red).
        assert!(fresh[0].color[1] > aged[0].color[1], "fresh is warmer (greener amber)");

        // Size: the fresh marker reaches farther from the unit than the aged one (bigger reticle).
        let reach = |verts: &[DetectionVertex]| {
            verts
                .iter()
                .map(|v| (v.world[0] - center.0).hypot(v.world[1] - center.1))
                .fold(0.0_f32, f32::max)
        };
        assert!(reach(&fresh) > reach(&aged), "fresh reticle is larger");
    }

    #[test]
    fn chevron_points_down_at_the_contact() {
        // The chevron tips sit ABOVE the unit center and the arms rise to either side — i.e. the V
        // opens downward, aiming at the contact below it. So the highest vertices straddle the
        // center in x (the chevron top corners), and the chevron region is entirely above center.
        let (cx, cy) = (0.0_f32, 0.0_f32);
        let v = detection_vertices(&[DetectionMarker { x: cx, y: cy, alpha: 1.0 }]);
        let topmost = v
            .iter()
            .max_by(|a, b| a.world[1].partial_cmp(&b.world[1]).unwrap())
            .unwrap();
        assert!(topmost.world[1] > cy, "the chevron stack floats above the contact");
        // There exist vertices to the left and to the right of center near the top (the V arms).
        assert!(v.iter().any(|p| p.world[0] < cx - 0.5 && p.world[1] > cy + DIAMOND_RADIUS));
        assert!(v.iter().any(|p| p.world[0] > cx + 0.5 && p.world[1] > cy + DIAMOND_RADIUS));
    }

    #[test]
    fn detection_wgsl_parses_and_validates() {
        // Validate detection.wgsl offline with naga (the compiler wgpu uses), so a WGSL regression
        // (e.g. a mismatched vertex attribute or a bad fwidth use) fails the unit suite, not a device.
        let src = include_str!("detection.wgsl");
        let module = naga::front::wgsl::parse_str(src).expect("detection.wgsl must parse");
        let mut validator = naga::valid::Validator::new(
            naga::valid::ValidationFlags::all(),
            naga::valid::Capabilities::all(),
        );
        validator
            .validate(&module)
            .expect("detection.wgsl must validate");
    }
}
