//! Cooked greybox mesh loading + GPU upload (decisions.md D44).
//!
//! The Blender pipeline (`tools/models/gen_models.py`) emits, per model, a cooked `.mesh` file
//! next to the `.glb`. This module owns the **runtime** side: it `include_bytes!`s those cooked
//! meshes (so they ride into the binary/APK with no on-device file IO or asset-pack plumbing),
//! parses the dead-simple format into a CPU triangle soup, and uploads it to GPU vertex/index
//! buffers, and owns the shared instanced, depth-tested [`MeshPipeline`] that *draws* them. The
//! per-pass placement math is the caller's: [`crate::world::weapon_view_model`] for the embodied
//! weapon viewmodel, and the command-view unit-token pass for world-space tokens.
//!
//! ## The cooked `.mesh` format (must match `tools/models/gen_models.py::export_mesh`)
//! Little-endian, Z-up (the game's ground-plane convention — world XY on `z = 0`, Z up), a
//! flat-shaded triangle soup (one vertex per triangle corner, each carrying its face normal):
//! ```text
//!   magic   : 4 bytes  b"GDM1"
//!   v_count : u32       number of vertices  (== 3 × triangle count)
//!   i_count : u32       number of indices
//!   verts   : v_count × [px,py,pz, nx,ny,nz]  f32   (24 bytes each)
//!   indices : i_count × u32
//! ```
//! This is the **float boundary** (invariant #1): every number here is already `f32`, and none of
//! it touches `core`/the sim — meshes are render-only. The crate stays `glam`/windowing-free (D19):
//! the host hands matrices in as plain column-major `[[f32; 4]; 4]` arrays; the small amount of
//! transform math we *do* need ([`model_matrix`]) is hand-rolled scalar `f32`.

use wgpu::util::DeviceExt;

/// Magic bytes at the head of a cooked `.mesh` file.
pub const MESH_MAGIC: [u8; 4] = *b"GDM1";

/// Number of LOD tiers the cook pipeline emits per model: LOD0 (full) + two gltfpack-decimated
/// tiers (`<name>.lod1.mesh` ≈ ½ tris, `<name>.lod2.mesh` ≈ ¼). See `tools/models/gen_models.py`
/// and `docs/content-pipeline.md`. The library loads all tiers; [`select_lod`] picks one by
/// camera distance at draw time.
pub const LOD_COUNT: usize = 3;

/// Distance (world metres, eye→mesh) at/after which the renderer drops from LOD0→LOD1.
pub const LOD1_DISTANCE: f32 = 10.0;
/// Distance (world metres) at/after which the renderer drops from LOD1→LOD2.
pub const LOD2_DISTANCE: f32 = 22.0;

/// Pick a LOD tier (`0..LOD_COUNT`) for a mesh `distance` (world metres) from the camera. Coarser
/// tiers kick in past [`LOD1_DISTANCE`]/[`LOD2_DISTANCE`]; nearer than that we keep full detail.
/// Pure + testable; the renderer calls this per world-space prop so distant scenery costs fewer
/// triangles on the 200-unit mobile budget (content-pipeline §2). Negative/NaN distances clamp to
/// the nearest tier (0).
#[inline]
pub fn select_lod(distance: f32) -> usize {
    if distance >= LOD2_DISTANCE {
        2
    } else if distance >= LOD1_DISTANCE {
        1
    } else {
        0
    }
}

/// Depth buffer format for the 3D mesh passes. `Depth32Float` is universally supported (incl.
/// mobile/WebGPU) and gives plenty of precision for our small scenes.
pub const DEPTH_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Depth32Float;

/// One vertex of a cooked mesh: position + flat (face) normal, both world-space `f32`.
/// `repr(C)` + `Pod` so a parsed `Vec<MeshVertex>` uploads straight into a vertex buffer; the
/// field order MUST match [`MeshGpu::vertex_layout`] and the mesh shaders' `@location`s.
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, bytemuck::Pod, bytemuck::Zeroable)]
pub struct MeshVertex {
    /// Position in Z-up world metres (base at `z ≈ 0`).
    pub pos: [f32; 3],
    /// Unit face normal (flat shading) for greybox facets.
    pub normal: [f32; 3],
}

/// A parsed cooked mesh on the CPU — a flat-shaded triangle soup. Pure data, no GPU handle, so it
/// is fully unit-testable (parse the committed `.mesh`, assert the geometry).
#[derive(Clone, Debug, PartialEq)]
pub struct MeshCpu {
    pub vertices: Vec<MeshVertex>,
    pub indices: Vec<u32>,
}

/// Why a cooked `.mesh` blob failed to parse. A clear, typed failure beats a panic deep in the
/// renderer — though in practice the only inputs are our own committed, golden-tested files.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MeshParseError {
    /// Fewer than the 12-byte header.
    TooShort,
    /// Magic bytes are not `b"GDM1"`.
    BadMagic,
    /// The declared vertex/index counts don't match the blob length.
    LengthMismatch,
    /// An index points past the vertex array.
    IndexOutOfRange,
}

