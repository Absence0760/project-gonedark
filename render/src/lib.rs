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
use gonedark_core::components::{Army, Faction, UnitKind};
use gonedark_core::fixed::Fixed;
use gonedark_core::fog::Visibility;
use gonedark_core::snapshot::Snapshot;
use gonedark_core::trig::{Angle, ANGLE_FULL};
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
/// Embodied **tank** gunner-sight HUD (tank embodiment P8, D55). Owns `TankHudRenderer`: the
/// hull-relative turret indicator, the dispersion reticle, the LEAD pip, and the reload ring, drawn
/// as a LOAD pass over the dark embodied frame while the local player drives a tank. Public so the
/// host fills the [`tank_hud::TankHudState`] from the embodied tank's (read-only) sim state.
pub mod tank_hud;
/// Band-select marquee. Owns `MarqueeRenderer`: the selection rectangle drawn in the command view
/// while a band-drag is in flight. Public so the host can describe the box via [`marquee::Marquee`].
pub mod marquee;
/// Debug hitbox / facet overlay. Owns `DebugRenderer`: the command-view, world-space line pass that
/// draws each unit's hit-radius ring (colored by armour facet for tanks), a hull-heading spoke, and
/// shell tracers — the visual "see the hitboxes" half of the duel sandbox, behind a developer
/// toggle. Drawn as a LOAD pass by [`Renderer::render_debug`].
pub mod debug;
/// "Gone dark" detection tell. Owns `DetectionRenderer`: the command-view, world-space line pass
/// that marks each hostile EMBODIED enemy the local commander can currently sense (`core::detection`,
/// D33) — a diamond + caret at the unit's live-or-last-seen position, fading as a `Subtle` linger
/// ages. Drawn as a LOAD pass by [`Renderer::render_detection`]. Public so the host (engine) builds
/// the markers via [`detection::DetectionMarker`]. Command-view only (invariant #6).
pub mod detection;
/// In-session shell overlay (Phase 4 WS-B). Owns `OverlayRenderer`: the pause / reconnect-prompt /
/// post-match-summary chrome, drawn on top of the (possibly dark) match frame. Public so the host
/// can describe which surface to draw via [`overlay::Overlay`].
pub mod overlay;
/// Radial command menu. Owns `RadialRenderer`: the wedge ring a held long-press opens over the
/// command vocabulary, drawn as a LOAD pass in the command view. Public so the host can describe the
/// open menu via [`radial::RadialMenu`].
pub mod radial;
/// Embody-unit picker (command view). A text-pass list of the selected units so the player chooses
/// which one to possess. Public so the host can describe the open list via [`picker::EmbodyPicker`]
/// and hit-test taps with [`picker::picker_row_at`].
pub mod picker;
/// Contextual command panel (command view). A boxed top-right panel whose rows change with the
/// selection (camp → train/upgrade, troops → composition/stance, nothing → build palette). Public so
/// the host can describe it via [`command_panel::CommandPanelView`].
pub mod command_panel;

/// Command-view **touch button bar** (build / train / upgrade). The mobile affordance for the RTS
/// half: a row of labelled buttons along the bottom that arm the command intents the desktop drives
/// off keys. Public so the host fills [`command_bar::CommandBarView`] from its hit-test layout.
pub mod command_bar;

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
/// In-match objective HUD (PvE WS-A) — a thin top-left presentation surface for the current mission
/// objective + progress, drawn through the W4 text pass + the overlay quad pipeline (the
/// [`command_panel`] pattern, opposite corner). Pure layout of a host-supplied [`objective_hud::
/// ObjectiveHudView`]; the host-side `engine::objectives` OBSERVES the sim to fill it, so nothing
/// here reads or folds sim state (invariant #1/#7). Public so the layout seams are reachable.
pub mod objective_hud;
/// Command-view upgrade panel — the readable per-camp tier display ("growth" half of command-and-
/// grow). Pure derivation of current tier / next-tier cost / production-speed effect / affordability
/// from a camp level + resources. No sim read (invariant #4); public so the `upgrade_view` seam (the
/// numbers the contextual [`command_panel`] renders) is reachable.
pub mod upgrade_panel;

/// Command-view build palette (Phase 2). Pure layout of the placeable-structure palette — label,
/// const cost, and a host-supplied affordability flag — for the W4 text pass; reaches into no sim
/// state (only the `core` const cost table). Public so the `build_menu_entries` seam is reachable.
pub mod build_menu;
/// Command-view troop-training data (Phase 2). Pure per-unit cost + production ETA from the static
/// `economy` tables. No sim read; public so the `train_options` / `eta_seconds` seams (the numbers
/// the contextual [`command_panel`] renders) are reachable.
pub mod train_panel;

/// Device quality tiers + dynamic-resolution + thermal-backoff policy (Phase 4 WS-C). Pure,
/// host-testable RENDER decisions (invariant #1/#4: never a sim input) — see the module docs.
pub mod tiers;

/// Dynamic-resolution intermediate render target + upscale blit (Phase 4 WS-C). The wgpu wiring the
/// `tiers`/`engine::tuning` `resolution_scale` policy drives: the heavy 3D scene renders into an
/// offscreen texture sized by the scale, then this upscales it to the swapchain (chrome stays
/// native). Render-only — never a sim input (invariant #1/#4).
pub mod scene_target;

pub use tiers::{next_resolution_scale, thermal_backoff, Backoff, QualityTier, TierParams};
pub use scene_target::{needs_realloc, scene_target_dims, SceneTarget};

/// Convert a Q16.16 fixed value to `f32` for the GPU. The ONLY sanctioned fixed→float hop.
#[inline]
pub fn fixed_to_f32(v: Fixed) -> f32 {
    v.to_bits() as f32 / Fixed::SCALE as f32
}

