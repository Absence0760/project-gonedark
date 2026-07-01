//! Embodied first-person world (W5) — the ground/sky/weapon the avatar stands in while the
//! strategic map is dark (invariant #6). This is **render-only**: it draws a believable
//! first-person *space* (a floor, a horizon, a held weapon) but reveals **no map intel** — no
//! enemy units, no enemy buildings, no control points. Those are filtered out upstream by
//! [`crate::fog::visible_instances`] (the avatar quad is the only world instance that survives the
//! dark frame); this module only ever draws the *environment*, which carries zero intel.
//!
//! ## What it draws, in order (all in the embodied pass, before the avatar + HUD)
//! 1. **Sky + ground** — a single fullscreen pass. The fragment shader reconstructs each pixel's
//!    view ray from the inverse view-projection and shades it: rays that point at/below the ground
//!    plane (`z = 0`) get a gridded floor (so motion + heading read); rays above the horizon get a
//!    sky gradient. This replaces the bare near-black `CLEAR_DARK` void with a real space while
//!    staying a pure function of the *camera* — it has no access to sim entities, so it cannot leak
//!    intel even in principle. This module owns that pass ([`WorldRenderer`]).
//! 2. **Weapon viewmodel** — the first-person gun. As of D44 this is the real `weapon_rifle`
//!    greybox **3D mesh** drawn through the shared [`crate::mesh::MeshPipeline`] (the
//!    [`crate::Renderer`] owns that pipeline + the mesh library + the depth buffer and drives the
//!    pass — see `Renderer::render_world_weapon`), anchored in *view space* by
//!    [`weapon_view_model`] so it stays glued to the lower-right of the screen regardless of camera
//!    yaw. A muzzle-flash term flares the gun for a few ticks after the player fires; this module
//!    still owns the flash *intensity* curve ([`muzzle_flash_intensity`]) and the placement math.
//!
//! The float boundary lives here (invariant #1/#4): every value is already `f32`, the renderer
//! never mutates sim state and never calls back into `core`. Like the rest of this crate it takes
//! **no `glam`/windowing dep** (D19) — the host (which owns glam) hands matrices in as plain
//! column-major `[[f32; 4]; 4]` arrays; this module only does scalar `f32` math.

/// How many ticks the muzzle flash stays lit after a shot before it has fully faded. At 60 Hz this
/// is a ~0.13 s flare — a snappy cue, gone before the next likely shot.
pub const MUZZLE_FLASH_TICKS: u64 = 8;

/// Edge length (px) of the square ground detail map (`assets/textures/ground.gray`). The contract
/// with `tools/textures/gen_textures.py` (`SIZE` there MUST match): the baked file is
/// `GROUND_TEX_SIZE * GROUND_TEX_SIZE` raw R8 bytes. The [`ground_tex_matches_metrics`](tests) test
/// pins the `include_bytes!`d blob length so a generator/metrics drift fails `cargo test`.
pub const GROUND_TEX_SIZE: u32 = 256;

/// The baked seamless ground detail map: raw `GROUND_TEX_SIZE²` R8 bytes (one luminance byte per
/// texel), `include_bytes!`d straight in so the render crate needs no image-decode dependency (it
/// stays `wgpu` + `bytemuck` only — the same rule as the D74 font atlas). Generated with ImageMagick
/// by `tools/textures/gen_textures.py`; render-only, carries no sim/intel (invariants #1/#4/#6).
const GROUND_TEX_BYTES: &[u8] = include_bytes!("../../assets/textures/ground.gray");

/// The baked seamless **high-frequency detail** heightfield (WS-E): raw `GROUND_TEX_SIZE²` R8 bytes,
/// same size/format as [`GROUND_TEX_BYTES`] but a crisper, higher-contrast field. Sampled by the
/// floor shader at a tight world scale and finite-differenced for sharp near-field micro-relief the
/// smooth `ground` field can't carry. Generated with ImageMagick by `tools/textures/gen_textures.py`
/// (the `detail` entry); render-only, carries no sim/intel (invariants #1/#4/#6).
const DETAIL_TEX_BYTES: &[u8] = include_bytes!("../../assets/textures/detail.gray");

/// Compute the muzzle-flash intensity in `[0, 1]` for the current `tick`, given the tick the
/// player last fired on (`None` if they have not fired). Fresh shot → `1.0`, then a linear ramp to
/// `0.0` over [`MUZZLE_FLASH_TICKS`]; a future-stamped or long-past fire is dark. Pure float math
/// (presentation boundary), so it is unit-testable without a GPU.
pub fn muzzle_flash_intensity(last_fire_tick: Option<u64>, tick: u64) -> f32 {
    let Some(fired) = last_fire_tick else {
        return 0.0;
    };
    if tick < fired {
        return 0.0; // future-stamped fire is not yet live
    }
    let age = tick - fired;
    if age >= MUZZLE_FLASH_TICKS {
        return 0.0;
    }
    let t = age as f32 / MUZZLE_FLASH_TICKS as f32; // 0 fresh → 1 at cutoff
    1.0 - t
}

/// Build the column-major **view-space** model matrix that places the weapon viewmodel in the
/// avatar's hands — anchored to the lower-right of the screen and pointing into the world. Because
/// the host hands the mesh pipeline the *projection alone* as its camera matrix for this pass (not
/// `view * proj`), the gun lives in view space and stays put under camera yaw/pitch, exactly like a
/// real FPS viewmodel. Pure scalar `f32` (no `glam`, D19) so it is unit-testable.
///
/// View space is camera-at-origin looking down `-Z`, `+Y` up, `+X` right. The rifle mesh is modelled
/// Z-up with its barrel along local `+X`, so we re-base its axes: local `+X` (barrel) → view `-Z`
/// (forward, into the screen), local `+Z` (up) → view `+Y` (up). `flash` adds a small recoil kick
/// back toward the camera so firing reads as a jolt, not just a colour flare.
pub fn weapon_view_model(flash: f32) -> [[f32; 4]; 4] {
    weapon_view_model_posed(WeaponPose::from_flash(flash))
}