impl MeshCpu {
    /// Parse a cooked `.mesh` blob (see the module/format docs). Validates the magic, that the
    /// declared counts exactly account for the byte length, and that every index is in range —
    /// so a corrupt or wrong-format blob is a typed error, never a GPU-side surprise.
    pub fn parse(bytes: &[u8]) -> Result<MeshCpu, MeshParseError> {
        if bytes.len() < 12 {
            return Err(MeshParseError::TooShort);
        }
        if bytes[0..4] != MESH_MAGIC {
            return Err(MeshParseError::BadMagic);
        }
        let u32_at = |o: usize| u32::from_le_bytes([bytes[o], bytes[o + 1], bytes[o + 2], bytes[o + 3]]);
        let v_count = u32_at(4) as usize;
        let i_count = u32_at(8) as usize;

        // Exact length check in u64 so huge declared counts can't overflow into a false match.
        let expected = 12u64 + v_count as u64 * 24 + i_count as u64 * 4;
        if expected != bytes.len() as u64 {
            return Err(MeshParseError::LengthMismatch);
        }

        let f32_at = |o: usize| f32::from_le_bytes([bytes[o], bytes[o + 1], bytes[o + 2], bytes[o + 3]]);
        let mut vertices = Vec::with_capacity(v_count);
        for k in 0..v_count {
            let o = 12 + k * 24;
            vertices.push(MeshVertex {
                pos: [f32_at(o), f32_at(o + 4), f32_at(o + 8)],
                normal: [f32_at(o + 12), f32_at(o + 16), f32_at(o + 20)],
            });
        }

        let ibase = 12 + v_count * 24;
        let mut indices = Vec::with_capacity(i_count);
        for k in 0..i_count {
            let idx = u32_at(ibase + k * 4);
            if idx as usize >= v_count {
                return Err(MeshParseError::IndexOutOfRange);
            }
            indices.push(idx);
        }

        Ok(MeshCpu { vertices, indices })
    }
}

/// Every greybox model the cooked pipeline produces. Order is the canonical index used by
/// [`MeshLibrary`] (`kind as usize`); keep [`ModelKind::ALL`] and the library array in lockstep.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ModelKind {
    Trooper,
    Tank,
    /// The tank's turret + barrel as its own mesh node, drawn atop the [`Tank`] hull and yawed
    /// independently by the sim's `turret_yaw` (tank embodiment P7, D55).
    TankTurret,
    CampHq,
    WeaponRifle,
    Crate,
    Turret,
    Tree,
    Rock,
    Barricade,
    /// A tank-shell tracer — a small bolt placed at an in-flight shell and yawed by its velocity,
    /// drawn with a hot emissive tint (tank embodiment P7, D55).
    Tracer,
    // --- Faction cosmetic silhouettes (factions-plan WS-C, D68). Presentation-only per-army
    // variants of the headline archetypes; [`crate::model_for_unit`] / [`crate::weapon_model_for`]
    // resolve `(Army, kind)` to one of these. They never reach `core` (no checksum surface). New
    // variants are APPENDED so the existing discriminants (the `model` field written into
    // [`crate::UnitInstance`]) are untouched. ---
    /// US Army infantry silhouette (rounded combat helmet, plate-carrier torso).
    TrooperUs,
    /// French Army infantry silhouette (flatter brimmed helmet, slimmer profile).
    TrooperFr,
    /// US M1 Abrams hull (long, low, flat — its [`TankTurretUs`](ModelKind::TankTurretUs) slews atop it).
    TankUs,
    /// US M1 Abrams turret (broad flat turret + long 120mm gun), yawed by `turret_yaw` (P7).
    TankTurretUs,
    /// French Leclerc hull (compact, steeper glacis — its [`TankTurretFr`](ModelKind::TankTurretFr) slews atop it).
    TankFr,
    /// French Leclerc turret (taller box + rear autoloader bustle), yawed by `turret_yaw` (P7).
    TankTurretFr,
    /// US M4 carbine first-person viewmodel.
    WeaponRifleUs,
    /// French FAMAS bullpup first-person viewmodel.
    WeaponRifleFr,
}

impl ModelKind {
    /// Every kind, in canonical (enum-discriminant) order. Faction silhouettes (WS-C) are appended
    /// after the shared kinds so existing discriminants stay put.
    pub const ALL: [ModelKind; 19] = [
        ModelKind::Trooper,
        ModelKind::Tank,
        ModelKind::TankTurret,
        ModelKind::CampHq,
        ModelKind::WeaponRifle,
        ModelKind::Crate,
        ModelKind::Turret,
        ModelKind::Tree,
        ModelKind::Rock,
        ModelKind::Barricade,
        ModelKind::Tracer,
        ModelKind::TrooperUs,
        ModelKind::TrooperFr,
        ModelKind::TankUs,
        ModelKind::TankTurretUs,
        ModelKind::TankFr,
        ModelKind::TankTurretFr,
        ModelKind::WeaponRifleUs,
        ModelKind::WeaponRifleFr,
    ];

