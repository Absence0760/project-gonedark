//! Renderer — consumes a READ-ONLY core snapshot and draws it (invariant #4).
//!
//! This is the float boundary: Q16.16 sim positions become `f32` HERE, never in `core`. The
//! renderer only ever *reads* a [`Snapshot`]; it never mutates sim state and never calls back
//! into `core`. It talks to `wgpu` (→ Vulkan/D3D12/Metal per device) and to no specific GPU
//! API and no windowing crate — the RHI-over-many-APIs property holds (D19).
//!
//! ## Ownership of the GPU device (D19)
//! The `wgpu::Device`/`Queue` are owned by the concrete platform backend and handed *in* by
//! the `app` wiring layer: [`Renderer::new`] borrows a `&wgpu::Device` to build its pipeline
//! once, and [`Renderer::render`] borrows `&Device`/`&Queue` each frame to upload and submit.
//!
//! ## What it draws (Phase 2)
//! Each renderable is one instanced quad carrying its world position, a half-extent (size),
//! an RGB color, a health fraction, and a flag word. The vertex shader places the quad; the
//! fragment shader colors it, draws a health bar across the top strip when `health >= 0`, and
//! renders control points as a hollow ring. Colors are baked per-instance on the CPU
//! ([`faction_color`]) so factions, the embodied avatar (amber), buildings, and control-point
//! owners read at a glance.
//!
//! ## "World goes dark" (invariant #6)
//! When `world_dark` is set (the local player is embodied) the frame clears to near-black and
//! **only embodied instances are uploaded** — the strategic map (other units, buildings, and
//! the territory control points, which are all map intel) genuinely disappears, leaving just
//! the avatar. Filtering happens at upload time in [`Renderer::render`]; [`Renderer::prepare`]
//! still builds the full set so a single un-embodied frame can light the whole map again.

use gonedark_core::alerts::AlertChannel;
use gonedark_core::components::Faction;
use gonedark_core::fixed::Fixed;
use gonedark_core::fog::Visibility;
use gonedark_core::snapshot::Snapshot;
use wgpu::util::DeviceExt;

/// Fog-of-war application (worker 1). Owns `visible_instances`: the visibility → drawn-instances
/// filter the unit pass runs each frame.
mod fog;
/// Embodied directional alert HUD (worker 2). Owns `HudRenderer`: the screen-space alert overlay
/// drawn on top of the embodied frame.
mod hud;
/// In-session shell overlay (Phase 4 WS-B). Owns `OverlayRenderer`: the pause / reconnect-prompt /
/// post-match-summary chrome, drawn on top of the (possibly dark) match frame. Public so the host
/// can describe which surface to draw via [`overlay::Overlay`].
pub mod overlay;

/// Device quality tiers + dynamic-resolution + thermal-backoff policy (Phase 4 WS-C). Pure,
/// host-testable RENDER decisions (invariant #1/#4: never a sim input) — see the module docs.
pub mod tiers;

pub use tiers::{next_resolution_scale, thermal_backoff, Backoff, QualityTier, TierParams};

/// Convert a Q16.16 fixed value to `f32` for the GPU. The ONLY sanctioned fixed→float hop.
#[inline]
pub fn fixed_to_f32(v: Fixed) -> f32 {
    v.to_bits() as f32 / Fixed::SCALE as f32
}

/// Instance flag bits.
pub const FLAG_EMBODIED: u32 = 1; // the possessed avatar — survives the dark-frame filter
pub const FLAG_RING: u32 = 2; // a territory control point — drawn as a hollow ring
pub const FLAG_SELECTED: u32 = 4; // command-layer selected — drawn with a bright rim (presentation)

/// Drawn half-extent (world units) per kind. Render-only cosmetic scale.
const UNIT_HALF: f32 = 0.5;
const BUILDING_HALF: f32 = 1.6;
const CONTROL_POINT_HALF: f32 = 2.2;

/// Sentinel health value meaning "draw no health bar" (control points).
const NO_HEALTH_BAR: f32 = -1.0;

/// The base RGB color for a faction (the embodied avatar overrides this to amber).
pub fn faction_color(faction: Faction) -> [f32; 3] {
    match faction {
        Faction::Player => [0.25, 0.60, 0.95],  // cool blue
        Faction::Enemy => [0.90, 0.32, 0.26],   // hostile red
        Faction::Neutral => [0.55, 0.55, 0.60], // neutral grey
    }
}

