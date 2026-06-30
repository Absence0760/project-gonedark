// Scene-present (upscale blit) — Phase 4 WS-C dynamic resolution.
//
// The heavy 3D scene is rendered into an offscreen intermediate texture sized by the dyn-res
// `resolution_scale` (a RENDERING choice, invariant #1/#4 — it never touches the sim). This pass
// samples that intermediate with a linear filter and stretches it across the full swapchain, so a
// sub-native scene is upscaled to the display. HUD/overlay/text chrome is drawn AFTER this pass,
// straight onto the swapchain, so it stays crisp at native resolution. At scale 1.0 the intermediate
// is full-size and this is a 1:1 (identity) blit.

@group(0) @binding(0) var scene_tex: texture_2d<f32>;
@group(0) @binding(1) var scene_samp: sampler;

struct VsOut {
    @builtin(position) pos: vec4<f32>,
    @location(0) uv: vec2<f32>,
};

// Fullscreen triangle generated from the vertex index — no vertex buffer. uv has v=0 at the top,
// matching the intermediate's top row, so the blit preserves the scene's orientation.
@vertex
fn vs_present(@builtin(vertex_index) vi: u32) -> VsOut {
    var out: VsOut;
    let uv = vec2<f32>(f32((vi << 1u) & 2u), f32(vi & 2u));
    out.uv = uv;
    out.pos = vec4<f32>(uv.x * 2.0 - 1.0, 1.0 - uv.y * 2.0, 0.0, 1.0);
    return out;
}

@fragment
fn fs_present(in: VsOut) -> @location(0) vec4<f32> {
    var c = textureSample(scene_tex, scene_samp, in.uv).rgb;

    // Gentle cinematic grade applied ONCE here over the whole world scene (HUD/text chrome is drawn
    // AFTER this pass, so it stays untouched and crisp). The scene is largely LDR, so this is a light
    // S-curve for contrast — NOT an HDR tonemap, which would wash out already-[0,1] colours.
    let s = c * c * (3.0 - 2.0 * c); // smoothstep S-curve (deepen shadows, firm up highlights)
    c = mix(c, s, 0.18);

    // Subtle vignette: darken toward the corners for a focused frame. uv is [0,1]; distance² from
    // centre peaks at 0.5 in the corners, so the corner falloff is ~0.45*0.5 ≈ 0.22.
    let d = in.uv - vec2<f32>(0.5, 0.5);
    let vignette = 1.0 - dot(d, d) * 0.45;
    c = c * vignette;

    return vec4<f32>(c, 1.0);
}
