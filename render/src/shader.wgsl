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

const FLAG_RING: u32 = 2u;     // a territory control point — drawn as a hollow ring
const FLAG_SELECTED: u32 = 4u; // command-layer selected — bright rim around the quad

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
    // Control point: a hollow ring (round the square quad, punch out the centre). The ring is
    // drawn thicker and with a brighter outer edge than the body fill so an objective reads
    // unmistakably as a ring at a glance over the ground grid — not just a faint outline. The band
    // is [0.55, 1.0] (was [0.6, 1.0]); the outermost slice gets a bright lift so the ring "pops".
    if (in.flags & FLAG_RING) != 0u {
        let radius = length(in.local);
        if radius > 1.0 || radius < 0.55 {
            discard;
        }
        // Brighten the outer 25% of the ring so its edge reads crisply against the grid.
        let edge = smoothstep(0.78, 0.96, radius);
        let ring = mix(in.color, min(in.color + vec3<f32>(0.25, 0.25, 0.3), vec3<f32>(1.0)), edge);
        return vec4<f32>(ring, 1.0);
    }

    // Selection rim (command view): a bright near-white border hugging the outer edge of the
    // quad, so a selected unit reads obviously different at a glance. Drawn first so it wins over
    // the body fill; the health bar (top strip) still overlays it where present. The rim is the
    // outermost ~25% of the quad on either axis (|x| or |y| past the threshold) — thick enough to
    // read on the small command-view unit quads (widened from 0.7 → 0.74 keeps it crisp but visible).
    let RIM: f32 = 0.74;
    if (in.flags & FLAG_SELECTED) != 0u
        && (abs(in.local.x) > RIM || abs(in.local.y) > RIM) {
        return vec4<f32>(0.98, 0.98, 1.0, 1.0); // bright cool-white selection rim
    }

    // Unit / building: body color, with a health bar across the top strip and a thin dark outline
    // so every quad has a crisp edge against the ground grid (an un-selected unit otherwise blends
    // into the lattice at small command-view sizes). The outline is the outermost ~8% of the quad.
    //
    // Color literals here (selection rim above, outline + health fill below) are hand-tuned to read
    // against the faction body palette — keep them in step with the Rust source of truth in `lib.rs`
    // (`faction_color` / `AVATAR_COLOR`); a designer retuning the palette in Rust must mirror it
    // here, since WGSL has no shared constant with the CPU side.
    if in.health >= 0.0 && in.local.y > 0.55 {
        // Map local x in [-1,1] to [0,1]; fill up to `health`.
        let t = in.local.x * 0.5 + 0.5;
        if t <= in.health {
            // Remaining health: green at full, ramping through amber to red as it drains, so a
            // near-dead unit glows red even when the fill is a tiny left-edge sliver — the
            // about-to-die state (embody/retreat decision) is the most legible, not the least.
            let fill = mix(vec3<f32>(0.9, 0.2, 0.15), vec3<f32>(0.2, 0.85, 0.25), in.health);
            return vec4<f32>(fill, 1.0);
        }
        // Lost health: a desaturated charcoal, deliberately off pure red so the empty segment
        // can't be mistaken for the enemy-faction red body at small command-view sizes.
        return vec4<f32>(0.18, 0.18, 0.2, 1.0);
    }

    // Thin dark body outline (outermost edge), so the quad has a defined border over the grid.
    let OUTLINE: f32 = 0.9;
    if abs(in.local.x) > OUTLINE || abs(in.local.y) > OUTLINE {
        return vec4<f32>(in.color * 0.35, 1.0);
    }

    return vec4<f32>(in.color, 1.0);
}
