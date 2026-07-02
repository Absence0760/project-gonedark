//! The **debug hitbox / facet overlay** — the visual half of the "load two tanks and see the
//! hitboxes work" sandbox. A command-view, world-space line pass that draws, per unit, the
//! shell-impact hit-radius ring (colored by which armour **facet** each arc is — front / side /
//! rear), a bright hull-heading spoke, and a tracer behind every in-flight shell. Toggle it on in
//! the duel (`app --scene duel`, **F3**) and surface to watch a shot land on the front (bounce) or
//! the flank (pen) and *see* the facet it struck.
//!
//! Like [`marquee`](crate::marquee) / [`hud`](crate::hud) it is a screen-composited **LOAD** pass
//! (never clears) and a **pure presentation derivation** — the renderable line set is built by the
//! GPU-free [`hitbox_lines`] / [`tracer_lines`] seams (unit-tested without a device) from data the
//! host reads out of the snapshot; the [`DebugRenderer`] is the thin GPU glue. It reuses the unit
//! pass's camera bind group (the top-down view-projection), so world points map to clip exactly as
//! the units do. No depth test, so the lines always read on top.
//!
//! ## Fairness (invariant #6)
//!
//! The host only draws this in the **command** view (never the dark embodied frame), and the lines
//! carry no fog mask — it is debug chrome over an already-visible scene, not intel. It is gated
//! behind a developer toggle, off by default outside the debug scenes.

use gonedark_core::flow_field::GRID;
use gonedark_core::terrain::{Cover, Terrain};
use std::f32::consts::PI;

/// World half-extent of the sim grid as f32 — mirrors `core::flow_field::HALF_EXTENT` (`GRID/2`,
/// with `CELL_SIZE == 1`). Cell `(cx,cy)` spans world `[-GRID_HALF + cx, -GRID_HALF + cx + 1)`, the
/// same mapping `core::terrain` uses, so the overlay squares land exactly on the sim's cells.
const GRID_HALF: f32 = (GRID / 2) as f32;

/// Cover-overlay colors: Light cover amber, Heavy cover (walls / water) steel.
const COLOR_COVER_LIGHT: [f32; 3] = [0.85, 0.70, 0.25];
const COLOR_COVER_HEAVY: [f32; 3] = [0.55, 0.62, 0.75];
/// Solid, movement-blocking cells (walls/water and the solid props — `Cover::Impassable`, Q24):
/// a hot red-orange so a blocked cell reads distinctly from passable Heavy concealment in the map
/// debug overlay.
const COLOR_COVER_IMPASSABLE: [f32; 3] = [0.90, 0.35, 0.20];

/// Segments approximating each unit's hit-radius ring. A multiple of 6 so the 60°/120° facet
/// boundaries land exactly on a segment edge (no segment straddles two facets).
const RING_SEGS: usize = 48;

/// Half-angle of the **front** facet arc (60°): a point within 60° of the hull heading is on the
/// frontal facet. Mirrors `core::combat::FACET_ARC_COS_HALF` (`cos 60°`), expressed as an angle
/// here because this is the float side (invariant #4) and we partition the ring by angle.
const FRONT_HALF: f32 = PI / 3.0;
/// Half-angle past which a point is on the **rear** facet (120° off the hull heading).
const REAR_HALF: f32 = 2.0 * PI / 3.0;

/// Facet colors — front is the danger color (thick armour you bounce off), rear the safe color
/// (thin armour you pen). Read at a glance: red nose, green tail.
const COLOR_FRONT: [f32; 3] = [1.0, 0.25, 0.20];
const COLOR_SIDE: [f32; 3] = [1.0, 0.82, 0.20];
const COLOR_REAR: [f32; 3] = [0.30, 1.0, 0.45];
/// A non-tank's ring (no meaningful facets) — a neutral hoop just marking the hit radius.
const COLOR_PLAIN: [f32; 3] = [0.80, 0.80, 0.85];
/// The bright hull-heading spoke (center → front), so the tank's facing is unmistakable.
const COLOR_SPOKE: [f32; 3] = [1.0, 1.0, 1.0];
/// Shell tracer tint (cyan), distinct from any facet color.
const COLOR_TRACER: [f32; 3] = [0.40, 0.90, 1.0];
/// Tracer length behind a shell, in world units.
const TRACER_LEN: f32 = 2.5;