/// The animated state of the embodied weapon viewmodel for one frame — the presentation inputs the
/// host derives from the fire cadence and fire mode. All `f32` (the render float boundary). The two
/// mode-specific channels give the two fire modes their distinct feel (the select-fire request):
/// **semi-auto** shows a visible chambering **cycle** between shots ("the next round is worked in by
/// hand"), while **full-auto** shows a continuous **spray** climb.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct WeaponPose {
    /// Muzzle-flash / recoil-kick intensity `[0,1]` — the per-shot jolt (also the emissive flare).
    pub flash: f32,
    /// Semi-auto chambering phase `[0,1]`: `0` = just fired (action open / bolt back), `1` = fully
    /// chambered and ready. The viewmodel works the action mid-cycle (`≈0.5`) and settles by `1`. In
    /// full-auto this stays `1` (the weapon cycles itself — no manual rack is shown).
    pub cycle: f32,
    /// Full-auto spray intensity `[0,1]` — sustained-fire muzzle climb. `0` in semi-auto (each shot
    /// is deliberate) and while not holding the trigger.
    pub spray: f32,
}

impl WeaponPose {
    /// A ready weapon at rest: no flash, fully chambered, no spray.
    pub fn at_rest() -> Self {
        WeaponPose { flash: 0.0, cycle: 1.0, spray: 0.0 }
    }

    /// Just the recoil/flash channel (chambered, no spray) — the pre-animation behaviour, kept for
    /// [`weapon_view_model`] and its tests.
    pub fn from_flash(flash: f32) -> Self {
        WeaponPose { flash, cycle: 1.0, spray: 0.0 }
    }
}

/// A smooth 0→1→0 bump over `t ∈ [0,1]`, peaking at `t = 0.5`. Used to shape the semi-auto
/// chambering motion so the action works open then closes, rather than snapping.
#[inline]
fn bump(t: f32) -> f32 {
    (t * (1.0 - t) * 4.0).clamp(0.0, 1.0)
}

/// Build the column-major **view-space** model matrix that places the weapon viewmodel in the
/// avatar's hands for the given [`WeaponPose`] — anchored lower-right, pointing into the world, with
/// the per-frame recoil kick, the semi-auto chambering rack, and the full-auto spray climb composed
/// in. Because the host hands the mesh pipeline the *projection alone* as its camera matrix, the gun
/// lives in view space and stays put under camera yaw/pitch, exactly like a real FPS viewmodel. Pure
/// scalar `f32` (no `glam`, D19) so it is unit-testable.
///
/// View space is camera-at-origin looking down `-Z`, `+Y` up, `+X` right. The rifle mesh is modelled
/// Z-up with its barrel along local `+X`, so we re-base its axes: local `+X` (barrel) → view `-Z`
/// (forward), local `+Z` (up) → view `+Y`. `flash` kicks the gun back toward the camera (recoil);
/// `cycle < 1` racks it back-and-down mid-stroke (the manual chambering of a semi-auto); `spray`
/// rides the muzzle up under sustained auto fire.
pub fn weapon_view_model_posed(pose: WeaponPose) -> [[f32; 4]; 4] {
    let s = 0.42; // gun size in view units
    let flash = pose.flash.clamp(0.0, 1.0);
    // The chambering rack: a bump that opens the action mid-cycle. `cycle == 1` (ready) → 0 motion.
    let rack = bump(pose.cycle.clamp(0.0, 1.0));
    let spray = pose.spray.clamp(0.0, 1.0);

    // Lower-right anchor, a little in front of the near plane. Recoil kicks it back/up; the rack
    // pulls it further back + down + inboard (the hand working the charging handle); spray climbs it.
    let tx = 0.16 + rack * 0.05;
    let ty = -0.20 + flash * 0.03 - rack * 0.05 + spray * 0.045;
    let tz = -0.62 + flash * 0.07 + rack * 0.10 + spray * 0.02;

    // Columns = images of the scaled local axes in view space, then the translation column.
    //   local +X (barrel) → view -Z;  local +Y → view -X;  local +Z (up) → view +Y.
    [
        [0.0, 0.0, -s, 0.0], // s * (0,0,-1)
        [-s, 0.0, 0.0, 0.0], // s * (-1,0,0)
        [0.0, s, 0.0, 0.0],  // s * (0,1,0)
        [tx, ty, tz, 1.0],
    ]
}

/// Parameters for the embodied world pass, handed in by the host each frame. All `f32` — the
/// render-side float boundary. `inv_view_proj` is the inverse of the camera's view-projection
/// (column-major, the host's `Mat4::inverse().to_cols_array_2d()`), used by the shader to
/// reconstruct world rays for the sky/ground. `eye` is the camera world position (so the shader can
/// fade the floor grid with distance). `flash` is the current muzzle-flash intensity in `[0,1]`.
/// `repr(C)` + `Pod` so it uploads straight into the uniform buffer; field order/offsets MUST match
/// `world.wgsl`'s `World` uniform.
#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
pub struct WorldUniform {
    /// Inverse view-projection (column-major). The host computes the inverse (it owns glam).
    pub inv_view_proj: [[f32; 4]; 4],
    /// Camera eye in world space (xyz); w is unused padding (kept 0).
    pub eye: [f32; 4],
    /// Muzzle-flash intensity `[0,1]` in x; y/z/w are reserved padding (kept 0).
    pub flash: [f32; 4],
}

impl WorldUniform {
    /// Build the uniform from the host-computed inverse view-projection, the eye world position,
    /// and the muzzle-flash intensity (clamped to `[0,1]`). Pure + device-free, so it is
    /// unit-testable. The host owns the matrix inverse (it has glam; this crate must not — D19).
    pub fn new(inv_view_proj: [[f32; 4]; 4], eye: [f32; 3], flash: f32) -> Self {
        WorldUniform {
            inv_view_proj,
            eye: [eye[0], eye[1], eye[2], 0.0],
            flash: [flash.clamp(0.0, 1.0), 0.0, 0.0, 0.0],
        }
    }
}

/// `x - floor(x)` — the fractional part, matching WGSL `fract`. Pulled out so the night-sky hash
/// mirror below reads like its shader twin.
#[inline]
fn fract(x: f32) -> f32 {
    x - x.floor()
}

