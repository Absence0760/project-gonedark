// Embodied first-person world shader (W5).
//
// TWO passes share this module:
//   * vs_sky/fs_sky — a fullscreen sky + ground. The vertex shader emits one big triangle; the
//     fragment shader reconstructs each pixel's world-space view ray from the inverse
//     view-projection, intersects it with the ground plane (z = 0), and shades a gridded floor
//     below the horizon / a sky gradient above it. It draws ONLY the environment (a function of
//     the camera) — no sim entities, so no map intel can leak (invariant #6).
//   * vs_weapon/fs_weapon — a screen-space first-person weapon viewmodel (NDC quads) with a
//     muzzle-flash flare driven by the shared `World.flash` uniform.
//
// Float side of invariant #4 — every value here is already f32; the Q16.16 → f32 hop happened on
// the CPU, never in `core`.

struct World {
    inv_view_proj: mat4x4<f32>,
    eye: vec4<f32>,   // xyz = camera world position; w unused
    flash: vec4<f32>, // x = muzzle-flash intensity [0,1]; yzw unused
};

@group(0) @binding(0)
var<uniform> world: World;

// Ground detail map — a seamlessly-tiling R8 grayscale noise baked by tools/textures/gen_textures.py
// (ImageMagick) and uploaded by `WorldRenderer`. Sampled (tiled by world XY) to break up the flat
// floor; render-only, carries no intel (invariant #6). The call site uses `textureSampleLevel`
// (explicit LOD 0) so the sample is legal inside the ground branch's non-uniform control flow.
@group(0) @binding(1)
var ground_tex: texture_2d<f32>;
@group(0) @binding(2)
var ground_samp: sampler;

// ---- sky / ground -----------------------------------------------------------------------------

struct SkyOut {
    @builtin(position) clip_pos: vec4<f32>,
    @location(0) ndc: vec2<f32>, // this pixel's NDC xy (interpolated)
};

// A fullscreen triangle (covers the whole NDC box with no vertex buffer).
@vertex
fn vs_sky(@builtin(vertex_index) vid: u32) -> SkyOut {
    var corners = array<vec2<f32>, 3>(
        vec2<f32>(-1.0, -1.0),
        vec2<f32>( 3.0, -1.0),
        vec2<f32>(-1.0,  3.0),
    );
    let p = corners[vid];
    var out: SkyOut;
    out.clip_pos = vec4<f32>(p, 0.0, 1.0);
    out.ndc = p;
    return out;
}

// Unproject an NDC point at a given clip depth back to a world-space point.
fn unproject(ndc: vec2<f32>, depth: f32) -> vec3<f32> {
    let clip = vec4<f32>(ndc.x, ndc.y, depth, 1.0);
    let w = world.inv_view_proj * clip;
    return w.xyz / w.w;
}

// Deterministic value hashes (Dave Hoskins) used for the procedural night-sky starfield. They are a
// pure function of a grid CELL coordinate (itself a function of the view ray), so the stars are
// stable frame to frame — no time input means no crawl/shimmer (fairness #6). `hash21` is mirrored
// in world.rs (`star_hash21`) and unit-tested off-GPU; keep the two in lockstep.
fn hash21(p: vec2<f32>) -> f32 {
    var p3 = fract(vec3<f32>(p.x, p.y, p.x) * 0.1031);
    p3 += dot(p3, vec3<f32>(p3.y, p3.z, p3.x) + 33.33);
    return fract((p3.x + p3.y) * p3.z);
}
fn hash22(p: vec2<f32>) -> vec2<f32> {
    var p3 = fract(vec3<f32>(p.x, p.y, p.x) * vec3<f32>(0.1031, 0.1030, 0.0973));
    p3 += dot(p3, vec3<f32>(p3.y, p3.z, p3.x) + 33.33);
    return fract((vec2<f32>(p3.x, p3.x) + vec2<f32>(p3.y, p3.z)) * vec2<f32>(p3.z, p3.y));
}

