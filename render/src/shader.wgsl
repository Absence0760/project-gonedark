// Instanced unit renderer (Phase 1 build-order step 4).
//
// One small unit quad (two triangles, corners in [-0.5, 0.5]^2) is drawn once per
// `UnitInstance`. The vertex shader offsets the quad to the instance's world position and
// transforms it by the camera view-projection; the fragment shader colors each unit by
// whether it is the embodied avatar.
//
// This is the float side of invariant #4 — every number here is already an `f32`; the
// Q16.16 → f32 hop happened on the CPU in `render::fixed_to_f32`, never in `core`.

// Column-major 4x4 view-projection, uploaded by the wiring layer (glam Mat4).
struct Camera {
    view_proj: mat4x4<f32>,
};

@group(0) @binding(0)
var<uniform> camera: Camera;

struct VertexOut {
    @builtin(position) clip_pos: vec4<f32>,
    @location(0) color: vec3<f32>,
};

// Per-vertex: the quad corner in local space (the six corners of the two-triangle quad).
// Per-instance: world position (x, y) and an `embodied` flag (u32, 0 or 1) — matching the
// CPU-side `repr(C)` `UnitInstance`. Color is chosen here from the flag rather than baked
// per-instance on the CPU, so the same instance buffer serves both lit and dark frames
// (the "world goes dark" filtering is done CPU-side in `Renderer::render`).
@vertex
fn vs_main(
    @location(0) corner: vec2<f32>,
    @location(1) inst_pos: vec2<f32>,
    @location(2) embodied: u32,
) -> VertexOut {
    // Half-extent of a drawn unit in world units. Render-only cosmetic scale.
    let half_size: f32 = 0.5;
    let world = vec2<f32>(
        inst_pos.x + corner.x * half_size,
        inst_pos.y + corner.y * half_size,
    );

    var out: VertexOut;
    // Units live on the ground plane (z = 0); the camera matrix supplies the projection.
    out.clip_pos = camera.view_proj * vec4<f32>(world.x, world.y, 0.0, 1.0);

    // Embodied avatar: a bright, distinct hue. Ordinary units: neutral grey-blue.
    if embodied != 0u {
        out.color = vec3<f32>(1.0, 0.85, 0.2); // warm amber — the avatar you possess
    } else {
        out.color = vec3<f32>(0.45, 0.5, 0.6); // neutral command-view unit
    }
    return out;
}

@fragment
fn fs_main(in: VertexOut) -> @location(0) vec4<f32> {
    return vec4<f32>(in.color, 1.0);
}