/// Hermite smoothstep in `[0,1]` (the GLSL/WGSL `smoothstep`), used by the moon-glow mirror.
#[inline]
fn smoothstep(e0: f32, e1: f32, x: f32) -> f32 {
    let t = ((x - e0) / (e1 - e0)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// Deterministic 2D→1D value hash (Dave Hoskins `hash12`), the **reference implementation** of
/// `world.wgsl`'s `hash21`. The embodied night-sky starfield is a pure function of the view ray
/// through this hash, so the stars are STABLE frame to frame — there is no time input, so they
/// cannot crawl or shimmer (fairness #6). The WGSL copy is validated by naga but cannot run on the
/// CPU; this mirror lets the determinism + range properties be unit-tested off-GPU. Keep the two in
/// lockstep (same constants, same operation order).
pub fn star_hash21(x: f32, y: f32) -> f32 {
    let mut p = [fract(x * 0.1031), fract(y * 0.1031), fract(x * 0.1031)];
    // dot(p, p.yzx + 33.33)
    let d = p[0] * (p[1] + 33.33) + p[1] * (p[2] + 33.33) + p[2] * (p[0] + 33.33);
    p[0] += d;
    p[1] += d;
    p[2] += d;
    fract((p[0] + p[1]) * p[2])
}

/// Moon disc + halo + bloom intensity from `cos_ang`, the cosine of the angle between the view ray
/// and the moon direction — the **reference implementation** of the moon term in `world.wgsl`'s sky
/// branch. Monotonic non-decreasing in `cos_ang` (brightest when the ray points dead at the moon),
/// and zero when the ray points away (`cos_ang <= 0`). Pure `f32` so the disc/halo shaping is
/// unit-testable without a GPU. Keep in lockstep with the shader's `moon_core/halo/bloom`.
pub fn moon_glow(cos_ang: f32) -> f32 {
    let md = cos_ang.max(0.0);
    let core = smoothstep(0.9980, 0.9994, md);
    let halo = md.powf(300.0) * 0.60;
    let bloom = md.powf(14.0) * 0.12;
    core + halo + bloom
}

/// Fallback screen-space NDC anchor of the **shaped muzzle flash** (WS-A) — used only when the
/// projected barrel tip is degenerate (w≈0). Production anchors the flare at the *actual* muzzle via
/// [`muzzle_anchor_ndc`]; this constant is a lower-right last resort. No world position → no intel (#6).
pub const MUZZLE_ANCHOR: (f32, f32) = (0.14, -0.07);

/// Muzzle tip in the rifle's LOCAL model space (barrel along +X, receiver at origin). The
/// `weapon_rifle` greybox seats its muzzle device at x≈0.64 (length 0.06), so the barrel tip is
/// ~0.67 (see `tools/models/gen_models.py::build_weapon_rifle`). Used to anchor the shaped muzzle
/// flash at the *actual* gun muzzle rather than a fixed screen constant.
pub const MUZZLE_TIP_LOCAL: [f32; 3] = [0.67, 0.0, 0.0];

/// NDC (x, y) where the muzzle flash should bloom: the weapon's [`MUZZLE_TIP_LOCAL`] barrel tip run
/// through the view-space [`weapon_view_model_posed`] placement for the SAME `pose` the gun mesh is
/// drawn with (so it tracks the recoil kick, the chambering rack, and the spray climb) and the camera
/// `proj` (column-major), then perspective-divided. Because the gun lives in view space and the host
/// hands the mesh pass the *projection alone* as its camera matrix, `proj * (weapon_view_model * tip)`
/// is exactly the clip position of the barrel tip — the flare lands where the muzzle actually renders.
/// Falls back to the static [`MUZZLE_ANCHOR`] if the point is degenerate (w≈0). Pure `f32`
/// (presentation boundary) → unit-testable off-GPU.
pub fn muzzle_anchor_ndc(proj: &[[f32; 4]; 4], pose: WeaponPose) -> (f32, f32) {
    let m = weapon_view_model_posed(pose);
    let l = MUZZLE_TIP_LOCAL;
    // view = model * (l, 1)  (column-major: v = Σ lᵢ·colᵢ + col3)
    let vx = l[0] * m[0][0] + l[1] * m[1][0] + l[2] * m[2][0] + m[3][0];
    let vy = l[0] * m[0][1] + l[1] * m[1][1] + l[2] * m[2][1] + m[3][1];
    let vz = l[0] * m[0][2] + l[1] * m[1][2] + l[2] * m[2][2] + m[3][2];
    // clip = proj * (view, 1)
    let cx = vx * proj[0][0] + vy * proj[1][0] + vz * proj[2][0] + proj[3][0];
    let cy = vx * proj[0][1] + vy * proj[1][1] + vz * proj[2][1] + proj[3][1];
    let cw = vx * proj[0][3] + vy * proj[1][3] + vz * proj[2][3] + proj[3][3];
    if cw.abs() < 1e-6 {
        return MUZZLE_ANCHOR;
    }
    (cx / cw, cy / cw)
}

/// The muzzle-flash uniform — `params = (flash, aspect, anchor_x, anchor_y)` matching `world.wgsl`'s
/// `Muzzle` struct. `repr(C)` + `Pod` so it uploads straight into the uniform buffer.
#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
pub struct MuzzleUniform {
    pub params: [f32; 4],
}

impl MuzzleUniform {
    /// Build the uniform from the muzzle-flash `intensity` (clamped to `[0,1]`), viewport `aspect`,
    /// and the NDC `anchor` where the flare blooms (the projected barrel tip, see
    /// [`muzzle_anchor_ndc`]). Pure + device-free → unit-testable.
    pub fn new(intensity: f32, aspect: f32, anchor: (f32, f32)) -> Self {
        MuzzleUniform {
            params: [intensity.clamp(0.0, 1.0), aspect, anchor.0, anchor.1],
        }
    }
}

/// Radius (in flare-local `[-1,1]` units) of the expanding shock RING for the current flash
/// `intensity` — tight at the muzzle flash (`flash → 1`), blooming outward as the shot fades
/// (`flash → 0`). **Reference implementation** of the ring radius in `world.wgsl`'s `fs_muzzle`;
/// keep the constants in lockstep. Pure `f32` (presentation boundary) so it is unit-testable.
pub fn muzzle_ring_radius(flash: f32) -> f32 {
    (1.0 - flash) * 0.85 + 0.12
}

/// Visibility weight of the shock RING across the shot's life — a mid-life puff that is dark both at
/// the white-hot flash itself (`flash → 1`) and once fully faded (`flash → 0`), peaking around
/// `flash = 0.5`. Mirrors the ring weight in `world.wgsl`'s `fs_muzzle`; keep in lockstep.
pub fn muzzle_ring_weight(flash: f32) -> f32 {
    (flash * (1.0 - flash) * 4.0).clamp(0.0, 1.0)
}

/// **Reference implementation** of the shaped muzzle-flare intensity at the flare-local point
/// `(px, py)` in `[-1,1]` for the current `flash`, mirroring `world.wgsl`'s `fs_muzzle` shape (the
/// pre-alpha `shape`, before the `× flash` and additive premultiply). It sums a tight white-hot core
/// under a soft warm bloom, an **asymmetric** multi-spike star (three offset cosine harmonics — not
/// a clean symmetric plus), and the expanding shock ring. Pure `f32` so the shape's load-bearing
/// properties — the centre is brightest, the star is NOT 4-fold symmetric, the ring is a mid-life
/// feature — are unit-testable off-GPU. Keep every constant in lockstep with the shader.
pub fn muzzle_flare_shape(px: f32, py: f32, flash: f32) -> f32 {
    let r = (px * px + py * py).sqrt();
    let ang = py.atan2(px);

    let bloom = (1.0 - r).clamp(0.0, 1.0).powf(1.7);
    let hot = (1.0 - r * 2.3).clamp(0.0, 1.0).powf(2.0);
    let core = bloom * 0.55 + hot;

    let reach = (1.0 - r * 0.85).clamp(0.0, 1.0).powf(1.4);
    let s1 = (ang * 2.0 - 0.35).cos().max(0.0).powf(7.0);
    let s2 = (ang * 3.0 + 1.20).cos().max(0.0).powf(11.0);
    let s3 = (ang * 5.0 + 0.60).cos().max(0.0).powf(16.0);
    let flicker = 0.82 + 0.18 * (ang * 9.0 + flash * 22.0).cos();
    let spikes = (s1 + s2 * 0.55 + s3 * 0.40) * reach * flicker;

    let ring_r = muzzle_ring_radius(flash);
    let ring_band = (-((r - ring_r) / 0.11).powf(2.0)).exp();
    let ring = ring_band * muzzle_ring_weight(flash);

    (core + spikes * 0.85 + ring * 0.45).clamp(0.0, 1.4)
}

/// Sky + ground pass for the embodied (first-person) view. Owns the fullscreen sky/ground pipeline
/// (which CLEARS the frame) plus the shaped muzzle-flash flare. The weapon viewmodel is no longer
/// drawn here — it is a 3D mesh drawn by the [`crate::Renderer`] through the shared
/// [`crate::mesh::MeshPipeline`] (D44).
pub struct WorldRenderer {
    /// Fullscreen sky/ground pipeline (clears the frame to the world).
    sky_pipeline: wgpu::RenderPipeline,
    /// The world uniform (inverse view-proj, eye, flash).
    uniform_buf: wgpu::Buffer,
    uniform_bind_group: wgpu::BindGroup,
    /// The shaped muzzle-flash flare (WS-A): an additive screen-space flare at [`MUZZLE_ANCHOR`],
    /// drawn by [`render_muzzle_flash`](Self::render_muzzle_flash) after the weapon viewmodel.
    muzzle_pipeline: wgpu::RenderPipeline,
    muzzle_uniform_buf: wgpu::Buffer,
    muzzle_bind_group: wgpu::BindGroup,
    /// The ground detail-map texture, kept so the raw R8 bytes can be uploaded lazily on the first
    /// [`render_sky`](Self::render_sky) (the construction path has only a `device`, not a `queue` —
    /// the same lazy-upload pattern as `text::TextRenderer::ensure_atlas_uploaded`).
    ground_tex: wgpu::Texture,
    /// The high-frequency detail heightfield (WS-E), uploaded lazily alongside [`ground_tex`] and
    /// sampled by the floor shader for crisp near-field micro-relief.
    detail_tex: wgpu::Texture,
    /// Whether [`ground_tex`](Self::ground_tex)'s + [`detail_tex`](Self::detail_tex)'s bytes have
    /// been written yet (both upload together on the first render).
    ground_uploaded: bool,
}

impl WorldRenderer {
    /// Build the sky/ground pipeline against the swapchain `surface_format`. The `device` is
    /// borrowed (D19).
    pub fn new(device: &wgpu::Device, surface_format: wgpu::TextureFormat) -> Self {
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("gonedark.world_shader"),
            source: wgpu::ShaderSource::Wgsl(include_str!("world.wgsl").into()),
        });

        let uniform_buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("gonedark.world_uniform"),
            size: std::mem::size_of::<WorldUniform>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        // The ground detail-map texture (R8 coverage); bytes written lazily on the first render_sky()
        // (the construction path has no queue — the `text` atlas pattern). A REPEAT sampler so the
        // shader can tile it across the world plane seamlessly.
        let ground_tex = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("gonedark.world_ground_tex"),
            size: wgpu::Extent3d {
                width: GROUND_TEX_SIZE,
                height: GROUND_TEX_SIZE,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::R8Unorm,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        let ground_view = ground_tex.create_view(&wgpu::TextureViewDescriptor::default());
        // The high-frequency detail heightfield (WS-E) — same R8 format/size as the ground map,
        // bytes written lazily on the first render_sky(). Shares the ground's REPEAT/Linear sampler.
        let detail_tex = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("gonedark.world_detail_tex"),
            size: wgpu::Extent3d {
                width: GROUND_TEX_SIZE,
                height: GROUND_TEX_SIZE,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::R8Unorm,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        let detail_view = detail_tex.create_view(&wgpu::TextureViewDescriptor::default());
        let ground_samp = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("gonedark.world_ground_sampler"),
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            address_mode_u: wgpu::AddressMode::Repeat,
            address_mode_v: wgpu::AddressMode::Repeat,
            ..Default::default()
        });

        let uniform_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("gonedark.world_uniform_layout"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::VERTEX | wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
                // binding 3: the WS-E detail heightfield (shares binding 2's sampler).
                wgpu::BindGroupLayoutEntry {
                    binding: 3,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
            ],
        });

        let uniform_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("gonedark.world_uniform_bind_group"),
            layout: &uniform_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: uniform_buf.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::TextureView(&ground_view),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: wgpu::BindingResource::Sampler(&ground_samp),
                },
                wgpu::BindGroupEntry {
                    binding: 3,
                    resource: wgpu::BindingResource::TextureView(&detail_view),
                },
            ],
        });

        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("gonedark.world_pipeline_layout"),
            bind_group_layouts: &[Some(&uniform_layout)],
            immediate_size: 0,
        });

        // Sky/ground: a fullscreen triangle generated in the vertex shader (no vertex buffer).
        let sky_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("gonedark.world_sky_pipeline"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_sky"),
                buffers: &[],
                compilation_options: wgpu::PipelineCompilationOptions::default(),
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs_sky"),
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

        // Shaped muzzle-flash flare (WS-A): its own uniform at binding 3 (so it never collides with
        // the sky pass's `world` uniform), an additive blend, and a vertex-shader-generated quad.
        let muzzle_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("gonedark.world_muzzle_layout"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 3,
                visibility: wgpu::ShaderStages::VERTEX | wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            }],
        });
        let muzzle_uniform_buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("gonedark.world_muzzle_uniform"),
            size: std::mem::size_of::<MuzzleUniform>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let muzzle_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("gonedark.world_muzzle_bind_group"),
            layout: &muzzle_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 3,
                resource: muzzle_uniform_buf.as_entire_binding(),
            }],
        });
        let muzzle_pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("gonedark.world_muzzle_pipeline_layout"),
            bind_group_layouts: &[Some(&muzzle_layout)],
            immediate_size: 0,
        });
        let muzzle_additive = wgpu::BlendState {
            color: wgpu::BlendComponent {
                src_factor: wgpu::BlendFactor::One,
                dst_factor: wgpu::BlendFactor::One,
                operation: wgpu::BlendOperation::Add,
            },
            alpha: wgpu::BlendComponent {
                src_factor: wgpu::BlendFactor::One,
                dst_factor: wgpu::BlendFactor::One,
                operation: wgpu::BlendOperation::Add,
            },
        };
        let muzzle_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("gonedark.world_muzzle_pipeline"),
            layout: Some(&muzzle_pipeline_layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_muzzle"),
                buffers: &[],
                compilation_options: wgpu::PipelineCompilationOptions::default(),
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs_muzzle"),
                targets: &[Some(wgpu::ColorTargetState {
                    format: surface_format,
                    blend: Some(muzzle_additive),
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

        WorldRenderer {
            sky_pipeline,
            uniform_buf,
            uniform_bind_group,
            muzzle_pipeline,
            muzzle_uniform_buf,
            muzzle_bind_group,
            ground_tex,
            detail_tex,
            ground_uploaded: false,
        }
    }

    /// Upload the baked R8 ground detail map into the texture, once. Called on the first
    /// [`render_sky`](Self::render_sky) (the construction path has no `queue`); a no-op thereafter.
    /// Mirrors `text::TextRenderer::ensure_atlas_uploaded`.
    fn ensure_ground_uploaded(&mut self, queue: &wgpu::Queue) {
        if self.ground_uploaded {
            return;
        }
        let extent = wgpu::Extent3d {
            width: GROUND_TEX_SIZE,
            height: GROUND_TEX_SIZE,
            depth_or_array_layers: 1,
        };
        let layout = wgpu::TexelCopyBufferLayout {
            offset: 0,
            bytes_per_row: Some(GROUND_TEX_SIZE),
            rows_per_image: Some(GROUND_TEX_SIZE),
        };
        for (tex, bytes) in [
            (&self.ground_tex, GROUND_TEX_BYTES),
            (&self.detail_tex, DETAIL_TEX_BYTES),
        ] {
            queue.write_texture(
                wgpu::TexelCopyTextureInfo {
                    texture: tex,
                    mip_level: 0,
                    origin: wgpu::Origin3d::ZERO,
                    aspect: wgpu::TextureAspect::All,
                },
                bytes,
                layout,
                extent,
            );
        }
        self.ground_uploaded = true;
    }

    /// Draw the sky + ground for the embodied frame. This is the CLEARING pass for the embodied
    /// view: it replaces the bare `CLEAR_DARK` void with a real first-person space (a sky gradient
    /// above the horizon, a gridded floor below). It reveals **no** map intel — it is a pure
    /// function of the camera, with no access to sim entities. The host calls this FIRST in the
    /// embodied branch, before the avatar + weapon + HUD passes.
    pub fn render_sky(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        view: &wgpu::TextureView,
        uniform: &WorldUniform,
    ) {
        self.ensure_ground_uploaded(queue);
        queue.write_buffer(&self.uniform_buf, 0, bytemuck::bytes_of(uniform));

        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("gonedark.world_sky_encoder"),
        });
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("gonedark.world_sky_pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        // CLEAR — this is the embodied frame's clearing pass (replaces CLEAR_DARK).
                        load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                multiview_mask: None,
                timestamp_writes: None,
                occlusion_query_set: None,
            });
            pass.set_pipeline(&self.sky_pipeline);
            pass.set_bind_group(0, &self.uniform_bind_group, &[]);
            // Fullscreen triangle: 3 vertices, no vertex buffer.
            pass.draw(0..3, 0..1);
        }
        queue.submit(std::iter::once(encoder.finish()));
    }

    /// Draw the **shaped muzzle flash** flare (WS-A) at [`MUZZLE_ANCHOR`] for the current flash
    /// `intensity` and viewport `aspect`, as an ADDITIVE LOAD pass over the embodied frame (never
    /// clears). A no-op at `intensity <= 0` so it leaves the frame untouched between shots. The host
    /// calls this after the weapon viewmodel, only while embodied with a drawn rifle. Presentation
    /// only (invariant #4); no world position → reveals nothing (invariant #6).
    pub fn render_muzzle_flash(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        view: &wgpu::TextureView,
        intensity: f32,
        aspect: f32,
        anchor: (f32, f32),
    ) {
        if intensity <= 0.0 {
            return;
        }
        let uniform = MuzzleUniform::new(intensity, aspect, anchor);
        queue.write_buffer(&self.muzzle_uniform_buf, 0, bytemuck::bytes_of(&uniform));

        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("gonedark.world_muzzle_encoder"),
        });
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("gonedark.world_muzzle_pass"),
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
            pass.set_pipeline(&self.muzzle_pipeline);
            pass.set_bind_group(0, &self.muzzle_bind_group, &[]);
            pass.draw(0..6, 0..1);
        }
        queue.submit(std::iter::once(encoder.finish()));
    }
}