/// The embodied avatar's color — warm amber, the unit you possess.
const AVATAR_COLOR: [f32; 3] = [1.0, 0.85, 0.2];

/// Build render instances from two sim snapshots interpolated by `alpha` in `[0,1]` (invariant
/// #4 — interpolation lives in the renderer, not the sim). Units are matched by index (the
/// shorter snapshot wins, so a mismatched count never panics); positions cross the float
/// boundary via [`fixed_to_f32`] and are lerped, while faction/health/embodied are read from
/// the *current* snapshot. Control points are appended from the current snapshot (they are
/// static, so they are not interpolated). `selected` is the set of currently-selected world
/// (ECS) indices (command-view-only presentation state — empty while embodied); a unit whose
/// `entity_index` is in `selected` gets [`FLAG_SELECTED`] so the shader rims it. Device-free
/// and pure, so it is unit-testable.
pub fn interpolate_instances(
    prev: &Snapshot,
    curr: &Snapshot,
    alpha: f32,
    selected: &[u32],
) -> Vec<UnitInstance> {
    let n = prev.units.len().min(curr.units.len());
    let mut out = Vec::with_capacity(n + curr.control_points.len());

    for i in 0..n {
        let a = &prev.units[i];
        let b = &curr.units[i];
        let (ax, ay) = (fixed_to_f32(a.pos.x), fixed_to_f32(a.pos.y));
        let (bx, by) = (fixed_to_f32(b.pos.x), fixed_to_f32(b.pos.y));

        let mut flags = 0u32;
        let color = if b.embodied {
            flags |= FLAG_EMBODIED;
            AVATAR_COLOR
        } else {
            faction_color(b.faction)
        };
        // Command-layer selection highlight (presentation only — never sim state).
        if selected.contains(&b.entity_index) {
            flags |= FLAG_SELECTED;
        }
        let half_extent = if b.building { BUILDING_HALF } else { UNIT_HALF };
        let health = fixed_to_f32(b.health).clamp(0.0, 1.0);

        out.push(UnitInstance {
            x: ax + (bx - ax) * alpha,
            y: ay + (by - ay) * alpha,
            half_extent,
            r: color[0],
            g: color[1],
            b: color[2],
            health,
            flags,
        });
    }

    // Control points — static map markers, drawn as hollow rings in the owner's color. They
    // carry no embodied flag, so the dark-frame filter hides them (they are map intel).
    for cp in &curr.control_points {
        let color = faction_color(cp.owner);
        out.push(UnitInstance {
            x: fixed_to_f32(cp.pos.x),
            y: fixed_to_f32(cp.pos.y),
            half_extent: CONTROL_POINT_HALF,
            r: color[0],
            g: color[1],
            b: color[2],
            health: NO_HEALTH_BAR,
            flags: FLAG_RING,
        });
    }

    out
}

/// Column-major 4x4 view-projection matrix, built by `app` (glam `Mat4::to_cols_array_2d()`).
#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
pub struct Camera {
    pub view_proj: [[f32; 4]; 4],
}

/// One renderable instance in float space (render-only). `repr(C)` + `Pod` so it uploads
/// straight into the per-instance vertex buffer. Layout (byte offsets) MUST match the shader's
/// instance attribute locations and the pipeline's `vertex_attr_array` below.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, bytemuck::Pod, bytemuck::Zeroable)]
pub struct UnitInstance {
    pub x: f32,
    pub y: f32,
    /// Drawn half-extent in world units.
    pub half_extent: f32,
    pub r: f32,
    pub g: f32,
    pub b: f32,
    /// Health fraction in `[0,1]`; negative ([`NO_HEALTH_BAR`]) draws no bar.
    pub health: f32,
    /// [`FLAG_EMBODIED`] | [`FLAG_RING`] | [`FLAG_SELECTED`].
    pub flags: u32,
}

/// A unit-quad corner in local space. Two triangles cover `[-1, 1]^2` (the shader scales by
/// the per-instance half-extent). `repr(C)` so it uploads as the per-vertex stream.
#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct QuadVertex {
    corner: [f32; 2],
}

/// The two triangles of a unit quad, corners in `[-1, 1]^2`.
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

