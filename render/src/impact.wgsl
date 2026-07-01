// Embodied bullet-impact VFX shader (WS-A, CP-2 game-feel bar) — a screen-space spark/dust burst at
// the point the avatar's OWN shot landed, drawn as an ADDITIVE LOAD pass over the embodied frame so
// it reads as light, not an alpha-cutout sticker. One small quad per burst element (a hot radial
// core + an expanding dust ring), positioned in NDC by the host (which projected the world hit point
// through the camera). Reveals nothing — it sits at a point the player just shot at (invariant #6).
//
// Float side of invariant #4 — every number here is already f32; no sim state, never `core`.

struct VertexOut {
    @builtin(position) clip_pos: vec4<f32>,
    @location(0) color: vec4<f32>,
    @location(1) local: vec2<f32>,              // quad corner in [-1,1] (interpolated)
    @location(2) @interpolate(flat) shape: f32, // 0 = radial core, 1 = expanding ring
};

@vertex
fn vs_main(
    @location(0) corner: vec2<f32>,
    @location(1) center: vec2<f32>,  // NDC center
    @location(2) hext: vec2<f32>,    // per-axis NDC half-size (aspect-corrected → round)
    @location(3) color: vec4<f32>,   // rgb + alpha (already faded by intensity)
    @location(4) shape: f32,
) -> VertexOut {
    var out: VertexOut;
    let ndc = vec2<f32>(center.x + corner.x * hext.x, center.y + corner.y * hext.y);
    out.clip_pos = vec4<f32>(ndc.x, ndc.y, 0.0, 1.0);
    out.color = color;
    out.local = corner;
    out.shape = shape;
    return out;
}

@fragment
fn fs_main(in: VertexOut) -> @location(0) vec4<f32> {
    let r = length(in.local);
    let soft = clamp(1.0 - r, 0.0, 1.0);
    var cov = 0.0;
    if (in.shape < 0.5) {
        // Hot flash core: a soft warm halo with a blown-out pinpoint at the strike — under additive
        // blend the centre saturates to white, so the heat reads as a punch, not a flat blob.
        let halo = soft * soft;
        let hot = pow(soft, 6.0);
        cov = halo * 0.55 + hot;
    } else if (in.shape < 1.5) {
        // Dust puff: a soft, wide, expanding ring of lit dust kicked up by the strike (low + warm).
        let ring = 1.0 - smoothstep(0.0, 0.5, abs(r - 0.70));
        let inside = step(r, 1.0); // clip the corners outside the unit circle
        cov = ring * inside * 0.8;
    } else {
        // Spark ember: a crisp, tight dot (flying debris catching the impact light) — a sharper
        // falloff than the flash so the embers stay pin-bright rather than smearing into the halo.
        cov = pow(soft, 2.5) + pow(soft, 9.0);
    }
    let a = in.color.a * cov;
    if (a <= 0.001) {
        discard;
    }
    // ADDITIVE: premultiply the color by coverage·alpha; the pipeline blends src + dst so the burst
    // adds light to the frame (a spark glows, it does not punch a hole).
    return vec4<f32>(in.color.rgb * a, a);
}
