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
//! When `world_dark` is set (the local player is embodied) **only embodied instances are uploaded**
//! — the strategic map (other units, buildings, and the territory control points, which are all map
//! intel) genuinely disappears, leaving just the avatar. Filtering happens at upload time in
//! [`Renderer::render`]; [`Renderer::prepare`] still builds the full set so a single un-embodied
//! frame can light the whole map again.
//!
//! "Goes dark" means losing *intel*, not staring at a black void: the host paints a real
//! first-person space underneath — a sky gradient, a gridded ground, and a weapon viewmodel (W5,
//! [`world::WorldRenderer`]) — BEFORE this unit pass loads. That world is a pure function of the
//! *camera* (it has no access to sim entities), so no enemy/building/control-point intel can leak
//! through it; the fairness boundary stays exactly where the [`fog`] filter draws it. The embodied
//! frame is therefore: `world sky/ground (clears)` → this avatar pass (LOADs) → weapon viewmodel →
//! alert HUD.

use gonedark_core::alerts::AlertChannel;
use gonedark_core::components::{Faction, UnitKind};
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
/// On-screen FPS touch controls (the COD-style embodied HUD, Android only). Owns
/// `TouchControlsRenderer`: the move stick + Fire/Crouch/Reload/Surface buttons, drawn as a LOAD
/// pass over the dark embodied frame. Public so the host describes them via [`touch_controls::TouchControlsHud`].
pub mod touch_controls;
/// Band-select marquee. Owns `MarqueeRenderer`: the selection rectangle drawn in the command view
/// while a band-drag is in flight. Public so the host can describe the box via [`marquee::Marquee`].
pub mod marquee;
/// In-session shell overlay (Phase 4 WS-B). Owns `OverlayRenderer`: the pause / reconnect-prompt /
/// post-match-summary chrome, drawn on top of the (possibly dark) match frame. Public so the host
/// can describe which surface to draw via [`overlay::Overlay`].
pub mod overlay;
/// Radial command menu. Owns `RadialRenderer`: the wedge ring a held long-press opens over the
/// command vocabulary, drawn as a LOAD pass in the command view. Public so the host can describe the
/// open menu via [`radial::RadialMenu`].
pub mod radial;

/// Screen-space text pass (W4). Owns `TextRenderer`: a baked bitmap-glyph LOAD pass that draws
/// labels/numbers at an NDC position with a size + color. Public so the host and other render passes
/// can `queue` strings (e.g. radial action names, summary numbers, button labels) and flush them.
pub mod text;

/// Cooked greybox mesh loading + GPU upload (D44). Owns the cooked-`.mesh` parser, the embedded
/// model library ([`mesh::MeshLibrary`]), the depth-texture helper, and the pure `model_matrix`
/// transform math the 3D mesh passes (weapon viewmodel + command-view unit tokens) build on.
pub mod mesh;

/// Embodied first-person world (W5). Owns `WorldRenderer`: the sky/ground + weapon-viewmodel passes
/// that replace the bare near-black embodied void with a real first-person space while keeping the
/// strategic map dark (invariant #6 — it draws ONLY the camera-derived environment + a screen-space
/// gun, never any sim entity). Public so the host can build the [`world::WorldUniform`].
pub mod world;

/// Command-view ground grid (W6). Owns `TerrainRenderer`: a world-space lattice drawn under the units
/// (first in the command pass) so position/motion read against a fixed reference instead of flat
/// slate. Public so the pure `grid_lines` layout seam is reachable; the `Renderer` drives the pass.
pub mod terrain;

/// Command-view readouts (W6). Pure derivation of a unit/point/objective tally from the draw set the
/// renderer already holds, laid out as corner labels for the W4 text pass — no new sim read. Public
/// so the `tally` / `readout_labels` seams are reachable; the `Renderer` drives the text.
pub mod readout;
/// Command-view upgrade panel — the readable per-camp tier display ("growth" half of command-and-
/// grow). Pure derivation of current tier / next-tier cost / production-speed effect / affordability
/// from host-supplied camp level + resources, laid out as a corner panel for the text pass. No sim
/// read (invariant #4); public so the `upgrade_view` / `upgrade_labels` seams are reachable.
pub mod upgrade_panel;

/// Command-view build palette (Phase 2). Pure layout of the placeable-structure palette — label,
/// const cost, and a host-supplied affordability flag — for the W4 text pass; reaches into no sim
/// state (only the `core` const cost table). Public so the `build_menu_entries` seam is reachable.
pub mod build_menu;
/// Command-view troop-training panel (Phase 2). Pure layout of per-unit cost + production ETA + the
/// live queue from the static `economy` tables + host-supplied dynamic state (camp level, resources,
/// queue) — no new sim read, the `readout` pattern. Public so the `train_options` / `train_panel_labels`
/// seams are reachable; the host drives the text pass.
pub mod train_panel;

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
pub const FLAG_MESH: u32 = 8; // a 3D token mesh draws this body — the quad is UI decals only (D44)