/// Lit-frame clear (command view): a dark slate the units read against.
const CLEAR_LIT: wgpu::Color = wgpu::Color {
    r: 0.02,
    g: 0.03,
    b: 0.05,
    a: 1.0,
};

/// Dark-frame clear (embodied "world goes dark"): near-black. The map is gone.
const CLEAR_DARK: wgpu::Color = wgpu::Color {
    r: 0.0,
    g: 0.0,
    b: 0.0,
    a: 1.0,
};

/// The renderer: an instanced pipeline plus its GPU buffers and camera uniform.
pub struct Renderer {
    pipeline: wgpu::RenderPipeline,
    camera_buf: wgpu::Buffer,
    camera_bind_group: wgpu::BindGroup,
    quad_buf: wgpu::Buffer,
    /// Per-instance GPU buffer; reallocated only when it must grow.
    instance_buf: wgpu::Buffer,
    /// Capacity (in instances) currently allocated in `instance_buf`.
    instance_cap: usize,
    /// CPU-side interpolated instances from the last [`Renderer::prepare`].
    instances: Vec<UnitInstance>,
    /// The embodied directional-alert overlay (worker 2). Drawn as a second LOAD pass by
    /// [`Renderer::render_hud`] when the local player is embodied.
    hud: hud::HudRenderer,
    /// The in-session shell overlay (Phase 4 WS-B). Drawn as a LOAD pass by
    /// [`Renderer::render_overlay`] when an in-session surface (pause/reconnect/summary) is up.
    overlay: overlay::OverlayRenderer,
}