    /// The cooked `.mesh` bytes for every LOD tier, embedded at build time so they ride into the
    /// binary/APK with no runtime file IO (no PAL storage round-trip, no Android asset-pack
    /// plumbing). Index by LOD level (`0` = full detail, `1`/`2` = gltfpack-decimated). The
    /// committed files are the golden output of `pnpm assets:models` (`tools/models/gen_models.py`).
    pub fn cooked_lods(self) -> [&'static [u8]; LOD_COUNT] {
        match self {
            ModelKind::Trooper => [
                include_bytes!("../../assets/models/units/trooper.mesh"),
                include_bytes!("../../assets/models/units/trooper.lod1.mesh"),
                include_bytes!("../../assets/models/units/trooper.lod2.mesh"),
            ],
            ModelKind::Tank => [
                include_bytes!("../../assets/models/units/tank.mesh"),
                include_bytes!("../../assets/models/units/tank.lod1.mesh"),
                include_bytes!("../../assets/models/units/tank.lod2.mesh"),
            ],
            ModelKind::TankTurret => [
                include_bytes!("../../assets/models/units/tank_turret.mesh"),
                include_bytes!("../../assets/models/units/tank_turret.lod1.mesh"),
                include_bytes!("../../assets/models/units/tank_turret.lod2.mesh"),
            ],
            ModelKind::CampHq => [
                include_bytes!("../../assets/models/structures/camp_hq.mesh"),
                include_bytes!("../../assets/models/structures/camp_hq.lod1.mesh"),
                include_bytes!("../../assets/models/structures/camp_hq.lod2.mesh"),
            ],
            ModelKind::WeaponRifle => [
                include_bytes!("../../assets/models/weapons/weapon_rifle.mesh"),
                include_bytes!("../../assets/models/weapons/weapon_rifle.lod1.mesh"),
                include_bytes!("../../assets/models/weapons/weapon_rifle.lod2.mesh"),
            ],
            ModelKind::Crate => [
                include_bytes!("../../assets/models/props/crate.mesh"),
                include_bytes!("../../assets/models/props/crate.lod1.mesh"),
                include_bytes!("../../assets/models/props/crate.lod2.mesh"),
            ],
            ModelKind::Turret => [
                include_bytes!("../../assets/models/structures/turret.mesh"),
                include_bytes!("../../assets/models/structures/turret.lod1.mesh"),
                include_bytes!("../../assets/models/structures/turret.lod2.mesh"),
            ],
            ModelKind::Tree => [
                include_bytes!("../../assets/models/props/tree.mesh"),
                include_bytes!("../../assets/models/props/tree.lod1.mesh"),
                include_bytes!("../../assets/models/props/tree.lod2.mesh"),
            ],
            ModelKind::Rock => [
                include_bytes!("../../assets/models/props/rock.mesh"),
                include_bytes!("../../assets/models/props/rock.lod1.mesh"),
                include_bytes!("../../assets/models/props/rock.lod2.mesh"),
            ],
            ModelKind::Barricade => [
                include_bytes!("../../assets/models/structures/barricade.mesh"),
                include_bytes!("../../assets/models/structures/barricade.lod1.mesh"),
                include_bytes!("../../assets/models/structures/barricade.lod2.mesh"),
            ],
            ModelKind::Tracer => [
                include_bytes!("../../assets/models/fx/tracer.mesh"),
                include_bytes!("../../assets/models/fx/tracer.lod1.mesh"),
                include_bytes!("../../assets/models/fx/tracer.lod2.mesh"),
            ],
            // --- Faction cosmetic silhouettes (WS-C) ---
            ModelKind::TrooperUs => [
                include_bytes!("../../assets/models/units/trooper_us.mesh"),
                include_bytes!("../../assets/models/units/trooper_us.lod1.mesh"),
                include_bytes!("../../assets/models/units/trooper_us.lod2.mesh"),
            ],
            ModelKind::TrooperFr => [
                include_bytes!("../../assets/models/units/trooper_fr.mesh"),
                include_bytes!("../../assets/models/units/trooper_fr.lod1.mesh"),
                include_bytes!("../../assets/models/units/trooper_fr.lod2.mesh"),
            ],
            ModelKind::TankUs => [
                include_bytes!("../../assets/models/units/tank_us.mesh"),
                include_bytes!("../../assets/models/units/tank_us.lod1.mesh"),
                include_bytes!("../../assets/models/units/tank_us.lod2.mesh"),
            ],
            ModelKind::TankTurretUs => [
                include_bytes!("../../assets/models/units/tank_turret_us.mesh"),
                include_bytes!("../../assets/models/units/tank_turret_us.lod1.mesh"),
                include_bytes!("../../assets/models/units/tank_turret_us.lod2.mesh"),
            ],
            ModelKind::TankFr => [
                include_bytes!("../../assets/models/units/tank_fr.mesh"),
                include_bytes!("../../assets/models/units/tank_fr.lod1.mesh"),
                include_bytes!("../../assets/models/units/tank_fr.lod2.mesh"),
            ],
            ModelKind::TankTurretFr => [
                include_bytes!("../../assets/models/units/tank_turret_fr.mesh"),
                include_bytes!("../../assets/models/units/tank_turret_fr.lod1.mesh"),
                include_bytes!("../../assets/models/units/tank_turret_fr.lod2.mesh"),
            ],
            ModelKind::WeaponRifleUs => [
                include_bytes!("../../assets/models/weapons/weapon_rifle_us.mesh"),
                include_bytes!("../../assets/models/weapons/weapon_rifle_us.lod1.mesh"),
                include_bytes!("../../assets/models/weapons/weapon_rifle_us.lod2.mesh"),
            ],
            ModelKind::WeaponRifleFr => [
                include_bytes!("../../assets/models/weapons/weapon_rifle_fr.mesh"),
                include_bytes!("../../assets/models/weapons/weapon_rifle_fr.lod1.mesh"),
                include_bytes!("../../assets/models/weapons/weapon_rifle_fr.lod2.mesh"),
            ],
        }
    }