/// Drawn half-extent (world units) per kind. Render-only cosmetic scale.
const UNIT_HALF: f32 = 0.5;
const BUILDING_HALF: f32 = 1.6;
const CONTROL_POINT_HALF: f32 = 2.2;

/// Uniform scale applied to the 3D token mesh per model (D44), tuned so the greybox model roughly
/// fills its command-view footprint marker. The infantry mesh is ~0.45 m wide, so ~2.2× brings it
/// up to the ~1 m unit marker; the tank hull is ~3 m, so it scales *down* to read as a comparable
/// (slightly heavier) token; the camp is already structure-sized. Render-only cosmetic scale.
const UNIT_TOKEN_SCALE: f32 = 2.2;
const TANK_TOKEN_SCALE: f32 = 0.42;
const BUILDING_TOKEN_SCALE: f32 = 2.2;

/// The 3D token mesh for a snapshot unit: buildings are the camp structure; units map by their
/// producible archetype (`Heavy`→tank, `Rifleman`/default→infantry). This is the honest greybox
/// wiring now that the sim carries a per-unit [`UnitKind`] (snapshot `unit_kind`). Pure + testable.
pub(crate) fn model_for_unit(building: bool, kind: UnitKind) -> mesh::ModelKind {
    if building {
        mesh::ModelKind::CampHq
    } else {
        match kind {
            UnitKind::Heavy => mesh::ModelKind::Tank,
            UnitKind::Rifleman => mesh::ModelKind::Trooper,
        }
    }
}

/// The command-view token scale for a resolved [`mesh::ModelKind`].
fn token_scale(kind: mesh::ModelKind) -> f32 {
    match kind {
        mesh::ModelKind::CampHq => BUILDING_TOKEN_SCALE,
        mesh::ModelKind::Tank => TANK_TOKEN_SCALE,
        _ => UNIT_TOKEN_SCALE,
    }
}

/// Pick the 3D token mesh + scale for a command-view instance, or `None` for instances that are not
/// drawn as a mesh (control-point rings keep their hollow-ring quad). The mesh kind was resolved
/// from the snapshot's unit-kind/building flag into [`UnitInstance::model`] (see [`model_for_unit`])
/// when the instance was built; here we just decode it and pick the cosmetic scale. Pure + testable.
fn token_for(inst: &UnitInstance) -> Option<(mesh::ModelKind, f32)> {
    if inst.flags & FLAG_RING != 0 {
        return None; // control points stay hollow rings (no mesh for them yet)
    }
    // `model` is always a valid ModelKind discriminant (written by `model_for_unit`); a direct
    // index panics loudly in debug if some future path forgets to set it, rather than silently
    // drawing the wrong mesh.
    let kind = mesh::ModelKind::ALL[inst.model as usize];
    Some((kind, token_scale(kind)))
}

/// Sentinel health value meaning "draw no health bar" (control points).
const NO_HEALTH_BAR: f32 = -1.0;

/// Static first-person world dressing (W5 follow-on): scenery + cover props placed around the
/// battlefield so the embodied view reads as a *place*, not a bare ground/sky void. Each entry is
/// `(kind, x, y, yaw_radians, scale)` in world metres. This is **render-only environment** — a
/// fixed cosmetic layout with no sim entity behind it, so it reveals no map intel (it is terrain,
/// not unit/enemy positions) and stays fair under "world goes dark" (invariant #6). Drawn at the
/// LOD [`mesh::select_lod`] picks from the eye distance, so distant scenery costs fewer triangles.
const PROP_LAYOUT: &[(mesh::ModelKind, f32, f32, f32, f32)] = &[
    // Tree line / scenery (soft cover) — desaturated greens, varied scale.
    (mesh::ModelKind::Tree, -19.0, 14.0, 0.4, 1.10),
    (mesh::ModelKind::Tree, -22.0, 6.0, 1.9, 0.95),
    (mesh::ModelKind::Tree, 17.0, 18.0, 2.7, 1.20),
    (mesh::ModelKind::Tree, 24.0, -9.0, 0.9, 1.00),
    (mesh::ModelKind::Tree, -6.0, 26.0, 3.5, 1.05),
    // Boulders (hard cover) — grey, low.
    (mesh::ModelKind::Rock, -12.0, -8.0, 0.2, 1.30),
    (mesh::ModelKind::Rock, 9.0, 11.0, 2.2, 0.90),
    (mesh::ModelKind::Rock, 20.0, 4.0, 4.0, 1.10),
    (mesh::ModelKind::Rock, -2.0, -22.0, 1.1, 1.00),
    // Supply crates (light cover) clustered near the centre.
    (mesh::ModelKind::Crate, 3.0, 4.0, 0.3, 1.00),
    (mesh::ModelKind::Crate, 4.2, 4.0, 0.3, 1.00),
    (mesh::ModelKind::Crate, 3.6, 5.1, 0.8, 1.00),
    (mesh::ModelKind::Crate, -8.0, 7.0, 1.4, 1.00),
    // Sandbag berms (medium cover) — defensive lines.
    (mesh::ModelKind::Barricade, -4.0, -3.0, 0.0, 1.20),
    (mesh::ModelKind::Barricade, 2.0, -6.0, 1.57, 1.10),
    (mesh::ModelKind::Barricade, 12.0, -2.0, 0.5, 1.00),
    // Defensive turret emplacements — read as fortified points.
    (mesh::ModelKind::Turret, -14.0, 2.0, 0.6, 1.00),
    (mesh::ModelKind::Turret, 15.0, 9.0, 3.4, 1.00),
];