/// One world-space line endpoint + color, the GPU-uploadable vertex. `repr(C)` + `Pod`; the field
/// order MUST match `debug.wgsl`'s vertex attributes and the `vertex_attr_array` below.
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, bytemuck::Pod, bytemuck::Zeroable)]
pub struct DebugVertex {
    pub world: [f32; 2],
    pub color: [f32; 3],
}

/// A unit to draw the hitbox overlay for — already converted to f32 at the render boundary.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct DebugUnit {
    pub x: f32,
    pub y: f32,
    /// Hull heading in radians (`+X = 0`, CCW) — the facet partition is taken relative to this.
    pub hull_yaw: f32,
    /// The shell-impact hit radius (world units) — the ring's radius.
    pub radius: f32,
    /// Draw the facet coloring + heading spoke (a tank); otherwise a plain neutral ring.
    pub is_tank: bool,
}

/// One in-flight shell to draw a tracer for — f32 at the render boundary.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct DebugShell {
    pub x: f32,
    pub y: f32,
    pub vx: f32,
    pub vy: f32,
}

/// The armour facet a point sitting `rel` radians off the hull heading belongs to — the same
/// 120°-front / 120°-rear / two-60°-side partition `core::combat::shot_facet` uses, here on the
/// ring so each arc is colored by the facet a shot landing there strikes.
pub(crate) fn facet_color(rel: f32) -> [f32; 3] {
    let a = wrap_pi(rel).abs();
    if a <= FRONT_HALF {
        COLOR_FRONT
    } else if a >= REAR_HALF {
        COLOR_REAR
    } else {
        COLOR_SIDE
    }
}

/// Wrap an angle to `(-PI, PI]`.
fn wrap_pi(a: f32) -> f32 {
    (a + PI).rem_euclid(2.0 * PI) - PI
}

/// Build the world-space line list for every unit's hitbox: a hit-radius ring (per-segment colored
/// by facet for a tank, neutral otherwise) plus, for tanks, a hull-heading spoke from center to the
/// front of the ring. Pure (no GPU) — the testable seam.
pub fn hitbox_lines(units: &[DebugUnit]) -> Vec<DebugVertex> {
    let mut v = Vec::with_capacity(units.len() * (RING_SEGS * 2 + 2));
    for u in units {
        // The ring, one line segment per arc. Each segment is colored by the facet of its midpoint
        // angle (relative to the hull), so the ring reads as three colored arcs around a tank.
        for i in 0..RING_SEGS {
            let a0 = (i as f32) / (RING_SEGS as f32) * 2.0 * PI;
            let a1 = ((i + 1) as f32) / (RING_SEGS as f32) * 2.0 * PI;
            let color = if u.is_tank {
                facet_color((a0 + a1) * 0.5 - u.hull_yaw)
            } else {
                COLOR_PLAIN
            };
            v.push(ring_point(u, a0, color));
            v.push(ring_point(u, a1, color));
        }
        // The hull-heading spoke (center → front), so facing is obvious. Tanks only.
        if u.is_tank {
            v.push(DebugVertex {
                world: [u.x, u.y],
                color: COLOR_SPOKE,
            });
            v.push(ring_point(u, u.hull_yaw, COLOR_SPOKE));
        }
    }
    v
}

/// One point on a unit's ring at world angle `a`.
fn ring_point(u: &DebugUnit, a: f32, color: [f32; 3]) -> DebugVertex {
    DebugVertex {
        world: [u.x + u.radius * a.cos(), u.y + u.radius * a.sin()],
        color,
    }
}

/// Build the world-space line list of shell tracers: one segment per shell, from a point behind it
/// (along `-velocity`) to the shell. A near-stationary shell (no travel direction) is skipped. Pure.
pub fn tracer_lines(shells: &[DebugShell]) -> Vec<DebugVertex> {
    let mut v = Vec::with_capacity(shells.len() * 2);
    for s in shells {
        let len = (s.vx * s.vx + s.vy * s.vy).sqrt();
        if len <= f32::EPSILON {
            continue; // no direction → nothing to draw
        }
        let (nx, ny) = (s.vx / len, s.vy / len);
        v.push(DebugVertex {
            world: [s.x - nx * TRACER_LEN, s.y - ny * TRACER_LEN],
            color: COLOR_TRACER,
        });
        v.push(DebugVertex {
            world: [s.x, s.y],
            color: COLOR_TRACER,
        });
    }
    v
}

