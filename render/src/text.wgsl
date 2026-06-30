// Screen-space text shader — the in-match glyph pass (radial/button/summary/readout labels). Drawn
// as a LOAD pass on top of the already-rendered frame: one axis-aligned rectangle per GLYPH,
// sampling an **anti-aliased font atlas** (the `hud_atlas.gray` R8 coverage texture baked by
// tools/fonts/gen_hud_font.py) for the glyph's alpha. Coloured + alpha-blended over the frame (no
// clear). Replaces the legacy 5x7 bitmap that emitted one solid quad per lit cell.
//
// Float side of invariant #4 — every number here is already an f32. Text carries NO world position
// and no fog data (it is chrome, not intel — invariant #6 holds beneath it; the host only draws it
// where the overlay/radial/command chrome is already allowed).

@group(0) @binding(0) var atlas_tex: texture_2d<f32>;
@group(0) @binding(1) var atlas_samp: sampler;

struct VertexOut {
    @builtin(position) clip_pos: vec4<f32>,
    @location(0) uv: vec2<f32>,
    @location(1) color: vec4<f32>,
};

// Per-vertex: a unit-quad corner in [-1, 1]^2. Per-instance: the glyph-cell center (NDC), its NDC
// half-extent, the glyph's atlas UV origin (top-left, [0,1]) and UV size, RGB colour, and alpha —
// matching the CPU-side `repr(C)` `GlyphInstance`.
@vertex
fn vs_main(
    @location(0) corner: vec2<f32>,
    @location(1) center: vec2<f32>,
    @location(2) half: vec2<f32>,
    @location(3) uv0: vec2<f32>,
    @location(4) uv_size: vec2<f32>,
    @location(5) color: vec3<f32>,
    @location(6) alpha: f32,
) -> VertexOut {
    var out: VertexOut;
    let ndc = vec2<f32>(
        center.x + corner.x * half.x,
        center.y + corner.y * half.y,
    );
    out.clip_pos = vec4<f32>(ndc.x, ndc.y, 0.0, 1.0);
    // Quad corner (-1..1) -> atlas UV. corner.x = -1 is the glyph's left (u = uv0.x); corner.y = +1
    // is the glyph's top in NDC (+y up), which maps to the atlas's TOP row (v = uv0.y), so flip y.
    let s = corner.x * 0.5 + 0.5; // 0 at left .. 1 at right
    let t = corner.y * 0.5 + 0.5; // 0 at bottom .. 1 at top
    out.uv = vec2<f32>(uv0.x + s * uv_size.x, uv0.y + (1.0 - t) * uv_size.y);
    out.color = vec4<f32>(color, alpha);
    return out;
}

@fragment
fn fs_main(in: VertexOut) -> @location(0) vec4<f32> {
    // The atlas is an R8 coverage map (white glyph on black). Use the red channel as the glyph's
    // alpha coverage and modulate the instance alpha by it, so anti-aliased edges blend smoothly.
    let coverage = textureSample(atlas_tex, atlas_samp, in.uv).r;
    return vec4<f32>(in.color.rgb, in.color.a * coverage);
}
