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
    @location(2) hext: vec2<f32>,
    @location(3) color: vec3<f32>,
) -> VertexOut {
    let world = vec2<f32>(
        center.x + corner.x * hext.x,
        center.y + corner.y * hext.y,
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

// ---- ground fill (procedural TACTICAL MAP under the grid) -------------------------------------
//
// A single large world-space quad drawn FIRST (under the grid lines + units) so the top-down floor
// reads as a *designed tactical map* — a topographic, sectored military board — rather than a flat
// slate fill. It samples NO texture (the command pass's `terrain::draw` has no &Queue to upload one
// without touching lib.rs); everything is derived procedurally from world position, so it is
// identical every frame regardless of what is on the map. Pure render derivation: no sim state, no
// fog, no intel (invariant #6). All layers stay DARK / LOW-contrast / LOW-saturation so units,
// selection rims, control rings, fog and the marquee box keep popping cleanly on top — the map
// recedes, it never competes. Base palette aligned to the command clear (CLEAR_LIT ≈ 0.02/0.03/0.05)
// and the cool theme slate (deep blue-grey INK/PANEL family).
//
// Four cooperating layers, all world-space so they pan/frame with the camera:
//   1. a smooth procedural ELEVATION field (the single source of truth for shading + contours);
//   2. broad two-tone terrain ZONES (a high-ground sector vs a low basin) for a sense of *place*;
//   3. topographic CONTOUR lines read straight off the elevation field (the "military map" read),
//      with heavier index contours every few steps and constant ~1px width via screen derivatives;
//   4. a framing radial VIGNETTE that darkens the surround and focuses the play space.

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

// Smooth, alias-free procedural elevation at a world point, normalized to ~[-1, 1]. A few
// low-frequency, mutually-rotated sinusoids sum into broad rolling relief (ridges + hollows). This
// is the ONE source for both the tonal shading and the contour lines below, so the contours hug the
// shading exactly like a real topographic map. Low frequencies => only a couple of swells cross the
// ±40 framing => the relief reads as region-scale terrain, not noise.
fn elevation(p: vec2<f32>) -> f32 {
    let h = sin(p.x * 0.045 + 0.7) * cos(p.y * 0.039 - 0.3)
        + 0.60 * sin(p.x * 0.021 - p.y * 0.018 + 1.7)
        + 0.50 * cos(p.y * 0.030 + p.x * 0.013);
    return clamp(h / 2.10, -1.0, 1.0);
}

// One topographic contour layer: a constant-width bright line everywhere `elev` crosses a multiple
// of `1/bands`. `tri` is the triangle-wave distance to the nearest band edge (0 on a contour, rising
// between); dividing the smoothstep edge by the screen-space derivative `fwidth` holds every line at
// ~`px` pixels wide no matter the local slope — wide-spaced on flats, tight on steeps, exactly like
// a real map. Returns 0..1 line coverage.
fn contour(elev: f32, bands: f32, px: f32) -> f32 {
    let bv = elev * bands;
    let tri = 0.5 - abs(fract(bv) - 0.5);
    let w = max(fwidth(bv), 1e-4);
    return 1.0 - smoothstep(0.0, w * px, tri);
}

@fragment
fn fs_ground(in: GroundOut) -> @location(0) vec4<f32> {
    let p = in.world;

    // (1) Elevation field — drives both the shading and the contours so they agree.
    let elev = elevation(p);

    // (2) Broad terrain ZONES: a separate ultra-low-frequency field carves the board into a couple
    // of broad sectors (high ground vs low basin), smoothstep-blended so the boundary is a soft
    // tonal gradient, never a hard seam. Very low contrast — it gives the board *place*, not intel.
    let zone_f = sin(p.x * 0.018 - 0.4) + cos(p.y * 0.015 + 0.9) + 0.70 * sin((p.x + p.y) * 0.011);
    let zone = smoothstep(-0.85, 0.85, zone_f); // 0 = low basin, 1 = high-ground sector

    // Fine grain so the surface isn't dead-flat up close — well below grid/unit contrast.
    let grain = sin(p.x * 0.33 + p.y * 0.21) * cos(p.y * 0.29 - p.x * 0.17);

    // HILLSHADE: finite-difference the elevation field into a surface normal and light it from a
    // fixed cartographic NW key, so the relief reads as lit 3-D terrain (the contours then hug the
    // shading, exactly like a real topographic map) rather than flat tinting. Low contrast on
    // purpose — cosmetic map relief that recedes under the units, never intel: it is a pure function
    // of world position (invariant #6), identical every frame. Mirrored + range-tested on the CPU by
    // `terrain::hillshade` (the `world::moon_glow` reference-impl pattern); keep the constants in
    // lockstep.
    let e = 3.0;
    let hx = elevation(p + vec2<f32>(e, 0.0)) - elevation(p - vec2<f32>(e, 0.0));
    let hy = elevation(p + vec2<f32>(0.0, e)) - elevation(p - vec2<f32>(0.0, e));
    let hn = normalize(vec3<f32>(-hx * 6.0, -hy * 6.0, 1.0));
    let hkey = normalize(vec3<f32>(-0.55, 0.62, 0.56));
    let hill = mix(0.90, 1.14, clamp(dot(hn, hkey) * 0.5 + 0.5, 0.0, 1.0));

    // (3) CONTOURS: faint minor lines plus heavier index lines (~every 3rd step), topo-map style.
    let minor_c = contour(elev, 7.0, 1.3);
    let index_c = contour(elev, 7.0 / 3.0, 1.4);

    // (4) Framing VIGNETTE: the field falls off toward the framed edges so the board reads as a lit
    // centre fading into a dark surround (the camera frames ±40 about the origin). A floor below
    // keeps even the corners cold-grey, never near-black.
    let r = length(p) / 52.0;
    let vignette = clamp(r * r, 0.0, 1.0) * 0.52;

    // Cold blue-grey base just above the INK clear — blue leads, low saturation so units/icons pop.
    let base = vec3<f32>(0.030, 0.041, 0.061);
    // Zone tint: high-ground sector a touch brighter & cooler, basin a touch darker. Tiny deltas.
    let zone_tint = mix(vec3<f32>(-0.005, -0.005, -0.006), vec3<f32>(0.007, 0.009, 0.013), zone);
    // Gentle elevation tinting (rises lighter, hollows darker) + the fine grain, then the hillshade
    // relief on top. `elev` tint is eased back (the hillshade now carries the sense of relief).
    var col = (base + zone_tint) * (1.0 + elev * 0.08 + grain * 0.045) * hill;
    // Contour brightening: cool, faint for minor, a touch stronger for the index lines.
    col = col + minor_c * vec3<f32>(0.016, 0.021, 0.030)
        + index_c * vec3<f32>(0.022, 0.028, 0.040);
    // Subtract the cold framing vignette, then floor so the surround stays a deep cold grey.
    col = col - vignette * vec3<f32>(0.017, 0.021, 0.031);
    col = max(col, vec3<f32>(0.013, 0.018, 0.027));
    return vec4<f32>(col, 1.0);
}