    /// The full-detail (LOD0) cooked `.mesh` bytes — the tier under closest scrutiny. Shorthand for
    /// `self.cooked_lods()[0]`.
    pub fn cooked_bytes(self) -> &'static [u8] {
        self.cooked_lods()[0]
    }

    /// The model's greybox base tint. **Mirrors `COLORS` in `tools/models/gen_models.py`** (the
    /// `.mesh` is geometry-only, so the colour lives here on the render side) — keep the two in
    /// step; the manifest's `base_color` is the auditable record of the intended value. A unit
    /// token's faction colour overrides this at draw time; structures/props use it directly.
    pub fn base_color(self) -> [f32; 3] {
        match self {
            ModelKind::Trooper => [0.30, 0.34, 0.18],
            ModelKind::Tank => [0.18, 0.22, 0.14],
            ModelKind::TankTurret => [0.18, 0.22, 0.14], // matches the hull
            ModelKind::CampHq => [0.45, 0.40, 0.30],
            ModelKind::WeaponRifle => [0.12, 0.12, 0.13],
            ModelKind::Crate => [0.40, 0.28, 0.16],
            ModelKind::Turret => [0.22, 0.24, 0.26],
            ModelKind::Tree => [0.16, 0.30, 0.16],
            ModelKind::Rock => [0.40, 0.40, 0.42],
            ModelKind::Barricade => [0.34, 0.30, 0.22],
            ModelKind::Tracer => [1.00, 0.60, 0.20], // hot orange; the renderer drives the per-shell glow
            // --- Faction cosmetic silhouettes (WS-C). Mirrors COLORS in gen_models.py. A unit token's
            // faction allegiance tint overrides this at draw time; these are the greybox fallbacks. ---
            ModelKind::TrooperUs => [0.30, 0.34, 0.18],
            ModelKind::TrooperFr => [0.27, 0.31, 0.20],
            ModelKind::TankUs => [0.30, 0.31, 0.24],
            ModelKind::TankTurretUs => [0.30, 0.31, 0.24], // matches the US hull
            ModelKind::TankFr => [0.22, 0.27, 0.18],
            ModelKind::TankTurretFr => [0.22, 0.27, 0.18], // matches the FR hull
            ModelKind::WeaponRifleUs => [0.12, 0.12, 0.13],
            ModelKind::WeaponRifleFr => [0.13, 0.13, 0.12],
        }
    }
}

/// A GPU-resident mesh: a vertex buffer, an index buffer, and the index count to draw.
pub struct MeshGpu {
    pub vertex_buf: wgpu::Buffer,
    pub index_buf: wgpu::Buffer,
    pub index_count: u32,
}

impl MeshGpu {
    /// Upload a parsed [`MeshCpu`] into immutable GPU vertex/index buffers.
    pub fn upload(device: &wgpu::Device, cpu: &MeshCpu, label: &str) -> Self {
        let vertex_buf = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some(label),
            contents: bytemuck::cast_slice(&cpu.vertices),
            usage: wgpu::BufferUsages::VERTEX,
        });
        let index_buf = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some(label),
            contents: bytemuck::cast_slice(&cpu.indices),
            usage: wgpu::BufferUsages::INDEX,
        });
        MeshGpu {
            vertex_buf,
            index_buf,
            index_count: cpu.indices.len() as u32,
        }
    }

    /// The per-vertex buffer layout shared by every mesh pipeline: `0 => position (vec3)`,
    /// `1 => normal (vec3)`.
    pub fn vertex_layout() -> wgpu::VertexBufferLayout<'static> {
        const ATTRS: [wgpu::VertexAttribute; 2] =
            wgpu::vertex_attr_array![0 => Float32x3, 1 => Float32x3];
        wgpu::VertexBufferLayout {
            array_stride: std::mem::size_of::<MeshVertex>() as u64,
            step_mode: wgpu::VertexStepMode::Vertex,
            attributes: &ATTRS,
        }
    }
}

/// Every greybox mesh, uploaded once and indexed by [`ModelKind`]. Built in `Renderer::new`; the
/// weapon and unit-token passes borrow meshes out of it to draw.
pub struct MeshLibrary {
    /// Indexed `[kind as usize][lod]`; every kind carries [`LOD_COUNT`] uploaded tiers.
    meshes: Vec<[MeshGpu; LOD_COUNT]>,
}