@fragment
fn fs_sky(in: SkyOut) -> @location(0) vec4<f32> {
    // Reconstruct the world-space ray for this pixel: a near point and a far point through the
    // inverse view-projection give a direction from the eye.
    let near = unproject(in.ndc, 0.0);
    let far = unproject(in.ndc, 1.0);
    let dir = normalize(far - near);

    let eye = world.eye.xyz;

    // Ground plane is z = 0. A ray pointing downward (dir.z < 0) from an eye above the plane
    // hits the floor; otherwise it escapes to the sky.
    if (dir.z < -0.0005 && eye.z > 0.0) {
        let t = -eye.z / dir.z; // distance to the z=0 plane
        let hit = eye + dir * t;
        let p = hit.xy;

        // Distance fog: the floor fades into the horizon haze with distance so there is a real
        // sense of depth/motion rather than an infinite hard plane.
        let dist = length(p - eye.xy);
        let fog = clamp(dist / 80.0, 0.0, 1.0);

        // The baked ground map is a seamless HEIGHTFIELD (tools/textures/gen_textures.py: macro
        // swell + meso undulation + fine grit). Sample it at THREE world-space scales for albedo
        // tonal variation. `textureSampleLevel` (LOD 0) keeps the sample legal in this branch's
        // non-uniform control flow.
        let h_macro = textureSampleLevel(ground_tex, ground_samp, p / 38.0, 0.0).r;
        let h_meso = textureSampleLevel(ground_tex, ground_samp, p / 7.5, 0.0).r;
        let h_fine = textureSampleLevel(ground_tex, ground_samp, p / 2.1, 0.0).r;

        // Per-pixel surface NORMAL by finite-differencing the MESO height at small world-XY
        // offsets: the gradient of the heightfield becomes a tilt, so a dim key light gives the
        // floor real relief. Relief fades out with distance (the finite differences would alias
        // into shimmer far away), flattening the floor toward the horizon haze.
        let relief = 1.0 - clamp(dist / 42.0, 0.0, 1.0);
        let eps = 0.16;
        let scl = 7.5;
        let hL = textureSampleLevel(ground_tex, ground_samp, (p - vec2<f32>(eps, 0.0)) / scl, 0.0).r;
        let hR = textureSampleLevel(ground_tex, ground_samp, (p + vec2<f32>(eps, 0.0)) / scl, 0.0).r;
        let hD = textureSampleLevel(ground_tex, ground_samp, (p - vec2<f32>(0.0, eps)) / scl, 0.0).r;
        let hU = textureSampleLevel(ground_tex, ground_samp, (p + vec2<f32>(0.0, eps)) / scl, 0.0).r;
        let amp = 2.3 * relief; // height amplitude → relief strength
        let n = normalize(vec3<f32>(-(hR - hL) * amp, -(hU - hD) * amp, 1.0));

        // Lighting: a dim directional KEY (mostly from above so the floor stays lit) plus a high
        // AMBIENT term. Ambient is kept high on purpose — the embodied floor must ALWAYS read and
        // never crush to black, so nothing can hide in shadow (fairness, invariant #6).
        let key_dir = normalize(vec3<f32>(0.32, 0.22, 0.92));
        let lambert = max(dot(n, key_dir), 0.0);
        let ambient = 0.66;
        let key = 0.46;
        let shade = ambient + key * lambert;

        // Earthy albedo: blend damp dark mud ↔ drier lighter dirt by the MACRO height, so the
        // large swells read as wetter lows and drier rises. Both tones are LOW-saturation and DARK
        // (channels track within a narrow warm range, like the old slate) so no channel reads as
        // faction intel and the HUD/avatar still pop (invariant #6). Meso + fine add a subtle
        // tonal grain centred on 1.0 (fine fades with distance to avoid shimmer).
        let mud = vec3<f32>(0.090, 0.082, 0.072);  // damp dark earth (the lows)
        let dirt = vec3<f32>(0.155, 0.143, 0.122); // drier lighter dirt (the rises)
        let macro_t = smoothstep(0.32, 0.74, h_macro);
        var albedo = mix(mud, dirt, macro_t);
        let tone = 1.0 + (h_meso - 0.5) * 0.20 + (h_fine - 0.5) * 0.14 * relief;
        albedo = albedo * clamp(tone, 0.78, 1.22);

        // Broad terrain ZONES: an ultra-low-frequency field tints the floor between two earth moods
        // (a drier warm rise vs a cooler damp flat) for a sense of *place* rather than one uniform
        // dirt. Warm/neutral only — never blue — and tiny deltas, so it stays low-saturation and
        // reads as terrain, not faction intel (invariant #6). Pure function of world XY.
        let zone_f = sin(p.x * 0.021 - 0.5) + cos(p.y * 0.017 + 0.8);
        let zone = smoothstep(-1.4, 1.4, zone_f);
        albedo = albedo * mix(vec3<f32>(0.97, 0.98, 1.0), vec3<f32>(1.05, 1.01, 0.95), zone);

        var ground = albedo * shade;

        // Damp SHEEN: a soft warm specular glint off the wet lows (low macro height) from the moon
        // KEY, so the earth reads as a real material — waterlogged hollows catch the light while the
        // dry rises stay matte. Warm-white (never blue) and faded by distance/relief so it can only
        // ever whisper and can't read as intel (invariant #6). Blinn half-vector between the key and
        // the view ray (surface→eye = −dir).
        let half_v = normalize(key_dir - dir);
        let spec = pow(max(dot(n, half_v), 0.0), 24.0);
        let wet = (1.0 - macro_t) * relief;
        ground += vec3<f32>(0.10, 0.10, 0.095) * spec * wet * 0.5;

        // DEMOTED grid: a faint lattice on a ~2-unit cell, kept only as a heading/parallax cue. It
        // is a low, cool ADDITIVE lift (no longer a bright line colour) and fades out with distance,
        // so it reads as a subtle lattice over the terrain rather than the dominant feature.
        let cell = 2.0;
        let g = abs(fract(p / cell - 0.5) - 0.5);
        let line_w = fwidth(p / cell);
        let grid2 = min(g / max(line_w, vec2<f32>(1e-4)), vec2<f32>(1.0));
        let line = 1.0 - min(grid2.x, grid2.y);
        let grid_fade = 1.0 - clamp(dist / 24.0, 0.0, 1.0);
        ground += vec3<f32>(0.045, 0.055, 0.070) * line * grid_fade;

        // Horizon haze the floor dissolves into (matches the bottom of the sky gradient).
        let haze = vec3<f32>(0.16, 0.18, 0.22);
        return vec4<f32>(mix(ground, haze, fog), 1.0);
    }

    // SKY (above the horizon): a real night-battlefield atmosphere with depth — a multi-stop vertical
    // gradient, layered distance-haze banding hugging the skyline, a cold low moon that reads as the
    // dim KEY LIGHT the ground is lit by, a restrained warm horizon glow, and a faint deterministic
    // star sprinkle. Kept MUTED and low-saturation (cold blues/greys, channels track closely) so the
    // alert-HUD markers and the amber avatar still pop and no channel reads as faction intel
    // (invariant #6). Mood matches the title backdrop: dark, cold, with a thread of amber at the rim.
    // Palette aligned to gonedark_render::theme INK/PANEL (WGSL can't import the consts; see theme.rs).
    let elev = clamp(dir.z, 0.0, 1.0);

    // Base vertical gradient. The HORIZON stop is exactly the colour the ground branch dissolves into
    // (its `haze` = (0.16,0.18,0.22)), so the sky/ground seam is seamless.
    let zenith = vec3<f32>(0.020, 0.034, 0.078);  // deep night-blue overhead (toward theme::INK)
    let mid_sky = vec3<f32>(0.066, 0.086, 0.130); // body of the dome
    let horizon = vec3<f32>(0.160, 0.180, 0.220); // == ground haze, for a clean seam
    let lower = mix(horizon, mid_sky, smoothstep(0.0, 0.20, elev));
    var sky = mix(lower, zenith, smoothstep(0.16, 0.85, elev));

    // Layered atmospheric haze banding: two soft cool bands stacked just above the skyline give the
    // dome distance/depth (dust-and-distance) without lifting the overall brightness much.
    let band1 = (1.0 - smoothstep(0.0, 0.10, elev)) * 0.055;
    let band2 = (1.0 - smoothstep(0.04, 0.32, elev)) * 0.030;
    sky += vec3<f32>(0.12, 0.14, 0.17) * (band1 + band2);

    // Cold low moon — the dim key light the ground is lit by (same +x/+y quadrant as the ground
    // branch's key_dir, so the world reads as lit BY this). A crisp cold disc, a tight inner halo,
    // and a wide bloom that bleeds into the surrounding haze, giving the dome a light source and a
    // sense of place. Cold blue-white, never warm (would otherwise compete with the alert palette).
    let moon_dir = normalize(vec3<f32>(0.62, 0.18, 0.30));
    let md = max(dot(dir, moon_dir), 0.0);
    let moon_core = smoothstep(0.9980, 0.9994, md); // crisp disc (~2–3.6° radius)
    let moon_halo = pow(md, 300.0) * 0.60;          // tight inner glow
    let moon_bloom = pow(md, 14.0) * 0.12;          // wide atmospheric bleed (the key-light wash)
    let moon_col = vec3<f32>(0.62, 0.68, 0.78);     // cold blue-white
    sky += moon_col * (moon_core + moon_halo + moon_bloom);

    // A restrained WARM horizon glow for the title-backdrop amber mood — a thin thread of distant
    // light at the skyline, kept dim so it never reads as an alert marker.
    let warm = (1.0 - smoothstep(0.0, 0.18, elev)) * 0.045;
    sky += vec3<f32>(0.9, 0.66, 0.42) * warm;

    // Faint deterministic stars: a pure function of the view ray (no time → no crawl/shimmer). A
    // view-stable planar projection of the upper hemisphere is gridded; one hash-jittered star per
    // cell, sparsened so only a sprinkle lights. Stars fade IN with elevation (none in the hazy
    // horizon band) and are washed out under the moon bloom. Cold and faint — far below the
    // HUD/hitmarker brightness so they never read as intel or trip the centred hitmarker.
    let star_up = smoothstep(0.14, 0.46, elev);
    if (star_up > 0.0) {
        let proj = dir.xy / (dir.z + 0.55);      // view-stable dome projection (dir.z > 0 here)
        let scale = 26.0;
        let cell = floor(proj * scale);
        let f = fract(proj * scale);
        let star_pos = hash22(cell);              // jittered sub-cell position
        let d = length(f - star_pos);
        let bright = hash21(cell + vec2<f32>(17.0, 3.0));
        let lit = step(0.82, bright);             // ~18% of cells host a star
        let point = (1.0 - smoothstep(0.0, 0.055, d)) * lit * (bright - 0.5);
        let star = point * star_up * (1.0 - moon_bloom * 5.0);
        sky += vec3<f32>(0.55, 0.62, 0.72) * clamp(star, 0.0, 0.45);
    }

    return vec4<f32>(sky, 1.0);
}

