//! Small pure host-side helpers shared across the shell: the title build stamp, the build channel,
//! and the egui-pointer→backdrop-NDC mapping. All unit-tested with no window.

/// Format the title screen's build stamp — e.g. `build dev · v0.0.0`.
pub(crate) fn build_stamp(channel: &str, version: &str) -> String {
    format!(
        "build {} · v{}",
        channel.trim().to_ascii_lowercase(),
        version.trim()
    )
}

/// The build channel from cargo's debug-assertions flag: a debug build is "dev", a release "release".
pub(crate) fn build_channel(debug_assertions: bool) -> &'static str {
    if debug_assertions {
        "dev"
    } else {
        "release"
    }
}

/// Convert an egui pointer position (logical points, origin top-left, y down) into the title
/// backdrop's NDC ([-1, 1] on both axes, origin centre, **y up**) given the surface size in the same
/// logical points. Pure arithmetic — extracted from the [`EguiShell`](super::egui_shell) glue exactly
/// so the cursor mapping the 3D backdrop reacts to is unit-tested (the wgpu compositing around it is
/// exempt). This is host presentation math, not sim — the f32s here never touch `core` (invariant #1
/// is about the sim, not the renderer's float boundary).
pub(crate) fn pointer_to_ndc(pos: [f32; 2], size_points: [f32; 2]) -> [f32; 2] {
    // Guard a zero/negative extent (a not-yet-sized surface) so we never divide by zero.
    let w = if size_points[0] > 0.0 {
        size_points[0]
    } else {
        1.0
    };
    let h = if size_points[1] > 0.0 {
        size_points[1]
    } else {
        1.0
    };
    [(pos[0] / w) * 2.0 - 1.0, 1.0 - (pos[1] / h) * 2.0]
}

/// The inverse of [`pointer_to_ndc`]: NDC (`[-1,1]²`, origin centre, y up) → egui logical points
/// (origin top-left, y down). The operations-map overlay projects battle anchors to NDC
/// (`project_pin`) and paints egui shapes at them — this is the crossing back. Pure arithmetic,
/// unit-tested (round-trips with [`pointer_to_ndc`]); render-side floats only, never `core`
/// (invariant #1).
pub(crate) fn ndc_to_pointer(ndc: [f32; 2], size_points: [f32; 2]) -> [f32; 2] {
    [
        (ndc[0] + 1.0) * 0.5 * size_points[0],
        (1.0 - ndc[1]) * 0.5 * size_points[1],
    ]
}