impl MeshLibrary {
    /// Parse + upload every [`ModelKind`]'s every LOD tier from its embedded cooked bytes. The
    /// committed `.mesh` files are golden-tested, so `parse` cannot fail here in practice; an
    /// unexpected parse error panics with the offending model + tier rather than silently dropping
    /// a mesh.
    pub fn load(device: &wgpu::Device) -> Self {
        let meshes = ModelKind::ALL
            .iter()
            .map(|&kind| {
                let tiers = kind.cooked_lods();
                std::array::from_fn(|lod| {
                    let cpu = MeshCpu::parse(tiers[lod]).unwrap_or_else(|e| {
                        panic!("cooked mesh for {kind:?} LOD{lod} failed to parse: {e:?}")
                    });
                    MeshGpu::upload(device, &cpu, "gonedark.mesh")
                })
            })
            .collect();
        MeshLibrary { meshes }
    }

    /// Borrow the full-detail (LOD0) GPU mesh for a kind.
    pub fn get(&self, kind: ModelKind) -> &MeshGpu {
        self.get_lod(kind, 0)
    }

    /// Borrow a specific LOD tier's GPU mesh for a kind. `lod` is clamped into `0..LOD_COUNT`, so
    /// a [`select_lod`] result is always safe to pass.
    pub fn get_lod(&self, kind: ModelKind, lod: usize) -> &MeshGpu {
        &self.meshes[kind as usize][lod.min(LOD_COUNT - 1)]
    }
}

/// One drawn instance of a mesh: a column-major model matrix placing it, plus an RGBA tint
/// (`a` carries a muzzle-flash/emissive term, 0 for ordinary tokens). `repr(C)` + `Pod` so a
/// `Vec<MeshInstance>` uploads straight into the per-instance vertex buffer; field order MUST
/// match [`MeshPipeline`]'s instance attribute locations and `mesh.wgsl`.
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, bytemuck::Pod, bytemuck::Zeroable)]
pub struct MeshInstance {
    /// Column-major model matrix (world placement for tokens; view-space placement for the weapon).
    pub model: [[f32; 4]; 4],
    /// RGB tint × `a` = emissive/flash add (0 for tokens).
    pub color: [f32; 4],
}

/// Global per-pass uniform for the mesh pipeline: the camera matrix (`view_proj` for world-space
/// tokens, or the projection alone for the view-space weapon) and a world-space light direction.
/// `repr(C)` + `Pod`; field offsets MUST match `mesh.wgsl`'s `Globals`.
#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct MeshGlobals {
    view_proj: [[f32; 4]; 4],
    /// Light travel direction in xyz; w unused padding.
    light_dir: [f32; 4],
}

/// One drawable batch: a mesh and the instances to draw of it.
pub struct MeshBatch<'a> {
    pub mesh: &'a MeshGpu,
    pub instances: Vec<MeshInstance>,
}

/// The shared instanced, depth-tested 3D mesh pipeline. Both the embodied weapon viewmodel and the
/// command-view unit tokens draw through this one pipeline — they differ only in the camera matrix
/// and which meshes/instances they hand in. Lit with a single directional light + ambient (flat
/// normals → faceted greybox shading). Owns its global uniform + a growable instance buffer.
pub struct MeshPipeline {
    pipeline: wgpu::RenderPipeline,
    globals_buf: wgpu::Buffer,
    globals_bind_group: wgpu::BindGroup,
    instance_buf: wgpu::Buffer,
    instance_cap: usize,
}

impl MeshPipeline {
    /// A default key light: a high front-ish direction so the faceted greybox forms read with a lit
    /// top and shaded underside. (Direction of travel — surfaces facing `-light` are lit.)
    pub const DEFAULT_LIGHT: [f32; 3] = [-0.32, -0.45, -0.83];

