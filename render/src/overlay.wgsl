// In-session shell overlay shader (Phase 4 WS-B, D32 carve-out) — the pause / surrender /
// reconnect-prompt / post-match-summary chrome, drawn as a screen-space LOAD pass on top of the
// already-rendered (possibly dark) match frame. One axis-aligned rectangle per quad, positioned
// and sized in NDC, then shaped into a *card*: a signed-distance rounded rectangle with a crisp
// anti-aliased edge, an optional subtle vertical gradient (top a touch lighter), a hairline rim
// light along the top edge + a recessed lower lip (both gated on the gradient param, so flat
// structural roles stay clean), and an optional soft feather for drop-shadow quads. Alpha-blended
// over the frame (no clear).
//
// Float side of invariant #4 — every number here is already an f32; this overlay carries NO world
// position and no fog data (it is chrome, not intel — invariant #6 stays intact beneath it).
//
// CPU<->GPU lockstep: the per-instance inputs below MUST match, field-for-field and in order, the
// `repr(C)` `OverlayInstance` struct and the `vertex_attr_array` in `overlay.rs`. Adding a param
// means editing all three in lockstep (see the doc note on `OverlayInstance`).

struct VertexOut {
    @builtin(position) clip_pos: vec4<f32>,
    @location(0) color: vec4<f32>,
    // Corner in units of the core half-extent (interpolated) — drives the SDF and the gradient.
    @location(1) local: vec2<f32>,
    // NDC half-extent of this quad (flat).
    @location(2) half: vec2<f32>,
    // x = corner radius (NDC-y units), y = gradient amount [0,1], z = edge softness (drop shadow).
    @location(3) params: vec3<f32>,
    // Viewport aspect (width / height) so corners stay round in pixels, not egg-shaped (flat).
    @location(4) aspect: f32,
};

// Per-vertex: a unit-quad corner in [-1, 1]^2. Per-instance: the rect center (NDC), its NDC
// half-extent (vec2), RGB color, alpha, corner radius, gradient amount, edge softness, and the
// viewport aspect — matching the CPU-side `repr(C)` `OverlayInstance`.
@vertex
fn vs_main(
    @location(0) corner: vec2<f32>,
    @location(1) center: vec2<f32>,
    @location(2) half: vec2<f32>,
    @location(3) color: vec3<f32>,
    @location(4) alpha: f32,
    @location(5) radius: f32,
    @location(6) gradient: f32,
    @location(7) softness: f32,
    @location(8) aspect: f32,
) -> VertexOut {
    var out: VertexOut;
    // Pad the quad outward by the softness so a feathered drop shadow has room to fade beyond its
    // nominal rect (otherwise the soft edge would be clipped at the geometry boundary). The x pad
    // is divided by aspect so the pad is the same NDC-y amount on both axes once aspect-corrected.
    let pad = vec2<f32>(softness / max(aspect, 1e-4), softness);
    let ext_padded = half + pad;
    let ndc = vec2<f32>(
        center.x + corner.x * ext_padded.x,
        center.y + corner.y * ext_padded.y,
    );
    out.clip_pos = vec4<f32>(ndc.x, ndc.y, 0.0, 1.0);
    out.color = vec4<f32>(color, alpha);
    // Re-express the (possibly padded) corner in units of the *core* half-extent so the SDF below
    // measures distance to the un-padded rect (|local| > 1 in the padded feather band).
    out.local = vec2<f32>(
        corner.x * ext_padded.x / max(half.x, 1e-4),
        corner.y * ext_padded.y / max(half.y, 1e-4),
    );
    out.half = half;
    out.params = vec3<f32>(radius, gradient, softness);
    out.aspect = aspect;
    return out;
}

// Signed distance to an axis-aligned rounded rectangle: <0 inside, 0 on the edge, >0 outside.
// `b` is the half-extent, `r` the corner radius (in the same space as `p`/`b`).
fn rounded_box_sdf(p: vec2<f32>, b: vec2<f32>, r: f32) -> f32 {
    let q = abs(p) - b + vec2<f32>(r, r);
    return length(max(q, vec2<f32>(0.0, 0.0))) + min(max(q.x, q.y), 0.0) - r;
}

@fragment
fn fs_main(in: VertexOut) -> @location(0) vec4<f32> {
    let radius = in.params.x;
    let gradient = in.params.y;
    let softness = in.params.z;
    let aspect = max(in.aspect, 1e-4);

    // Work in an aspect-corrected space (scale x by aspect) so a corner radius reads as the same
    // pixel arc on both axes — round corners, never egg-shaped, on a wide window.
    let p = vec2<f32>(in.local.x * in.half.x * aspect, in.local.y * in.half.y);
    let ext = vec2<f32>(in.half.x * aspect, in.half.y);
    let r = clamp(radius, 0.0, min(ext.x, ext.y));
    let dist = rounded_box_sdf(p, ext, r);

    // Anti-aliased coverage. `fwidth` is the per-pixel slope of the SDF; a half-pixel band on each
    // side gives a crisp ~1px edge (was a softer ~2px band). `softness` widens it so a drop-shadow
    // quad fades softly outward instead of ending in a hard line.
    let aa = max(fwidth(dist), 1e-4);
    let edge = aa * 0.5 + softness;
    let coverage = 1.0 - smoothstep(-edge, edge, dist);

    // Subtle vertical gradient: top (local.y -> +1) slightly lighter, bottom slightly darker, so a
    // panel/button reads as a lit card rather than a flat fill.
    let shade = 1.0 + gradient * 0.16 * in.local.y;
    var rgb = clamp(in.color.rgb * shade, vec3<f32>(0.0), vec3<f32>(1.0));

    // Designed-card sculpting, gated on `gradient` so the flat structural roles (scrim, panel rim,
    // drop shadow, bar track — all gradient 0) stay perfectly clean. `dist` is negative inside, so
    // `inset` is a thin ramp that is 1 right at the border and falls to 0 a few px in: a hairline
    // that hugs the rounded edge. We light the TOP of that hairline (a cool rim light, lit-from-
    // above) and darken its BOTTOM (a recessed lower lip), which lifts the card off the dark map.
    let hairline = aa + 0.006;
    let inset = clamp(1.0 + dist / hairline, 0.0, 1.0);
    let top_w = clamp(in.local.y, 0.0, 1.0);
    let bot_w = clamp(-in.local.y, 0.0, 1.0);
    // A cool bone tint for the top rim light so it reads as light, not just a brighter fill.
    let rim_tint = vec3<f32>(0.80, 0.87, 1.0);
    let rim_light = gradient * inset * top_w * 0.22;
    let lower_lip = gradient * inset * bot_w * 0.14;
    rgb = clamp(rgb + rim_light * rim_tint - vec3<f32>(lower_lip), vec3<f32>(0.0), vec3<f32>(1.0));

    return vec4<f32>(rgb, in.color.a * coverage);
}