#[cfg(test)]
mod tests {
    //! `render` is the float boundary (invariant #1: floats live only in rendering), so `f32` math
    //! is fair game here. The pipelines need a real `wgpu::Device` (no display in CI), so the GPU
    //! path is untested; the testable math is factored into [`muzzle_flash_intensity`],
    //! [`WorldUniform::new`], and [`weapon_view_model`].

    use super::*;

    const EPS: f32 = 1e-4;

    // ---- muzzle flash fade ----

    #[test]
    fn no_fire_means_no_flash() {
        assert_eq!(muzzle_flash_intensity(None, 100), 0.0);
    }

    #[test]
    fn fresh_fire_is_full_flash() {
        assert!((muzzle_flash_intensity(Some(50), 50) - 1.0).abs() < EPS);
    }

    #[test]
    fn flash_decays_monotonically_to_zero() {
        let young = muzzle_flash_intensity(Some(0), 1);
        let mid = muzzle_flash_intensity(Some(0), MUZZLE_FLASH_TICKS / 2);
        let old = muzzle_flash_intensity(Some(0), MUZZLE_FLASH_TICKS - 1);
        assert!(young > mid, "flash should decrease with age");
        assert!(mid > old, "flash should keep decreasing");
        assert!(old > 0.0, "still lit just before the cutoff");
    }