    /// Build the mesh pipeline against the swapchain `surface_format`, depth-testing against
    /// [`DEPTH_FORMAT`]. The `device` is borrowed (D19).
    pub fn new(device: &wgpu::Device, surface_format: wgpu::TextureFormat) -> Self {
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("gonedark.mesh_shader"),
            source: wgpu::ShaderSource::Wgsl(include_str!("mesh.wgsl").into()),
        });

        let globals_buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("gonedark.mesh_globals"),
            size: std::mem::size_of::<MeshGlobals>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let globals_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("gonedark.mesh_globals_layout"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX | wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            }],
        });
        let globals_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("gonedark.mesh_globals_bind_group"),
            layout: &globals_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: globals_buf.as_entire_binding(),
            }],
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("gonedark.mesh_pipeline_layout"),
            bind_group_layouts: &[Some(&globals_layout)],
            immediate_size: 0,
        });

        // Per-instance: model matrix as 4 vec4 columns (loc 2..=5) + RGBA tint (loc 6).
        const INSTANCE_ATTRS: [wgpu::VertexAttribute; 5] = wgpu::vertex_attr_array![
            2 => Float32x4, 3 => Float32x4, 4 => Float32x4, 5 => Float32x4, 6 => Float32x4
        ];
        let instance_layout = wgpu::VertexBufferLayout {
            array_stride: std::mem::size_of::<MeshInstance>() as u64,
            step_mode: wgpu::VertexStepMode::Instance,
            attributes: &INSTANCE_ATTRS,
        };

        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("gonedark.mesh_pipeline"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_main"),
                buffers: &[MeshGpu::vertex_layout(), instance_layout],
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
                // Greybox meshes are exported CCW; cull backfaces so interiors don't show through.
                cull_mode: Some(wgpu::Face::Back),
                ..Default::default()
            },
            depth_stencil: Some(wgpu::DepthStencilState {
                format: DEPTH_FORMAT,
                depth_write_enabled: Some(true),
                depth_compare: Some(wgpu::CompareFunction::Less),
                stencil: wgpu::StencilState::default(),
                bias: wgpu::DepthBiasState::default(),
            }),
            multisample: wgpu::MultisampleState::default(),
            multiview_mask: None,
            cache: None,
        });

        let instance_cap = 64;
        let instance_buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("gonedark.mesh_instance_vbo"),
            size: (instance_cap * std::mem::size_of::<MeshInstance>()) as u64,
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        MeshPipeline {
            pipeline,
            globals_buf,
            globals_bind_group,
            instance_buf,
            instance_cap,
        }
    }

    /// Draw `batches` into `view` with depth-testing against `depth_view`. The depth attachment is
    /// always CLEARED (each mesh pass owns its own depth); `color_load` chooses whether the colour
    /// target is cleared (command-view tokens) or loaded over (the weapon, drawn atop the world).
    /// `camera` is the matrix the shader multiplies positions by (`view_proj` for tokens, the
    /// projection alone for the view-space weapon). Instances across all batches share one growable
    /// buffer; each batch draws its own slice. A no-op if there's nothing to draw.
    #[allow(clippy::too_many_arguments)]
    pub fn draw(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        view: &wgpu::TextureView,
        depth_view: &wgpu::TextureView,
        camera: &[[f32; 4]; 4],
        light_dir: [f32; 3],
        color_load: wgpu::LoadOp<wgpu::Color>,
        batches: &[MeshBatch<'_>],
    ) {
        let total: usize = batches.iter().map(|b| b.instances.len()).sum();
        if total == 0 {
            return;
        }

        // Flatten instances into one buffer; remember each batch's [start, count) slice.
        let mut all: Vec<MeshInstance> = Vec::with_capacity(total);
        let mut ranges: Vec<(usize, u32)> = Vec::with_capacity(batches.len());
        for b in batches {
            let start = all.len();
            all.extend_from_slice(&b.instances);
            ranges.push((start, b.instances.len() as u32));
        }

        if all.len() > self.instance_cap {
            let new_cap = all.len().next_power_of_two();
            self.instance_buf = device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("gonedark.mesh_instance_vbo"),
                size: (new_cap * std::mem::size_of::<MeshInstance>()) as u64,
                usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });
            self.instance_cap = new_cap;
        }
        queue.write_buffer(&self.instance_buf, 0, bytemuck::cast_slice(&all));

        let globals = MeshGlobals {
            view_proj: *camera,
            light_dir: [light_dir[0], light_dir[1], light_dir[2], 0.0],
        };
        queue.write_buffer(&self.globals_buf, 0, bytemuck::bytes_of(&globals));

        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("gonedark.mesh_encoder"),
        });
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("gonedark.mesh_pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: color_load,
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view: depth_view,
                    depth_ops: Some(wgpu::Operations {
                        load: wgpu::LoadOp::Clear(1.0),
                        // Each mesh pass clears depth itself and nothing samples it afterward, so
                        // discard rather than store — avoids flushing the depth tile back to DRAM
                        // on tile-based GPUs (the Adreno 750 target, D22).
                        store: wgpu::StoreOp::Discard,
                    }),
                    stencil_ops: None,
                }),
                multiview_mask: None,
                timestamp_writes: None,
                occlusion_query_set: None,
            });
            pass.set_pipeline(&self.pipeline);
            pass.set_bind_group(0, &self.globals_bind_group, &[]);
            pass.set_vertex_buffer(1, self.instance_buf.slice(..));
            for (batch, &(start, count)) in batches.iter().zip(&ranges) {
                if count == 0 {
                    continue;
                }
                pass.set_vertex_buffer(0, batch.mesh.vertex_buf.slice(..));
                pass.set_index_buffer(batch.mesh.index_buf.slice(..), wgpu::IndexFormat::Uint32);
                let end = start as u32 + count;
                pass.draw_indexed(0..batch.mesh.index_count, 0, start as u32..end);
            }
        }
        queue.submit(std::iter::once(encoder.finish()));
    }
}

/// Create a depth texture view sized to the target, for the 3D mesh passes. Recreated by the host
/// whenever the surface resizes (cheap; only on resize). Render-only — depth never touches the sim.
pub fn create_depth_view(device: &wgpu::Device, width: u32, height: u32) -> wgpu::TextureView {
    let texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("gonedark.depth"),
        size: wgpu::Extent3d {
            width: width.max(1),
            height: height.max(1),
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: DEPTH_FORMAT,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
        view_formats: &[],
    });
    texture.create_view(&wgpu::TextureViewDescriptor::default())
}

