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

        let floor_base = vec3<f32>(0.10, 0.12, 0.14); // dark earthy slate
        let floor_line = vec3<f32>(0.28, 0.34, 0.40); // lighter grid line
        let ground = mix(floor_base, floor_line, line);

        // Horizon haze the floor dissolves into (matches the bottom of the sky gradient).
        let haze = vec3<f32>(0.16, 0.18, 0.22);
        return vec4<f32>(mix(ground, haze, fog), 1.0);
    }

    // Sky: a vertical gradient from a deep zenith to a lighter horizon band. Drive it by the ray's
    // elevation (dir.z), clamped so the band reads even when looking level/slightly down.
    let elev = clamp(dir.z, 0.0, 1.0);
    let zenith = vec3<f32>(0.04, 0.06, 0.12);  // deep night-blue overhead
    let horizon = vec3<f32>(0.16, 0.18, 0.22); // pale haze at the skyline
    return vec4<f32>(mix(horizon, zenith, elev), 1.0);
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