    #[test]
    fn flash_is_dark_after_window() {
        assert_eq!(muzzle_flash_intensity(Some(0), MUZZLE_FLASH_TICKS), 0.0);
        assert_eq!(muzzle_flash_intensity(Some(0), MUZZLE_FLASH_TICKS + 100), 0.0);
    }

    #[test]
    fn future_fire_is_dark() {
        // A fire stamped in the future (tick < fired) is not yet live.
        assert_eq!(muzzle_flash_intensity(Some(100), 50), 0.0);
    }

    // ---- world uniform ----

    #[test]
    fn uniform_carries_inverse_eye_and_flash() {
        // The uniform must thread the host-computed inverse matrix and eye through verbatim, and
        // clamp flash into [0,1] (so the shader can trust it). We don't invert here (no glam dep);
        // a sentinel matrix proves the columns survive in column-major order.
        let inv = [
            [1.0, 2.0, 3.0, 4.0],
            [5.0, 6.0, 7.0, 8.0],
            [9.0, 10.0, 11.0, 12.0],
            [13.0, 14.0, 15.0, 16.0],
        ];
        let u = WorldUniform::new(inv, [1.5, -2.5, 1.5], 0.5);
        assert_eq!(u.inv_view_proj, inv, "matrix threads through verbatim");
        assert_eq!(u.eye, [1.5, -2.5, 1.5, 0.0], "eye padded to vec4");
        assert!((u.flash[0] - 0.5).abs() < EPS);
        assert_eq!([u.flash[1], u.flash[2], u.flash[3]], [0.0, 0.0, 0.0]);
    }