impl Renderer {
    /// Build the instanced pipeline, camera UBO, unit-quad vertex buffer, and a small initial
    /// instance buffer for `surface_format`. The `device` is borrowed (D19).
    pub fn new(device: &wgpu::Device, surface_format: wgpu::TextureFormat) -> Self {
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("gonedark.unit_shader"),
            source: wgpu::ShaderSource::Wgsl(include_str!("shader.wgsl").into()),
        });

        let camera_buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("gonedark.camera_ubo"),
            size: std::mem::size_of::<Camera>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        let camera_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("gonedark.camera_layout"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            }],
        });

        let camera_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("gonedark.camera_bind_group"),
            layout: &camera_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: camera_buf.as_entire_binding(),
            }],
        });

        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("gonedark.pipeline_layout"),
            bind_group_layouts: &[Some(&camera_layout)],
            immediate_size: 0,
        });

        let quad_layout = wgpu::VertexBufferLayout {
            array_stride: std::mem::size_of::<QuadVertex>() as u64,
            step_mode: wgpu::VertexStepMode::Vertex,
            attributes: &wgpu::vertex_attr_array![0 => Float32x2],
        };
        let instance_layout = wgpu::VertexBufferLayout {
            array_stride: std::mem::size_of::<UnitInstance>() as u64,
            step_mode: wgpu::VertexStepMode::Instance,
            // 1=pos(vec2), 2=half_extent(f32), 3=color(vec3), 4=health(f32), 5=flags(u32).
            attributes: &wgpu::vertex_attr_array![
                1 => Float32x2,
                2 => Float32,
                3 => Float32x3,
                4 => Float32,
                5 => Uint32
            ],
        };

        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("gonedark.unit_pipeline"),
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

        let quad_buf = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("gonedark.quad_vbo"),
            contents: bytemuck::cast_slice(&QUAD_VERTS),
            usage: wgpu::BufferUsages::VERTEX,
        });

        let instance_cap = 64;
        let instance_buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("gonedark.instance_vbo"),
            size: (instance_cap * std::mem::size_of::<UnitInstance>()) as u64,
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        let hud = hud::HudRenderer::new(device, surface_format);
        let overlay = overlay::OverlayRenderer::new(device, surface_format);

        Renderer {
            pipeline,
            camera_buf,
            camera_bind_group,
            quad_buf,
            instance_buf,
            instance_cap,
            instances: Vec::new(),
            hud,
            overlay,
        }
    }

    /// Build render instances by interpolating between the previous and current sim snapshots
    /// by `alpha` in `[0,1]` (invariant #4). Produces CPU data only; the GPU upload happens in
    /// [`Renderer::render`]. `selected` carries the command-layer selected world indices so the
    /// renderer rims them (empty while embodied — presentation state only, never sim state).
    pub fn prepare(&mut self, prev: &Snapshot, curr: &Snapshot, alpha: f32, selected: &[u32]) {
        self.instances = interpolate_instances(prev, curr, alpha, selected);
    }

    /// The CPU-side interpolated instances from the last [`Renderer::prepare`].
    pub fn instances(&self) -> &[UnitInstance] {
        &self.instances
    }

    /// Upload instances + camera, clear, record one render pass into `view`, and submit.
    ///
    /// `world_dark` is the embodied "world goes dark" state: when set, the frame clears to
    /// near-black (invariant #6). `fog` is the computed visibility mask for the local viewpoint;
    /// the drawn set is chosen by [`fog::visible_instances`] (worker 1) so unseen enemies vanish
    /// in command view and the map collapses to the avatar's sight while embodied.
    pub fn render(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        view: &wgpu::TextureView,
        camera: &Camera,
        world_dark: bool,
        fog: &Visibility,
    ) {
        queue.write_buffer(&self.camera_buf, 0, bytemuck::bytes_of(camera));

        // Pick the draw set: the fog layer applies visibility (and the dark-frame avatar-only
        // rule) — see `render/src/fog.rs` (worker 1).
        let draw_set: Vec<UnitInstance> = fog::visible_instances(&self.instances, fog, world_dark);

        if draw_set.len() > self.instance_cap {
            let new_cap = draw_set.len().next_power_of_two();
            self.instance_buf = device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("gonedark.instance_vbo"),
                size: (new_cap * std::mem::size_of::<UnitInstance>()) as u64,
                usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });
            self.instance_cap = new_cap;
        }
        if !draw_set.is_empty() {
            queue.write_buffer(&self.instance_buf, 0, bytemuck::cast_slice(&draw_set));
        }

        let clear = if world_dark { CLEAR_DARK } else { CLEAR_LIT };

        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("gonedark.frame_encoder"),
        });
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("gonedark.unit_pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(clear),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                multiview_mask: None,
                timestamp_writes: None,
                occlusion_query_set: None,
            });

            if !draw_set.is_empty() {
                pass.set_pipeline(&self.pipeline);
                pass.set_bind_group(0, &self.camera_bind_group, &[]);
                pass.set_vertex_buffer(0, self.quad_buf.slice(..));
                pass.set_vertex_buffer(1, self.instance_buf.slice(..));
                pass.draw(0..QUAD_VERTS.len() as u32, 0..draw_set.len() as u32);
            }
        }

        queue.submit(std::iter::once(encoder.finish()));
    }

    /// Draw the embodied directional-alert HUD on top of the current frame (a LOAD pass — it
    /// never clears). Delegates to the [`hud::HudRenderer`] (worker 2). The host calls this only
    /// while the local player is embodied (the strategic map is dark and alerts are the only
    /// thread back — invariant #6). `avatar_world` is the listener position, `yaw` its facing.
    #[allow(clippy::too_many_arguments)]
    pub fn render_hud(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        view: &wgpu::TextureView,
        alerts: &AlertChannel,
        avatar_world: (f32, f32),
        yaw: f32,
        viewport: (u32, u32),
        tick: u64,
    ) {
        self.hud.render(
            device,
            queue,
            view,
            alerts,
            avatar_world,
            yaw,
            viewport,
            tick,
        );
    }

    /// Draw the in-session shell overlay (pause / reconnect prompt / post-match summary) on top of
    /// the current frame (a LOAD pass — it never clears), delegating to [`overlay::OverlayRenderer`]
    /// (Phase 4 WS-B). The host hands an [`overlay::Overlay`] describing which surface is up;
    /// [`overlay::Overlay::None`] is a no-op. The overlay is screen-space chrome only — it carries
    /// no world position and never widens the avatar-only fog beneath it (invariant #6).
    pub fn render_overlay(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        view: &wgpu::TextureView,
        overlay: &overlay::Overlay,
    ) {
        self.overlay.render(device, queue, view, overlay);
    }
}

#[cfg(test)]
mod tests {
    //! `render` is the float boundary (invariant #1: floats live only in rendering), so `f32`
    //! math and epsilon comparisons are fair game here — they exercise the device-free
    //! interpolation math, never the GPU. `Renderer::new` needs a real `wgpu::Device` (no
    //! display in CI), so the pipeline path is intentionally untested; the testable math is
    //! factored into `interpolate_instances`.