// ---- weapon viewmodel -------------------------------------------------------------------------

struct WeaponOut {
    @builtin(position) clip_pos: vec4<f32>,
    @location(0) shade: f32,
    @location(1) @interpolate(flat) kind: f32,
};

@vertex
fn vs_weapon(
    @location(0) ndc: vec2<f32>,
    @location(1) shade: f32,
    @location(2) kind: f32,
) -> WeaponOut {
    var out: WeaponOut;
    out.clip_pos = vec4<f32>(ndc, 0.0, 1.0); // already in NDC
    out.shade = shade;
    out.kind = kind;
    return out;
}

@fragment
fn fs_weapon(in: WeaponOut) -> @location(0) vec4<f32> {
    let flash = world.flash.x;
    if (in.kind > 0.5) {
        // Muzzle quad: a hot flare when firing (flash → 1), invisible when idle (flash → 0). Warm
        // white-yellow, alpha tracks the flash so it composites only while lit.
        let flare = vec3<f32>(1.0, 0.92, 0.6);
        return vec4<f32>(flare, flash);
    }
    // Gun body: flat metal at the vertex shade, fully opaque, with a faint flash-lit rim so the
    // weapon kicks when fired.
    let metal = vec3<f32>(in.shade, in.shade, in.shade * 1.05);
    let lit = metal + vec3<f32>(0.25, 0.22, 0.12) * flash;
    return vec4<f32>(lit, 1.0);
}

