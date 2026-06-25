// In-session shell overlay shader (Phase 4 WS-B, D32 carve-out) — the pause / surrender /
// reconnect-prompt / post-match-summary chrome, drawn as a screen-space LOAD pass on top of the
// already-rendered (possibly dark) match frame. One axis-aligned rectangle per quad, positioned
// and sized in NDC, colored + alpha-blended over the frame (no clear).
//
// Float side of invariant #4 — every number here is already an f32; this overlay carries NO world
// position and no fog data (it is chrome, not intel — invariant #6 stays intact beneath it).

struct VertexOut {
    @builtin(position) clip_pos: vec4<f32>,
    @location(0) color: vec4<f32>,
};

// Per-vertex: a unit-quad corner in [-1, 1]^2. Per-instance: the rect center (NDC), its NDC
// half-extent (vec2), RGB color, and alpha — matching the CPU-side `repr(C)` `OverlayInstance`.
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
