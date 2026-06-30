// "Gone dark" detection-tell shader — a WORLD-SPACE marker pass for the COMMAND view. Draws a
// designed threat reticle (a diamond ring + a double downward chevron) over each hostile EMBODIED
// enemy the local commander can currently sense, at its live-or-last-seen position. A LOAD pass
// (composites over the command frame, no clear) with no depth test, so the marker always reads on
// top of the units.
//
// It reuses the unit pass's camera bind group (group 0, binding 0 — the same top-down
// view-projection), so world (x, y, 0) maps to clip exactly as the units do — the marker sits ON
// the sensed unit. Float side of invariant #4 (the Fixed -> f32 hop happened on the CPU).
//
// Each marker stroke is a thin world-space QUAD (a ribbon), not a 1px line: the per-vertex `edge`
// runs -1..+1 across the ribbon's width (0 down its spine), and the fragment turns that into a crisp
// ANALYTIC anti-aliased coverage via `fwidth` — so the reticle reads clean at any zoom without MSAA.
// The per-vertex RGBA carries the tell's URGENCY: a fresh / in-sight / Marked tell is warm amber at
// full alpha; a `Subtle` linger fades (dimmer + cooler-red) as it ages out of its window. The pass
// alpha-blends over the frame.
//
// Fairness (invariant #6): the host only draws this in the command view, never the dark embodied
// frame, and the pure `engine::detection_markers` seam refuses to emit any marker while the local
// player is embodied — so the tell can never paint over the avatar-only frame. It is "alerts, not
// intel" for the COMMANDER: a directional marker on a sensed unit, never a reveal of the rest of
// the map.

struct Camera {
    view_proj: mat4x4<f32>,
};

@group(0) @binding(0)
var<uniform> camera: Camera;

struct VertexOut {
    @builtin(position) clip_pos: vec4<f32>,
    @location(0) color: vec4<f32>,
    // Signed cross-ribbon coordinate in [-1, 1]: 0 on the stroke's spine, ±1 at its anti-aliased
    // edges. The fragment derives crisp coverage from how fast this changes per pixel.
    @location(1) edge: f32,
};

// Per-vertex: a world-space ribbon corner (xy on the ground plane), its RGBA color (alpha = the
// tell's urgency/freshness; fades as a Subtle linger ages), and the cross-ribbon `edge`.
@vertex
fn vs_main(
    @location(0) world: vec2<f32>,
    @location(1) color: vec4<f32>,
    @location(2) edge: f32,
) -> VertexOut {
    var out: VertexOut;
    out.clip_pos = camera.view_proj * vec4<f32>(world.x, world.y, 0.0, 1.0);
    out.color = color;
    out.edge = edge;
    return out;
}

@fragment
fn fs_main(in: VertexOut) -> @location(0) vec4<f32> {
    // Analytic anti-aliasing: feather the last ~1px at the ribbon's edge (|edge| -> 1). `fwidth`
    // is the per-pixel rate of change of `edge`, so the feather band is screen-space ~1px wide at
    // any zoom — crisp when the stroke is wide, soft only right at the boundary.
    let d = abs(in.edge);
    let fw = max(fwidth(in.edge), 1e-5);
    let coverage = 1.0 - smoothstep(1.0 - fw, 1.0, d);
    return vec4<f32>(in.color.rgb, in.color.a * coverage);
}
