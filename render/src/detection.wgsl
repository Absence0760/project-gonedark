// "Gone dark" detection-tell shader — a WORLD-SPACE line pass for the COMMAND view. Draws a small
// marker (a diamond ring + a downward caret) over each hostile EMBODIED enemy the local commander
// can currently sense, at its live-or-last-seen position. A LOAD pass (composites over the command
// frame, no clear) with no depth test, so the marker always reads on top of the units.
//
// It reuses the unit pass's camera bind group (group 0, binding 0 — the same top-down
// view-projection), so world (x, y, 0) maps to clip exactly as the units do — the marker sits ON
// the sensed unit. Float side of invariant #4 (the Fixed -> f32 hop happened on the CPU). The
// per-vertex color carries an ALPHA channel so a `Subtle` tell can FADE as it ages out of its
// linger window; the pass alpha-blends over the frame.
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
};

// Per-vertex: a world-space line endpoint (xy on the ground plane) + its RGBA color (alpha = the
// tell's freshness; fades as a Subtle linger ages).
@vertex
fn vs_main(
    @location(0) world: vec2<f32>,
    @location(1) color: vec4<f32>,
) -> VertexOut {
    var out: VertexOut;
    out.clip_pos = camera.view_proj * vec4<f32>(world.x, world.y, 0.0, 1.0);
    out.color = color;
    return out;
}

@fragment
fn fs_main(in: VertexOut) -> @location(0) vec4<f32> {
    return in.color;
}