/// Map the static [`PROP_LAYOUT`] to concrete draw items for an `eye` position: each prop's kind,
/// the LOD tier [`mesh::select_lod`] picks from its eye distance, and its world-space mesh instance
/// (greybox base tint, no flash). Pure + GPU-free, so the LOD selection + placement is unit-tested
/// without a device; [`Renderer::render_world_props`] just groups the result into batches.
fn prop_draw_plan(eye: [f32; 3]) -> Vec<(mesh::ModelKind, usize, mesh::MeshInstance)> {
    PROP_LAYOUT
        .iter()
        .map(|&(kind, x, y, yaw, scale)| {
            let (dx, dy, dz) = (x - eye[0], y - eye[1], -eye[2]);
            let dist = (dx * dx + dy * dy + dz * dz).sqrt();
            let c = kind.base_color();
            let inst = mesh::MeshInstance {
                model: mesh::model_matrix([x, y, 0.0], scale, yaw),
                color: [c[0], c[1], c[2], 0.0],
            };
            (kind, mesh::select_lod(dist), inst)
        })
        .collect()
}

/// The base RGB color for a faction (the embodied avatar overrides this to amber).
pub fn faction_color(faction: Faction) -> [f32; 3] {
    match faction {
        Faction::Player => [0.25, 0.60, 0.95],  // cool blue
        Faction::Enemy => [0.90, 0.32, 0.26],   // hostile red
        Faction::Neutral => [0.55, 0.55, 0.60], // neutral grey
    }
}

