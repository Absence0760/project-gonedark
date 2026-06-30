// Animated 3D parallax TITLE BACKDROP shader (the mood-setting "almost interactive" background
// behind the desktop title/landing screen). See `render/src/title_backdrop.rs` for the host side.
//
// FIVE entry-point pairs share this one module + one uniform bind group, drawn back-to-front in
// `TitleBackdrop::render`:
//   * vs_fs/fs_sky      — a fullscreen sky gradient (deep ink overhead → warmer dark horizon),
//                         the CLEARING layer. No depth.
//   * vs_grid/fs_grid   — a large ground plane (y = 0) with a receding grid that dims into horizon
//                         fog (a tactical map fading into the dark). Depth-tested.
//   * vs_box/fs_box     — instanced extruded boxes: the distant camp/city silhouette, near-black
//                         against the sky, a few with a faint AMBER fresnel rim ("embers in the
//                         dark"). Depth-tested.
//   * vs_ember/fs_ember — instanced billboard motes drifting upward + twinkling, additively blended
//                         — the signature warm accent. No depth.
//   * vs_fs/fs_vignette — a final fullscreen pass darkening the corners so centred title text reads.
//
// This is the render-side float boundary (invariant #1 forbids floats only in `core`/the sim, never
// here): every value is `f32`. WGSL cannot import the Rust palette, so the few colours baked into
// source below carry their `gonedark_render::theme` name — keep them in step with `theme.rs` (the
// same convention `world.wgsl`/`mesh.wgsl` follow). The values that vary per box/ember travel from
// the CPU as instance data derived from `theme`, so those are genuinely shared, not duplicated.

const FOG_FAR: f32 = 130.0;
// theme::AMBER (#E0791F) — the warm signal accent: ember glow + box rim light.
const AMBER: vec3<f32> = vec3<f32>(0.92, 0.55, 0.16);
// The dark horizon haze the floor + far towers dissolve into (bottom of the sky gradient).
const HORIZON_HAZE: vec3<f32> = vec3<f32>(0.10, 0.11, 0.135);

struct Uniform {
    view_proj: mat4x4<f32>,
    eye: vec4<f32>,  // xyz = camera world position; w = time (seconds)
    misc: vec4<f32>, // x = aspect (w/h); yzw reserved
};

@group(0) @binding(0)
var<uniform> u: Uniform;

// ---- fullscreen helpers (sky + vignette) ------------------------------------------------------

struct FsOut {
    @builtin(position) clip_pos: vec4<f32>,
    @location(0) ndc: vec2<f32>,
};

// A single fullscreen triangle (covers the whole NDC box with no vertex buffer).
@vertex
fn vs_fs(@builtin(vertex_index) vid: u32) -> FsOut {
    var corners = array<vec2<f32>, 3>(
        vec2<f32>(-1.0, -1.0),
        vec2<f32>(3.0, -1.0),
        vec2<f32>(-1.0, 3.0),
    );
    let p = corners[vid];
    var out: FsOut;
    out.clip_pos = vec4<f32>(p, 0.0, 1.0);
    out.ndc = p;
    return out;
}

@fragment
fn fs_sky(in: FsOut) -> @location(0) vec4<f32> {
    // 0 at the bottom of the screen → 1 at the top.
    let t = clamp(in.ndc.y * 0.5 + 0.5, 0.0, 1.0);
    let top = vec3<f32>(0.018, 0.026, 0.045);    // deep blue-black ink overhead (toward theme::INK)
    let horizon = vec3<f32>(0.058, 0.064, 0.082); // a touch warmer/lighter at the horizon
    var col = mix(horizon, top, smoothstep(0.0, 1.0, t));
    // A soft warm glow hugging the skyline — distant light bleeding up (toward theme::AMBER).
    let glow = (1.0 - smoothstep(0.0, 0.38, t)) * 0.05;
    col += vec3<f32>(0.5, 0.32, 0.16) * glow;
    return vec4<f32>(col, 1.0);
}

@fragment
fn fs_vignette(in: FsOut) -> @location(0) vec4<f32> {
    // Darken toward the corners; output black with an alpha ramp (standard alpha blend over the scene).
    let r = length(in.ndc);
    let d = smoothstep(0.72, 1.55, r) * 0.85;
    return vec4<f32>(0.0, 0.0, 0.0, d);
}

// ---- ground grid ------------------------------------------------------------------------------

struct GridOut {
    @builtin(position) clip_pos: vec4<f32>,
    @location(0) world: vec3<f32>,
};

@vertex
fn vs_grid(@location(0) pos: vec3<f32>) -> GridOut {
    var out: GridOut;
    out.clip_pos = u.view_proj * vec4<f32>(pos, 1.0);
    out.world = pos;
    return out;
}