/// Interpolate two binary-radian [`Angle`]s by `alpha ∈ [0,1]` and return the result in **f32
/// radians** for the mesh-yaw transform (invariant #4 — angle interpolation lives in the renderer,
/// not the sim). Crosses the wrap seam the **shortest way around**, mirroring
/// [`gonedark_core::trig::rotate_toward`]'s signed delta, so a turret slewing 359°→1° sweeps 2°
/// forward instead of spinning 358° back. The binary-radian convention matches the sim's `sin`/`cos`
/// and the renderer's [`mesh::model_matrix`] yaw: `+X = 0`, increasing counter-clockwise toward `+Y`.
#[inline]
pub fn interp_angle(prev: Angle, curr: Angle, alpha: f32) -> f32 {
    // Signed shortest delta in (−ANGLE_FULL/2, ANGLE_FULL/2], as in `trig::rotate_toward`.
    let raw = (curr.0 - prev.0) & (ANGLE_FULL - 1);
    let delta = if raw > ANGLE_FULL / 2 { raw - ANGLE_FULL } else { raw };
    let units = prev.0 as f32 + delta as f32 * alpha;
    units * (std::f32::consts::TAU / ANGLE_FULL as f32)
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

/// Uniform scale applied to every 3D token mesh. The greybox models are authored **in real-world
/// metres** by `tools/models/gen_models.py` (a trooper ~1.74 m tall, a tank ~3.2 m long, the camp
/// ~3.5 m across), so drawing them at `1.0` is **true scale** — relative sizes are honest and a unit
/// stands at its real height against the metre-scale scenery props in the embodied first-person view.
/// (Earlier this was per-kind cosmetic exaggeration so top-down tokens read as map markers — a
/// trooper ×2.2, a tank ×0.42 — but that distorted relative size, e.g. a trooper drawn bigger than a
/// tank and a 3.8 m soldier towering over the 1.5 m eye. True scale everywhere replaces it.)
const TOKEN_SCALE: f32 = 1.0;

/// The 3D token mesh for a snapshot unit, resolved from its [`Army`] identity + producible
/// [`UnitKind`] / building flag (factions-plan WS-C, D68). Buildings are the camp structure; units
/// map by archetype (`Heavy`/`Tank`→tank silhouette, `Rifleman`/`Medic`→infantry). The **army**
/// selects the *silhouette*: a US-side rifleman is an M1-helmeted [`TrooperUs`](mesh::ModelKind::TrooperUs),
/// a French tank is a Leclerc [`TankFr`](mesh::ModelKind::TankFr); [`Army::Neutral`] (legacy / debug
/// scenes that never select an army) falls back to the original shared greybox, so a non-faction
/// scene draws exactly as before factions existed.
///
/// This is **pure presentation** (invariant #6): the army never reaches `core` and adds no checksum
/// surface — it only picks which committed `.mesh` the renderer draws. Pure + testable (no device).
/// Per-army weapon viewmodels resolve through the sibling [`weapon_model_for`]; the human-readable
/// army name through [`faction_name`]. Buildings are army-agnostic for now (camps share a silhouette);
/// per-faction structures can layer on later without touching this seam.
pub(crate) fn model_for_unit(army: Army, building: bool, kind: UnitKind) -> mesh::ModelKind {
    use mesh::ModelKind as M;
    if building {
        return M::CampHq;
    }
    // Is this archetype an infantry body or a tank chassis? (D65: the produced Tank reuses the Heavy
    // chassis token; the Medic is infantry.)
    let is_tank = matches!(kind, UnitKind::Heavy | UnitKind::Tank);
    match (army, is_tank) {
        (Army::Us, true) => M::TankUs,
        (Army::Us, false) => M::TrooperUs,
        (Army::Fr, true) => M::TankFr,
        (Army::Fr, false) => M::TrooperFr,
        // Neutral / non-aligned → the original shared greybox (byte-identical pre-factions behaviour).
        (Army::Neutral, true) => M::Tank,
        (Army::Neutral, false) => M::Trooper,
    }
}

/// The first-person weapon viewmodel mesh for an embodied unit of the given [`Army`] (WS-C): the US
/// Army carries an [`M4`](mesh::ModelKind::WeaponRifleUs) carbine, the French Army a
/// [`FAMAS`](mesh::ModelKind::WeaponRifleFr) bullpup; [`Army::Neutral`] keeps the original shared
/// rifle. Pure presentation (the gun you *see* — never the sim's [`UnitKind`]/weapon stats, which
/// invariant #1/#7 keep army-agnostic). Pure + testable; [`Renderer::render_world_weapon`] picks the
/// mesh from this once the host plumbs the embodied unit's army (WS-D).
pub fn weapon_model_for(army: Army) -> mesh::ModelKind {
    match army {
        Army::Us => mesh::ModelKind::WeaponRifleUs,
        Army::Fr => mesh::ModelKind::WeaponRifleFr,
        Army::Neutral => mesh::ModelKind::WeaponRifle,
    }
}

/// The human-readable faction name for an [`Army`] (WS-C) — for the army-select UI, the post-match
/// shell, and embodied/command HUD labels. Presentation-only text; never sim state. (`Army::Neutral`
/// is the non-aligned default of legacy/debug scenes, so it reads simply as "Neutral".)
pub fn faction_name(army: Army) -> &'static str {
    match army {
        Army::Us => "US Army",
        Army::Fr => "French Army",
        Army::Neutral => "Neutral",
    }
}

/// The independently-slewing turret mesh that seats atop a given hull silhouette, if any (WS-C / P7).
/// Every tank silhouette — the shared [`Tank`](mesh::ModelKind::Tank) and the per-faction
/// [`TankUs`](mesh::ModelKind::TankUs)/[`TankFr`](mesh::ModelKind::TankFr) — pairs with the matching
/// turret; non-tank bodies have none. Keeps [`token_meshes`] from hard-coding the shared turret so a
/// faction tank gets its own turret silhouette. Pure + testable.
fn turret_for(hull: mesh::ModelKind) -> Option<mesh::ModelKind> {
    use mesh::ModelKind as M;
    match hull {
        M::Tank => Some(M::TankTurret),
        M::TankUs => Some(M::TankTurretUs),
        M::TankFr => Some(M::TankTurretFr),
        _ => None,
    }
}

