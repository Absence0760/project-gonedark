// Embodied alert-HUD shader (invariant #6) — a screen-space overlay drawn as a second LOAD
// pass on top of the already-rendered embodied frame. One small quad per directional alert
// marker, positioned in NDC by the alert's bearing relative to the avatar's yaw, colored by
// AlertKind, and alpha-faded by age. Alpha-blended over the frame (no clear).
//
// Float side of invariant #4 — every number here is already an f32; the Q16.16 → f32 hop
// happened on the CPU in `render::fixed_to_f32`, never in `core`.

struct VertexOut {
    @builtin(position) clip_pos: vec4<f32>,
    @location(0) color: vec4<f32>,
};

// Per-vertex: a unit-quad corner in [-1, 1]^2. Per-instance: the marker center in NDC,
// its RGB color, alpha, and half-size in NDC — matching the CPU-side `repr(C)` `HudMarker`.
@vertex
fn vs_main(
    @location(0) corner: vec2<f32>,
    @location(1) center: vec2<f32>,
    @location(2) color: vec3<f32>,
    @location(3) alpha: f32,
    @location(4) half_size: f32,
) -> VertexOut {
    var out: VertexOut;
    let ndc = vec2<f32>(
        center.x + corner.x * half_size,
        center.y + corner.y * half_size,
    );
    out.clip_pos = vec4<f32>(ndc.x, ndc.y, 0.0, 1.0);
    out.color = vec4<f32>(color, alpha);
    return out;
}

@fragment
fn fs_main(in: VertexOut) -> @location(0) vec4<f32> {
    return in.color;
}