    use super::*;
    use gonedark_core::components::{Faction, Vec2};
    use gonedark_core::snapshot::{ControlPointSnapshot, Snapshot, UnitSnapshot};

    const EPS: f32 = 1e-4;

    fn unit(x: Fixed, y: Fixed, embodied: bool) -> UnitSnapshot {
        UnitSnapshot {
            entity_index: 0,
            pos: Vec2::new(x, y),
            vel: Vec2::ZERO,
            embodied,
            faction: Faction::Player,
            health: Fixed::ONE,
            building: false,
        }
    }

    fn snapshot(tick: u64, units: Vec<UnitSnapshot>) -> Snapshot {
        Snapshot {
            tick,
            units,
            control_points: Vec::new(),
        }
    }

    // ---- fixed_to_f32 ----

    #[test]
    fn fixed_to_f32_one() {
        assert_eq!(fixed_to_f32(Fixed::ONE), 1.0);
    }

    #[test]
    fn fixed_to_f32_half() {
        assert_eq!(fixed_to_f32(Fixed::HALF), 0.5);
    }

    #[test]
    fn fixed_to_f32_negative() {
        assert_eq!(fixed_to_f32(Fixed::from_int(-3)), -3.0);
        assert_eq!(fixed_to_f32(Fixed::ZERO - Fixed::HALF), -0.5);
    }

    // ---- interpolate_instances: position ----

    #[test]
    fn interpolate_alpha_zero_yields_prev() {
        let prev = snapshot(0, vec![unit(Fixed::from_int(2), Fixed::from_int(4), false)]);
        let curr = snapshot(
            1,
            vec![unit(Fixed::from_int(10), Fixed::from_int(20), false)],
        );
        let out = interpolate_instances(&prev, &curr, 0.0, &[]);
        assert_eq!(out.len(), 1);
        assert!((out[0].x - 2.0).abs() < EPS);
        assert!((out[0].y - 4.0).abs() < EPS);
    }

    #[test]
    fn interpolate_alpha_half_yields_midpoint() {
        let prev = snapshot(0, vec![unit(Fixed::from_int(2), Fixed::from_int(4), false)]);
        let curr = snapshot(
            1,
            vec![unit(Fixed::from_int(10), Fixed::from_int(20), false)],
        );
        let out = interpolate_instances(&prev, &curr, 0.5, &[]);
        assert_eq!(out.len(), 1);
        assert!((out[0].x - 6.0).abs() < EPS);
        assert!((out[0].y - 12.0).abs() < EPS);
    }

    #[test]
    fn interpolate_mismatched_lengths_use_min_no_panic() {
        let prev = snapshot(
            0,
            vec![
                unit(Fixed::ZERO, Fixed::ZERO, false),
                unit(Fixed::ONE, Fixed::ONE, false),
            ],
        );
        let curr = snapshot(
            1,
            vec![unit(Fixed::from_int(10), Fixed::from_int(10), false)],
        );
        let out = interpolate_instances(&prev, &curr, 1.0, &[]);
        assert_eq!(out.len(), 1);
        assert!((out[0].x - 10.0).abs() < EPS);
    }

    // ---- faction color + embodied + flags ----

    #[test]
    fn embodied_unit_is_amber_and_flagged() {
        // curr says embodied → amber color, FLAG_EMBODIED set (survives the dark filter).
        let prev = snapshot(0, vec![unit(Fixed::ZERO, Fixed::ZERO, true)]);
        let curr = snapshot(1, vec![unit(Fixed::ONE, Fixed::ONE, true)]);
        let out = interpolate_instances(&prev, &curr, 0.5, &[]);
        assert_eq!(out[0].flags & FLAG_EMBODIED, FLAG_EMBODIED);
        assert_eq!([out[0].r, out[0].g, out[0].b], AVATAR_COLOR);
    }

    #[test]
    fn faction_drives_color_when_not_embodied() {
        let mut enemy = unit(Fixed::ZERO, Fixed::ZERO, false);
        enemy.faction = Faction::Enemy;
        let s = snapshot(0, vec![enemy]);
        let out = interpolate_instances(&s, &s, 0.0, &[]);
        assert_eq!(
            [out[0].r, out[0].g, out[0].b],
            faction_color(Faction::Enemy)
        );
        assert_eq!(out[0].flags & FLAG_EMBODIED, 0);
    }