// ---- shaped muzzle flash (WS-A) ---------------------------------------------------------------
//
// A dedicated ADDITIVE screen-space flare at the gun muzzle, drawn over the world+weapon while the
// player fires. Promotes the flat alpha muzzle quad above to a SHAPED gunshot flare — a tight
// white-hot core under a soft warm bloom, an ASYMMETRIC multi-spike star (three offset harmonics,
// not a clean symmetric plus), and a quick expanding shock RING that puffs outward mid-life — all
// flaring with `Muzzle.params.x` (the host-clock flash intensity) and gone between shots. Its own
// uniform at a distinct binding so it never disturbs the sky pass's `world` uniform. The shape math
// is mirrored in world.rs (`muzzle_flare_shape`/`muzzle_ring_radius`/`muzzle_ring_weight`) and
// unit-tested off-GPU; keep the two in lockstep. Presentation only (invariant #4); no world
// position → reveals nothing (#6).

struct Muzzle {
    // x = flash intensity [0,1]; y = viewport aspect (w/h); z = anchor NDC x; w = anchor NDC y.
    params: vec4<f32>,
};

@group(0) @binding(3)
var<uniform> muzzle: Muzzle;

struct MuzzleOut {
    @builtin(position) clip_pos: vec4<f32>,
    @location(0) local: vec2<f32>, // quad corner in [-1,1]
};

