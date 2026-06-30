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

        // Distance fog: the floor fades into the horizon haze with distance so there is a real
        // sense of depth/motion rather than an infinite hard plane.
        let dist = length(hit.xy - eye.xy);
        let fog = clamp(dist / 80.0, 0.0, 1.0);

        // A grid on the floor: bright lines on a ~2-unit lattice give parallax/heading cues as the
        // avatar moves. `fwidth` keeps the lines a constant screen width (anti-aliased).
        let cell = 2.0;
        let g = abs(fract(hit.xy / cell - 0.5) - 0.5);
        let line_w = fwidth(hit.xy / cell);
        let grid2 = min(g / max(line_w, vec2<f32>(1e-4)), vec2<f32>(1.0));
        let line = 1.0 - min(grid2.x, grid2.y);

        // Ground detail: sample the seamless noise map at TWO world-space scales — a coarse octave
        // for slow macro tonal variation and a fine octave for near-field grain — and combine them
        // into a brightness modulation centred on 1.0. `textureSampleLevel` (LOD 0) keeps the sample
        // legal in this branch's non-uniform control flow. The fine grain fades out with distance
        // (it would alias into shimmer far away), leaving only the gentle macro variation near the
        // horizon. Subtle by design: the floor reads as grounded terrain, never busy.
        let coarse = textureSampleLevel(ground_tex, ground_samp, hit.xy / 26.0, 0.0).r;
        let fine = textureSampleLevel(ground_tex, ground_samp, hit.xy / 5.5, 0.0).r;
        let fine_fade = 1.0 - clamp(dist / 30.0, 0.0, 1.0);
        let detail = (coarse - 0.5) * 0.26 + (fine - 0.5) * 0.18 * fine_fade;
        let tint = clamp(1.0 + detail, 0.7, 1.3);

        let floor_base = vec3<f32>(0.10, 0.12, 0.14) * tint; // dark earthy slate, detail-modulated
        let floor_line = vec3<f32>(0.28, 0.34, 0.40);        // lighter grid line (kept crisp)
        let ground = mix(floor_base, floor_line, line);

        // Horizon haze the floor dissolves into (matches the bottom of the sky gradient).
        let haze = vec3<f32>(0.16, 0.18, 0.22);
        return vec4<f32>(mix(ground, haze, fog), 1.0);
    }

    // Sky: a richer multi-stop vertical gradient (zenith → mid → horizon) with a subtle warm haze
    // glow hugging the skyline, so the dome reads as atmospheric depth rather than a flat two-colour
    // ramp. Driven by the ray's elevation (dir.z), clamped so the band reads even when looking
    // level/slightly down. Kept a MUTED low-saturation blue-grey (channels track within ~0.06) so the
    // alert-HUD markers and avatar still pop and no channel is misread as faction intel (invariant #6).
    // Palette aligned to gonedark_render::theme INK/PANEL (WGSL can't import the consts; see theme.rs).
    let elev = clamp(dir.z, 0.0, 1.0);
    let zenith = vec3<f32>(0.025, 0.040, 0.090); // deep night-blue overhead (toward theme::INK)
    let mid_sky = vec3<f32>(0.075, 0.095, 0.140); // body of the dome
    let horizon = vec3<f32>(0.150, 0.175, 0.215); // pale haze at the skyline
    // Two smoothstep stops: horizon→mid over the low band, mid→zenith over the upper band.
    let lower = mix(horizon, mid_sky, smoothstep(0.0, 0.22, elev));
    var sky = mix(lower, zenith, smoothstep(0.18, 0.85, elev));
    // A soft warm haze glow concentrated just above the horizon for a sense of distant light.
    let glow = (1.0 - smoothstep(0.0, 0.20, elev)) * 0.06;
    sky += vec3<f32>(0.9, 0.7, 0.5) * glow;
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
// player fires. Promotes the flat alpha muzzle quad above to a SHAPED flare — a hot round core plus
// a soft four-point star — that flares with `Muzzle.params.x` (the host-clock flash intensity) and
// is gone between shots. Its own uniform at a distinct binding so it never disturbs the sky pass's
// `world` uniform. Presentation only (invariant #4); no world position → reveals nothing (#6).

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
    // Base half-size in NDC-y, grown slightly by the flash so the flare pops on the shot.
    let size_y = 0.13 * (0.65 + 0.35 * flash);
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
    // Hot round core, soft falloff.
    var core = clamp(1.0 - r, 0.0, 1.0);
    core = core * core;
    // Four-point star: brightest along the axes, so it reads as a flash, not a disc.
    let ang = atan2(p.y, p.x);
    let spikes = pow(max(abs(cos(2.0 * ang)), 0.0), 6.0) * clamp(1.0 - r, 0.0, 1.0);
    let shape = clamp(core + spikes * 0.8, 0.0, 1.0);
    let a = shape * flash;
    if (a <= 0.001) {
        discard;
    }
    // Warm white-yellow, premultiplied for the additive blend (a flash adds light).
    let col = vec3<f32>(1.0, 0.90, 0.6);
    return vec4<f32>(col * a, a);
}
