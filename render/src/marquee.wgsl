// Band-select marquee shader — the selection rectangle drawn in the command view while the player
// is dragging a band-select. A screen-space LOAD pass (composites over the command frame, no clear):
// one axis-aligned rectangle per quad in NDC, colored + alpha-blended. Structurally identical to
// overlay.wgsl / radial.wgsl, kept separate so the marquee pass never contends for a shader source.
//
// Float side of invariant #4 — every number here is already an f32. The marquee carries no fog data
// and only ever draws in the command view (never the dark embodied frame), so invariant #6 holds.

struct VertexOut {
    @builtin(position) clip_pos: vec4<f32>,
    @location(0) color: vec4<f32>,
};

// Per-vertex: a unit-quad corner in [-1, 1]^2. Per-instance: the rect center (NDC), its NDC
// half-extent (vec2), RGB color, and alpha — matching the CPU-side `repr(C)` `MarqueeInstance`.
@vertex
fn vs_main(
    @location(0) corner: vec2<f32>,
    @location(1) center: vec2<f32>,
    @location(2) hext: vec2<f32>,
    @location(3) color: vec3<f32>,
    @location(4) alpha: f32,
) -> VertexOut {
    var out: VertexOut;
    let ndc = vec2<f32>(
        center.x + corner.x * hext.x,
        center.y + corner.y * hext.y,
    );
    out.clip_pos = vec4<f32>(ndc.x, ndc.y, 0.0, 1.0);
    out.color = vec4<f32>(color, alpha);
    return out;
}

@fragment
fn fs_main(in: VertexOut) -> @location(0) vec4<f32> {
    return in.color;
}
