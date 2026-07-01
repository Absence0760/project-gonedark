// Radial command-menu shader — the on-screen wedge ring a held long-press opens over the command
// vocabulary (engine::command_ui's radial preview). Drawn as a screen-space LOAD pass on top of the
// already-rendered command frame: one axis-aligned rectangle per quad, positioned and sized in NDC,
// colored + alpha-blended over the frame (no clear). Structurally identical to overlay.wgsl, kept
// separate so the radial pass never contends with the overlay/HUD pass for a shader source.
//
// Float side of invariant #4 — every number here is already an f32. The menu carries NO world
// position and no fog data (it is a command-layer affordance, NDC chrome, not intel — invariant #6
// holds, and the host only ever draws it in the command view, never over the dark embodied frame).

struct VertexOut {
    @builtin(position) clip_pos: vec4<f32>,
    @location(0) color: vec4<f32>,
};

// Per-vertex: a unit-quad corner in [-1, 1]^2. Per-instance: the rect center (NDC), its NDC
// half-extent (vec2), RGB color, and alpha — matching the CPU-side `repr(C)` `RadialInstance`.
@vertex
fn vs_main(
    @location(0) corner: vec2<f32>,
    @location(1) center: vec2<f32>,
    @location(2) hext: vec2<f32>,
    @location(3) color: vec3<f32>,
    @location(4) alpha: f32,
) -> VertexOut {
    var out: VertexOut;
    let ndc = vec2<f32>(
        center.x + corner.x * hext.x,
        center.y + corner.y * hext.y,
    );
    out.clip_pos = vec4<f32>(ndc.x, ndc.y, 0.0, 1.0);
    out.color = vec4<f32>(color, alpha);
    return out;
}

@fragment
fn fs_main(in: VertexOut) -> @location(0) vec4<f32> {
    return in.color;
}