    #[test]
    fn building_is_drawn_larger_and_carries_health() {
        let mut b = unit(Fixed::ZERO, Fixed::ZERO, false);
        b.building = true;
        b.health = Fixed::HALF;
        let s = snapshot(0, vec![b]);
        let out = interpolate_instances(&s, &s, 0.0, &[]);
        assert!(out[0].half_extent > UNIT_HALF);
        assert!((out[0].health - 0.5).abs() < EPS);
    }

    #[test]
    fn control_points_append_as_owner_colored_rings() {
        let mut s = snapshot(0, vec![unit(Fixed::ZERO, Fixed::ZERO, false)]);
        s.control_points = vec![ControlPointSnapshot {
            pos: Vec2::new(Fixed::from_int(7), Fixed::from_int(-3)),
            owner: Faction::Enemy,
            progress: Fixed::ZERO,
        }];
        let out = interpolate_instances(&s, &s, 0.0, &[]);
        assert_eq!(out.len(), 2, "one unit + one control point");
        let cp = &out[1];
        assert_eq!(cp.flags & FLAG_RING, FLAG_RING);
        assert_eq!([cp.r, cp.g, cp.b], faction_color(Faction::Enemy));
        assert!((cp.x - 7.0).abs() < EPS && (cp.y + 3.0).abs() < EPS);
        assert!(cp.health < 0.0, "rings carry no health bar");
    }

    #[test]
    fn empty_snapshots_yield_empty() {
        let empty = snapshot(0, vec![]);
        assert!(interpolate_instances(&empty, &empty, 0.5, &[]).is_empty());
    }

    // ---- selection highlight (command-view presentation) ----

    /// Build a unit snapshot carrying an explicit world index, so the selection match has
    /// something to key on.
    fn unit_at(index: u32, x: Fixed, y: Fixed) -> UnitSnapshot {
        let mut u = unit(x, y, false);
        u.entity_index = index;
        u
    }

    /// A unit whose world index is in `selected` gets `FLAG_SELECTED`; others don't.
    #[test]
    fn selected_index_sets_flag_only_on_matching_unit() {
        let s = snapshot(
            0,
            vec![
                unit_at(3, Fixed::ZERO, Fixed::ZERO),
                unit_at(7, Fixed::ONE, Fixed::ONE),
            ],
        );
        let out = interpolate_instances(&s, &s, 0.0, &[7]);
        assert_eq!(out[0].flags & FLAG_SELECTED, 0, "index 3 not selected");
        assert_eq!(
            out[1].flags & FLAG_SELECTED,
            FLAG_SELECTED,
            "index 7 selected"
        );
    }

    /// An empty selection (the embodied case) flags nothing.
    #[test]
    fn empty_selection_flags_nothing() {
        let s = snapshot(0, vec![unit_at(3, Fixed::ZERO, Fixed::ZERO)]);
        let out = interpolate_instances(&s, &s, 0.0, &[]);
        assert_eq!(out[0].flags & FLAG_SELECTED, 0);
    }

    /// Selection rides alongside the embodied flag without clobbering it (both bits coexist).
    #[test]
    fn selection_and_embodied_flags_coexist() {
        let mut u = unit(Fixed::ZERO, Fixed::ZERO, true);
        u.entity_index = 5;
        let s = snapshot(0, vec![u]);
        let out = interpolate_instances(&s, &s, 0.0, &[5]);
        assert_eq!(out[0].flags & FLAG_EMBODIED, FLAG_EMBODIED);
        assert_eq!(out[0].flags & FLAG_SELECTED, FLAG_SELECTED);
    }

    /// Validate `shader.wgsl` offline with naga (the compiler wgpu uses), so a WGSL regression
    /// fails the test suite instead of only blowing up at pipeline creation on a real GPU.
    #[test]
    fn shader_wgsl_parses_and_validates() {
        let src = include_str!("shader.wgsl");
        let module = naga::front::wgsl::parse_str(src).expect("shader.wgsl must parse");
        let mut validator = naga::valid::Validator::new(
            naga::valid::ValidationFlags::all(),
            naga::valid::Capabilities::all(),
        );
        validator
            .validate(&module)
            .expect("shader.wgsl must validate");
    }
}