/// A hitscan unit to draw the infantry overlay for — already f32 at the render boundary. Unlike a
/// tank (whose hitbox is a shell impact-radius + armour facets), an infantryman's relevant geometry
/// is its weapon **range** and firing **cone** about its facing.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct DebugInfantry {
    pub x: f32,
    pub y: f32,
    /// Facing in radians (`+X = 0`, CCW) — the cone is centered here.
    pub facing: f32,
    /// Weapon range (world units) — the range ring radius.
    pub range: f32,
    /// Cosine of the firing cone's half-angle (`combat::FIRE_CONE_COS_HALF`) — the wedge half-width.
    pub cone_cos_half: f32,
    /// Faction tint for the range ring.
    pub ring_color: [f32; 3],
}

/// Segments along the cone's far arc (closing the wedge between its two edge spokes).
const CONE_ARC_SEGS: usize = 8;
/// The firing-cone wedge color (a warm "fire arc").
const COLOR_CONE: [f32; 3] = [1.0, 0.65, 0.20];

/// Build the world-space line list for every infantryman: a faction-tinted **range ring** and a
/// **firing-cone wedge** (two edge spokes about `facing` at the cone half-angle, closed by a far
/// arc) out to `range`. Pure (no GPU) — the testable seam. Line-of-sight connectors are composed by
/// the host (they need the terrain) and appended separately.
pub fn infantry_lines(units: &[DebugInfantry]) -> Vec<DebugVertex> {
    let mut v = Vec::with_capacity(units.len() * (RING_SEGS * 2 + CONE_ARC_SEGS * 2 + 4));
    for u in units {
        // Range ring.
        for i in 0..RING_SEGS {
            let a0 = (i as f32) / (RING_SEGS as f32) * 2.0 * PI;
            let a1 = ((i + 1) as f32) / (RING_SEGS as f32) * 2.0 * PI;
            v.push(range_point(u, a0));
            v.push(range_point(u, a1));
        }
        // Firing cone: two edge spokes at facing ± half-angle, then the far arc between them.
        let half = u.cone_cos_half.clamp(-1.0, 1.0).acos();
        let edge = |a: f32| DebugVertex {
            world: [u.x + u.range * a.cos(), u.y + u.range * a.sin()],
            color: COLOR_CONE,
        };
        let center = DebugVertex {
            world: [u.x, u.y],
            color: COLOR_CONE,
        };
        for s in [-half, half] {
            v.push(center);
            v.push(edge(u.facing + s));
        }
        for i in 0..CONE_ARC_SEGS {
            let a0 = u.facing - half + (i as f32) / (CONE_ARC_SEGS as f32) * 2.0 * half;
            let a1 = u.facing - half + ((i + 1) as f32) / (CONE_ARC_SEGS as f32) * 2.0 * half;
            v.push(edge(a0));
            v.push(edge(a1));
        }
    }
    v
}

/// A unit that fired this tick (snapshot `firing` flag), to draw a muzzle flash for — f32 at the
/// render boundary. Kind-agnostic: a tank and an infantryman flash the same way, so a single seam
/// covers both. The host derives the flag from the weapon cooldown (`core::snapshot`), so this is
/// the command-view analogue of the embodied viewmodel flash ([`crate::world::muzzle_flash_intensity`]).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct DebugMuzzle {
    pub x: f32,
    pub y: f32,
    /// Gun bearing in radians (`+X = 0`, CCW) — the forward spike points down it (the shot direction).
    pub facing: f32,
    /// Flash size in world units (the star-spoke length; the forward spike is twice this).
    pub size: f32,
}

/// Star spokes in a muzzle-flash burst (besides the forward spike), so the flash reads from any facing.
const MUZZLE_SPOKES: usize = 6;
/// Hot muzzle-flash tint — a bright yellow-white that pops against the facet / cone / tracer colors.
const COLOR_MUZZLE: [f32; 3] = [1.0, 0.95, 0.55];

