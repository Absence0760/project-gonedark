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
    let l = normalize(g.light_dir.xyz);
    // Surfaces facing into the light (-l) are lit; ambient floor keeps shadowed faces readable.
    let diffuse = max(dot(n, -l), 0.0);
    let shade = 0.38 + 0.62 * diffuse;
    // Warm muzzle-flash/emissive add, driven by the per-instance alpha (0 for ordinary tokens).
    let flash = vec3<f32>(1.0, 0.7, 0.35) * in.color.a;
    return vec4<f32>(in.color.rgb * shade + flash, 1.0);
}
