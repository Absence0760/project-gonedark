// Instanced 3D greybox mesh shader (decisions.md D44).
//
// One cooked `.mesh` (flat-shaded triangle soup, position + face normal) drawn per instance. The
// vertex shader transforms the mesh by the per-instance model matrix and the global camera matrix
// (`view_proj` for world-space unit tokens, the projection alone for the view-space weapon
// viewmodel). The fragment shader lights it with a single directional key light + ambient — flat
// normals give the faceted greybox look — and adds the per-instance emissive/flash term.
//
// This is the float side of the renderer (invariant #1/#4): every value is already `f32`; nothing
// here touches `core`. Keep the attribute locations in lockstep with `mesh.rs`
// (`MeshGpu::vertex_layout` + `MeshPipeline`'s instance layout, `MeshInstance`, `MeshGlobals`).

struct Globals {
    view_proj: mat4x4<f32>,
    light_dir: vec4<f32>, // xyz = light travel direction; w unused
};

@group(0) @binding(0)
var<uniform> g: Globals;

struct VertexOut {
    @builtin(position) clip_pos: vec4<f32>,
    @location(0) world_normal: vec3<f32>,
    @location(1) color: vec4<f32>, // rgb tint, a = emissive/flash
};

@vertex
fn vs_main(
    // per-vertex (mesh)
    @location(0) pos: vec3<f32>,
    @location(1) normal: vec3<f32>,
    // per-instance (model matrix columns + tint)
    @location(2) m0: vec4<f32>,
    @location(3) m1: vec4<f32>,
    @location(4) m2: vec4<f32>,
    @location(5) m3: vec4<f32>,
    @location(6) color: vec4<f32>,
) -> VertexOut {
    let model = mat4x4<f32>(m0, m1, m2, m3);
    let world = model * vec4<f32>(pos, 1.0);

    var out: VertexOut;
    out.clip_pos = g.view_proj * world;
    // Uniform scale + rotation only, so the model's upper 3x3 transforms normals fine (normalized
    // in the fragment shader). w = 0 drops the translation.
    out.world_normal = (model * vec4<f32>(normal, 0.0)).xyz;
    out.color = color;
    return out;
}

@fragment
fn fs_main(in: VertexOut) -> @location(0) vec4<f32> {
    let n = normalize(in.world_normal);

    // --- three-point-ish lighting over the flat greybox normals (art-direction constants; the only
    // runtime input is the key direction in `g.light_dir`). Replaces the old single-key + flat-grey
    // `0.38 + 0.62*diffuse` wash so forms read with warm/cool directionality and a rim. The ground
    // plane is z = 0, so world up is +z. ---

    // Key light: the warm primary. Surfaces facing into -key are lit.
    let key_l = normalize(g.light_dir.xyz);
    let key = max(dot(n, -key_l), 0.0);
    let key_col = vec3<f32>(1.0, 0.95, 0.86);

    // Fill light: a softer cool light from a fixed high-side direction, lifting shadowed faces
    // without flattening them.
    let fill_l = normalize(vec3<f32>(0.55, 0.35, -0.40));
    let fill = max(dot(n, -fill_l), 0.0);
    let fill_col = vec3<f32>(0.42, 0.52, 0.70);

    // Hemispheric ambient: a cool sky tint from above, a warm dark bounce from below (+z up).
    let up = clamp(n.z * 0.5 + 0.5, 0.0, 1.0);
    let ambient = mix(vec3<f32>(0.13, 0.12, 0.11), vec3<f32>(0.28, 0.33, 0.40), up);

    // Rim/back term: a thin highlight on faces turning away from the key, reading the silhouette of
    // the faceted forms against the world.
    let rim = pow(1.0 - key, 4.0) * 0.15;

    let lit = ambient + key_col * (key * 0.70) + fill_col * (fill * 0.22) + vec3<f32>(rim);

    // Warm muzzle-flash/emissive add, driven by the per-instance alpha (0 for ordinary tokens).
    let flash = vec3<f32>(1.0, 0.7, 0.35) * in.color.a;
    return vec4<f32>(in.color.rgb * lit + flash, 1.0);
}