/// Build the world-space line list for every firing unit's muzzle flash: a `MUZZLE_SPOKES`-armed star
/// burst centered on the unit, plus a longer spike down `facing` so you can read *which way* it is
/// shooting. Pure (no GPU) — the testable seam.
pub fn muzzle_flash_lines(flashes: &[DebugMuzzle]) -> Vec<DebugVertex> {
    let mut v = Vec::with_capacity(flashes.len() * (MUZZLE_SPOKES + 1) * 2);
    for f in flashes {
        let center = DebugVertex {
            world: [f.x, f.y],
            color: COLOR_MUZZLE,
        };
        // Star burst: evenly spaced spokes radiating from the unit.
        for i in 0..MUZZLE_SPOKES {
            let a = (i as f32) / (MUZZLE_SPOKES as f32) * 2.0 * PI;
            v.push(center);
            v.push(DebugVertex {
                world: [f.x + f.size * a.cos(), f.y + f.size * a.sin()],
                color: COLOR_MUZZLE,
            });
        }
        // Forward spike (twice as long) down the gun bearing — the shot direction.
        v.push(center);
        v.push(DebugVertex {
            world: [
                f.x + 2.0 * f.size * f.facing.cos(),
                f.y + 2.0 * f.size * f.facing.sin(),
            ],
            color: COLOR_MUZZLE,
        });
    }
    v
}

/// One point on an infantryman's range ring at world angle `a`.
fn range_point(u: &DebugInfantry, a: f32) -> DebugVertex {
    DebugVertex {
        world: [u.x + u.range * a.cos(), u.y + u.range * a.sin()],
        color: u.ring_color,
    }
}

/// World-space line renderer for the debug overlay. Owns a `LineList` pipeline + a grow-on-demand
/// vertex buffer; reuses the caller's camera bind group (the command-view view-projection).
pub struct DebugRenderer {
    pipeline: wgpu::RenderPipeline,
    vbuf: wgpu::Buffer,
    /// Capacity in vertices currently allocated in `vbuf`.
    cap: usize,
}

impl DebugRenderer {
    /// Build the line pipeline against `surface_format`, using `camera_layout` (the unit pass's
    /// camera bind group layout) so its bind group can be reused at draw time. `device` borrowed (D19).
    pub fn new(
        device: &wgpu::Device,
        surface_format: wgpu::TextureFormat,
        camera_layout: &wgpu::BindGroupLayout,
    ) -> Self {
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("gonedark.debug_shader"),
            source: wgpu::ShaderSource::Wgsl(include_str!("debug.wgsl").into()),
        });

        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("gonedark.debug_pipeline_layout"),
            bind_group_layouts: &[Some(camera_layout)],
            immediate_size: 0,
        });

        let vertex_layout = wgpu::VertexBufferLayout {
            array_stride: std::mem::size_of::<DebugVertex>() as u64,
            step_mode: wgpu::VertexStepMode::Vertex,
            // 0=world(vec2), 1=color(vec3) — matching the `repr(C)` `DebugVertex`.
            attributes: &wgpu::vertex_attr_array![0 => Float32x2, 1 => Float32x3],
        };

        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("gonedark.debug_pipeline"),
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

        let cap = RING_SEGS * 2 + 2; // one tank's worth; grows on demand
        let vbuf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("gonedark.debug_vbo"),
            size: (cap * std::mem::size_of::<DebugVertex>()) as u64,
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        DebugRenderer {
            pipeline,
            vbuf,
            cap,
        }
    }

    /// Draw the pre-composed world-space line list `verts` over `view` (a LOAD pass — never clears),
    /// using `camera_bind_group` (the command-view view-projection the host just uploaded). The host
    /// builds `verts` from the pure seams ([`hitbox_lines`], [`tracer_lines`], [`infantry_lines`],
    /// plus any LoS connectors), so this stays the thin GPU glue. (Re)allocates the vertex buffer if
    /// it must grow; a no-op when `verts` is empty.
    pub fn render(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        view: &wgpu::TextureView,
        camera_bind_group: &wgpu::BindGroup,
        verts: &[DebugVertex],
    ) {
        if verts.is_empty() {
            return;
        }

        if verts.len() > self.cap {
            self.cap = verts.len().next_power_of_two();
            self.vbuf = device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("gonedark.debug_vbo"),
                size: (self.cap * std::mem::size_of::<DebugVertex>()) as u64,
                usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });
        }
        queue.write_buffer(&self.vbuf, 0, bytemuck::cast_slice(verts));

        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("gonedark.debug_encoder"),
        });
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("gonedark.debug_pass"),
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
        queue.submit(std::iter::once(encoder.finish()));
    }
}