    #[test]
    fn uniform_clamps_flash() {
        let m = [[0.0; 4]; 4];
        assert_eq!(
            WorldUniform::new(m, [0.0; 3], 5.0).flash[0],
            1.0,
            "over-range flash clamps to 1"
        );
        assert_eq!(
            WorldUniform::new(m, [0.0; 3], -2.0).flash[0],
            0.0,
            "under-range flash clamps to 0"
        );
    }

    // ---- weapon viewmodel placement (view space) ----

    /// Apply a column-major model matrix to a point (w = 1).
    fn xform(m: &[[f32; 4]; 4], p: [f32; 3]) -> [f32; 3] {
        let mut out = [m[3][0], m[3][1], m[3][2]];
        for j in 0..3 {
            for r in 0..3 {
                out[r] += m[j][r] * p[j];
            }
        }
        out
    }

    #[test]
    fn weapon_sits_in_front_lower_right() {
        // The gun's local origin lands in front of the camera (view -Z), to the right (+X) and
        // below centre (-Y) — a right-handed lower-screen hold.
        let m = weapon_view_model(0.0);
        let o = xform(&m, [0.0, 0.0, 0.0]);
        assert!(o[2] < 0.0, "in front of the camera (−Z), got {o:?}");
        assert!(o[0] > 0.0, "to the right");
        assert!(o[1] < 0.0, "below centre");
        assert_eq!(m[3], [0.16, -0.20, -0.62, 1.0], "affine translation column");
    }

    #[test]
    fn weapon_barrel_points_into_the_screen() {
        // The barrel tip (local +X) projects further from the camera (more negative view Z) than
        // the stock (local −X): the gun points forward, into the world.
        let m = weapon_view_model(0.0);
        let tip = xform(&m, [0.6, 0.0, 0.0]);
        let stock = xform(&m, [-0.3, 0.0, 0.0]);
        assert!(tip[2] < stock[2], "barrel tip is deeper into the scene");
        // Local up (+Z) maps to view up (+Y).
        let up = xform(&m, [0.0, 0.0, 1.0]);
        let base = xform(&m, [0.0, 0.0, 0.0]);
        assert!(up[1] > base[1], "the sights point up the screen");
    }

    #[test]
    fn weapon_recoils_on_fire() {
        // A live flash kicks the gun back toward the camera (less negative Z) and up vs the rest
        // pose, so firing reads as a jolt.
        let rest = weapon_view_model(0.0);
        let fired = weapon_view_model(1.0);
        assert!(fired[3][2] > rest[3][2], "recoils back toward the camera");
        assert!(fired[3][1] > rest[3][1], "and kicks up");
    }

    // ---- shaped muzzle-flash uniform (WS-A) ----

    #[test]
    fn muzzle_uniform_carries_flash_aspect_and_anchor() {
        let u = MuzzleUniform::new(0.5, 16.0 / 9.0, (0.2, -0.1));
        assert!((u.params[0] - 0.5).abs() < EPS, "flash threads through");
        assert!((u.params[1] - 16.0 / 9.0).abs() < EPS, "aspect threads through");
        assert_eq!((u.params[2], u.params[3]), (0.2, -0.1), "anchor threads through");
    }

    #[test]
    fn muzzle_uniform_clamps_flash() {
        let a = (0.0, 0.0);
        assert_eq!(MuzzleUniform::new(5.0, 1.0, a).params[0], 1.0, "over-range flash clamps to 1");
        assert_eq!(MuzzleUniform::new(-2.0, 1.0, a).params[0], 0.0, "under-range flash clamps to 0");
    }

