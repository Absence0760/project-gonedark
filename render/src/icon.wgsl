// Screen-space **icon** shader — the command-view glyph pass that draws small tactical icons beside
// the text-only command-bar / readout labels (infantry, armor, build, upgrade, resources, …). Drawn
// as a LOAD pass on top of the already-rendered frame: one axis-aligned rectangle per ICON, sampling
// the `icons_atlas.rgba` RGBA8 texture baked by tools/icons/gen_icons.py. Tinted + alpha-blended over
// the frame (no clear). The exact sibling of text.wgsl — the icon atlas is the visual analogue of the
// font atlas, so they share a pipeline shape (instanced quads carrying NDC center/half + atlas UV +
// tint + alpha).
//
// Float side of invariant #4 — every number here is already an f32. Icons carry NO world position and
// no fog data (chrome, not intel — invariant #6 holds beneath them; the host only draws them where the
// command-view chrome is already allowed).

@group(0) @binding(0) var atlas_tex: texture_2d<f32>;
@group(0) @binding(1) var atlas_samp: sampler;

struct VertexOut {
    @builtin(position) clip_pos: vec4<f32>,
    @location(0) uv: vec2<f32>,
    @location(1) tint: vec4<f32>,
};

// Per-vertex: a unit-quad corner in [-1, 1]^2. Per-instance: the icon-cell center (NDC), its NDC
// half-extent, the icon's atlas UV origin (top-left, [0,1]) and UV size, RGB tint, and alpha —
// matching the CPU-side `repr(C)` `IconInstance`.
@vertex
fn vs_main(
    @location(0) corner: vec2<f32>,
    @location(1) center: vec2<f32>,
    @location(2) half: vec2<f32>,
    @location(3) uv0: vec2<f32>,
    @location(4) uv_size: vec2<f32>,
    @location(5) tint: vec3<f32>,
    @location(6) alpha: f32,
) -> VertexOut {
    var out: VertexOut;
    let ndc = vec2<f32>(
        center.x + corner.x * half.x,
        center.y + corner.y * half.y,
    );
    out.clip_pos = vec4<f32>(ndc.x, ndc.y, 0.0, 1.0);
    // Quad corner (-1..1) -> atlas UV. corner.x = -1 is the icon's left (u = uv0.x); corner.y = +1
    // is the icon's top in NDC (+y up), which maps to the atlas's TOP row (v = uv0.y), so flip y.
    let s = corner.x * 0.5 + 0.5; // 0 at left .. 1 at right
    let t = corner.y * 0.5 + 0.5; // 0 at bottom .. 1 at top
    out.uv = vec2<f32>(uv0.x + s * uv_size.x, uv0.y + (1.0 - t) * uv_size.y);
    out.tint = vec4<f32>(tint, alpha);
    return out;
}

@fragment
fn fs_main(in: VertexOut) -> @location(0) vec4<f32> {
    // The atlas is straight-alpha RGBA (white shapes on a transparent ground). Modulate the texel's
    // colour by the instance tint and its alpha by the texel coverage, so an anti-aliased icon edge
    // blends smoothly and the white shape takes the tint. (A future multi-colour icon can set tint =
    // white and keep its own RGB.)
    let texel = textureSample(atlas_tex, atlas_samp, in.uv);
    return vec4<f32>(texel.rgb * in.tint.rgb, texel.a * in.tint.a);
}
