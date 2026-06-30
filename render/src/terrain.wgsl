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

    // Macro tonal variation: a few low-frequency, mutually-rotated sinusoids sum into smooth rolling
    // blobs — a cheap, alias-free stand-in for large-scale terrain mottling (lighter rises, darker
    // hollows) so the board has region-scale depth rather than reading as one flat slate. The
    // frequencies are low enough that a couple of broad swells cross the ±40 framing. Norm ~[-1,1].
    let macro_n = sin(p.x * 0.052 + 0.7) * cos(p.y * 0.044 - 0.3)
        + 0.55 * sin(p.x * 0.024 - p.y * 0.020)
        + 0.45 * cos(p.y * 0.034 + p.x * 0.015);
    let mottle = clamp(macro_n / 2.0, -1.0, 1.0);

    // Fine grain: a higher-frequency, low-amplitude ripple so the surface isn't dead-flat up close —
    // well below the grid/unit contrast, just enough to texture the fill.
    let grain = sin(p.x * 0.33 + p.y * 0.21) * cos(p.y * 0.29 - p.x * 0.17);

    // Soft radial vignette: the field falls off toward the framed edges for depth + centre focus.
    // Keyed off world distance from the origin (the command camera frames ±40 around it). Stronger
    // than before so the board reads as a lit centre fading to a dark surround, but the floor below
    // keeps every pixel cold-grey, never near-black.
    let r = length(p) / 60.0;
    let vignette = clamp(r * r, 0.0, 1.0) * 0.36;

    // Cold blue-grey base just above the INK clear — blue leads, low saturation so units/icons pop.
    // Mottle brightens the rises / darkens the hollows; the vignette subtracts a small cold amount at
    // the edges; the grain adds a faint texture. A floor keeps it from sinking toward the dark bucket.
    let base = vec3<f32>(0.031, 0.042, 0.062);
    let lit = base * (1.0 + mottle * 0.46 + grain * 0.05)
        - vignette * vec3<f32>(0.013, 0.016, 0.024);
    let col = max(lit, vec3<f32>(0.017, 0.023, 0.034));
    return vec4<f32>(col, 1.0);
}
