// Instanced 3D greybox mesh shader (decisions.md D44).
//
// One cooked `.mesh` (flat-shaded triangle soup, position + face normal) drawn per instance. The
// vertex shader transforms the mesh by the per-instance model matrix and the global camera matrix
// (`view_proj` for world-space unit tokens, the projection alone for the view-space weapon
// viewmodel). The fragment shader lights it with a key/fill/hemispheric-ambient rig plus a tight
// specular glint (a machined-metal catch-light on sloped facets), adds a faint procedural material
// mottle + grime so the flat facets read as worn surfaces rather than paint,
// gives the silhouette a team/identity-tinted rim so friend/enemy/avatar separate against the dark
// ground, and adds the per-instance emissive/flash term.
//
// This is the float side of the renderer (invariant #1/#4): every value is already `f32`; nothing
// here touches `core`. Keep the attribute locations in lockstep with `mesh.rs`
// (`MeshGpu::vertex_layout` + `MeshPipeline`'s instance layout, `MeshInstance`, `MeshGlobals`).
// The procedural mottle math is **mirrored** by `mesh.rs::surface_mottle` (the Rust-side golden
// reference + tests); keep the two formulae in step.

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
    @location(2) world_pos: vec3<f32>, // model-space → world (or view, for the weapon) position
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
    out.world_pos = world.xyz;
    return out;
}

// --- Procedural value noise (mirrored by mesh.rs::surface_mottle). Transcendental-free (a
// multiply/fract lattice hash + trilinear smoothstep interpolation) so it reads the same on CPU and
// GPU, and so the Rust side can golden-test its range/continuity. Drives a LOW-contrast material
// mottle — it must read as worn material, never as painted-on pattern. ---

fn hash_lattice(c: vec3<f32>) -> f32 {
    var p = fract(c * vec3<f32>(0.1031, 0.1030, 0.0973));
    p += dot(p, p.yxz + 33.33);
    return fract((p.x + p.y) * p.z);
}

fn value_noise(x: vec3<f32>) -> f32 {
    let i = floor(x);
    let f = fract(x);
    let u = f * f * (3.0 - 2.0 * f); // smoothstep weights
    let c000 = hash_lattice(i + vec3<f32>(0.0, 0.0, 0.0));
    let c100 = hash_lattice(i + vec3<f32>(1.0, 0.0, 0.0));
    let c010 = hash_lattice(i + vec3<f32>(0.0, 1.0, 0.0));
    let c110 = hash_lattice(i + vec3<f32>(1.0, 1.0, 0.0));
    let c001 = hash_lattice(i + vec3<f32>(0.0, 0.0, 1.0));
    let c101 = hash_lattice(i + vec3<f32>(1.0, 0.0, 1.0));
    let c011 = hash_lattice(i + vec3<f32>(0.0, 1.0, 1.0));
    let c111 = hash_lattice(i + vec3<f32>(1.0, 1.0, 1.0));
    let x00 = mix(c000, c100, u.x);
    let x10 = mix(c010, c110, u.x);
    let x01 = mix(c001, c101, u.x);
    let x11 = mix(c011, c111, u.x);
    let y0 = mix(x00, x10, u.y);
    let y1 = mix(x01, x11, u.y);
    return mix(y0, y1, u.z);
}

// Two octaves of value noise, remapped to ~[-0.5, 0.5]. Low-frequency body + a finer break-up.
fn surface_mottle(p: vec3<f32>) -> f32 {
    let n1 = value_noise(p * 0.6);
    let n2 = value_noise(p * 1.9 + vec3<f32>(11.3, 5.1, 7.7));
    return (n1 - 0.5) * 0.66 + (n2 - 0.5) * 0.34;
}

@fragment
fn fs_main(in: VertexOut) -> @location(0) vec4<f32> {
    let n = normalize(in.world_normal);

    // --- material: the per-instance tint, enriched. ---
    var base = in.color.rgb;

    // Gentle saturation lift so the muted military palette still reads friend/enemy/faction clearly
    // (luminance-preserving: we pull away from grey, we don't brighten). Bounded low to stay within
    // the dark, low-saturation art direction — and edge-only team read is added by the rim below, so
    // this stays modest enough not to leak intel (#6).
    let lum = dot(base, vec3<f32>(0.299, 0.587, 0.114));
    base = clamp(mix(vec3<f32>(lum), base, 1.16), vec3<f32>(0.0), vec3<f32>(1.0));

    // Procedural mottle + a directional grime/dust cue: undersides accumulate grime (darker), tops
    // catch a faint dust lift. All low-contrast — material, not paint. (+z is world up.)
    let mottle = surface_mottle(in.world_pos);
    let dust = max(n.z, 0.0) * 0.05;
    let grime = max(-n.z, 0.0) * 0.08;
    let tint = base * clamp(1.0 + mottle * 0.16 + dust - grime, 0.55, 1.35);

    // --- lighting: key (warm) + fill (cool) + hemispheric ambient with a cavity term. ---

    // Key light: the warm primary. Surfaces facing into -key are lit.
    let key_l = normalize(g.light_dir.xyz);
    let key = max(dot(n, -key_l), 0.0);
    let key_col = vec3<f32>(1.0, 0.95, 0.86);

    // Fill light: a softer cool light from a fixed high-side direction, lifting shadowed faces
    // without flattening them.
    let fill_l = normalize(vec3<f32>(0.55, 0.35, -0.40));
    let fill = max(dot(n, -fill_l), 0.0);
    let fill_col = vec3<f32>(0.40, 0.50, 0.68);

    // Hemispheric ambient: a cool sky tint from above, a warm dark bounce from below (+z up).
    let h = clamp(n.z * 0.5 + 0.5, 0.0, 1.0);
    let ambient = mix(vec3<f32>(0.12, 0.115, 0.11), vec3<f32>(0.27, 0.32, 0.39), h);
    // Cheap cavity AO: down/side-facing facets sit in their own occlusion, grounding forms.
    let ao = mix(0.66, 1.0, h);

    let lit = ambient * ao + key_col * (key * 0.74) + fill_col * (fill * 0.22);

    // Specular glint: a tight Blinn highlight off the key light, assuming a high 3/4 command
    // camera (view ≈ up-and-slightly-toward-key). Gives sloped armour / weapon facets a hard
    // catch-light that sells them as machined metal rather than matte paint; on cloth greybox it
    // decays to a faint sheen. Gated by key visibility so the shaded side never glints, and kept
    // low-intensity so it sharpens material read without flood-lighting the form (#6 stays fair).
    let view_dir = normalize(vec3<f32>(0.16, -0.22, 0.96));
    let half_v = normalize(-key_l + view_dir);
    let spec = pow(max(dot(n, half_v), 0.0), 26.0) * key;
    let specular = key_col * (spec * 0.28);

    // Identity-tinted rim: a thin backlight on facets turning away from the key. Picks up the
    // instance/team colour so silhouettes read friend/enemy/avatar against the dark ground. Edge-only
    // and low-intensity — it sharpens read without flood-lighting the unit (#6 stays fair).
    let rim_t = pow(1.0 - key, 3.0);
    let rim_hue = normalize(base + vec3<f32>(0.0015));
    let rim_col = mix(vec3<f32>(0.52, 0.58, 0.70), rim_hue, 0.6);
    let rim = rim_col * (rim_t * 0.20);

    // Warm muzzle-flash/emissive add, driven by the per-instance alpha (0 for ordinary tokens).
    let flash = vec3<f32>(1.0, 0.7, 0.35) * in.color.a;

    return vec4<f32>(tint * lit + specular + rim + flash, 1.0);
}