/// Outline every non-open cover cell of `terrain` as a world-space square, colored by cover level
/// (Light = amber, Heavy = steel). The **map-diagnostics** seam: it makes the sim's actual cover
/// grid — the cells the flow field and line-of-sight read — visible in the command view, so a
/// mis-baked or misaligned map (a wall one cell off, a sealed pocket, water where it shouldn't be)
/// is obvious at a glance. Open cells draw nothing (an empty map ⇒ no verts). Each cell is 4 edges
/// as a `LineList` (8 verts). Pure (no GPU) — the testable seam.
pub fn covergrid_lines(terrain: &Terrain) -> Vec<DebugVertex> {
    let mut v = Vec::new();
    for cy in 0..GRID as i32 {
        for cx in 0..GRID as i32 {
            let color = match terrain.cover_at_cell(cx, cy) {
                Cover::None => continue,
                Cover::Light => COLOR_COVER_LIGHT,
                Cover::Heavy => COLOR_COVER_HEAVY,
                Cover::Impassable => COLOR_COVER_IMPASSABLE,
            };
            let x0 = -GRID_HALF + cx as f32;
            let y0 = -GRID_HALF + cy as f32;
            let x1 = x0 + 1.0;
            let y1 = y0 + 1.0;
            let corners = [[x0, y0], [x1, y0], [x1, y1], [x0, y1]];
            for i in 0..4 {
                v.push(DebugVertex { world: corners[i], color });
                v.push(DebugVertex { world: corners[(i + 1) % 4], color });
            }
        }
    }
    v
}

#[cfg(test)]
mod tests {
    //! `render` is the float boundary, so the f32 line math is fair game. `DebugRenderer::new` needs
    //! a real `wgpu::Device` (no display in CI), so the pipeline path is untested; the testable
    //! geometry is factored into [`hitbox_lines`] / [`tracer_lines`] / [`facet_color`].

    use super::*;

    #[test]
    fn covergrid_open_field_draws_nothing() {
        assert!(covergrid_lines(&Terrain::open()).is_empty());
    }

    #[test]
    fn covergrid_outlines_one_cell_at_the_right_world_square() {
        let mut t = Terrain::open();
        t.set_cover(0, 0, Cover::Heavy); // south-west corner cell
        let v = covergrid_lines(&t);
        assert_eq!(v.len(), 8, "one cell = 4 edges = 8 line verts");
        assert!(v.iter().all(|d| d.color == COLOR_COVER_HEAVY));
        // Cell (0,0) spans world [-GRID_HALF, -GRID_HALF+1) on each axis.
        let lo = -GRID_HALF;
        let hi = lo + 1.0;
        let xs: Vec<f32> = v.iter().map(|d| d.world[0]).collect();
        let ys: Vec<f32> = v.iter().map(|d| d.world[1]).collect();
        assert_eq!(xs.iter().cloned().fold(f32::INFINITY, f32::min), lo);
        assert_eq!(xs.iter().cloned().fold(f32::NEG_INFINITY, f32::max), hi);
        assert_eq!(ys.iter().cloned().fold(f32::INFINITY, f32::min), lo);
        assert_eq!(ys.iter().cloned().fold(f32::NEG_INFINITY, f32::max), hi);
    }

    #[test]
    fn covergrid_colors_by_cover_level_and_counts_cells() {
        let mut t = Terrain::open();
        t.set_cover(3, 4, Cover::Light);
        t.set_cover(5, 6, Cover::Heavy);
        let v = covergrid_lines(&t);
        assert_eq!(v.len(), 16, "two covered cells = 16 verts");
        assert_eq!(v.iter().filter(|d| d.color == COLOR_COVER_LIGHT).count(), 8);
        assert_eq!(v.iter().filter(|d| d.color == COLOR_COVER_HEAVY).count(), 8);
    }