/// The embodied avatar's color — warm amber, the unit you possess. `pub(crate)` so the command-view
/// readout (`readout.rs`) can exclude the avatar from the per-faction unit tally.
pub(crate) const AVATAR_COLOR: [f32; 3] = [1.0, 0.85, 0.2];

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
        // Resolve the 3D token mesh from the sim's unit-kind / building flag (Heavy→tank etc.).
        let model = model_for_unit(b.building, b.unit_kind) as u32;

        out.push(UnitInstance {
            x: ax + (bx - ax) * alpha,
            y: ay + (by - ay) * alpha,
            half_extent,
            r: color[0],
            g: color[1],
            b: color[2],
            health,
            flags,
            model,
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
            model: 0, // unused: FLAG_RING makes `token_for` return None (rings stay hollow quads)
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

/// The player's currently-active camp, as the plain numbers the per-camp panels need. The host
/// resolves *which* camp is active (deterministically, over the sim) and copies these out — the
/// renderer never reads the sim. `queue` is the FIFO production queue as `(kind, ticks_remaining)`
/// pairs, front = in-production.
pub struct ActiveCamp<'a> {
    /// Upgrade tier (0 = base) — drives production-speed + the upgrade panel's "next tier".
    pub level: u8,
    /// FIFO production queue: `(unit, ticks_remaining)`, front item in production.
    pub queue: &'a [(UnitKind, u16)],
}

/// Plain sim-derived inputs for [`Renderer::render_command_panels`] — the command-view build/train/
/// upgrade chrome. The host fills this from the (checksummed) sim each command frame; the renderer
/// holds no sim read. `active_camp` is `None` when the player has no camp (only the build palette
/// renders then).
pub struct CommandPanels<'a> {
    /// The player faction's banked credits — drives affordability across all three panels.
    pub resources: i64,
    /// Unit kinds the player can train (the train panel lists these). Order is the display order.
    pub trainable: &'a [UnitKind],
    /// The active camp's per-camp state, or `None` if the player has no built camp.
    pub active_camp: Option<ActiveCamp<'a>>,
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
    /// The 3D token mesh this instance draws as ([`mesh::ModelKind`] `as u32`), resolved from the
    /// snapshot's unit-kind / building flag by [`model_for_unit`]. CPU-side only — [`token_for`]
    /// reads it to bucket the mesh pass; it is the trailing field so the quad pipeline's instance
    /// attributes (locations 1..=5, fixed offsets) are untouched and the GPU never reads it.
    pub model: u32,
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
    /// The on-screen FPS touch-control HUD (Android). Drawn as a LOAD pass by
    /// [`Renderer::render_touch_controls`] when the local player is embodied on a touch device.
    touch_controls: touch_controls::TouchControlsRenderer,
    /// The in-session shell overlay (Phase 4 WS-B). Drawn as a LOAD pass by
    /// [`Renderer::render_overlay`] when an in-session surface (pause/reconnect/summary) is up.
    overlay: overlay::OverlayRenderer,
    /// The radial command menu. Drawn as a LOAD pass by [`Renderer::render_radial`] in the command
    /// view when a held long-press has a menu open.
    radial: radial::RadialRenderer,
    /// The band-select marquee. Drawn as a LOAD pass by [`Renderer::render_marquee`] in the command
    /// view while a band-drag is in flight.
    marquee: marquee::MarqueeRenderer,
    /// The embodied first-person world (W5). The host calls [`Renderer::render_world_sky`] FIRST in
    /// the embodied branch (it clears to a sky/ground) and [`Renderer::render_world_weapon`] after
    /// the avatar pass (the gun viewmodel). Draws only the camera-derived environment — no intel.
    world: world::WorldRenderer,
    /// The command-view ground grid (W6). A world-space lattice drawn FIRST in the command pass
    /// (under the units) so position/motion read against a fixed reference. Shares the unit pass's
    /// camera bind group, so it uses the same top-down view-projection.
    terrain: terrain::TerrainRenderer,
    /// The screen-space text pass (W4), owned here so the command pass can draw its readout labels
    /// (unit/enemy/point counts) as a final LOAD pass over the command frame. Other hosts still own
    /// their own `TextRenderer` for menus/summaries; this one is dedicated to the command readouts.
    text: text::TextRenderer,
    /// Embedded greybox mesh library (D44): the cooked `.mesh` for every [`mesh::ModelKind`],
    /// parsed + uploaded once. The weapon viewmodel and command-view unit tokens draw from it.
    mesh_lib: mesh::MeshLibrary,
    /// The shared instanced, depth-tested 3D mesh pipeline (D44) — drives both the weapon viewmodel
    /// and the unit tokens.
    mesh_pipeline: mesh::MeshPipeline,
    /// Depth buffer for the 3D mesh passes, lazily (re)created to match the surface size
    /// ([`Renderer::ensure_depth`]). Render-only — depth never touches the sim (invariant #1/#4).
    depth_view: wgpu::TextureView,
    /// The `(width, height)` `depth_view` is currently sized for.
    depth_size: (u32, u32),
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
        let touch_controls = touch_controls::TouchControlsRenderer::new(device, surface_format);
        let overlay = overlay::OverlayRenderer::new(device, surface_format);
        let radial = radial::RadialRenderer::new(device, surface_format);
        let marquee = marquee::MarqueeRenderer::new(device, surface_format);
        let world = world::WorldRenderer::new(device, surface_format);
        // The ground grid shares the unit pass's camera layout so it uses the same view-projection.
        let terrain = terrain::TerrainRenderer::new(device, surface_format, &camera_layout);
        let text = text::TextRenderer::new(device, surface_format);
        // Cooked greybox meshes + the shared 3D mesh pipeline + an initial (placeholder) depth
        // buffer; the depth buffer is resized to the surface on the first mesh pass (D44).
        let mesh_lib = mesh::MeshLibrary::load(device);
        let mesh_pipeline = mesh::MeshPipeline::new(device, surface_format);
        let depth_view = mesh::create_depth_view(device, 1, 1);

        Renderer {
            pipeline,
            camera_buf,
            camera_bind_group,
            quad_buf,
            instance_buf,
            instance_cap,
            instances: Vec::new(),
            hud,
            touch_controls,
            overlay,
            radial,
            marquee,
            world,
            terrain,
            text,
            mesh_lib,
            mesh_pipeline,
            depth_view,
            depth_size: (1, 1),
        }
    }

    /// Ensure the mesh-pass depth buffer matches `(width, height)`, recreating it only when the
    /// surface size changes. Cheap — a no-op on an unchanged size.
    fn ensure_depth(&mut self, device: &wgpu::Device, width: u32, height: u32) {
        let size = (width.max(1), height.max(1));
        if self.depth_size != size {
            self.depth_view = mesh::create_depth_view(device, size.0, size.1);
            self.depth_size = size;
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

    /// Upload the camera + fog-filtered draw set and render the frame (invariant #4/#6).
    ///
    /// `world_dark` is the embodied "world goes dark" state. While **embodied** the host has already
    /// cleared the frame to the first-person world ([`Renderer::render_world_sky`]); this LOADs the
    /// avatar quad over it — no ground grid, no 3D tokens, just the one instance the fog filter
    /// leaves (invariant #6). In **command view** the frame is composited in three passes so the 3D
    /// greybox tokens (D44) sit between the ground and the UI:
    ///  1. **ground grid** — CLEARS to the lit slate the field reads against (W6);
    ///  2. **3D unit/structure tokens** — depth-tested meshes ([`token_for`] picks infantry vs
    ///     structure) LOADed over the grid;
    ///  3. **2D quad UI** — health bars, selection rims, control-point rings — LOADed on top, with
    ///     each token's body fill suppressed ([`FLAG_MESH`]) so the mesh shows through.
    ///
    /// Either way [`fog::visible_instances`] (worker 1) chooses the draw set, so unseen enemies
    /// vanish in command view and the map collapses to the avatar alone while embodied — the
    /// fairness boundary is unchanged; the 3D tokens are drawn only from that already-fogged set.
    /// `width`/`height` size the depth buffer for the token pass.
    #[allow(clippy::too_many_arguments)]
    pub fn render(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        view: &wgpu::TextureView,
        camera: &Camera,
        world_dark: bool,
        fog: &Visibility,
        width: u32,
        height: u32,
        economy: Option<readout::EconomyReadout>,
    ) {
        queue.write_buffer(&self.camera_buf, 0, bytemuck::bytes_of(camera));

        // Pick the draw set: the fog layer applies visibility (and the dark-frame avatar-only
        // rule) — see `render/src/fog.rs` (worker 1).
        let draw_set: Vec<UnitInstance> = fog::visible_instances(&self.instances, fog, world_dark);

        if world_dark {
            // Embodied: LOAD the avatar over the first-person world the host already drew. No grid,
            // no tokens — the map is dark and only the avatar survives the fog filter (invariant #6).
            self.draw_quads(device, queue, view, &draw_set, wgpu::LoadOp::Load);
            return;
        }

        // --- Command view ---------------------------------------------------------------------
        // 1. Ground grid, which CLEARS the frame to the lit slate (under everything else).
        self.draw_terrain_clear(device, queue, view);

        // 2. Build the 3D token batches (units → infantry/tank by unit-kind, buildings → structure)
        //    and, in lockstep, a quad set flagged FLAG_MESH so the quad shader draws only the UI
        //    decals over them. One bucket per `ModelKind` so any token mesh (incl. the Heavy tank)
        //    draws without special-casing. Command-view tokens use LOD0 — top-down close scrutiny.
        self.ensure_depth(device, width, height);
        let mut buckets: Vec<Vec<mesh::MeshInstance>> =
            (0..mesh::ModelKind::ALL.len()).map(|_| Vec::new()).collect();
        let mut quad_set = draw_set.clone();
        for inst in &mut quad_set {
            if let Some((kind, scale)) = token_for(inst) {
                inst.flags |= FLAG_MESH;
                buckets[kind as usize].push(mesh::MeshInstance {
                    model: mesh::model_matrix([inst.x, inst.y, 0.0], scale, 0.0),
                    color: [inst.r, inst.g, inst.b, 0.0], // faction tint; a=0 → no flash
                });
            }
        }
        let batches: Vec<mesh::MeshBatch> = mesh::ModelKind::ALL
            .iter()
            .zip(buckets)
            .filter(|(_, instances)| !instances.is_empty())
            .map(|(&kind, instances)| mesh::MeshBatch {
                mesh: self.mesh_lib.get(kind),
                instances,
            })
            .collect();
        self.mesh_pipeline.draw(
            device,
            queue,
            view,
            &self.depth_view,
            &camera.view_proj,
            mesh::MeshPipeline::DEFAULT_LIGHT,
            wgpu::LoadOp::Load,
            &batches,
        );
        drop(batches); // release the &self.mesh_lib borrow before the &mut self quad pass

        // 3. The 2D quad UI (LOAD), with token bodies suppressed so the meshes show through.
        self.draw_quads(device, queue, view, &quad_set, wgpu::LoadOp::Load);

        // Command-view readouts (W6): a unit/enemy/point tally derived from the SAME fog-filtered
        // draw set (the un-flagged copy) — no new sim read — laid out as corner labels and drawn via
        // the W4 text pass over the command frame. Screen-space chrome only (invariant #6). The
        // `economy` seam is the host-supplied `EconomyReadout` (banked credits + income; the renderer
        // has no sim read, so the host reads it off the sim and hands it in). Both the tally and the
        // economy lines are gated by the real `world_dark`: while embodied `readout_labels` returns an
        // EMPTY set, so the command readout never draws over the dark frame (invariant #6).
        let t = readout::tally(&draw_set);
        for label in readout::readout_labels(&t, economy, world_dark) {
            self.text.queue(
                label.text,
                label.pos,
                label.px_size,
                label.anchor,
                label.color,
                label.alpha,
            );
        }
        self.text.render(device, queue, view);
    }

    /// Clear the frame to the lit command-view slate and draw the ground grid (W6) — the command
    /// view's clearing pass, drawn under the 3D tokens and the quad UI.
    fn draw_terrain_clear(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        view: &wgpu::TextureView,
    ) {
        // The grid uses the camera uniform already uploaded at the top of `render()` — no per-pass
        // upload needed here, so nothing writes to `queue` until the final submit.
        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("gonedark.terrain_encoder"),
        });
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("gonedark.terrain_pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(CLEAR_LIT),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                multiview_mask: None,
                timestamp_writes: None,
                occlusion_query_set: None,
            });
            self.terrain.draw(&mut pass, &self.camera_bind_group);
        }
        queue.submit(std::iter::once(encoder.finish()));
    }

    /// Upload `instances` and draw them through the 2D quad pipeline into `view` with `load`. Grows
    /// the instance buffer as needed; the pass still runs when empty so `load` (clear/load) applies.
    fn draw_quads(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        view: &wgpu::TextureView,
        instances: &[UnitInstance],
        load: wgpu::LoadOp<wgpu::Color>,
    ) {
        if instances.len() > self.instance_cap {
            let new_cap = instances.len().next_power_of_two();
            self.instance_buf = device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("gonedark.instance_vbo"),
                size: (new_cap * std::mem::size_of::<UnitInstance>()) as u64,
                usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });
            self.instance_cap = new_cap;
        }
        if !instances.is_empty() {
            queue.write_buffer(&self.instance_buf, 0, bytemuck::cast_slice(instances));
        }

        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("gonedark.quad_encoder"),
        });
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("gonedark.unit_pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load,
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                multiview_mask: None,
                timestamp_writes: None,
                occlusion_query_set: None,
            });
            if !instances.is_empty() {
                pass.set_pipeline(&self.pipeline);
                pass.set_bind_group(0, &self.camera_bind_group, &[]);
                pass.set_vertex_buffer(0, self.quad_buf.slice(..));
                pass.set_vertex_buffer(1, self.instance_buf.slice(..));
                pass.draw(0..QUAD_VERTS.len() as u32, 0..instances.len() as u32);
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

    /// Draw the on-screen FPS touch-control HUD (move stick + Fire/Crouch/Reload/Surface) on top of
    /// the current frame (a LOAD pass — never clears), delegating to
    /// [`touch_controls::TouchControlsRenderer`]. The host calls this only while embodied on a touch
    /// device (the GUI is Android-only); `hud` describes the controls to draw this frame.
    pub fn render_touch_controls(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        view: &wgpu::TextureView,
        hud: &touch_controls::TouchControlsHud,
    ) {
        self.touch_controls.render(device, queue, view, hud);
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

    /// Draw the radial command menu on top of the current frame (a LOAD pass — it never clears),
    /// delegating to [`radial::RadialRenderer`]. The host calls this only in the command view, when a
    /// held long-press has a menu open ([`radial::RadialMenu::slots`] > 0). It is screen-space chrome
    /// with no world position and is never drawn over the dark embodied frame (invariant #6).
    pub fn render_radial(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        view: &wgpu::TextureView,
        menu: &radial::RadialMenu,
    ) {
        self.radial.render(device, queue, view, menu);
    }

    /// Draw the band-select marquee on top of the current frame (a LOAD pass — never clears),
    /// delegating to [`marquee::MarqueeRenderer`]. The host calls this only in the command view while
    /// a band-drag is in flight. Screen-space chrome with no world position; never over the dark
    /// embodied frame (invariant #6).
    pub fn render_marquee(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        view: &wgpu::TextureView,
        marquee: &marquee::Marquee,
    ) {
        self.marquee.render(device, queue, view, marquee);
    }

    /// Draw the command-view **build / train / upgrade** panels — the "command and grow your camps"
    /// chrome — on top of the current command frame (a LOAD text pass; never clears). The host calls
    /// this ONLY in the command view (never embodied → never over the dark frame, invariant #6) and
    /// supplies plain sim-derived numbers in [`CommandPanels`]; this method calls the pure layout
    /// seams ([`build_menu`]/[`train_panel`]/[`upgrade_panel`]) and queues their labels through the
    /// dedicated readout text pass. The build palette is always offered; the train + upgrade panels
    /// only render when a camp is active ([`CommandPanels::active_camp`] is `Some`). Screen-space
    /// chrome with no world position — it leaks no intel the (fog-filtered) command frame doesn't
    /// already show.
    pub fn render_command_panels(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        view: &wgpu::TextureView,
        panels: &CommandPanels,
    ) {
        // Build palette — always offered in the command view (what you can place + cost + whether
        // you can afford it). Static info, so it needs no active camp.
        for e in build_menu::build_menu_entries(panels.resources) {
            self.text
                .queue(e.text, e.pos, e.px_size, e.anchor, e.color, e.alpha);
        }

        // Train + upgrade panels are per-camp — only when a camp is active.
        if let Some(camp) = &panels.active_camp {
            for l in train_panel::train_panel_labels(
                panels.trainable,
                camp.level,
                panels.resources,
                camp.queue,
            ) {
                self.text
                    .queue(l.text, l.pos, l.px_size, l.anchor, l.color, l.alpha);
            }
            let uview = upgrade_panel::upgrade_view(camp.level, panels.resources);
            for l in upgrade_panel::upgrade_labels(&uview) {
                self.text
                    .queue(l.text, l.pos, l.px_size, l.anchor, l.color, l.alpha);
            }
        }

        self.text.render(device, queue, view);
    }

    /// Draw the embodied first-person world's sky + ground (W5), delegating to
    /// [`world::WorldRenderer`]. This is the CLEARING pass of the embodied frame — the host calls it
    /// FIRST in the embodied branch (before [`Renderer::render`]'s now-LOADing avatar pass), so the
    /// avatar composites onto a real first-person space instead of a black void. The world is a pure
    /// function of the camera (no sim entities), so it reveals **no** map intel (invariant #6 stays
    /// intact — the fog filter is the fairness boundary, and this only paints the environment).
    pub fn render_world_sky(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        view: &wgpu::TextureView,
        uniform: &world::WorldUniform,
    ) {
        self.world.render_sky(device, queue, view, uniform);
    }

    /// Draw the static first-person world dressing (scenery + cover props, [`PROP_LAYOUT`]) over the
    /// embodied sky/ground. The host calls this in the embodied branch AFTER [`render_world_sky`]
    /// (the clearing pass) and before [`Renderer::render`]'s avatar pass, so props composite onto
    /// the real first-person space. `view_proj` is the embodied camera matrix and `eye` its world
    /// position (so [`mesh::select_lod`] can pick a tier per prop by distance). Render-only
    /// environment → reveals no map intel (invariant #6); `width`/`height` size the depth buffer.
    #[allow(clippy::too_many_arguments)]
    pub fn render_world_props(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        view: &wgpu::TextureView,
        view_proj: &[[f32; 4]; 4],
        eye: [f32; 3],
        width: u32,
        height: u32,
    ) {
        self.ensure_depth(device, width, height);
        let plan = prop_draw_plan(eye);
        // One batch per (kind, lod) actually used, so each draws its own decimated mesh tier.
        let mut batches: Vec<mesh::MeshBatch> = Vec::new();
        for &kind in mesh::ModelKind::ALL.iter() {
            for lod in 0..mesh::LOD_COUNT {
                let instances: Vec<mesh::MeshInstance> = plan
                    .iter()
                    .filter(|(k, l, _)| *k == kind && *l == lod)
                    .map(|(_, _, inst)| *inst)
                    .collect();
                if !instances.is_empty() {
                    batches.push(mesh::MeshBatch {
                        mesh: self.mesh_lib.get_lod(kind, lod),
                        instances,
                    });
                }
            }
        }
        self.mesh_pipeline.draw(
            device,
            queue,
            view,
            &self.depth_view,
            view_proj,
            mesh::MeshPipeline::DEFAULT_LIGHT,
            wgpu::LoadOp::Load,
            &batches,
        );
    }

    /// Draw the embodied weapon viewmodel (W5/D44) on top of the current frame (a LOAD colour pass —
    /// never clears colour; clears its own depth). The gun is the real `weapon_rifle` greybox **3D
    /// mesh** drawn through the shared [`mesh::MeshPipeline`], anchored in **view space** by
    /// [`world::weapon_view_model`] so it stays glued to the lower-right under camera yaw. The host
    /// hands in the **projection alone** (`proj`, column-major) as the camera matrix — the model
    /// matrix is the view-space placement — plus the muzzle-`flash` intensity (clamped `[0,1]`),
    /// which both flares the gun and kicks it back as recoil. `width`/`height` size the depth buffer.
    /// The host calls this AFTER [`Renderer::render`] (so the gun sits over the world + avatar) and
    /// before the alert HUD. It has no world position → reveals no intel (invariant #6).
    #[allow(clippy::too_many_arguments)]
    pub fn render_world_weapon(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        view: &wgpu::TextureView,
        proj: &[[f32; 4]; 4],
        flash: f32,
        width: u32,
        height: u32,
    ) {
        self.ensure_depth(device, width, height);
        let f = flash.clamp(0.0, 1.0);
        let c = mesh::ModelKind::WeaponRifle.base_color();
        let batch = mesh::MeshBatch {
            mesh: self.mesh_lib.get(mesh::ModelKind::WeaponRifle),
            // color.a carries the flash so the shader adds a warm emissive flare on fire.
            instances: vec![mesh::MeshInstance {
                model: world::weapon_view_model(f),
                color: [c[0], c[1], c[2], f],
            }],
        };
        self.mesh_pipeline.draw(
            device,
            queue,
            view,
            &self.depth_view,
            proj,
            mesh::MeshPipeline::DEFAULT_LIGHT,
            wgpu::LoadOp::Load,
            &[batch],
        );
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
            unit_kind: UnitKind::Rifleman,
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

    // ---- 3D token mapping (D44) ----

    #[test]
    fn model_for_unit_maps_archetype_and_building() {
        assert_eq!(
            model_for_unit(false, UnitKind::Rifleman),
            mesh::ModelKind::Trooper
        );
        assert_eq!(model_for_unit(false, UnitKind::Heavy), mesh::ModelKind::Tank);
        // A building is the camp structure regardless of the (irrelevant) unit-kind tag.
        assert_eq!(
            model_for_unit(true, UnitKind::Heavy),
            mesh::ModelKind::CampHq
        );
    }

    #[test]
    fn token_for_decodes_model_scale_and_skips_rings() {
        // A Rifleman token (model = Trooper) → infantry mesh at the unit scale.
        let mut u = UnitInstance {
            model: mesh::ModelKind::Trooper as u32,
            ..Default::default()
        };
        assert_eq!(
            token_for(&u),
            Some((mesh::ModelKind::Trooper, UNIT_TOKEN_SCALE))
        );

        // A Heavy token (model = Tank) → tank mesh at the (smaller) tank scale.
        u.model = mesh::ModelKind::Tank as u32;
        assert_eq!(token_for(&u), Some((mesh::ModelKind::Tank, TANK_TOKEN_SCALE)));

        // A building token (model = CampHq) → structure mesh at the building scale.
        u.model = mesh::ModelKind::CampHq as u32;
        assert_eq!(
            token_for(&u),
            Some((mesh::ModelKind::CampHq, BUILDING_TOKEN_SCALE))
        );

        // A control-point ring gets no mesh (it stays a hollow-ring quad).
        let ring = UnitInstance {
            half_extent: CONTROL_POINT_HALF,
            flags: FLAG_RING,
            ..Default::default()
        };
        assert_eq!(token_for(&ring), None);
    }

    /// `interpolate_instances` resolves each unit's token mesh from its snapshot `unit_kind`:
    /// a Heavy carries the tank model index, a Rifleman the infantry model index.
    #[test]
    fn interpolate_sets_token_model_from_unit_kind() {
        let mut heavy = unit(Fixed::ZERO, Fixed::ZERO, false);
        heavy.unit_kind = UnitKind::Heavy;
        let rifle = unit(Fixed::ONE, Fixed::ONE, false);
        let s = snapshot(0, vec![heavy, rifle]);
        let out = interpolate_instances(&s, &s, 0.0, &[]);
        assert_eq!(out[0].model, mesh::ModelKind::Tank as u32, "Heavy → tank");
        assert_eq!(
            out[1].model,
            mesh::ModelKind::Trooper as u32,
            "Rifleman → infantry"
        );
        // Both decode back through token_for to their mesh.
        assert_eq!(token_for(&out[0]).unwrap().0, mesh::ModelKind::Tank);
        assert_eq!(token_for(&out[1]).unwrap().0, mesh::ModelKind::Trooper);
    }

    /// The FPS world-dressing plan covers every prop, picks coarser LODs as the eye recedes, and
    /// never indexes past the library.
    #[test]
    fn prop_draw_plan_covers_layout_and_picks_lod_by_distance() {
        // Eye on top of the crate cluster (~(3.6, 4.5)) at eye height: nearby props are LOD0,
        // the far tree line drops to a coarser tier.
        let plan = prop_draw_plan([3.6, 4.5, 1.5]);
        assert_eq!(plan.len(), PROP_LAYOUT.len(), "every prop is planned");
        for (_, lod, _) in &plan {
            assert!(*lod < mesh::LOD_COUNT, "lod is a valid library index");
        }
        // A crate at the cluster centre is within LOD1_DISTANCE → full detail.
        let crate_near = plan
            .iter()
            .find(|(k, _, _)| *k == mesh::ModelKind::Crate)
            .unwrap();
        assert_eq!(crate_near.1, 0, "the near crate cluster keeps LOD0");
        // The far tree line (>22 m away) drops to the coarsest tier.
        let far_tree = plan
            .iter()
            .filter(|(k, _, _)| *k == mesh::ModelKind::Tree)
            .map(|(_, lod, _)| *lod)
            .max()
            .unwrap();
        assert_eq!(far_tree, 2, "a distant tree uses LOD2");
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