/// Build a column-major model matrix from a translation, a uniform `scale`, and a `yaw` (radians,
/// about the Z/up axis) — the placement of one mesh token in world space. Column-major `[[f32;4];4]`
/// matches the host's `glam` convention (`Mat4::to_cols_array_2d`), so the mesh shader can compute
/// `view_proj * model * vec4(pos, 1)`. Pure scalar `f32` (no `glam` dep, D19) and unit-testable.
pub fn model_matrix(translation: [f32; 3], scale: f32, yaw: f32) -> [[f32; 4]; 4] {
    let (sy, cy) = yaw.sin_cos();
    let s = scale;
    // Columns: image of each scaled+rotated basis vector, then the translation column.
    [
        [s * cy, s * sy, 0.0, 0.0],
        [-s * sy, s * cy, 0.0, 0.0],
        [0.0, 0.0, s, 0.0],
        [translation[0], translation[1], translation[2], 1.0],
    ]
}

#[cfg(test)]
mod tests {
    //! `render` is the float boundary (invariant #1), so `f32` math is fair game. These exercise
    //! the pure parser + transform math against the **committed** cooked meshes; the GPU upload
    //! path needs a real device (no display in CI) and is left to `viz-runner`'s smoke test.

    use super::*;

    const EPS: f32 = 1e-5;

    // ---- parser ----

    /// A minimal valid blob: one triangle (3 verts), sequential indices.
    fn one_triangle_blob() -> Vec<u8> {
        let mut b = Vec::new();
        b.extend_from_slice(&MESH_MAGIC);
        b.extend_from_slice(&3u32.to_le_bytes()); // v_count
        b.extend_from_slice(&3u32.to_le_bytes()); // i_count
        for k in 0..3u32 {
            // pos
            for c in [k as f32, 0.0, 0.0] {
                b.extend_from_slice(&c.to_le_bytes());
            }
            // normal (+Z)
            for c in [0.0f32, 0.0, 1.0] {
                b.extend_from_slice(&c.to_le_bytes());
            }
        }
        for k in 0..3u32 {
            b.extend_from_slice(&k.to_le_bytes());
        }
        b
    }

    #[test]
    fn parses_a_minimal_triangle() {
        let m = MeshCpu::parse(&one_triangle_blob()).expect("valid blob parses");
        assert_eq!(m.vertices.len(), 3);
        assert_eq!(m.indices, vec![0, 1, 2]);
        assert_eq!(m.vertices[1].pos, [1.0, 0.0, 0.0]);
        assert_eq!(m.vertices[0].normal, [0.0, 0.0, 1.0]);
    }

    #[test]
    fn rejects_short_bad_magic_and_bad_length() {
        assert_eq!(MeshCpu::parse(&[0u8; 4]), Err(MeshParseError::TooShort));
        let mut wrong = one_triangle_blob();
        wrong[0] = b'X';
        assert_eq!(MeshCpu::parse(&wrong), Err(MeshParseError::BadMagic));
        let mut truncated = one_triangle_blob();
        truncated.pop();
        assert_eq!(MeshCpu::parse(&truncated), Err(MeshParseError::LengthMismatch));
    }

    #[test]
    fn rejects_out_of_range_index() {
        let mut b = one_triangle_blob();
        // Overwrite the last index (offset 12 + 3*24 + 2*4) with 99.
        let o = 12 + 3 * 24 + 2 * 4;
        b[o..o + 4].copy_from_slice(&99u32.to_le_bytes());
        assert_eq!(MeshCpu::parse(&b), Err(MeshParseError::IndexOutOfRange));
    }

    /// Every committed cooked mesh parses, is a non-empty triangle soup, has unit-length normals,
    /// and is finite — the golden check that the Blender pipeline and this parser agree.
    #[test]
    fn every_committed_model_parses_and_is_sane() {
        for kind in ModelKind::ALL {
            let m = MeshCpu::parse(kind.cooked_bytes())
                .unwrap_or_else(|e| panic!("{kind:?} failed to parse: {e:?}"));
            assert!(!m.vertices.is_empty(), "{kind:?} has geometry");
            assert_eq!(m.indices.len() % 3, 0, "{kind:?} is a triangle list");
            assert_eq!(
                m.vertices.len() % 3,
                0,
                "{kind:?} is a flat-shaded soup (3 verts/tri)"
            );
            for v in &m.vertices {
                assert!(
                    v.pos.iter().chain(&v.normal).all(|c| c.is_finite()),
                    "{kind:?} has finite data"
                );
                let n = (v.normal[0].powi(2) + v.normal[1].powi(2) + v.normal[2].powi(2)).sqrt();
                assert!((n - 1.0).abs() < 1e-3, "{kind:?} normals are unit length, got {n}");
            }
        }
    }