    #[test]
    fn covergrid_scales_with_a_baked_map() {
        // A real baked map produces a non-trivial, deterministic vertex count (8 per covered cell).
        let t = Terrain::from_map_id(Terrain::POINTE_DU_HOC_MAP_ID).unwrap();
        let covered = (0..GRID as i32)
            .flat_map(|cy| (0..GRID as i32).map(move |cx| (cx, cy)))
            .filter(|&(cx, cy)| t.cover_at_cell(cx, cy) != Cover::None)
            .count();
        assert_eq!(covergrid_lines(&t).len(), covered * 8);
    }

    fn tank(hull_yaw: f32) -> DebugUnit {
        DebugUnit {
            x: 0.0,
            y: 0.0,
            hull_yaw,
            radius: 1.0,
            is_tank: true,
        }
    }

    #[test]
    fn facet_partition_matches_the_combat_arcs() {
        // Head-on (0 off the hull) is the front facet; the tail (PI) is rear; the flank (PI/2) side.
        assert_eq!(facet_color(0.0), COLOR_FRONT);
        assert_eq!(facet_color(PI), COLOR_REAR);
        assert_eq!(facet_color(PI / 2.0), COLOR_SIDE);
        // Just inside each boundary buckets as expected (the exact float boundary is meaningless —
        // a real ring midpoint never lands on it, and angle-wrapping perturbs it by an ULP).
        assert_eq!(facet_color(FRONT_HALF - 0.01), COLOR_FRONT);
        assert_eq!(facet_color(FRONT_HALF + 0.01), COLOR_SIDE);
        assert_eq!(facet_color(REAR_HALF + 0.01), COLOR_REAR);
        assert_eq!(facet_color(REAR_HALF - 0.01), COLOR_SIDE);
        // Wrapping: a negative / >2PI angle buckets the same as its wrapped value.
        assert_eq!(facet_color(-0.1), COLOR_FRONT);
        assert_eq!(facet_color(2.0 * PI), COLOR_FRONT);
    }

    #[test]
    fn tank_ring_has_a_spoke_and_all_three_facets() {
        let v = hitbox_lines(&[tank(0.0)]);
        // RING_SEGS line segments (2 verts each) + a 2-vert spoke.
        assert_eq!(v.len(), RING_SEGS * 2 + 2);
        // The ring shows all three facet colors (a tank is armoured all the way round).
        let has = |c: [f32; 3]| v.iter().any(|x| x.color == c);
        assert!(has(COLOR_FRONT) && has(COLOR_SIDE) && has(COLOR_REAR));
        // The last two verts are the spoke: center → the front of the ring (along +X for yaw 0).
        let spoke_tail = v[v.len() - 2];
        let spoke_head = v[v.len() - 1];
        assert_eq!(spoke_tail.world, [0.0, 0.0]);
        assert_eq!(spoke_tail.color, COLOR_SPOKE);
        assert!((spoke_head.world[0] - 1.0).abs() < 1e-5, "front spoke points +X");
        assert!(spoke_head.world[1].abs() < 1e-5);
    }

    #[test]
    fn front_arc_follows_the_hull_heading() {
        // Rotate the hull to face +Y (PI/2): the ring point at +Y is now the FRONT facet, and the
        // point at +X (90° off the hull) is a SIDE.
        let v = hitbox_lines(&[tank(PI / 2.0)]);
        // Find the ring vertex nearest +Y (0, radius) and assert it's a front-colored arc.
        let near = |tx: f32, ty: f32| {
            v.iter()
                .filter(|p| p.color != COLOR_SPOKE)
                .min_by(|a, b| {
                    let da = (a.world[0] - tx).hypot(a.world[1] - ty);
                    let db = (b.world[0] - tx).hypot(b.world[1] - ty);
                    da.partial_cmp(&db).unwrap()
                })
                .unwrap()
                .color
        };
        assert_eq!(near(0.0, 1.0), COLOR_FRONT, "the +Y arc is now the front");
        assert_eq!(near(0.0, -1.0), COLOR_REAR, "the -Y arc is the rear");
    }

