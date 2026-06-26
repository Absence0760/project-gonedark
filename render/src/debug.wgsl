// Debug hitbox/facet overlay shader — a WORLD-SPACE line pass for the command view. Draws the
// per-unit hit-radius ring (colored by armour facet for tanks), a hull-heading spoke, and shell
// tracers, so a developer can SEE the hitboxes the duel sandbox exercises. A LOAD pass (composites
// over the command frame, no clear) with no depth test, so the lines always read on top.
//
// It reuses the unit pass's camera bind group (group 0, binding 0 — the same top-down
// view-projection), so world (x, y, 0) maps to clip exactly as the units do. Float side of
// invariant #4 — every number is already f32 (the Fixed -> f32 hop happened on the CPU). The
// overlay carries no fog mask and the host only draws it in the command view (never the dark
// embodied frame), so invariant #6 holds.

struct Camera {
    view_proj: mat4x4<f32>,
};

@group(0) @binding(0)
var<uniform> camera: Camera;

struct VertexOut {
    @builtin(position) clip_pos: vec4<f32>,
    @location(0) color: vec3<f32>,
};

// Per-vertex: a world-space line endpoint (xy on the ground plane) + its RGB color.
@vertex
fn vs_main(
    @location(0) world: vec2<f32>,
    @location(1) color: vec3<f32>,
) -> VertexOut {
    var out: VertexOut;
    out.clip_pos = camera.view_proj * vec4<f32>(world.x, world.y, 0.0, 1.0);
    out.color = color;
    return out;
}

@fragment
fn fs_main(in: VertexOut) -> @location(0) vec4<f32> {
    return vec4<f32>(in.color, 1.0);
}
