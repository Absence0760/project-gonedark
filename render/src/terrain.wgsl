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

// ---- ground fill (procedural tonal variation under the grid) ----------------------------------
//
// A single large world-space quad drawn FIRST (under the grid lines + units) so the top-down floor
// is grounded terrain rather than a flat slate fill. It samples NO texture (the command pass's
// `terrain::draw` has no &Queue to upload one without touching lib.rs) — instead it derives a gentle
// LARGE-SCALE tonal variation procedurally from world position plus a soft radial vignette, kept
// subtle so the grid stays readable and units keep popping. Pure render derivation: no sim, no fog,
// no intel (invariant #6). Base palette aligned to the command-view clear (gonedark_render CLEAR_LIT
// ≈ 0.02/0.03/0.05) and the cool theme slate.

struct GroundOut {
    @builtin(position) clip_pos: vec4<f32>,
    @location(0) world: vec2<f32>,
};

// Per-vertex: a world-space XY corner of the big ground quad (already in world units).
@vertex
fn vs_ground(@location(0) world: vec2<f32>) -> GroundOut {
    var out: GroundOut;
    out.clip_pos = camera.view_proj * vec4<f32>(world.x, world.y, 0.0, 1.0);
    out.world = world;
    return out;
}

@fragment
fn fs_ground(in: GroundOut) -> @location(0) vec4<f32> {
    let p = in.world;
    // Large-scale tonal variation: a few low-frequency, mutually-rotated sinusoids sum into smooth
    // rolling blobs (a cheap, alias-free stand-in for macro terrain mottling). Normalised to ~[-1,1].
    let n = sin(p.x * 0.090 + 0.7) * cos(p.y * 0.075 - 0.3)
        + 0.55 * sin(p.x * 0.031 - p.y * 0.027)
        + 0.45 * cos(p.y * 0.052 + p.x * 0.019);
    let mottle = clamp(n / 2.0, -1.0, 1.0);

    // Soft radial vignette: the field dims gently toward the framed edges for depth/focus on centre.
    // Keyed off world distance from the origin (the command camera frames ±40 around it); clamped
    // mild so no pixel ever sinks toward the near-black "dark" bucket the viz harness keys on.
    let r = length(p) / 70.0;
    let vignette = clamp(r * r, 0.0, 1.0) * 0.22;

    // Base just above the clear slate so the grid + units still read, modulated by the mottle and
    // darkened a touch by the vignette. Cool blue-grey; blue leads but stays low-saturation.
    let base = vec3<f32>(0.030, 0.040, 0.060);
    let tint = 1.0 + mottle * 0.30 - vignette;
    let col = base * clamp(tint, 0.55, 1.45);
    return vec4<f32>(col, 1.0);
}
