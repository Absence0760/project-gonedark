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
    return textureSample(scene_tex, scene_samp, in.uv);
}