// A 6-vertex quad generated without a vertex buffer, anchored at the muzzle NDC and scaled by the
// flash (so it grows out of nothing as the shot fires). Aspect-corrected in x so the flare is round.
@vertex
fn vs_muzzle(@builtin(vertex_index) vid: u32) -> MuzzleOut {
    var corners = array<vec2<f32>, 6>(
        vec2<f32>(-1.0, -1.0), vec2<f32>(1.0, -1.0), vec2<f32>(1.0, 1.0),
        vec2<f32>(-1.0, -1.0), vec2<f32>(1.0, 1.0), vec2<f32>(-1.0, 1.0),
    );
    let c = corners[vid];
    let flash = muzzle.params.x;
    let aspect = max(muzzle.params.y, 1e-4);
    let anchor = vec2<f32>(muzzle.params.z, muzzle.params.w);
    // Base half-size in NDC-y, kept nearly constant (a small pop on the shot) so the expanding
    // shock ring genuinely grows in screen space as the flash fades rather than shrinking with it.
    let size_y = 0.155 * (0.85 + 0.20 * flash);
    let half = vec2<f32>(size_y / aspect, size_y);
    var out: MuzzleOut;
    out.clip_pos = vec4<f32>(anchor.x + c.x * half.x, anchor.y + c.y * half.y, 0.0, 1.0);
    out.local = c;
    return out;
}

@fragment
fn fs_muzzle(in: MuzzleOut) -> @location(0) vec4<f32> {
    let flash = muzzle.params.x;
    let p = in.local;
    let r = length(p);
    let ang = atan2(p.y, p.x);

    // Core: a soft warm BLOOM under a tight white-HOT centre, so the flash has a punchy white pip
    // that bleeds into a warmer halo rather than a single flat disc.
    let bloom = pow(clamp(1.0 - r, 0.0, 1.0), 1.7);
    let hot = pow(clamp(1.0 - r * 2.3, 0.0, 1.0), 2.0);
    let core = bloom * 0.55 + hot;

    // ASYMMETRIC multi-spike star: three cosine harmonics at unequal frequencies, phase offsets,
    // and weights, so the rays read as a ragged real flash — not a clean symmetric plus. They reach
    // further than the core and shimmer with a flash-driven flicker (heat-haze; flash is the time
    // proxy as the shot decays).
    let reach = pow(clamp(1.0 - r * 0.85, 0.0, 1.0), 1.4);
    let s1 = pow(max(cos(ang * 2.0 - 0.35), 0.0), 7.0);
    let s2 = pow(max(cos(ang * 3.0 + 1.20), 0.0), 11.0);
    let s3 = pow(max(cos(ang * 5.0 + 0.60), 0.0), 16.0);
    let flicker = 0.82 + 0.18 * cos(ang * 9.0 + flash * 22.0);
    let spikes = (s1 + s2 * 0.55 + s3 * 0.40) * reach * flicker;

    // Quick expanding SHOCK RING — a mid-life puff: tight at the muzzle flash, blooming outward as
    // the shot fades. Radius grows as flash → 0; visibility peaks around flash = 0.5 (dark both at
    // the white-hot flash itself and once fully faded), so it reads as a fast smoke/heat ring.
    let ring_r = (1.0 - flash) * 0.85 + 0.12;
    let ring_band = exp(-pow((r - ring_r) / 0.11, 2.0));
    let ring = ring_band * clamp(flash * (1.0 - flash) * 4.0, 0.0, 1.0);

    let shape = clamp(core + spikes * 0.85 + ring * 0.45, 0.0, 1.4);
    let a = shape * flash;
    if (a <= 0.001) {
        discard;
    }
    // Warm white-yellow that whitens at the white-hot core and along the fresh ring. Premultiplied
    // for the additive blend (a flash only adds light). Stays in the warm muzzle family.
    let warm = vec3<f32>(1.0, 0.88, 0.58);
    let white = vec3<f32>(1.0, 0.97, 0.86);
    let col = mix(warm, white, clamp(hot + ring * 0.30, 0.0, 1.0));
    return vec4<f32>(col * a, a);
}
