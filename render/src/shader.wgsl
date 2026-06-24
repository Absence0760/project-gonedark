// Instanced renderable shader (Phase 2).
//
// One quad (two triangles, corners in [-1, 1]^2) is drawn per `UnitInstance`. The vertex
// shader scales the quad by the instance's per-entity half-extent, offsets it to the world
// position, and transforms it by the camera view-projection. The fragment shader colors the
// quad in the instance color, draws a health bar across the top strip when `health >= 0`, and
// renders control points (FLAG_RING) as a hollow ring.
//
// This is the float side of invariant #4 — every number here is already an `f32`; the
// Q16.16 → f32 hop happened on the CPU in `render::fixed_to_f32`, never in `core`.

const FLAG_RING: u32 = 2u; // a territory control point — drawn as a hollow ring

// Column-major 4x4 view-projection, uploaded by the wiring layer (glam Mat4).
struct Camera {
    view_proj: mat4x4<f32>,
};

@group(0) @binding(0)
var<uniform> camera: Camera;

struct VertexOut {
    @builtin(position) clip_pos: vec4<f32>,
    @location(0) color: vec3<f32>,
    @location(1) local: vec2<f32>,        // the quad corner in [-1, 1] (interpolated)
    @location(2) health: f32,             // [0,1], or negative for "no bar"
    @location(3) @interpolate(flat) flags: u32,
};

// Per-vertex: the quad corner in local space. Per-instance: world position, half-extent,
// color, health fraction, and flag bits — matching the CPU-side `repr(C)` `UnitInstance`.
@vertex
fn vs_main(
    @location(0) corner: vec2<f32>,
    @location(1) inst_pos: vec2<f32>,
    @location(2) half_extent: f32,
    @location(3) color: vec3<f32>,
    @location(4) health: f32,
    @location(5) flags: u32,
) -> VertexOut {
    let world = vec2<f32>(
        inst_pos.x + corner.x * half_extent,
        inst_pos.y + corner.y * half_extent,
    );

    var out: VertexOut;
    // Renderables live on the ground plane (z = 0); the camera supplies the projection.
    out.clip_pos = camera.view_proj * vec4<f32>(world.x, world.y, 0.0, 1.0);
    out.color = color;
    out.local = corner;
    out.health = health;
    out.flags = flags;
    return out;
}

@fragment
fn fs_main(in: VertexOut) -> @location(0) vec4<f32> {
    // Control point: a hollow ring (round the square quad, punch out the centre).
    if (in.flags & FLAG_RING) != 0u {
        let radius = length(in.local);
        if radius > 1.0 || radius < 0.6 {
            discard;
        }
        return vec4<f32>(in.color, 1.0);
    }

    // Unit / building: body color, with a health bar across the top strip.
    if in.health >= 0.0 && in.local.y > 0.55 {
        // Map local x in [-1,1] to [0,1]; fill up to `health`.
        let t = in.local.x * 0.5 + 0.5;
        if t <= in.health {
            return vec4<f32>(0.2, 0.85, 0.25, 1.0); // remaining health — green
        }
        return vec4<f32>(0.35, 0.05, 0.05, 1.0); // lost health — dark red
    }

    return vec4<f32>(in.color, 1.0);
}
