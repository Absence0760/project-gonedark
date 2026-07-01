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

// WS-E: the "going dark" amount — `params.x` is 0 in command view, 1 while embodied. Drives the
// embodied dark intensification below (a tunnel vignette + shadow crush) so the strategic map going
// dark reads as visceral tunnel vision. Presentation only (invariant #1/#4); mirrored by
// `present.rs::going_dark_grade` and unit-tested — keep the two in lockstep.
struct Present {
    params: vec4<f32>,
};
@group(0) @binding(2) var<uniform> present: Present;

// Hermite smoothstep — WGSL builtin `smoothstep`, spelled out here for the mirror's clarity.
fn ss(e0: f32, e1: f32, x: f32) -> f32 {
    let t = clamp((x - e0) / (e1 - e0), 0.0, 1.0);
    return t * t * (3.0 - 2.0 * t);
}

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

// Rec. 709 luminance weights — the perceptual grey a colour reads as. Shared by the split-tone and
// the shadow desaturation below; mirrored by `theme::luminance` on the Rust side.
const LUMA: vec3<f32> = vec3<f32>(0.2126, 0.7152, 0.0722);

@fragment
fn fs_present(in: VsOut) -> @location(0) vec4<f32> {
    var c = textureSample(scene_tex, scene_samp, in.uv).rgb;

    // Cohesive cinematic grade applied ONCE here over the whole world scene (HUD/text chrome is drawn
    // AFTER this pass, so it stays untouched and crisp). The scene is largely LDR, so this is a shaping
    // grade — NOT an HDR tonemap, which would wash out already-[0,1] colours. The reference math lives
    // in `theme::present_grade` (Rust) and is unit-tested; keep the two in lockstep.

    // 1. Contrast S-curve — deepen shadows, firm up highlights. A smoothstep about the mid-point,
    //    mixed in at a restrained weight so detail in both tails survives.
    let s = c * c * (3.0 - 2.0 * c);
    c = mix(c, s, 0.22);

    // 2. Split-tone for identity: cool the shadows toward the deep blue-black "ink" the palette is
    //    built on, and lift the highlights a touch warm toward the amber signal accent. This is what
    //    unifies the cold command view and the warmer embodied view into one graded image. Kept
    //    subtle — and the shadow cool is SUBTRACTIVE (pull the warm channels down, never raise blue)
    //    so the grade can't manufacture a bright blue-dominant pixel and be misread as "player-blue"
    //    intel by the fairness harness (invariant #6). The warm highlight likewise only pulls blue
    //    down, never up.
    let l = dot(c, LUMA);
    let shadow_w = 1.0 - smoothstep(0.0, 0.55, l); // strongest in the darks
    let highlight_w = smoothstep(0.5, 1.0, l);     // strongest in the lights
    c += vec3<f32>(-0.018, -0.006, 0.0) * shadow_w;     // cool the shadows (warm channels down)
    c += vec3<f32>(0.028, 0.012, -0.014) * highlight_w; // warm the highlights toward amber

    // 3. "Going dark" mood: gently desaturate the deepest shadows toward their own luminance, so the
    //    darkness reads as ink rather than muddy colour. Only the darks are touched (highlights keep
    //    full chroma), and only partially.
    let grey = vec3<f32>(dot(c, LUMA));
    c = mix(c, grey, shadow_w * 0.12);

    // 4. Vignette: a smooth radial falloff that darkens only toward the corners, keeping the centre
    //    fully bright so the frame focuses without crushing the edges. `r` is 0 at centre and ~1 at
    //    the extreme corner; the smoothstep leaves the inner ~55% untouched.
    let d = in.uv - vec2<f32>(0.5, 0.5);
    let r = length(d) * 1.41421356;
    let vignette = 1.0 - smoothstep(0.55, 1.15, r) * 0.34;
    c = c * vignette;

    // 5. "World goes dark" (WS-E, invariant #6): while embodied (`present.params.x` → 1) deepen the
    //    frame into visceral tunnel vision — but FAIRLY. A tunnel vignette darkens only toward the
    //    edges (the lit centre stays readable, so it's tunnel vision, not a black screen); the darks
    //    desaturate + ink-cool (subtractive on the warm channels only, never raising blue) + deepen,
    //    all weighted by `shadow_w` so lit surfaces and the amber avatar are untouched. The HUD /
    //    alert cues are drawn AFTER this pass at native res, so the fairness channel is never dimmed.
    //    Mirrored by `present.rs::going_dark_grade`; keep in lockstep.
    let dark = present.params.x;
    if (dark > 0.0) {
        let tunnel = 1.0 - ss(0.30, 1.05, r) * 0.55 * dark;
        let l = dot(c, LUMA);
        let shadow_w = 1.0 - ss(0.0, 0.5, l);
        let grey = vec3<f32>(dot(c, LUMA));
        c = mix(c, grey, shadow_w * 0.35 * dark);
        let ink_tint = vec3<f32>(-0.020, -0.012, 0.0);
        let deepen = 1.0 - shadow_w * 0.22 * dark;
        c = (c + ink_tint * shadow_w * dark) * deepen * tunnel;
    }

    return vec4<f32>(clamp(c, vec3<f32>(0.0), vec3<f32>(1.0)), 1.0);
}