    /// Every LOD tier of every model parses + is a sane flat-shaded soup, and the tier triangle
    /// counts are monotone non-increasing (LOD0 ≥ LOD1 ≥ LOD2) — the golden check that the gltfpack
    /// LOD chain (`tools/models/gen_models.py`) and this loader agree.
    #[test]
    fn every_lod_tier_parses_and_is_monotone() {
        for kind in ModelKind::ALL {
            let mut prev_tris = usize::MAX;
            for (lod, bytes) in kind.cooked_lods().iter().enumerate() {
                let m = MeshCpu::parse(bytes)
                    .unwrap_or_else(|e| panic!("{kind:?} LOD{lod} failed to parse: {e:?}"));
                assert!(!m.vertices.is_empty(), "{kind:?} LOD{lod} has geometry");
                assert_eq!(m.vertices.len() % 3, 0, "{kind:?} LOD{lod} is 3 verts/tri");
                let tris = m.vertices.len() / 3;
                assert!(
                    tris <= prev_tris,
                    "{kind:?} LOD{lod} has {tris} tris > previous tier {prev_tris}"
                );
                prev_tris = tris;
            }
        }
    }

    /// `select_lod` switches tiers at the documented distance thresholds and clamps the extremes.
    #[test]
    fn select_lod_switches_at_thresholds() {
        assert_eq!(select_lod(0.0), 0);
        assert_eq!(select_lod(-5.0), 0, "negative distance clamps to nearest tier");
        assert_eq!(select_lod(LOD1_DISTANCE - 0.01), 0);
        assert_eq!(select_lod(LOD1_DISTANCE), 1, "LOD1 threshold is inclusive");
        assert_eq!(select_lod(LOD2_DISTANCE - 0.01), 1);
        assert_eq!(select_lod(LOD2_DISTANCE), 2, "LOD2 threshold is inclusive");
        assert_eq!(select_lod(1.0e6), 2);
        // Every result is a valid library index.
        for d in [0.0, LOD1_DISTANCE, LOD2_DISTANCE, 1.0e6] {
            assert!(select_lod(d) < LOD_COUNT);
        }
    }

    #[test]
    fn base_colors_are_in_range() {
        for kind in ModelKind::ALL {
            for c in kind.base_color() {
                assert!((0.0..=1.0).contains(&c), "{kind:?} colour channel in [0,1]");
            }
        }
    }

    // ---- model matrix ----

    fn mul_point(m: &[[f32; 4]; 4], p: [f32; 3]) -> [f32; 3] {
        // Column-major: out = Σ_j col_j * p_j  (+ translation column for w=1).
        let mut out = [m[3][0], m[3][1], m[3][2]];
        for j in 0..3 {
            for r in 0..3 {
                out[r] += m[j][r] * p[j];
            }
        }
        out
    }

    #[test]
    fn identity_when_unit_scale_no_rotation_no_translation() {
        let m = model_matrix([0.0, 0.0, 0.0], 1.0, 0.0);
        let p = mul_point(&m, [2.0, -3.0, 4.0]);
        assert!((p[0] - 2.0).abs() < EPS && (p[1] + 3.0).abs() < EPS && (p[2] - 4.0).abs() < EPS);
        assert_eq!(m[3], [0.0, 0.0, 0.0, 1.0], "affine bottom row");
    }

    #[test]
    fn translation_places_the_origin() {
        let m = model_matrix([5.0, 7.0, 1.0], 1.0, 0.0);
        assert_eq!(mul_point(&m, [0.0, 0.0, 0.0]), [5.0, 7.0, 1.0]);
    }

    #[test]
    fn uniform_scale_scales_all_axes() {
        let m = model_matrix([0.0, 0.0, 0.0], 3.0, 0.0);
        let p = mul_point(&m, [1.0, 1.0, 1.0]);
        assert!((p[0] - 3.0).abs() < EPS && (p[1] - 3.0).abs() < EPS && (p[2] - 3.0).abs() < EPS);
    }

    /// Validate `mesh.wgsl` offline with naga (the compiler wgpu uses), so a shader regression
    /// fails the suite instead of only blowing up at pipeline creation on a real GPU.
    #[test]
    fn mesh_wgsl_parses_and_validates() {
        let src = include_str!("mesh.wgsl");
        let module = naga::front::wgsl::parse_str(src).expect("mesh.wgsl must parse");
        let mut validator = naga::valid::Validator::new(
            naga::valid::ValidationFlags::all(),
            naga::valid::Capabilities::all(),
        );
        validator.validate(&module).expect("mesh.wgsl must validate");
    }

    #[test]
    fn yaw_90_maps_x_to_y() {
        let m = model_matrix([0.0, 0.0, 0.0], 1.0, std::f32::consts::FRAC_PI_2);
        let p = mul_point(&m, [1.0, 0.0, 0.0]);
        assert!(p[0].abs() < EPS && (p[1] - 1.0).abs() < EPS, "X→Y, got {p:?}");
        // Z (up) is untouched by a yaw about Z.
        let up = mul_point(&m, [0.0, 0.0, 1.0]);
        assert!((up[2] - 1.0).abs() < EPS, "Z stays Z");
    }
}