    #[test]
    fn non_tank_ring_is_plain_with_no_spoke() {
        let unit = DebugUnit {
            is_tank: false,
            ..tank(0.0)
        };
        let v = hitbox_lines(&[unit]);
        assert_eq!(v.len(), RING_SEGS * 2, "no spoke for a non-tank");
        assert!(v.iter().all(|p| p.color == COLOR_PLAIN));
    }

    #[test]
    fn tracer_points_backward_from_the_shell_along_velocity() {
        let shell = DebugShell {
            x: 5.0,
            y: 0.0,
            vx: 2.0,
            vy: 0.0,
        };
        let v = tracer_lines(&[shell]);
        assert_eq!(v.len(), 2);
        let tail = v[0].world;
        let head = v[1].world;
        // Head is the shell; tail sits behind it along -velocity (smaller x for +X travel).
        assert_eq!(head, [5.0, 0.0]);
        assert!(tail[0] < head[0], "tail trails the shell");
        assert!((tail[0] - (5.0 - TRACER_LEN)).abs() < 1e-5);
    }

    fn rifleman(facing: f32) -> DebugInfantry {
        DebugInfantry {
            x: 0.0,
            y: 0.0,
            facing,
            range: 14.0,
            cone_cos_half: 0.866, // ~30° half-angle (FIRE_CONE_COS_HALF)
            ring_color: [0.3, 0.5, 1.0],
        }
    }

    #[test]
    fn infantry_draws_a_range_ring_and_a_cone_wedge() {
        let v = infantry_lines(&[rifleman(0.0)]);
        // RING_SEGS ring segments + 2 cone edge spokes + CONE_ARC_SEGS arc segments, 2 verts each.
        assert_eq!(v.len(), (RING_SEGS + 2 + CONE_ARC_SEGS) * 2);
        // The ring is faction-tinted; the cone is the warm fire-arc color.
        assert!(v.iter().any(|p| p.color == [0.3, 0.5, 1.0]), "range ring tinted");
        assert!(v.iter().any(|p| p.color == COLOR_CONE), "cone wedge drawn");
        // Every ring/cone point sits within the range radius (+epsilon) of the unit.
        assert!(v
            .iter()
            .all(|p| (p.world[0] * p.world[0] + p.world[1] * p.world[1]).sqrt() <= 14.0 + 1e-3));
    }

    #[test]
    fn cone_wedge_straddles_the_facing() {
        // Facing +X (0): the cone's two edge spokes sit symmetrically above and below the X axis,
        // and the cone center spoke endpoints are within ±30° of +X (both have x > 0).
        let v = infantry_lines(&[rifleman(0.0)]);
        let cone: Vec<&DebugVertex> = v.iter().filter(|p| p.color == COLOR_CONE).collect();
        // The wedge points downrange: every cone endpoint that isn't the muzzle has positive x.
        assert!(cone
            .iter()
            .filter(|p| !(p.world[0] == 0.0 && p.world[1] == 0.0))
            .all(|p| p.world[0] > 0.0));
        // The edges reach above and below the axis (a real wedge, not a line).
        assert!(cone.iter().any(|p| p.world[1] > 0.1));
        assert!(cone.iter().any(|p| p.world[1] < -0.1));
    }

    #[test]
    fn muzzle_flash_is_a_star_plus_a_forward_spike() {
        let v = muzzle_flash_lines(&[DebugMuzzle {
            x: 0.0,
            y: 0.0,
            facing: 0.0,
            size: 1.0,
        }]);
        assert_eq!(v.len(), (MUZZLE_SPOKES + 1) * 2);
        assert!(v.iter().all(|p| p.color == COLOR_MUZZLE), "all hot muzzle tint");
        let tip = v[v.len() - 1].world;
        assert!(
            (tip[0] - 2.0).abs() < 1e-5 && tip[1].abs() < 1e-5,
            "spike points downrange to 2*size"
        );
    }

    #[test]
    fn no_firing_units_draw_no_flash() {
        assert!(muzzle_flash_lines(&[]).is_empty());
    }

    #[test]
    fn a_still_shell_draws_no_tracer() {
        let v = tracer_lines(&[DebugShell {
            x: 1.0,
            y: 1.0,
            vx: 0.0,
            vy: 0.0,
        }]);
        assert!(v.is_empty(), "no velocity → no tracer direction");
    }
}
