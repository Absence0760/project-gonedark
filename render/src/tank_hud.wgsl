// Embodied **tank** HUD shader (tank embodiment P8, D55) — a screen-space LOAD pass drawn over the
// dark embodied frame while the local player is driving a tank. One alpha-blended quad per element,
// each a procedural shader glyph (no binary art assets, content-pipeline §6):
//   shape 0  RETICLE — the dispersion crosshair ring (radius set host-side by `dispersion`)
//   shape 1  RELOAD   — an arc ring filled clockwise from the top by `param` ∈ [0,1] (reload progress)
//   shape 2  TURRET   — a chevron marking the hull-relative gun bearing on the top compass strip
//   shape 3  LEAD     — the lead pip (a small hollow ring) offset from center toward the aim-ahead point
//
// Per-axis half-size keeps the round elements circular regardless of viewport aspect (the host packs
// `half = (r/aspect, r)`). Float side of invariant #4: every number is already f32 (host-side
// presentation), never `core`.

struct VertexOut {
    @builtin(position) clip_pos: vec4<f32>,
    @location(0) color: vec4<f32>,
    @location(1) local: vec2<f32>,              // quad corner in [-1, 1] (interpolated)
    @location(2) @interpolate(flat) shape: f32, // 0 reticle, 1 reload, 2 turret, 3 lead
    @location(3) @interpolate(flat) param: f32, // reload fill fraction (shape 1); unused otherwise
};

@vertex
fn vs_main(
    @location(0) corner: vec2<f32>,
    @location(1) center: vec2<f32>,  // NDC center
    @location(2) half: vec2<f32>,    // per-axis NDC half-size (so circles stay round)
    @location(3) color: vec4<f32>,
    @location(4) shape: f32,
    @location(5) param: f32,
) -> VertexOut {
    var out: VertexOut;
    let ndc = vec2<f32>(center.x + corner.x * half.x, center.y + corner.y * half.y);
    out.clip_pos = vec4<f32>(ndc.x, ndc.y, 0.0, 1.0);
    out.color = color;
    out.local = corner;
    out.shape = shape;
    out.param = param;
    return out;
}

const AA: f32 = 0.05;
const TAU: f32 = 6.28318531;

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

// A chevron mask pointing toward +y: two soft arms forming a "^".
fn chevron(p: vec2<f32>) -> f32 {
    let d = abs(abs(p.x) - (p.y * 0.5 + 0.45));
    let arm = 1.0 - smoothstep(0.16, 0.16 + AA, d);
    let inside = 1.0 - smoothstep(0.7 - AA, 0.7 + AA, length(p));
    return arm * inside;
}

@fragment
fn fs_main(in: VertexOut) -> @location(0) vec4<f32> {
    let p = in.local;
    var cov: f32 = 0.0;
    if in.shape < 0.5 {
        // RETICLE: a thin crosshair ring near the quad edge + a small center dot. A settled gun draws
        // a small quad (tight ring); a blooming one draws a big quad (wide ring) — the radius is the
        // host-side `dispersion_reticle_radius`, baked into the quad's half-size.
        let band = ring(p, 0.80, 0.96);
        let dot = disc(p, 0.06);
        cov = max(band, dot);
    } else if in.shape < 1.5 {
        // RELOAD: an annulus masked to the filled fraction, sweeping CLOCKWISE from the top (+y).
        // `param` 0 → nothing, 1 → full ring (loaded). atan2(x, y) is 0 at +y and grows toward +x
        // (clockwise), so the swept wedge fills top-first like a reloading clock hand.
        let band = ring(p, 0.74, 0.96);
        let ang = atan2(p.x, p.y);                       // 0 at +y, +pi/2 at +x (clockwise)
        let a = select(ang, ang + TAU, ang < 0.0);       // [0, TAU)
        let filled = step(a, in.param * TAU);
        cov = band * filled;
    } else if in.shape < 2.5 {
        // TURRET: an upward chevron marking the gun bearing on the top compass strip.
        cov = chevron(p);
    } else {
        // LEAD: a small hollow pip ring at the aim-ahead point.
        cov = ring(p, 0.45, 0.95);
    }
    if cov <= 0.001 {
        discard;
    }
    return vec4<f32>(in.color.rgb, in.color.a * cov);
}
