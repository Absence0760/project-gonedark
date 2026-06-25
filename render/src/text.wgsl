// Screen-space text shader — the in-match glyph pass (radial/button/summary labels). Drawn as a
// LOAD pass on top of the already-rendered frame: one small axis-aligned rectangle per LIT bitmap
// cell of each glyph, positioned and sized in NDC, colored + alpha-blended over the frame (no
// clear). Structurally identical to overlay.wgsl (a solid-color quad), kept separate so the text
// pass never contends with the overlay/HUD/radial pass for a shader source.
//
// Float side of invariant #4 — every number here is already an f32. Text carries NO world position
// and no fog data (it is chrome, not intel — invariant #6 holds beneath it; the host only draws it
// where the overlay/radial/command chrome is already allowed).

struct VertexOut {
    @builtin(position) clip_pos: vec4<f32>,
    @location(0) color: vec4<f32>,
};

// Per-vertex: a unit-quad corner in [-1, 1]^2. Per-instance: the cell center (NDC), its NDC
// half-extent (vec2), RGB color, and alpha — matching the CPU-side `repr(C)` `CellInstance`.
@vertex
fn vs_main(
    @location(0) corner: vec2<f32>,
    @location(1) center: vec2<f32>,
    @location(2) half: vec2<f32>,
    @location(3) color: vec3<f32>,
    @location(4) alpha: f32,
) -> VertexOut {
    var out: VertexOut;
    let ndc = vec2<f32>(
        center.x + corner.x * half.x,
        center.y + corner.y * half.y,
    );
    out.clip_pos = vec4<f32>(ndc.x, ndc.y, 0.0, 1.0);
    out.color = vec4<f32>(color, alpha);
    return out;
}

@fragment
fn fs_main(in: VertexOut) -> @location(0) vec4<f32> {
    return in.color;
}
