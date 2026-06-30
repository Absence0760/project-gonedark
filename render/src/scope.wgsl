// Embodied **sniper / zoom gun-sight** scope shader (tank embodiment P9) — a screen-space LOAD pass
// drawn over the dark embodied frame while the local player aims down sight in a tank. One alpha-
// blended quad per element, each a procedural shader glyph (no binary art assets, content-pipeline
// §6):
//   shape 0  RING     — the bright scope aperture ring (a thin annulus near the quad edge)
//   shape 1  BAR      — a solid crosshair bar (the horizontal + vertical reticle lines)
//   shape 2  DOT      — the centered aiming dot
//   shape 3  VIGNETTE — the scope tunnel: a full-screen quad that DARKENS outside the aperture circle
//
// Aspect correctness (the fat-reticle-on-a-wide-window footgun): the round elements get a per-axis
// half-size host-side (`half = (r/aspect, r)`) so they stay circular; the VIGNETTE reconstructs a
// round aperture in the fragment by scaling local-x by `aspect` (params.x). Float side of invariant
// #4: every number is already f32 host presentation, never `core`.

struct VertexOut {
    @builtin(position) clip_pos: vec4<f32>,
    @location(0) color: vec4<f32>,
    @location(1) local: vec2<f32>,              // quad corner in [-1, 1] (interpolated)
    @location(2) @interpolate(flat) shape: f32, // 0 ring, 1 bar, 2 dot, 3 vignette
    @location(3) @interpolate(flat) params: vec2<f32>, // vignette: (aspect, aperture_radius); else unused
};

@vertex
fn vs_main(
    @location(0) corner: vec2<f32>,
    @location(1) center: vec2<f32>,  // NDC center
    @location(2) half: vec2<f32>,    // per-axis NDC half-size (so circles stay round)
    @location(3) color: vec4<f32>,
    @location(4) shape: f32,
    @location(5) params: vec2<f32>,
) -> VertexOut {
    var out: VertexOut;
    let ndc = vec2<f32>(center.x + corner.x * half.x, center.y + corner.y * half.y);
    out.clip_pos = vec4<f32>(ndc.x, ndc.y, 0.0, 1.0);
    out.color = color;
    out.local = corner;
    out.shape = shape;
    out.params = params;
    return out;
}

const AA: f32 = 0.04;

// Annulus (ring) coverage between `inner` and `outer` radii.
fn ring(p: vec2<f32>, inner: f32, outer: f32) -> f32 {
    let r = length(p);
    let o = 1.0 - smoothstep(outer - AA, outer + AA, r);
    let i = smoothstep(inner - AA, inner + AA, r);
    return o * i;
}

// Filled disc coverage out to `rad`.
fn disc(p: vec2<f32>, rad: f32) -> f32 {
    return 1.0 - smoothstep(rad - AA, rad + AA, length(p));
}

@fragment
fn fs_main(in: VertexOut) -> @location(0) vec4<f32> {
    let p = in.local;
    var cov: f32 = 0.0;
    if in.shape < 0.5 {
        // RING: a thin bright aperture ring near the quad edge.
        cov = ring(p, 0.93, 0.99);
    } else if in.shape < 1.5 {
        // BAR: a solid crosshair line — the quad IS the bar (thin in one axis, host-sized).
        cov = 1.0;
    } else if in.shape < 2.5 {
        // DOT: the centered aiming dot.
        cov = disc(p, 0.85);
    } else {
        // VIGNETTE: the scope tunnel. `p` spans [-1,1] across the whole screen; rebuild a ROUND
        // aperture by scaling x by the aspect (params.x), then DARKEN everything outside the
        // aperture radius (params.y). Inside the circle stays fully transparent (you see the world).
        let aspect = in.params.x;
        let aperture = in.params.y;
        let corrected = vec2<f32>(p.x * aspect, p.y);
        let d = length(corrected);
        cov = smoothstep(aperture - 0.02, aperture + 0.06, d);
    }
    if cov <= 0.001 {
        discard;
    }
    return vec4<f32>(in.color.rgb, in.color.a * cov);
}
