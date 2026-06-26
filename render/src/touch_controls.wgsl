// On-screen FPS touch-control HUD shader (the COD-style embodied controls) — a screen-space LOAD
// pass drawn over the dark embodied frame, Android only. One alpha-blended quad per control:
// the floating move-stick base (ring) + thumb (disc), and the Fire / Crouch / Reload / Surface
// buttons (a faint disc fill + outline ring + a procedural icon). Glyphs are shader-drawn shape
// ids — no binary art assets (real Inkscape icons are a later polish). Per-axis half-size keeps
// the circles round regardless of viewport aspect.
//
// Float side of invariant #4: every number is already f32 (host-side presentation), never `core`.

struct VertexOut {
    @builtin(position) clip_pos: vec4<f32>,
    @location(0) color: vec4<f32>,
    @location(1) local: vec2<f32>,              // quad corner in [-1, 1] (interpolated)
    @location(2) @interpolate(flat) shape: f32, // 0 ring, 1 disc, 2 fire, 3 crouch, 4 reload, 5 surface
};

@vertex
fn vs_main(
    @location(0) corner: vec2<f32>,
    @location(1) center: vec2<f32>,  // NDC center
    @location(2) half: vec2<f32>,    // per-axis NDC half-size (so the disc stays circular)
    @location(3) color: vec4<f32>,
    @location(4) shape: f32,
) -> VertexOut {
    var out: VertexOut;
    let ndc = vec2<f32>(center.x + corner.x * half.x, center.y + corner.y * half.y);
    out.clip_pos = vec4<f32>(ndc.x, ndc.y, 0.0, 1.0);
    out.color = color;
    out.local = corner;
    out.shape = shape;
    return out;
}

const AA: f32 = 0.06;
const PI: f32 = 3.14159265;

// Filled disc coverage out to `rad`.
fn disc(p: vec2<f32>, rad: f32) -> f32 {
    return 1.0 - smoothstep(rad - AA, rad + AA, length(p));
}

// Annulus (ring) coverage between `inner` and `outer`.
fn ring(p: vec2<f32>, inner: f32, outer: f32) -> f32 {
    let r = length(p);
    let o = 1.0 - smoothstep(outer - AA, outer + AA, r);
    let i = smoothstep(inner - AA, inner + AA, r);
    return o * i;
}

// A chevron mask pointing toward +y (`dir = +1`) or −y (`dir = -1`): two soft arms forming a "v".
fn chevron(p: vec2<f32>, dir: f32) -> f32 {
    let y = p.y * dir;
    let d = abs(abs(p.x) - (y * 0.5 + 0.45));
    let arm = 1.0 - smoothstep(0.16, 0.16 + AA, d);
    let inside = 1.0 - smoothstep(0.7 - AA, 0.7 + AA, length(p));
    return arm * inside;
}

// Per-button icon mask (shapes 2..5). Returns [0,1] coverage of the glyph strokes.
fn button_icon(p: vec2<f32>, shape: f32) -> f32 {
    if shape < 2.5 {
        // FIRE: a crosshair — thin plus inside the circle + a center dot.
        let arm = 0.62;
        let vbar = step(abs(p.x), 0.09) * (1.0 - smoothstep(arm - AA, arm + AA, abs(p.y)));
        let hbar = step(abs(p.y), 0.09) * (1.0 - smoothstep(arm - AA, arm + AA, abs(p.x)));
        let dot = disc(p, 0.16);
        return clamp(max(max(vbar, hbar), dot), 0.0, 1.0);
    } else if shape < 3.5 {
        // CROUCH: a downward chevron (duck down).
        return chevron(p, -1.0);
    } else if shape < 4.5 {
        // RELOAD: a curved arrow — a ~3/4 ring with a gap at the top.
        let band = ring(p, 0.42, 0.66);
        let ang = atan2(p.y, p.x);                 // (-pi, pi]
        // Open a gap around the top (+y ≈ +pi/2): suppress the band there.
        let gap = smoothstep(0.5, 0.9, abs(ang - PI * 0.5) / PI);
        return band * gap;
    } else {
        // SURFACE: an upward chevron (eject up, back to command).
        return chevron(p, 1.0);
    }
}

@fragment
fn fs_main(in: VertexOut) -> @location(0) vec4<f32> {
    var cov: f32;
    if in.shape < 0.5 {
        // Stick base ring.
        cov = ring(in.local, 0.70, 0.96);
    } else if in.shape < 1.5 {
        // Stick thumb disc.
        cov = disc(in.local, 0.92);
    } else {
        // Button: faint disc fill + an outline ring + the icon, the icon/outline at full alpha.
        let fill = disc(in.local, 0.94) * 0.30;
        let outline = ring(in.local, 0.80, 0.95);
        let icon = button_icon(in.local, in.shape);
        cov = max(fill, max(outline, icon));
    }
    if cov <= 0.001 {
        discard;
    }
    return vec4<f32>(in.color.rgb, in.color.a * cov);
}
