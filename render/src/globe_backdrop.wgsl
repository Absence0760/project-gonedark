// Globe backdrop (D103) — the campaign atlas earth behind the desktop Operations hub.
// Four layers, drawn by `globe_backdrop.rs`: sky gradient (clears), the earth sphere (land/sea
// from the equirectangular R8 mask, fresnel-rimmed), the conflict pins (additive, screen-space
// billboards riding the globe's rotation), and a corner vignette. Palette values are baked from
// `render::theme` (INK sky, BONE-muted land, AMBER pins) — names point back to the source.

struct GlobeUniform {
    view_proj: mat4x4<f32>,
    // The globe's model matrix: yaw (focus + slow drift) then translate/scale into place.
    model: mat4x4<f32>,
    // xyz = camera world position; w = time (seconds).
    eye: vec4<f32>,
    // x = aspect (w/h); yzw reserved (0).
    misc: vec4<f32>,
}

@group(0) @binding(0) var<uniform> u: GlobeUniform;
@group(0) @binding(1) var mask_tex: texture_2d<f32>;
@group(0) @binding(2) var mask_samp: sampler;

// ---- fullscreen helpers (sky + vignette) -------------------------------------------------------

struct FsOut {
    @builtin(position) pos: vec4<f32>,
    @location(0) ndc: vec2<f32>,
}

@vertex
fn vs_fs(@builtin(vertex_index) vi: u32) -> FsOut {
    // One oversized triangle covering the screen.
    var corners = array<vec2<f32>, 3>(vec2(-1.0, -1.0), vec2(3.0, -1.0), vec2(-1.0, 3.0));
    var out: FsOut;
    out.pos = vec4(corners[vi], 0.0, 1.0);
    out.ndc = corners[vi];
    return out;
}

@fragment
fn fs_sky(in: FsOut) -> @location(0) vec4<f32> {
    // Deep ink overhead -> a faint warm band low behind the globe (theme::INK family).
    let t = clamp(in.ndc.y * 0.5 + 0.5, 0.0, 1.0);
    let top = vec3(0.012, 0.018, 0.034);
    let low = vec3(0.052, 0.042, 0.036);
    // A whisper of star-noise so the void isn't flat (hash of the ndc; static, not twinkling).
    let h = fract(sin(dot(floor(in.ndc * 240.0), vec2(12.9898, 78.233))) * 43758.5453);
    let star = step(0.9985, h) * 0.35;
    return vec4(mix(low, top, t) + vec3(star), 1.0);
}

@fragment
fn fs_vignette(in: FsOut) -> @location(0) vec4<f32> {
    // Darken the corners so the centred hub card reads (same trick as the title diorama).
    let r = length(in.ndc * vec2(1.0, 0.85));
    let a = smoothstep(0.75, 1.55, r) * 0.55;
    return vec4(0.0, 0.0, 0.0, a);
}

// ---- the earth ---------------------------------------------------------------------------------

struct GlobeOut {
    @builtin(position) pos: vec4<f32>,
    // Earth-fixed unit position (pre-model), for the mask lookup + graticule.
    @location(0) unit: vec3<f32>,
    // World-space normal (post-model rotation), for lighting.
    @location(1) normal: vec3<f32>,
    // World-space position, for the eye ray.
    @location(2) world: vec3<f32>,
}

@vertex
fn vs_globe(@location(0) pos: vec3<f32>) -> GlobeOut {
    var out: GlobeOut;
    let world = u.model * vec4(pos, 1.0);
    out.pos = u.view_proj * world;
    out.unit = pos;
    // The model is rotation+uniform-scale+translation, so rotating the unit normal suffices.
    out.normal = normalize((u.model * vec4(pos, 0.0)).xyz);
    out.world = world.xyz;
    return out;
}

const PI: f32 = 3.14159265;