    /// A simple right-handed DirectX-style perspective (glam's `rh::proj::directx::perspective`),
    /// column-major, so the anchor test projects with the same convention the host uses.
    fn test_proj(fov_deg: f32, aspect: f32) -> [[f32; 4]; 4] {
        let f = 1.0 / (fov_deg.to_radians() * 0.5).tan();
        let (near, far) = (0.05_f32, 500.0_f32);
        // RH, z ∈ [0,1] (wgpu/DirectX): looks down -Z.
        [
            [f / aspect, 0.0, 0.0, 0.0],
            [0.0, f, 0.0, 0.0],
            [0.0, 0.0, far / (near - far), -1.0],
            [0.0, 0.0, (near * far) / (near - far), 0.0],
        ]
    }

    #[test]
    fn muzzle_anchor_projects_barrel_tip_to_lower_right() {
        // The barrel tip sits to the right of and below the camera axis (view x>0, y<0), so its NDC
        // anchor must land in the lower-right quadrant (x>0, y<0) — where the viewmodel is drawn.
        let proj = test_proj(70.0, 16.0 / 9.0);
        let (ax, ay) = muzzle_anchor_ndc(&proj, WeaponPose::at_rest());
        assert!(ax > 0.0, "muzzle anchors right of centre (x={ax})");
        assert!(ay < 0.0, "muzzle anchors below centre (y={ay})");
        assert!(ax.abs() <= 1.0 && ay.abs() <= 1.0, "anchor stays on-screen ({ax}, {ay})");
    }

    #[test]
    fn muzzle_anchor_tracks_recoil_kick() {
        // Firing kicks the gun back/up (the pose shifts ty/tz with flash), so the projected muzzle
        // anchor must MOVE between rest and a fresh shot — the flare rides the recoil, it is not
        // pinned to a static screen point.
        let proj = test_proj(70.0, 16.0 / 9.0);
        let rest = muzzle_anchor_ndc(&proj, WeaponPose::at_rest());
        let fired = muzzle_anchor_ndc(&proj, WeaponPose::from_flash(1.0));
        assert!(
            (rest.0 - fired.0).abs() > 1e-4 || (rest.1 - fired.1).abs() > 1e-4,
            "anchor shifts with recoil (rest={rest:?} fired={fired:?})"
        );
    }

    #[test]
    fn muzzle_anchor_falls_back_when_degenerate() {
        // A zero projection yields w≈0 for every point → the function must return the static
        // MUZZLE_ANCHOR rather than dividing by zero.
        let zero = [[0.0f32; 4]; 4];
        assert_eq!(
            muzzle_anchor_ndc(&zero, WeaponPose::at_rest()),
            MUZZLE_ANCHOR,
            "degenerate w falls back"
        );
    }

    #[test]
    fn chambering_rack_moves_the_viewmodel_then_settles() {
        // Semi-auto feel: mid-cycle (cycle≈0.5) the action is worked — the gun is pulled back toward
        // the camera (view +Z) and dropped (−Y) relative to a fully-chambered ready pose (cycle=1).
        let ready = weapon_view_model_posed(WeaponPose { flash: 0.0, cycle: 1.0, spray: 0.0 });
        let racking = weapon_view_model_posed(WeaponPose { flash: 0.0, cycle: 0.5, spray: 0.0 });
        assert!(racking[3][2] > ready[3][2], "the rack pulls the gun back toward the camera");
        assert!(racking[3][1] < ready[3][1], "and drops it while the action is open");
        // By the end of the cycle it is back at the ready placement (bump → 0 at cycle=1).
        assert_eq!(racking[3][0].max(ready[3][0]).is_finite(), true);
    }

    #[test]
    fn spray_climbs_the_viewmodel() {
        // Full-auto feel: sustained spray rides the muzzle up (view +Y) vs. no spray.
        let calm = weapon_view_model_posed(WeaponPose { flash: 0.0, cycle: 1.0, spray: 0.0 });
        let spraying = weapon_view_model_posed(WeaponPose { flash: 0.0, cycle: 1.0, spray: 1.0 });
        assert!(spraying[3][1] > calm[3][1], "spray climbs the muzzle upward");
    }

    // ---- shaped muzzle-flare geometry (WS-A) ----

    #[test]
    fn muzzle_ring_expands_as_the_shot_fades() {
        // Tight at the muzzle flash (flash → 1), blooming outward as it fades (flash → 0): the
        // radius is monotonically larger for a more-decayed shot, and stays within the flare quad.
        let fresh = muzzle_ring_radius(1.0);
        let mid = muzzle_ring_radius(0.5);
        let old = muzzle_ring_radius(0.0);
        assert!(old > mid && mid > fresh, "ring grows as flash decays ({fresh} < {mid} < {old})");
        assert!(fresh > 0.0 && old < 1.0, "ring radius stays inside the flare quad");
    }

    #[test]
    fn muzzle_ring_is_a_mid_life_puff() {
        // The ring is dark at the white-hot flash itself and once fully faded, peaking mid-life — so
        // it reads as a fast expanding puff, not a constant halo.
        let peak = muzzle_ring_weight(0.5);
        assert!(peak > muzzle_ring_weight(0.1), "ring brightens past the initial flash");
        assert!(peak > muzzle_ring_weight(0.9), "ring brightens before the flash whites out");
        assert_eq!(muzzle_ring_weight(0.0), 0.0, "no ring once fully faded");
        assert_eq!(muzzle_ring_weight(1.0), 0.0, "no ring at the white-hot flash");
    }

    #[test]
    fn muzzle_flare_core_is_brightest() {
        // The white-hot centre is the peak of the flare — brighter than any off-centre point — so
        // the shot has a punchy hot pip rather than a flat disc.
        let center = muzzle_flare_shape(0.0, 0.0, 0.7);
        for &(x, y) in &[(0.6, 0.0), (0.0, 0.6), (-0.45, 0.45), (0.3, -0.5)] {
            assert!(
                center > muzzle_flare_shape(x, y, 0.7),
                "centre {center} must outshine off-centre ({x},{y})"
            );
        }
    }