/// The 3D mesh sub-parts that compose a unit/structure token: `(kind, scale, yaw_radians)` for each
/// mesh to draw, all placed at the instance's world `(x, y)` on the ground. Most tokens are a single
/// body mesh oriented by the unit's [`hull_yaw`](UnitInstance::hull_yaw); a **tank** is two parts —
/// the hull at `hull_yaw` plus the turret ([`mesh::ModelKind::TankTurret`]) at
/// [`turret_yaw`](UnitInstance::turret_yaw), slewed independently (tank embodiment P7, D55). The
/// turret mesh carries the hull's turret-ring pivot (local origin) and its real height, so drawing it
/// at the same `(x, y)` + scale yaws it about the ring and seats it on the hull. A control-point ring
/// (or any non-mesh instance) returns an empty list. Pure + testable (no device).
fn token_meshes(inst: &UnitInstance) -> Vec<(mesh::ModelKind, f32, f32)> {
    if inst.flags & FLAG_RING != 0 {
        return Vec::new(); // control points stay hollow rings (no mesh for them yet)
    }
    // `model` is always a valid ModelKind discriminant (written by `model_for_unit`); a direct
    // index panics loudly in debug if some future path forgets to set it, rather than silently
    // drawing the wrong mesh.
    let kind = mesh::ModelKind::ALL[inst.model as usize];
    // True metre scale for every part. The tank hull and its turret therefore share one scale, so
    // the turret (authored to seat on the hull at 1:1) sits correctly on the ring (P7). The turret
    // silhouette matches the hull's army (WS-C): a US hull gets the US turret, a French hull the
    // French one — [`turret_for`] resolves it (the shared tank keeps the shared turret).
    let mut parts = vec![(kind, TOKEN_SCALE, inst.hull_yaw)];
    if let Some(turret) = turret_for(kind) {
        parts.push((turret, TOKEN_SCALE, inst.turret_yaw));
    }
    parts
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
/// without a device; [`Renderer::render_world_meshes`] just groups the result into batches.
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

/// Build the embodied first-person draw plan for the dynamic sim units the avatar can SEE — the
/// allies and enemies standing in its line of sight (the missing half of the dark frame: losing the
/// strategic MAP is intel loss, but the soldier in front of your rifle is a fair physical target —
/// invariant #6). `instances` is the ALREADY fog-filtered draw set, so only units inside the
/// avatar's vision survive upstream ([`fog::visible_instances`]) and this never re-checks intel. It
/// drops the avatar's own body ([`FLAG_EMBODIED`] — you don't render yourself in first person) and
/// any non-mesh instance ([`token_meshes`] is empty for control-point rings, which are map intel
/// and never appear in the dark frame anyway), then stands each remaining unit on the ground
/// (`z = 0`) as a 3D token at the LOD its eye distance warrants — exactly mirroring [`prop_draw_plan`]
/// so distant units cost fewer triangles on the 200-unit budget. Each unit's body is oriented by its
/// [`hull_yaw`](UnitInstance::hull_yaw); a tank also emits its turret at
/// [`turret_yaw`](UnitInstance::turret_yaw) ([`token_meshes`], P7) — so a unit faces the way it moves
/// and a tank's gun tracks independently. Pure + GPU-free, so it is unit-tested without a device.
fn unit_draw_plan(
    instances: &[UnitInstance],
    eye: [f32; 3],
) -> Vec<(mesh::ModelKind, usize, mesh::MeshInstance)> {
    instances
        .iter()
        .filter(|inst| inst.flags & FLAG_EMBODIED == 0) // never draw the possessed avatar's own body
        .flat_map(|inst| {
            // One LOD + colour per unit; each body part (the hull, plus a tank's turret) shares them.
            let (dx, dy, dz) = (inst.x - eye[0], inst.y - eye[1], -eye[2]);
            let lod = mesh::select_lod((dx * dx + dy * dy + dz * dz).sqrt());
            let color = [inst.r, inst.g, inst.b, 0.0]; // faction tint; a=0 → no muzzle flash
            let pos = [inst.x, inst.y, 0.0];
            // token_meshes is empty for a control-point ring (map intel), so it is skipped here.
            token_meshes(inst).into_iter().map(move |(kind, scale, yaw)| {
                let mesh_inst = mesh::MeshInstance {
                    model: mesh::model_matrix(pos, scale, yaw),
                    color,
                };
                (kind, lod, mesh_inst)
            })
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

/// Uniform scale for a tracer bolt (the `tracer` mesh is ~0.6 m along its travel axis). Render-only
/// cosmetic scale.
const TRACER_SCALE: f32 = 1.0;

/// A shell tracer's RGBA, by firing faction. The `a` channel is the shader's emissive term
/// ([`mesh.wgsl`]'s warm flash add), so a high `a` makes the bolt glow hot in flight. Player shells
/// read warm yellow-orange, enemy shells hotter red-orange — a readable "whose round is that".
fn tracer_color(faction: Faction) -> [f32; 4] {
    match faction {
        Faction::Player => [1.0, 0.75, 0.30, 1.0],
        Faction::Enemy => [1.0, 0.40, 0.20, 1.0],
        Faction::Neutral => [0.90, 0.85, 0.55, 1.0],
    }
}

/// Build tracer mesh instances from the in-flight shells, extrapolated by `alpha` for smooth flight
/// between sim ticks (invariant #4 — interpolation lives in the renderer, not the sim). Each shell
/// flies at constant ground velocity, so we advance the **previous** snapshot's shells by
/// `pos + vel·alpha` (and `height + vz·alpha`): at `alpha = 1` a surviving shell lands exactly on its
/// current-tick position, while a just-spawned shell simply appears one tick later and a spent one
/// plays out its final segment — no fragile cross-tick index matching. Each bolt is yawed to its
/// travel heading (`atan2(vel)`, matching [`mesh::model_matrix`]'s `+X = 0`/CCW convention) and stood
/// at its `(x, y, height)`; a hot per-shell tint ([`tracer_color`]) drives the shader glow. These are
/// embodied-only by construction (invariant #3 — only an embodied unit fires a ballistic shell), so
/// every bolt is the firing player's own physical round, never strategic map intel (invariant #6).
/// Pure + GPU-free, so it is unit-tested without a device.
pub fn interpolate_projectiles(prev: &Snapshot, alpha: f32) -> Vec<mesh::MeshInstance> {
    prev.projectiles
        .iter()
        .map(|p| {
            let (vx, vy) = (fixed_to_f32(p.vel.x), fixed_to_f32(p.vel.y));
            let x = fixed_to_f32(p.pos.x) + vx * alpha;
            let y = fixed_to_f32(p.pos.y) + vy * alpha;
            // Clamp to the ground so a shell dipping below z=0 (an undershoot) never draws underground.
            let z = (fixed_to_f32(p.height) + fixed_to_f32(p.vz) * alpha).max(0.0);
            // A near-stationary shell has no travel heading; atan2(0,0)=0 keeps it axis-aligned.
            let yaw = vy.atan2(vx);
            mesh::MeshInstance {
                model: mesh::model_matrix([x, y, z], TRACER_SCALE, yaw),
                color: tracer_color(p.faction),
            }
        })
        .collect()
}

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
        // Resolve the 3D token mesh from the unit's army + unit-kind / building flag (Heavy→tank,
        // US→Abrams/M1-trooper, FR→Leclerc/FELIN-trooper — WS-C). The render snapshot does not yet
        // carry a per-unit army, so we draw the army-agnostic [`Army::Neutral`] silhouettes here
        // (byte-identical to pre-factions). Flipping faction silhouettes on is a one-line change at
        // this seam once WS-A/WS-D plumbs the per-side army (`sim.army_of(b.faction)`) into the
        // snapshot — `model_for_unit` and the faction meshes/turrets are already in place + tested.
        let model = model_for_unit(Army::Neutral, b.building, b.unit_kind) as u32;
        // Hull + turret facing, interpolated shortest-arc across the wrap seam (P7). Read from both
        // snapshots so a slewing turret tweens smoothly between ticks (invariant #4).
        let hull_yaw = interp_angle(a.hull_heading, b.hull_heading, alpha);
        let turret_yaw = interp_angle(a.turret_yaw, b.turret_yaw, alpha);

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
            hull_yaw,
            turret_yaw,
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
            model: 0, // unused: FLAG_RING makes `token_meshes` empty (rings stay hollow quads)
            hull_yaw: 0.0,    // unused for rings (no mesh)
            turret_yaw: 0.0,
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
    /// The 3D token mesh this instance draws as ([`mesh::ModelKind`] `as u32`), resolved from the
    /// snapshot's unit-kind / building flag by [`model_for_unit`]. CPU-side only — [`token_meshes`]
    /// reads it to bucket the mesh pass; it is a trailing field so the quad pipeline's instance
    /// attributes (locations 1..=5, fixed offsets) are untouched and the GPU never reads it.
    pub model: u32,
    /// Hull/body facing in **radians** (interpolated from the snapshot's `hull_heading`, shortest-arc
    /// — [`interp_angle`]). The mesh pass yaws the body about Z by this (tank embodiment P7, D55).
    /// CPU-side only, like [`model`](Self::model) — the GPU quad pipeline never reads it.
    pub hull_yaw: f32,
    /// Turret bearing in **radians** (interpolated from the snapshot's `turret_yaw`, shortest-arc).
    /// Only meaningful for a tank, whose turret mesh ([`mesh::ModelKind::TankTurret`]) is yawed by it
    /// independently of the hull (P7). CPU-side only — the GPU quad pipeline never reads it.
    pub turret_yaw: f32,
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
    /// CPU-side tracer mesh instances (in-flight shells) from the last [`Renderer::prepare`], drawn
    /// in the embodied world pass ([`Renderer::render_world_meshes`]) (tank embodiment P7).
    projectiles: Vec<mesh::MeshInstance>,
    /// The embodied directional-alert overlay (worker 2). Drawn as a second LOAD pass by
    /// [`Renderer::render_hud`] when the local player is embodied.
    hud: hud::HudRenderer,
    /// The on-screen FPS touch-control HUD (Android). Drawn as a LOAD pass by
    /// [`Renderer::render_touch_controls`] when the local player is embodied on a touch device.
    touch_controls: touch_controls::TouchControlsRenderer,
    /// The embodied **tank** gunner-sight HUD (tank embodiment P8). Drawn as a LOAD pass by
    /// [`Renderer::render_tank_hud`] when the local player is embodied in a tank.
    tank_hud: tank_hud::TankHudRenderer,
    /// The in-session shell overlay (Phase 4 WS-B). Drawn as a LOAD pass by
    /// [`Renderer::render_overlay`] when an in-session surface (pause/reconnect/summary) is up.
    overlay: overlay::OverlayRenderer,
    /// The radial command menu. Drawn as a LOAD pass by [`Renderer::render_radial`] in the command
    /// view when a held long-press has a menu open.
    radial: radial::RadialRenderer,
    /// The band-select marquee. Drawn as a LOAD pass by [`Renderer::render_marquee`] in the command
    /// view while a band-drag is in flight.
    marquee: marquee::MarqueeRenderer,
    /// The debug hitbox/facet overlay. Drawn as a LOAD pass by [`Renderer::render_debug`] in the
    /// command view when the developer toggle is on. Reuses the unit pass's camera bind group.
    debug: debug::DebugRenderer,
    /// The "gone dark" detection-tell overlay. Drawn as a LOAD pass by [`Renderer::render_detection`]
    /// in the command view, marking each hostile embodied enemy the commander can sense. Reuses the
    /// unit pass's camera bind group (the top-down view-projection).
    detection: detection::DetectionRenderer,
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
    /// The dynamic-resolution intermediate scene target + its upscale-blit pipeline (Phase 4 WS-C).
    /// The host renders the heavy 3D scene into [`scene_target::SceneTarget::view`] at the dyn-res
    /// size ([`Renderer::ensure_scene_target`]) then upscales it to the swapchain
    /// ([`Renderer::present_scene`]); chrome is drawn natively after. Render-only (invariant #1/#4).
    scene_target: scene_target::SceneTarget,
    /// The command-view unit/enemy/point tally derived during the last [`Renderer::render`] from the
    /// fog-filtered draw set. Stashed so the corner readout text can be drawn by
    /// [`Renderer::render_readout`] at NATIVE swapchain resolution AFTER [`present_scene`], rather than
    /// being rasterised into the (possibly sub-native) dyn-res scene target and upscaled soft. Pure
    /// presentation state — three counts off the visible draw set, never a sim read (invariant #1/#4).
    readout_tally: readout::Tally,
    /// Viewport aspect (width / height) of the most recent [`Renderer::render`], stashed so the chrome
    /// passes that run AFTER it this frame (radial menu, etc.) keep their glyphs square in pixels too.
    /// Pure presentation — never a sim input (invariant #1/#4). Defaults to `1.0` (square).
    chrome_aspect: f32,
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
        let tank_hud = tank_hud::TankHudRenderer::new(device, surface_format);
        let overlay = overlay::OverlayRenderer::new(device, surface_format);
        let radial = radial::RadialRenderer::new(device, surface_format);
        let marquee = marquee::MarqueeRenderer::new(device, surface_format);
        let world = world::WorldRenderer::new(device, surface_format);
        // The ground grid shares the unit pass's camera layout so it uses the same view-projection.
        let terrain = terrain::TerrainRenderer::new(device, surface_format, &camera_layout);
        // The debug overlay reuses the same camera layout (its bind group is the command view-proj).
        let debug = debug::DebugRenderer::new(device, surface_format, &camera_layout);
        // The detection-tell overlay likewise reuses the command view-projection camera layout.
        let detection = detection::DetectionRenderer::new(device, surface_format, &camera_layout);
        let text = text::TextRenderer::new(device, surface_format);
        // Cooked greybox meshes + the shared 3D mesh pipeline + an initial (placeholder) depth
        // buffer; the depth buffer is resized to the surface on the first mesh pass (D44).
        let mesh_lib = mesh::MeshLibrary::load(device);
        let mesh_pipeline = mesh::MeshPipeline::new(device, surface_format);
        let depth_view = mesh::create_depth_view(device, 1, 1);
        // The dyn-res intermediate target + upscale-blit pipeline (Phase 4 WS-C). The texture is
        // allocated lazily on the first `ensure_scene_target` (sized to the dyn-res scale).
        let scene_target = scene_target::SceneTarget::new(device, surface_format);

        Renderer {
            pipeline,
            camera_buf,
            camera_bind_group,
            quad_buf,
            instance_buf,
            instance_cap,
            instances: Vec::new(),
            projectiles: Vec::new(),
            hud,
            touch_controls,
            tank_hud,
            overlay,
            radial,
            marquee,
            debug,
            detection,
            world,
            terrain,
            text,
            mesh_lib,
            mesh_pipeline,
            depth_view,
            depth_size: (1, 1),
            scene_target,
            readout_tally: readout::Tally::default(),
            chrome_aspect: 1.0,
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

    /// Ensure the dynamic-resolution intermediate scene target matches the swapchain `(width,
    /// height)` drawn at `scale` (Phase 4 WS-C), recreating it only when the pixel size changes.
    /// Returns the intermediate's `(w, h)` so the host can size the scene passes' depth buffer +
    /// viewports to match it. Call this ONCE per frame before the scene passes; render those passes
    /// into [`scene_view`](Self::scene_view), then upscale with [`present_scene`](Self::present_scene).
    ///
    /// `scale` is the host's `RenderTuning::resolution_scale()` — a pure RENDERING choice (invariant
    /// #1/#4); it never reaches the sim, so the per-tick checksum is byte-identical at every scale.
    pub fn ensure_scene_target(
        &mut self,
        device: &wgpu::Device,
        width: u32,
        height: u32,
        scale: f32,
    ) -> (u32, u32) {
        self.scene_target.ensure(device, width, height, scale)
    }

    /// A view onto the dynamic-resolution intermediate (Phase 4 WS-C) — the host renders the heavy 3D
    /// scene passes into this instead of the swapchain. Returns a cheap (Arc) clone, so the host can
    /// hold it across the `&mut self` scene-pass calls without borrowing the renderer.
    ///
    /// # Panics
    /// Panics if [`ensure_scene_target`](Self::ensure_scene_target) has not run this frame.
    pub fn scene_view(&self) -> wgpu::TextureView {
        self.scene_target.view()
    }

    /// Upscale the intermediate scene onto the swapchain `view` (Phase 4 WS-C) — a fullscreen blit
    /// with a linear filter (identity at scale 1.0). The host calls this once, AFTER every scene pass
    /// and BEFORE any chrome (HUD/overlay/text) pass, so the native-resolution chrome LOADs on top of
    /// the upscaled scene. Render-only.
    pub fn present_scene(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        view: &wgpu::TextureView,
    ) {
        self.scene_target.present(device, queue, view);
    }

    /// Build render instances by interpolating between the previous and current sim snapshots
    /// by `alpha` in `[0,1]` (invariant #4). Produces CPU data only; the GPU upload happens in
    /// [`Renderer::render`]. `selected` carries the command-layer selected world indices so the
    /// renderer rims them (empty while embodied — presentation state only, never sim state).
    pub fn prepare(&mut self, prev: &Snapshot, curr: &Snapshot, alpha: f32, selected: &[u32]) {
        self.instances = interpolate_instances(prev, curr, alpha, selected);
        self.projectiles = interpolate_projectiles(prev, alpha);
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
    ///  2. **3D unit/structure tokens** — depth-tested meshes ([`token_meshes`] picks infantry vs
    ///     structure, and a tank's hull + turret) LOADed over the grid;
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
    ) {
        queue.write_buffer(&self.camera_buf, 0, bytemuck::bytes_of(camera));

        // Keep screen-space chrome glyphs square in pixels: hand this frame's viewport aspect to the
        // text pass (used by every chrome flush that follows — readout, command panel/bar, objective
        // HUD, embody picker, tank shell label) and stash it for the radial pass, which runs later this
        // frame. Pure presentation — it never reaches the sim (invariant #1/#4).
        self.chrome_aspect = width.max(1) as f32 / height.max(1) as f32;
        self.text.set_aspect(self.chrome_aspect);

        // Pick the draw set: the fog layer applies visibility (and the dark-frame avatar-only
        // rule) — see `render/src/fog.rs` (worker 1).
        let draw_set: Vec<UnitInstance> = fog::visible_instances(&self.instances, fog, world_dark);

        // Stash the command-view tally derived from the fog-filtered draw set, so the corner readout
        // text can be drawn at NATIVE resolution by `render_readout` AFTER `present_scene` instead of
        // riding the (possibly sub-native) dyn-res scene target and upscaling soft. Pure count of the
        // visible set — no new sim read (invariant #4). While embodied this is the avatar-only set, but
        // `readout_labels` withholds the readout over the dark frame anyway (invariant #6).
        self.readout_tally = readout::tally(&draw_set);

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
            // Each token's body mesh(es): one per unit, two for a tank (hull + independently-yawed
            // turret, P7). An empty list is a control-point ring — it keeps its hollow-ring quad.
            let parts = token_meshes(inst);
            if parts.is_empty() {
                continue;
            }
            inst.flags |= FLAG_MESH;
            for (kind, scale, yaw) in parts {
                buckets[kind as usize].push(mesh::MeshInstance {
                    model: mesh::model_matrix([inst.x, inst.y, 0.0], scale, yaw),
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

        // The command-view readout text is NOT drawn here: it is screen-space chrome and must stay
        // crisp at any dyn-res `resolution_scale < 1.0`, so the host draws it via `render_readout`
        // onto the NATIVE swapchain AFTER `present_scene`, alongside the rest of the chrome. The tally
        // it needs was stashed above (`self.readout_tally`) from this frame's fog-filtered draw set.
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

    /// Draw the embodied hitmarker — the centered "X" flash confirming the player's OWN connecting
    /// shot (WS-4) — as a LOAD pass over the current frame. Delegates to [`hud::HudRenderer`]. The
    /// host calls this only while embodied; it is a no-op unless a hit is live (`last_hit_tick`
    /// within the fade window). Presentation feedback on the player's own action, never map intel
    /// (invariant #6).
    pub fn render_hitmarker(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        view: &wgpu::TextureView,
        last_hit_tick: Option<u64>,
        tick: u64,
    ) {
        self.hud
            .render_hitmarker(device, queue, view, last_hit_tick, tick);
    }

    /// Draw the embodied **tank** gunner-sight HUD (tank embodiment P8) on top of the current frame (a
    /// LOAD pass — never clears): the hull-relative turret indicator, the dispersion reticle, the LEAD
    /// pip, and the reload ring (geometry via [`tank_hud::TankHudRenderer`]), then the selected-shell
    /// label through the shared [`text`](crate::text) pass. The host calls this only while the local
    /// player is embodied in a tank. Presentation-only chrome with no world position — it reveals
    /// nothing about unseen enemies and widens no fog beneath it (invariant #6); the renderer only
    /// READS the [`tank_hud::TankHudState`] / `shell_label` it is handed (invariant #4).
    ///
    /// `shell_label` is the selected shell readout (e.g. "AP"). **W2 dependency:** until W2's
    /// `ShellKind`/selected-shell field merges, the host passes a constant default; once merged it is a
    /// one-line swap to the live selected-shell name.
    pub fn render_tank_hud(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        view: &wgpu::TextureView,
        state: &tank_hud::TankHudState,
        shell_label: &str,
    ) {
        self.tank_hud.render(device, queue, view, state);
        // Shell-selector readout: a short label under the reticle, drawn through the shared text pass.
        if !shell_label.is_empty() {
            self.text.queue(
                shell_label,
                [0.0, -0.62],
                0.05,
                text::Anchor::TopCenter,
                [0.82, 0.86, 0.92],
                0.92,
            );
            self.text.render(device, queue, view);
        }
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
    ///
    /// `names` are the live per-slot action labels from the host's command vocabulary
    /// (`engine::Game::radial_menu`); each wedge is labelled with its real name instead of a
    /// placeholder slot number. The labels are kept square in pixels by the aspect stashed from this
    /// frame's [`Renderer::render`].
    pub fn render_radial(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        view: &wgpu::TextureView,
        menu: &radial::RadialMenu,
        names: &[&str],
    ) {
        self.radial.set_aspect(self.chrome_aspect);
        self.radial
            .render_with_labels(device, queue, view, menu, Some(names));
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

    /// Draw the debug overlay's pre-composed world-space line list `verts` on top of the current
    /// command frame (a LOAD pass — never clears), delegating to [`debug::DebugRenderer`]. The host
    /// composes `verts` from the pure seams (tank hitbox lines / shell tracers / infantry range+cone
    /// / LoS connectors). Reuses this renderer's camera bind group — the command view-projection the
    /// preceding [`Renderer::render`] uploaded this frame — so the world-space lines line up with the
    /// units. Command-view only (never over the dark embodied frame, invariant #6); a no-op on empty.
    pub fn render_debug(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        view: &wgpu::TextureView,
        verts: &[debug::DebugVertex],
    ) {
        self.debug
            .render(device, queue, view, &self.camera_bind_group, verts);
    }

    /// Draw the "gone dark" detection-tell overlay's pre-composed world-space line list `verts` on
    /// top of the current command frame (a LOAD pass — never clears), delegating to
    /// [`detection::DetectionRenderer`]. The host composes `verts` from
    /// [`detection::detection_vertices`] over the markers the pure `engine::detection_markers` seam
    /// produced. Reuses this renderer's camera bind group — the command view-projection the preceding
    /// [`Renderer::render`] uploaded this frame — so each marker sits on its sensed unit. Command-view
    /// only (the host never calls this over the dark embodied frame, and the seam emits nothing while
    /// the local player is embodied — invariant #6); a no-op on empty.
    pub fn render_detection(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        view: &wgpu::TextureView,
        verts: &[detection::DetectionVertex],
    ) {
        self.detection
            .render(device, queue, view, &self.camera_bind_group, verts);
    }

    /// Draw the contextual command panel (command view) — the boxed top-right panel whose rows
    /// reflect the current selection (camp → train/upgrade/resources, troops → composition/stance,
    /// nothing → build palette). The host derives the rows from the (checksummed) sim + selection
    /// and hands them in via [`command_panel::CommandPanelView`]; this draws the box through the
    /// shared overlay quad pipeline and the rows through the text pass. Command-view only (never over
    /// the dark frame, invariant #6); a no-op on an empty view.
    pub fn render_command_panel(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        view: &wgpu::TextureView,
        panel: &command_panel::CommandPanelView,
    ) {
        if panel.is_empty() {
            return;
        }
        let quads = command_panel::command_panel_quads(panel);
        self.overlay.draw_quads(device, queue, view, &quads);
        for l in command_panel::command_panel_labels(panel) {
            self.text
                .queue(l.text, l.pos, l.px_size, l.anchor, l.color, l.alpha);
        }
        self.text.render(device, queue, view);
    }

    /// Draw the command-view **touch button bar** (build / train / upgrade) along the bottom. The
    /// host fills the [`command_bar::CommandBarView`] from its `command_touch` hit-test layout
    /// (pixel rects → NDC), so the buttons drawn here are the exact shapes the engine hit-tests
    /// taps against (no drift). Box quads through the shared overlay pipeline + centered labels
    /// through the W4 text pass — the same construction as [`render_command_panel`](Self::
    /// render_command_panel). Command view only (the caller gates on `!embodied`); a no-op on an
    /// empty bar.
    pub fn render_command_bar(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        view: &wgpu::TextureView,
        bar: &command_bar::CommandBarView,
    ) {
        if bar.is_empty() {
            return;
        }
        let quads = command_bar::command_bar_quads(bar);
        self.overlay.draw_quads(device, queue, view, &quads);
        for l in command_bar::command_bar_labels(bar) {
            self.text
                .queue(l.text, l.pos, l.px_size, l.anchor, l.color, l.alpha);
        }
        self.text.render(device, queue, view);
    }

    /// Draw the in-match **objective HUD** (PvE WS-A) — a thin top-left panel showing the current
    /// mission objective + progress. The host derives the rows from its host-side `ObjectiveSet`
    /// (which observes the sim, never mutates it) and hands them in via [`objective_hud::
    /// ObjectiveHudView`]; this draws the box through the shared overlay quad pipeline and the text
    /// through the W4 text pass — exactly like [`render_command_panel`](Self::render_command_panel),
    /// anchored to the opposite corner. Command-view only (never over the dark frame, invariant #6);
    /// a no-op on an empty view.
    pub fn render_objective_hud(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        view: &wgpu::TextureView,
        hud: &objective_hud::ObjectiveHudView,
    ) {
        if hud.is_empty() {
            return;
        }
        let quads = objective_hud::objective_hud_quads(hud);
        self.overlay.draw_quads(device, queue, view, &quads);
        for l in objective_hud::objective_hud_labels(hud) {
            self.text
                .queue(l.text, l.pos, l.px_size, l.anchor, l.color, l.alpha);
        }
        self.text.render(device, queue, view);
    }

    /// Draw the command-view **readout** — the top-left unit/enemy/point tally (and the optional,
    /// host-supplied resource/income lines) — on top of the current frame (a LOAD text pass; never
    /// clears). The tally was derived during the preceding [`Renderer::render`] from this frame's
    /// fog-filtered draw set ([`readout_tally`](Self::readout_tally)); the host hands in the
    /// [`readout::EconomyReadout`] economy seam and the live `world_dark` state, and
    /// [`readout::readout_labels`] lays the lines out. **Drawn at NATIVE swapchain resolution** — the
    /// host calls this AFTER [`present_scene`](Self::present_scene), with the rest of the chrome, so
    /// the readout stays crisp at any dyn-res `resolution_scale < 1.0` instead of being rasterised into
    /// the sub-native scene target and upscaled soft. Fairness (invariant #6): `readout_labels` returns
    /// an EMPTY set while embodied, so the command readout never draws over the dark frame — a no-op
    /// then. Screen-space chrome with no world position.
    pub fn render_readout(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        view: &wgpu::TextureView,
        economy: Option<readout::EconomyReadout>,
        world_dark: bool,
    ) {
        let labels = readout::readout_labels(&self.readout_tally, economy, world_dark);
        if labels.is_empty() {
            return;
        }
        // A subtle backing card behind the corner stack so the readout reads as designed HUD chrome,
        // not bare debug text — sized to this frame's aspect-corrected label footprint, drawn through
        // the shared overlay quad pipeline BEFORE the text so the glyphs sit on top.
        let card = readout::readout_card(&labels, self.chrome_aspect);
        if !card.is_empty() {
            self.overlay.draw_quads(device, queue, view, &card);
        }
        for label in labels {
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

    /// Draw the embody-unit picker (command view) — the list of selected units the player chooses
    /// from to possess one — on top of the current command frame (a LOAD text pass; never clears).
    /// The host calls this ONLY in the command view (never embodied → never over the dark frame,
    /// invariant #6) while its picker is open, and supplies the rows in [`picker::EmbodyPicker`];
    /// this lays them out ([`picker::picker_labels`]) and queues them through the text pass. Pairs
    /// with [`picker::picker_row_at`], which the host runs against a tap to resolve the chosen row.
    /// Screen-space chrome with no world position — it leaks no intel the command frame doesn't show.
    pub fn render_embody_picker(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        view: &wgpu::TextureView,
        picker: &picker::EmbodyPicker,
    ) {
        for l in picker::picker_labels(picker) {
            self.text
                .queue(l.text, l.pos, l.px_size, l.anchor, l.color, l.alpha);
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

    /// Draw the embodied first-person WORLD MESHES — the static scenery/cover props
    /// ([`PROP_LAYOUT`]) **and** the dynamic sim units the avatar can SEE — over the embodied
    /// sky/ground. Both are drawn in a SINGLE mesh pass (one shared depth clear) so they occlude each
    /// other correctly: a unit standing behind a rock is hidden by it, rather than punching through.
    ///
    /// Fairness (invariant #6): "world goes dark" strips the strategic MAP — the overview, the
    /// control points, off-screen intel — NOT the enemy physically in your avatar's line of sight.
    /// `fog` is the avatar's vision mask; [`fog::visible_instances`] keeps only the units it actually
    /// sees (plus the avatar, which [`unit_draw_plan`] then drops — you don't render your own body in
    /// first person), so nothing beyond direct sight leaks in. The props are a fixed cosmetic layout
    /// and carry zero intel. The host calls this in the embodied branch AFTER [`render_world_sky`]
    /// (the clearing pass) and before [`Renderer::render`]'s avatar pass. `view_proj` is the embodied
    /// camera matrix and `eye` its world position (so [`mesh::select_lod`] can pick a tier per mesh by
    /// distance); `width`/`height` size the depth buffer.
    #[allow(clippy::too_many_arguments)]
    pub fn render_world_meshes(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        view: &wgpu::TextureView,
        view_proj: &[[f32; 4]; 4],
        eye: [f32; 3],
        fog: &Visibility,
        width: u32,
        height: u32,
    ) {
        self.ensure_depth(device, width, height);
        // Static scenery + the avatar-visible dynamic units, in one combined LOD plan. The fog filter
        // (avatar-only mask, world_dark = true) keeps only units the avatar can see; the avatar's own
        // body is dropped inside `unit_draw_plan`.
        let mut plan = prop_draw_plan(eye);
        let visible = fog::visible_instances(&self.instances, fog, true);
        plan.extend(unit_draw_plan(&visible, eye));
        // One batch per (kind, lod) actually used, so each draws its own decimated mesh tier; the
        // whole set shares the single depth clear inside `mesh_pipeline.draw` (correct occlusion).
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
        // In-flight shell tracers (P7), drawn in the same pass so they depth-test against the world
        // (a tracer behind a rock is occluded). Always full detail (the bolt is already minimal) and
        // hot-tinted by `interpolate_projectiles`. Embodied-only: every shell is the firing player's
        // own, so this leaks no map intel (invariant #6).
        if !self.projectiles.is_empty() {
            batches.push(mesh::MeshBatch {
                mesh: self.mesh_lib.get(mesh::ModelKind::Tracer),
                instances: self.projectiles.clone(),
            });
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
    use gonedark_core::components::{Army, Faction, Vec2};
    use gonedark_core::snapshot::{ControlPointSnapshot, ProjectileSnapshot, Snapshot, UnitSnapshot};

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
            hull_heading: Angle(0),
            turret_yaw: Angle(0),
            firing: false,
        }
    }

    fn snapshot(tick: u64, units: Vec<UnitSnapshot>) -> Snapshot {
        Snapshot {
            tick,
            units,
            control_points: Vec::new(),
            projectiles: Vec::new(),
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
        // Neutral (legacy / no-faction scenes) draws the original shared greybox — byte-identical to
        // pre-factions behaviour.
        assert_eq!(
            model_for_unit(Army::Neutral, false, UnitKind::Rifleman),
            mesh::ModelKind::Trooper
        );
        assert_eq!(
            model_for_unit(Army::Neutral, false, UnitKind::Heavy),
            mesh::ModelKind::Tank
        );
        // D65: the produced Tank reuses the tank mesh; the Medic is infantry (trooper mesh).
        assert_eq!(
            model_for_unit(Army::Neutral, false, UnitKind::Tank),
            mesh::ModelKind::Tank
        );
        assert_eq!(
            model_for_unit(Army::Neutral, false, UnitKind::Medic),
            mesh::ModelKind::Trooper
        );
        // A building is the camp structure regardless of the (irrelevant) unit-kind tag or army.
        assert_eq!(
            model_for_unit(Army::Neutral, true, UnitKind::Heavy),
            mesh::ModelKind::CampHq
        );
        assert_eq!(
            model_for_unit(Army::Us, true, UnitKind::Rifleman),
            mesh::ModelKind::CampHq
        );
    }

    /// WS-C: every `(Army, kind)` resolves to a faction silhouette — US → Abrams/M1 trooper, FR →
    /// Leclerc/FELIN trooper — and Neutral keeps the shared greybox. The headline cosmetic-identity
    /// table (factions-plan WS-C, D68).
    #[test]
    fn model_for_unit_resolves_each_army_to_its_silhouette() {
        use mesh::ModelKind as M;
        // US side.
        assert_eq!(model_for_unit(Army::Us, false, UnitKind::Rifleman), M::TrooperUs);
        assert_eq!(model_for_unit(Army::Us, false, UnitKind::Medic), M::TrooperUs);
        assert_eq!(model_for_unit(Army::Us, false, UnitKind::Heavy), M::TankUs);
        assert_eq!(model_for_unit(Army::Us, false, UnitKind::Tank), M::TankUs);
        // French side.
        assert_eq!(model_for_unit(Army::Fr, false, UnitKind::Rifleman), M::TrooperFr);
        assert_eq!(model_for_unit(Army::Fr, false, UnitKind::Medic), M::TrooperFr);
        assert_eq!(model_for_unit(Army::Fr, false, UnitKind::Heavy), M::TankFr);
        assert_eq!(model_for_unit(Army::Fr, false, UnitKind::Tank), M::TankFr);
    }

    /// WS-C: the full `(Army, kind)` × building matrix resolves to *some* committed mesh and never
    /// panics — and US/FR units are visually distinct from each other and from Neutral (the whole
    /// point of cosmetic identity). Exercises every combination (the "no panic on unmapped" floor).
    #[test]
    fn model_for_unit_total_distinct_and_panic_free() {
        for &kind in &[UnitKind::Rifleman, UnitKind::Heavy, UnitKind::Tank, UnitKind::Medic] {
            for &building in &[false, true] {
                let neutral = model_for_unit(Army::Neutral, building, kind);
                let us = model_for_unit(Army::Us, building, kind);
                let fr = model_for_unit(Army::Fr, building, kind);
                // Every resolved kind is a real library entry (index into ModelKind::ALL is valid).
                for m in [neutral, us, fr] {
                    assert!((m as usize) < mesh::ModelKind::ALL.len());
                }
                if building {
                    // Buildings are army-agnostic for now (shared camp silhouette).
                    assert_eq!(us, mesh::ModelKind::CampHq);
                    assert_eq!(fr, mesh::ModelKind::CampHq);
                } else {
                    // A faction unit reads as a *different* silhouette from the shared greybox and
                    // from the other army.
                    assert_ne!(us, neutral, "{kind:?}: US unit differs from the shared greybox");
                    assert_ne!(fr, neutral, "{kind:?}: FR unit differs from the shared greybox");
                    assert_ne!(us, fr, "{kind:?}: US and FR units differ from each other");
                }
            }
        }
    }

    /// WS-C: per-army weapon viewmodels — US M4, FR FAMAS, Neutral shared rifle — all distinct, with
    /// every army resolving (no panic on unmapped). The embodied-view half of cosmetic identity.
    #[test]
    fn weapon_model_for_each_army_is_distinct() {
        use mesh::ModelKind as M;
        assert_eq!(weapon_model_for(Army::Us), M::WeaponRifleUs);
        assert_eq!(weapon_model_for(Army::Fr), M::WeaponRifleFr);
        assert_eq!(weapon_model_for(Army::Neutral), M::WeaponRifle);
        let all: Vec<M> = Army::ALL.iter().map(|&a| weapon_model_for(a)).collect();
        for (i, a) in all.iter().enumerate() {
            assert!((*a as usize) < M::ALL.len());
            for b in &all[i + 1..] {
                assert_ne!(a, b, "each army's viewmodel is a distinct mesh");
            }
        }
    }

    /// WS-C: faction names cover every army and are distinct, human-readable strings.
    #[test]
    fn faction_name_covers_every_army() {
        assert_eq!(faction_name(Army::Us), "US Army");
        assert_eq!(faction_name(Army::Fr), "French Army");
        assert_eq!(faction_name(Army::Neutral), "Neutral");
        for &a in &Army::ALL {
            assert!(!faction_name(a).is_empty(), "{a:?} has a name");
        }
    }

    /// WS-C: a faction tank emits its army's turret silhouette (US hull → US turret, FR hull → FR
    /// turret); the shared tank keeps the shared turret; non-tank bodies emit no turret.
    #[test]
    fn faction_tank_tokens_emit_matching_turret() {
        use mesh::ModelKind as M;
        let token = |model: M| {
            let u = UnitInstance {
                model: model as u32,
                hull_yaw: 0.5,
                turret_yaw: 1.2,
                ..Default::default()
            };
            token_meshes(&u)
        };
        assert_eq!(
            token(M::TankUs),
            vec![(M::TankUs, TOKEN_SCALE, 0.5), (M::TankTurretUs, TOKEN_SCALE, 1.2)],
            "a US tank emits the US hull + US turret"
        );
        assert_eq!(
            token(M::TankFr),
            vec![(M::TankFr, TOKEN_SCALE, 0.5), (M::TankTurretFr, TOKEN_SCALE, 1.2)],
            "a French tank emits the French hull + French turret"
        );
        assert_eq!(
            token(M::Tank),
            vec![(M::Tank, TOKEN_SCALE, 0.5), (M::TankTurret, TOKEN_SCALE, 1.2)],
            "the shared tank keeps the shared turret"
        );
        // Faction infantry is a single body part (no turret).
        assert_eq!(token(M::TrooperUs), vec![(M::TrooperUs, TOKEN_SCALE, 0.5)]);
        assert_eq!(token(M::TrooperFr), vec![(M::TrooperFr, TOKEN_SCALE, 0.5)]);
    }

    /// WS-C: every faction silhouette ModelKind has a committed asset-manifest entry carrying the
    /// generator-pipeline provenance (`source`/`license`/`sha256`) — the script-not-binary rule
    /// (D41/D46). Reads the committed `assets/models/manifest.json`.
    #[test]
    fn faction_models_have_manifest_entries() {
        let manifest = include_str!("../../assets/models/manifest.json");
        for name in [
            "trooper_us",
            "trooper_fr",
            "tank_us",
            "tank_turret_us",
            "tank_fr",
            "tank_turret_fr",
            "weapon_rifle_us",
            "weapon_rifle_fr",
        ] {
            assert!(
                manifest.contains(&format!("\"name\": \"{name}\"")),
                "manifest is missing a `{name}` entry"
            );
        }
        // Provenance markers exist (license + generator source) — the script-not-binary record.
        assert!(manifest.contains("\"license\": \"CC0-1.0\""));
        assert!(manifest.contains("tools/models/gen_models.py"));
        assert!(manifest.contains("\"sha256\""));
    }

    #[test]
    fn token_meshes_decodes_parts_scale_yaw_and_skips_rings() {
        // A Rifleman token (model = Trooper) → one infantry mesh at true scale, yawed by hull.
        let mut u = UnitInstance {
            model: mesh::ModelKind::Trooper as u32,
            hull_yaw: 0.7,
            turret_yaw: 1.3,
            ..Default::default()
        };
        assert_eq!(
            token_meshes(&u),
            vec![(mesh::ModelKind::Trooper, TOKEN_SCALE, 0.7)],
            "infantry is a single body part oriented by hull_yaw (turret_yaw ignored)"
        );

        // A Heavy token (model = Tank) → TWO parts: hull at hull_yaw + turret at turret_yaw, both at
        // the same (true) scale so the turret seats on the hull (P7).
        u.model = mesh::ModelKind::Tank as u32;
        assert_eq!(
            token_meshes(&u),
            vec![
                (mesh::ModelKind::Tank, TOKEN_SCALE, 0.7),
                (mesh::ModelKind::TankTurret, TOKEN_SCALE, 1.3),
            ],
            "a tank emits hull (hull_yaw) + independently-slewed turret (turret_yaw)"
        );

        // A building token (model = CampHq) → one structure mesh at true scale.
        u.model = mesh::ModelKind::CampHq as u32;
        assert_eq!(
            token_meshes(&u),
            vec![(mesh::ModelKind::CampHq, TOKEN_SCALE, 0.7)]
        );

        // A control-point ring gets no mesh (it stays a hollow-ring quad).
        let ring = UnitInstance {
            half_extent: CONTROL_POINT_HALF,
            flags: FLAG_RING,
            ..Default::default()
        };
        assert!(token_meshes(&ring).is_empty());
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
        // Both decode back through token_meshes to their body mesh.
        assert_eq!(token_meshes(&out[0])[0].0, mesh::ModelKind::Tank);
        assert_eq!(token_meshes(&out[1])[0].0, mesh::ModelKind::Trooper);
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

    // ---- unit_draw_plan: embodied first-person dynamic units ----

    /// Build a bare [`UnitInstance`] for the embodied-unit-plan tests. `model` is the snapshot's
    /// resolved [`mesh::ModelKind`] (as `token_for` decodes it); `rgb` the faction tint.
    fn uinst(x: f32, y: f32, flags: u32, model: mesh::ModelKind, rgb: [f32; 3]) -> UnitInstance {
        UnitInstance {
            x,
            y,
            half_extent: UNIT_HALF,
            r: rgb[0],
            g: rgb[1],
            b: rgb[2],
            health: 1.0,
            flags,
            model: model as u32,
            hull_yaw: 0.0,
            turret_yaw: 0.0,
        }
    }

    #[test]
    fn unit_draw_plan_drops_avatar_and_rings_keeps_visible_units() {
        // The fog-filtered set the embodied pass sees: the avatar's own body (FLAG_EMBODIED), a
        // control-point ring (FLAG_RING — map intel), and an in-sight enemy. Only the enemy is drawn.
        let avatar = uinst(0.0, 0.0, FLAG_EMBODIED, mesh::ModelKind::Trooper, AVATAR_COLOR);
        let ring = uinst(1.0, 0.0, FLAG_RING, mesh::ModelKind::Trooper, [0.5, 0.5, 0.6]);
        let enemy = uinst(5.0, 0.0, 0, mesh::ModelKind::Trooper, faction_color(Faction::Enemy));
        let plan = unit_draw_plan(&[avatar, ring, enemy], [0.0, 0.0, 1.6]);
        assert_eq!(plan.len(), 1, "only the visible non-avatar, non-ring unit is drawn");
        assert_eq!(plan[0].0, mesh::ModelKind::Trooper);
    }

    #[test]
    fn unit_draw_plan_picks_lod_by_eye_distance() {
        // Near unit keeps full detail; a distant one drops to the coarsest tier (200-unit budget).
        let near = uinst(2.0, 0.0, 0, mesh::ModelKind::Trooper, [0.0, 0.0, 0.0]);
        let far = uinst(40.0, 0.0, 0, mesh::ModelKind::Trooper, [0.0, 0.0, 0.0]);
        let plan = unit_draw_plan(&[near, far], [0.0, 0.0, 0.0]);
        assert_eq!(plan.len(), 2);
        assert_eq!(plan[0].1, 0, "the near unit keeps LOD0");
        assert_eq!(plan[1].1, 2, "the distant unit drops to LOD2");
    }

    #[test]
    fn unit_draw_plan_carries_faction_tint_kind_and_grounds_at_z0() {
        // A Heavy → tank token: hull + turret (P7), tinted by faction with no flash (a=0), both
        // standing on the ground (z=0) at the unit's world (x, y).
        let tank = uinst(3.0, -1.0, 0, mesh::ModelKind::Tank, faction_color(Faction::Enemy));
        let plan = unit_draw_plan(&[tank], [0.0, 0.0, 1.6]);
        assert_eq!(plan.len(), 2, "a tank draws as hull + turret");
        assert_eq!(plan[0].0, mesh::ModelKind::Tank, "first part is the hull");
        assert_eq!(plan[1].0, mesh::ModelKind::TankTurret, "second part is the turret");
        let c = faction_color(Faction::Enemy);
        for (_kind, _lod, inst) in &plan {
            assert_eq!(
                [inst.color[0], inst.color[1], inst.color[2]],
                c,
                "faction tint carried through to every part"
            );
            assert_eq!(inst.color[3], 0.0, "no muzzle flash on a unit body");
            // model_matrix's translation column places each part at the world (x, y) on the ground.
            assert_eq!(inst.model[3], [3.0, -1.0, 0.0, 1.0]);
        }
    }

    /// `interp_angle` tweens binary-radian angles the SHORT way across the wrap seam and matches the
    /// `model_matrix`/sim `+X = 0`, CCW convention. (render is the float boundary — f32 math is fair.)
    #[test]
    fn interp_angle_is_shortest_arc_and_matches_convention() {
        use std::f32::consts::{FRAC_PI_2, PI, TAU};
        let q = ANGLE_FULL / 4; // a quarter turn in binary radians
        // alpha endpoints return the endpoints (mod TAU).
        assert!((interp_angle(Angle(0), Angle(q), 0.0)).abs() < 1e-5);
        assert!((interp_angle(Angle(0), Angle(q), 1.0) - FRAC_PI_2).abs() < 1e-5);
        // Half-way from 0 → 90° is 45°.
        assert!((interp_angle(Angle(0), Angle(q), 0.5) - FRAC_PI_2 / 2.0).abs() < 1e-5);
        // Shortest arc across the seam: 350° → 10° sweeps +20° FORWARD through 360°/0°, not −340°.
        let a350 = Angle(ANGLE_FULL * 35 / 36); // 350°
        let a10 = Angle(ANGLE_FULL / 36); // 10°
        let mid = interp_angle(a350, a10, 0.5); // expect ≈ 360° ≡ 0 (mod TAU)
        let wrapped = mid.rem_euclid(TAU);
        assert!(
            wrapped < 1e-3 || (wrapped - TAU).abs() < 1e-3,
            "midpoint of 350°→10° is ~0°, got {wrapped} rad"
        );
        // A binary-radian half-turn maps to π.
        assert!((interp_angle(Angle(0), Angle(ANGLE_FULL / 2), 1.0) - PI).abs() < 1e-5);
    }

    /// `interpolate_instances` carries each unit's hull/turret facing into the instance, tweened
    /// shortest-arc from the two snapshots (a Heavy's turret slews independently of its hull).
    #[test]
    fn interpolate_carries_hull_and_turret_yaw() {
        let q = ANGLE_FULL / 4;
        let mut prev_u = unit(Fixed::ZERO, Fixed::ZERO, false);
        prev_u.unit_kind = UnitKind::Heavy;
        let mut curr_u = prev_u.clone();
        curr_u.hull_heading = Angle(q); // hull turns 0 → 90°
        curr_u.turret_yaw = Angle(ANGLE_FULL / 2); // turret turns 0 → 180°
        let prev = snapshot(0, vec![prev_u]);
        let curr = snapshot(1, vec![curr_u]);
        let out = interpolate_instances(&prev, &curr, 0.5, &[]);
        assert!(
            (out[0].hull_yaw - std::f32::consts::FRAC_PI_2 / 2.0).abs() < 1e-5,
            "hull tweens to 45°"
        );
        assert!(
            (out[0].turret_yaw - std::f32::consts::FRAC_PI_2).abs() < 1e-5,
            "turret tweens to 90° (independent of the hull)"
        );
    }

    // ---- interpolate_projectiles: tank-shell tracers (P7) ----

    /// Build a snapshot carrying one in-flight shell.
    fn shell_snapshot(pos: (i32, i32), vel: (i32, i32), height: Fixed, vz: Fixed, f: Faction) -> Snapshot {
        Snapshot {
            tick: 0,
            units: Vec::new(),
            control_points: Vec::new(),
            projectiles: vec![ProjectileSnapshot {
                pos: Vec2::new(Fixed::from_int(pos.0), Fixed::from_int(pos.1)),
                vel: Vec2::new(Fixed::from_int(vel.0), Fixed::from_int(vel.1)),
                height,
                vz,
                faction: f,
            }],
        }
    }

    /// A shell is extrapolated along its velocity by `alpha`, stood at (x, y, height), and tinted hot.
    #[test]
    fn interpolate_projectiles_extrapolates_position_and_tints() {
        let s = shell_snapshot((10, 0), (4, 0), Fixed::ONE, Fixed::ZERO, Faction::Player);
        let out = interpolate_projectiles(&s, 0.5);
        assert_eq!(out.len(), 1);
        // pos.x = 10 + 4·0.5 = 12; y = 0; z = height = 1.
        assert_eq!(out[0].model[3], [12.0, 0.0, 1.0, 1.0]);
        assert_eq!(out[0].color, tracer_color(Faction::Player));
        assert!(out[0].color[3] > 0.0, "tracer glows (emissive a > 0)");
        // alpha = 1 lands on the next-tick ground position (10 + 4 = 14).
        let at1 = interpolate_projectiles(&s, 1.0);
        assert!((at1[0].model[3][0] - 14.0).abs() < EPS);
    }

    /// The bolt is yawed to its travel heading (matching `model_matrix`'s +X=0/CCW convention): a
    /// shell flying +Y points 90°, so its local +X basis maps to world +Y.
    #[test]
    fn interpolate_projectiles_yaws_to_travel_heading() {
        let s = shell_snapshot((0, 0), (0, 5), Fixed::ONE, Fixed::ZERO, Faction::Enemy);
        let out = interpolate_projectiles(&s, 0.0);
        // model_matrix col0 = [s·cos, s·sin, 0, 0]; yaw=90° → [0, TRACER_SCALE, 0, 0].
        assert!(out[0].model[0][0].abs() < EPS, "cos(90°)≈0");
        assert!((out[0].model[0][1] - TRACER_SCALE).abs() < EPS, "sin(90°)≈1");
    }

    /// A shell that has dipped below the ground plane clamps to z=0 (never drawn underground).
    #[test]
    fn interpolate_projectiles_clamps_to_ground() {
        // height -0.25, vz 0 → z = max(0, -0.25) = 0.
        let s = shell_snapshot((1, 1), (1, 0), Fixed::ZERO - Fixed::from_ratio(1, 4), Fixed::ZERO, Faction::Player);
        let out = interpolate_projectiles(&s, 0.0);
        assert_eq!(out[0].model[3][2], 0.0, "below-ground shell clamps to z=0");
    }

    /// No shells in flight → no tracer instances.
    #[test]
    fn interpolate_projectiles_empty_when_no_shells() {
        let s = snapshot(0, vec![unit(Fixed::ZERO, Fixed::ZERO, false)]);
        assert!(interpolate_projectiles(&s, 0.5).is_empty());
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
