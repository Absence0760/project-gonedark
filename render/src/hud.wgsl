// Embodied alert-HUD shader (invariant #6) — a screen-space overlay drawn as a second LOAD
// pass on top of the already-rendered embodied frame. One small quad per directional alert
// marker, positioned in NDC by the alert's bearing relative to the avatar's yaw, colored by
// AlertKind, and alpha-faded by age. Alpha-blended over the frame (no clear).
//
// Float side of invariant #4 — every number here is already an f32; the Q16.16 → f32 hop
// happened on the CPU in `render::fixed_to_f32`, never in `core`.

struct VertexOut {
    @builtin(position) clip_pos: vec4<f32>,
    @location(0) color: vec4<f32>,
    @location(1) local: vec2<f32>,             // the quad corner in [-1, 1] (interpolated)
    @location(2) @interpolate(flat) shape: f32, // glyph: 0 dot, 1 chevron, 2 triangle, 3 ring, 4 hitmarker
};

// Per-vertex: a unit-quad corner in [-1, 1]^2. Per-instance: the marker center in NDC,
// its RGB color, alpha, half-size in NDC, and a shape id — matching the CPU `repr(C)` `HudMarker`.
@vertex
fn vs_main(
    @location(0) corner: vec2<f32>,
    @location(1) center: vec2<f32>,
    @location(2) color: vec3<f32>,
    @location(3) alpha: f32,
    @location(4) half_size: f32,
    @location(5) shape: f32,
) -> VertexOut {
    var out: VertexOut;
    let ndc = vec2<f32>(
        center.x + corner.x * half_size,
        center.y + corner.y * half_size,
    );
    out.clip_pos = vec4<f32>(ndc.x, ndc.y, 0.0, 1.0);
    out.color = vec4<f32>(color, alpha);
    out.local = corner;
    out.shape = shape;
    return out;
}

// Shape the marker by its glyph id, returning a soft [0,1] coverage over the quad-local coord
// `p` in [-1, 1]^2. A square block has no directional read and aliases hard (invariant #6 wants a
// soft directional flash), so every glyph is masked with an anti-aliased `smoothstep` edge:
//   0 = filled dot, 1 = chevron (points up = "incoming"), 2 = triangle, 3 = hollow ring,
//   4 = hitmarker (centered "X" — the player's own connecting shot).
fn glyph_coverage(p: vec2<f32>, shape: f32) -> f32 {
    // `aa` is the half-width of the soft edge in local units (one quad ~ 2 units across).
    let aa = 0.14;

    if shape < 0.5 {
        // Filled dot: coverage falls off at the unit circle.
        let r = length(p);
        return 1.0 - smoothstep(0.78 - aa, 0.78 + aa, r);
    } else if shape < 1.5 {
        // Chevron pointing up (+y): two soft arms forming a ">" rotated to point at top-center.
        // Distance to the V made by |x| vs a downward slope; fade across the stroke width.
        let d = abs(abs(p.x) - (p.y * 0.5 + 0.5));
        let arm = 1.0 - smoothstep(0.18, 0.18 + aa, d);
        // Clip the long tails so it reads as a chevron, not an infinite V.
        let inside = 1.0 - smoothstep(0.85 - aa, 0.85 + aa, length(p));
        return arm * inside;
    } else if shape < 2.5 {
        // Upward triangle: keep points whose distance below the three edges is non-negative.
        // Edges of an equilateral-ish triangle pointing at +y, softened on the boundary.
        let top = smoothstep(0.85, 0.85 - aa, p.y);                    // below the apex line
        let left = smoothstep(-0.85, -0.85 + aa, p.x + p.y * 0.0);     // right of left edge
        let bottom = smoothstep(-0.7, -0.7 + aa, p.y);                 // above the base
        // Slanted sides: x bounded by a width that grows toward the base.
        let half_w = (0.7 - p.y) * 0.6;
        let sides = 1.0 - smoothstep(half_w, half_w + aa, abs(p.x));
        return min(min(top, bottom), min(left, sides));
    } else if shape < 3.5 {
        // Hollow ring: coverage between an inner and outer radius (a place you no longer hold).
        let r = length(p);
        let outer = 1.0 - smoothstep(0.82 - aa, 0.82 + aa, r);
        let inner = smoothstep(0.5 - aa, 0.5 + aa, r);
        return outer * inner;
    } else {
        // Hitmarker (shape 4): four short diagonal ticks forming an "X" with an empty center gap —
        // the classic "I hit him" confirmation flash. Coverage near either diagonal (|x-y| or
        // |x+y|), clipped between an inner gap radius and the outer edge so it reads as four ticks,
        // not a solid X. (WS-4: feedback on the player's OWN shot, never map intel — invariant #6.)
        let d1 = abs(p.x - p.y) * 0.70711;
        let d2 = abs(p.x + p.y) * 0.70711;
        let on_diag = max(
            1.0 - smoothstep(0.12, 0.12 + aa, d1),
            1.0 - smoothstep(0.12, 0.12 + aa, d2),
        );
        let r = length(p);
        let gap = smoothstep(0.30 - aa, 0.30 + aa, r);    // empty center
        let outer = 1.0 - smoothstep(0.92 - aa, 0.92 + aa, r);
        return on_diag * gap * outer;
    }
}

@fragment
fn fs_main(in: VertexOut) -> @location(0) vec4<f32> {
    let cov = glyph_coverage(in.local, in.shape);
    // Discard the fully-transparent rim so neighboring pings don't overdraw as hard blocks.
    if cov <= 0.001 {
        discard;
    }
    // Multiply coverage into alpha for an anti-aliased, soft-edged directional marker.
    return vec4<f32>(in.color.rgb, in.color.a * cov);
}