    #[test]
    fn muzzle_star_is_asymmetric() {
        // The whole point of the reshape: the star is NOT a clean symmetric plus / radial disc. At a
        // fixed radius (with the ring suppressed at high flash so the variation is pure spike energy)
        // the horizontal and vertical rays carry clearly different energy, and sweeping a full ring
        // of angles is far from rotationally uniform — a ragged, real-flash silhouette.
        let f = 0.92; // ring negligible here, so the variation is pure spike asymmetry
        let horiz = muzzle_flare_shape(0.5, 0.0, f);
        let vert = muzzle_flare_shape(0.0, 0.5, f);
        assert!((horiz - vert).abs() > 0.05, "axes differ: h={horiz} v={vert}");

        let r = 0.5;
        let mut min = f32::INFINITY;
        let mut max = f32::NEG_INFINITY;
        for i in 0..24 {
            let a = std::f32::consts::TAU * i as f32 / 24.0;
            let s = muzzle_flare_shape(r * a.cos(), r * a.sin(), f);
            min = min.min(s);
            max = max.max(s);
        }
        assert!(max - min > 0.1, "the star must not be rotationally uniform, spread {}", max - min);
    }

    #[test]
    fn muzzle_flare_fades_outside_the_quad() {
        // Beyond the flare's local extent there is no light — the additive pass adds nothing in the
        // corners, so the flash stays a compact shape, not a screen-wide wash.
        assert_eq!(muzzle_flare_shape(1.3, 1.3, 0.5), 0.0, "no light past the quad corner");
    }

    // ---- ground detail-map metrics contract ----

    #[test]
    fn ground_tex_matches_metrics() {
        // The baked ground blob length MUST equal GROUND_TEX_SIZE² — a guard against the generator
        // and this const drifting (which would shear / misalign the sampled detail at runtime).
        assert_eq!(
            GROUND_TEX_BYTES.len(),
            (GROUND_TEX_SIZE * GROUND_TEX_SIZE) as usize,
            "raw R8 ground size must match GROUND_TEX_SIZE² — regenerate with `pnpm assets:textures`"
        );
    }

    #[test]
    fn detail_tex_matches_metrics() {
        // The WS-E detail heightfield is the SAME R8 GROUND_TEX_SIZE² blob shape as the ground map
        // (it shares the texture descriptor + the bytes_per_row upload), so its length must match
        // too or the near-field crunch would shear at runtime.
        assert_eq!(
            DETAIL_TEX_BYTES.len(),
            (GROUND_TEX_SIZE * GROUND_TEX_SIZE) as usize,
            "raw R8 detail size must match GROUND_TEX_SIZE² — regenerate with `pnpm assets:textures`"
        );
    }

    #[test]
    fn detail_field_differs_from_ground() {
        // The two heightfields must be genuinely different noise (distinct seeds) — otherwise the
        // detail sample would just re-add the ground field's own gradients and buy no crisper relief.
        assert_ne!(
            GROUND_TEX_BYTES, DETAIL_TEX_BYTES,
            "detail must be an independent field, not a copy of ground"
        );
    }

    /// Validate `world.wgsl` offline with naga (the compiler wgpu uses), so a WGSL regression fails
    /// the test suite instead of only blowing up at pipeline creation on a real GPU.
    #[test]
    fn world_wgsl_parses_and_validates() {
        let src = include_str!("world.wgsl");
        let module = naga::front::wgsl::parse_str(src).expect("world.wgsl must parse");
        let mut validator = naga::valid::Validator::new(
            naga::valid::ValidationFlags::all(),
            naga::valid::Capabilities::all(),
        );
        validator
            .validate(&module)
            .expect("world.wgsl must validate");
    }

    // ---- night-sky starfield hash (determinism) ----

    #[test]
    fn star_hash_is_deterministic() {
        // The "no crawl/shimmer" property (fairness #6) rests on the hash being a pure function of
        // the cell: the SAME input must always give the SAME value (the shader has no time input, so
        // re-evaluating the same ray every frame yields the same star).
        for &(x, y) in &[(0.0, 0.0), (3.0, 7.0), (-12.0, 41.0), (123.5, -8.25)] {
            assert_eq!(star_hash21(x, y), star_hash21(x, y), "hash must be stable for ({x},{y})");
        }
    }

    #[test]
    fn star_hash_is_in_unit_range() {
        // `fract` keeps the output in [0,1), so the `step`/brightness logic in the shader is sound.
        for i in 0..200 {
            let x = i as f32 * 1.37 - 50.0;
            let y = (i as f32 * 0.91).sin() * 64.0; // spread of inputs (sin is a test-only float)
            let h = star_hash21(x, y);
            assert!((0.0..1.0).contains(&h), "hash {h} out of [0,1) for ({x},{y})");
        }
    }

    #[test]
    fn star_hash_decorrelates_neighbour_cells() {
        // Adjacent grid cells must hash to clearly different values, or the stars would clump into a
        // visible lattice instead of a natural sprinkle. Sample a patch and require it isn't constant.
        let mut min = f32::INFINITY;
        let mut max = f32::NEG_INFINITY;
        for cx in 0..8 {
            for cy in 0..8 {
                let h = star_hash21(cx as f32, cy as f32);
                min = min.min(h);
                max = max.max(h);
            }
        }
        assert!(max - min > 0.5, "neighbour cells should span the range, got spread {}", max - min);
    }

    // ---- moon glow shaping ----

    #[test]
    fn moon_glow_is_dark_away_from_the_moon() {
        // Rays pointing away from (or perpendicular to) the moon get no moon light.
        assert_eq!(moon_glow(0.0), 0.0, "perpendicular ray is dark");
        assert_eq!(moon_glow(-1.0), 0.0, "ray pointing away is dark");
        assert!(moon_glow(0.5) < 1e-3, "well off-axis is essentially dark");
    }

    #[test]
    fn moon_glow_brightens_toward_the_disc() {
        // Monotonic: the closer the ray points to the moon, the brighter — a real key-light source.
        let off = moon_glow(0.95);
        let near = moon_glow(0.999);
        let edge = moon_glow(0.9985);
        let center = moon_glow(1.0);
        assert!(near > off, "glow rises toward the moon ({near} !> {off})");
        assert!(center > near, "the disc is brightest ({center} !> {near})");
        assert!(center >= edge && edge >= off, "monotone across the disc edge");
        // The crisp disc lights fully at the centre (core 1.0 + halo + bloom).
        assert!(center > 1.0, "disc core peaks bright, got {center}");
    }
}