@fragment
fn fs_globe(in: GlobeOut) -> @location(0) vec4<f32> {
    let n = normalize(in.unit);
    // Earth-fixed lat/lon from the unit position (lon 0 faces +Z pre-rotation) -> mask UV.
    // Mirrors `globe_backdrop.rs::latlon_to_unit` — the two must stay inverses.
    let lat = asin(clamp(n.y, -1.0, 1.0));
    let lon = atan2(n.x, n.z);
    let uv = vec2(lon / (2.0 * PI) + 0.5, 0.5 - lat / PI);
    let land = textureSample(mask_tex, mask_samp, uv).r;

    // Palette: sea = near-black ink; land = muted bone-grey (theme::MUTED family), kept far
    // enough above the sea that the coastlines read through the vignette.
    let sea = vec3(0.010, 0.015, 0.028);
    let land_col = vec3(0.148, 0.150, 0.136);
    var base = mix(sea, land_col, smoothstep(0.35, 0.65, land));

    // Faint graticule every 30 degrees — the "atlas" read, kept a whisper above the base.
    let lat_deg = lat * 180.0 / PI;
    let lon_deg = lon * 180.0 / PI;
    let g_lat = abs(fract(lat_deg / 30.0 + 0.5) - 0.5) * 30.0;
    let g_lon = abs(fract(lon_deg / 30.0 + 0.5) - 0.5) * 30.0;
    let line = 1.0 - smoothstep(0.0, 0.4, min(g_lat, g_lon * max(cos(lat), 0.05)));
    base += vec3(0.010, 0.012, 0.016) * line;

    // Lighting: a cool key from the upper-left plus an amber fresnel rim toward the horizon —
    // the diorama's signature accent, so the two backdrops read as one family.
    let wn = normalize(in.normal);
    let key = max(dot(wn, normalize(vec3(-0.45, 0.65, 0.62))), 0.0);
    let view_dir = normalize(u.eye.xyz - in.world);
    let rim = pow(1.0 - max(dot(wn, view_dir), 0.0), 2.6);
    var col = base * (0.35 + 0.85 * key);
    col += vec3(0.55, 0.33, 0.10) * rim * 0.16;
    return vec4(col, 1.0);
}

// ---- conflict pins -----------------------------------------------------------------------------

struct PinOut {
    @builtin(position) pos: vec4<f32>,
    @location(0) corner: vec2<f32>,
    @location(1) glow: f32,
}

@vertex
fn vs_pin(
    @builtin(vertex_index) vi: u32,
    @location(0) unit: vec3<f32>,
    @location(1) focused: f32,
) -> PinOut {
    var corners = array<vec2<f32>, 6>(
        vec2(-1.0, -1.0), vec2(1.0, -1.0), vec2(1.0, 1.0),
        vec2(-1.0, -1.0), vec2(1.0, 1.0), vec2(-1.0, 1.0),
    );
    // Lift the pin just off the surface so it never z-fights the sphere.
    let world = u.model * vec4(unit * 1.012, 1.0);
    var clip = u.view_proj * world;

    // Screen-space billboard: constant on-screen size, focused pins pulse gently.
    let t = u.eye.w;
    let pulse = select(0.0, (sin(t * 2.4) * 0.5 + 0.5) * 0.35, focused > 0.5);
    let size = (0.011 + 0.008 * focused + 0.006 * pulse) * clip.w;
    let c = corners[vi];
    clip.x += c.x * size / max(u.misc.x, 0.05);
    clip.y += c.y * size;

    // Fade a pin as it rotates onto the far side (the horizon swallow), instead of popping.
    let wn = normalize((u.model * vec4(unit, 0.0)).xyz);
    let facing = dot(wn, normalize(u.eye.xyz - world.xyz));
    var out: PinOut;
    out.pos = clip;
    out.corner = c;
    out.glow = clamp(facing * 3.0, 0.0, 1.0) * (0.55 + 0.45 * focused + pulse);
    return out;
}

@fragment
fn fs_pin(in: PinOut) -> @location(0) vec4<f32> {
    // A soft amber mote with a hot core (theme::AMBER) — additive, like the diorama embers.
    let r = length(in.corner);
    let core = smoothstep(0.45, 0.0, r);
    let halo = smoothstep(1.0, 0.15, r) * 0.35;
    let a = (core + halo) * in.glow;
    return vec4(vec3(0.96, 0.62, 0.20) * a, a);
}
