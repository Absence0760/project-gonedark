// Command-view ground-grid shader (W6 — command-view polish).
//
// Draws the top-down ground as a tiled grid of thin world-space line quads under the units, so
// position and motion read against a stable reference instead of floating on flat slate. Each
// instance is one axis-aligned line segment (a long, thin rectangle) carrying its world center,
// half-extents, and a solid RGB color. The vertex shader places + sizes the quad and transforms
// it by the same top-down camera the units use, so the grid sits on the ground plane (z = 0) and
// shares the world frame exactly.
//
// This is the float side of invariant #4 — every number here is already an `f32`; the grid is a
// pure render-side derivation (no sim state, no fog) built on the CPU in `terrain::grid_lines`.

// Column-major 4x4 view-projection (the SAME camera the unit pass uses), uploaded by the host.
struct Camera {
    view_proj: mat4x4<f32>,
};

@group(0) @binding(0)
var<uniform> camera: Camera;

struct VertexOut {
    @builtin(position) clip_pos: vec4<f32>,
    @location(0) color: vec3<f32>,
};

// Per-vertex: the quad corner in [-1,1]^2. Per-instance: world center, half-extents, color —
// matching the CPU-side `repr(C)` `LineInstance`.
@vertex
fn vs_main(
    @location(0) corner: vec2<f32>,
    @location(1) center: vec2<f32>,
    @location(2) half: vec2<f32>,
    @location(3) color: vec3<f32>,
) -> VertexOut {
    let world = vec2<f32>(
        center.x + corner.x * half.x,
        center.y + corner.y * half.y,
    );
    var out: VertexOut;
    // The grid lives on the ground plane (z = 0), just like the units.
    out.clip_pos = camera.view_proj * vec4<f32>(world.x, world.y, 0.0, 1.0);
    out.color = color;
    return out;
}

@fragment
fn fs_main(in: VertexOut) -> @location(0) vec4<f32> {
    return vec4<f32>(in.color, 1.0);
}