@fragment
fn fs_grid(in: GridOut) -> @location(0) vec4<f32> {
    let eye = u.eye.xyz;
    let dist = length(in.world.xz - eye.xz);
    let fog = clamp(dist / FOG_FAR, 0.0, 1.0);

    // Grid lines on a 4-unit lattice in the ground (xz) plane; `fwidth` keeps them a constant
    // screen width (anti-aliased) as they recede to the vanishing point.
    let cell = 4.0;
    let g = abs(fract(in.world.xz / cell - 0.5) - 0.5);
    let line_w = fwidth(in.world.xz / cell);
    let grid2 = min(g / max(line_w, vec2<f32>(1e-4)), vec2<f32>(1.0));
    let line = 1.0 - min(grid2.x, grid2.y);

    let base = vec3<f32>(0.028, 0.038, 0.052);   // dark slate floor (toward theme::PANEL)
    let line_col = vec3<f32>(0.10, 0.13, 0.18);  // faint cool grid line (toward theme::HAIRLINE)
    // Lines fade out with distance so the lattice melts into the haze rather than aliasing.
    var col = mix(base, line_col, line * (1.0 - fog));
    col = mix(col, HORIZON_HAZE, fog);
    return vec4<f32>(col, 1.0);
}

// ---- silhouette boxes -------------------------------------------------------------------------

struct BoxOut {
    @builtin(position) clip_pos: vec4<f32>,
    @location(0) world: vec3<f32>,
    @location(1) normal: vec3<f32>,
    @location(2) tint: vec4<f32>, // rgb = silhouette base; a = rim amount
};

@vertex
fn vs_box(
    @location(0) pos: vec3<f32>,
    @location(1) normal: vec3<f32>,
    @location(2) c0: vec4<f32>,
    @location(3) c1: vec4<f32>,
    @location(4) c2: vec4<f32>,
    @location(5) c3: vec4<f32>,
    @location(6) tint: vec4<f32>,
) -> BoxOut {
    let model = mat4x4<f32>(c0, c1, c2, c3);
    let world = model * vec4<f32>(pos, 1.0);
    var out: BoxOut;
    out.clip_pos = u.view_proj * world;
    out.world = world.xyz;
    out.normal = normalize((model * vec4<f32>(normal, 0.0)).xyz);
    out.tint = tint;
    return out;
}

@fragment
fn fs_box(in: BoxOut) -> @location(0) vec4<f32> {
    let eye = u.eye.xyz;
    let v = normalize(eye - in.world);
    let n = normalize(in.normal);
    // A fresnel rim: edges facing away from the camera catch the faint amber edge light.
    let fres = pow(1.0 - clamp(dot(n, v), 0.0, 1.0), 3.0);
    var col = in.tint.rgb + AMBER * fres * in.tint.a;
    // Far towers melt into the horizon haze — but kept restrained so the nearer ones stay
    // near-black silhouettes against the sky rather than washing out to grey.
    let dist = length(in.world.xz - eye.xz);
    let fog = clamp(dist / FOG_FAR, 0.0, 1.0);
    col = mix(col, HORIZON_HAZE, fog * 0.5);
    return vec4<f32>(col, 1.0);
}

// ---- drifting embers / motes ------------------------------------------------------------------

struct EmberOut {
    @builtin(position) clip_pos: vec4<f32>,
    @location(0) corner: vec2<f32>,
    @location(1) glow: f32,
};

@vertex
fn vs_ember(
    @location(0) corner: vec2<f32>,
    @location(1) anchor_phase: vec4<f32>, // xyz = world anchor; w = phase [0,1)
    @location(2) params: vec4<f32>,       // x = size(NDC); y = speed; z = rise range; w = twinkle freq
) -> EmberOut {
    let time = u.eye.w;
    let phase = anchor_phase.w;
    let speed = params.y;
    let range = params.z;

    // Slow upward drift, wrapping back to the bottom.
    let cycle = fract(time * speed + phase);
    var world = anchor_phase.xyz;
    world.y += cycle * range;

    var clip = u.view_proj * vec4<f32>(world, 1.0);
    // Billboard: add a screen-facing corner offset, * clip.w so it survives the perspective divide
    // at a constant pixel size. Divide x by aspect so the mote stays round on a wide window.
    let size = params.x;
    let aspect = u.misc.x;
    clip += vec4<f32>(corner.x * size / aspect, corner.y * size, 0.0, 0.0) * clip.w;

    // Twinkle, plus fade in from the bottom and out at the top of the rise so motes don't pop.
    let twinkle = 0.55 + 0.45 * sin(time * params.w + phase * 6.2831853);
    let fade = smoothstep(0.0, 0.15, cycle) * (1.0 - smoothstep(0.7, 1.0, cycle));

    var out: EmberOut;
    out.clip_pos = clip;
    out.corner = corner;
    out.glow = twinkle * fade;
    return out;
}

@fragment
fn fs_ember(in: EmberOut) -> @location(0) vec4<f32> {
    // Soft round falloff from the sprite centre.
    let falloff = 1.0 - smoothstep(0.0, 1.0, length(in.corner));
    let a = clamp(in.glow * falloff, 0.0, 1.0);
    // Premultiplied warm amber → additive One/One blend adds a glow over the cold scene.
    return vec4<f32>(AMBER * a, a);
}
