//! The embodied alert HUD (invariant #6, game-design §6) — the *only* visual thread back to
//! command while the map is dark: directional pings ("taking fire to the south-east"), never a
//! map reveal. It draws ON TOP of the already-rendered embodied frame (a second pass that LOADs,
//! does not clear), so it is its own tiny screen-space pipeline + shader (`hud.wgsl`), kept
//! separate from the unit pipeline so the two never contend for the same shader/source.
//!
//! Data comes from `core::alerts::AlertChannel` (a presentation derivation, never sim state).
//! The HUD places each recent [`Alert`](gonedark_core::alerts::Alert) by the bearing of its
//! `pos` relative to the avatar's `yaw`, fading older ones out by tick age. Float boundary.
//!
//! IMPLEMENTATION OWNER: worker 2 (alert HUD). This stub is a no-op (draws nothing), so the
//! embodied frame is unchanged until you build it. Create `render/src/hud.wgsl`, build the
//! pipeline in `new`, and draw the directional markers in `render`. KEEP both public signatures
//! intact (the renderer constructs and calls them) — you own everything inside.

use gonedark_core::alerts::AlertChannel;

/// Screen-space directional-alert overlay for the embodied view. Owns its own pipeline + buffers
/// (worker 2 to add the fields); this stub holds none.
pub struct HudRenderer {
    // worker 2: pipeline, vertex/instance buffers, a screen-size uniform, etc.
}

impl HudRenderer {
    /// Build the HUD pipeline against the swapchain `surface_format`. The `device` is borrowed
    /// (D19). STUB: builds nothing.
    pub fn new(_device: &wgpu::Device, _surface_format: wgpu::TextureFormat) -> Self {
        HudRenderer {}
    }

    /// Draw this frame's directional alert markers on top of `view` (a LOAD pass — never clears).
    ///
    /// - `alerts`: the rolling alert channel (most recent last).
    /// - `avatar_world`: the listener/avatar position in world units.
    /// - `yaw`: avatar facing (radians) — markers are placed by bearing relative to this.
    /// - `viewport`: surface size in pixels (for the screen-space projection).
    /// - `tick`: the current sim tick (to fade alerts by age).
    ///
    /// STUB: no-op. KEEP this signature; the body is yours.
    #[allow(clippy::too_many_arguments)]
    pub fn render(
        &mut self,
        _device: &wgpu::Device,
        _queue: &wgpu::Queue,
        _view: &wgpu::TextureView,
        _alerts: &AlertChannel,
        _avatar_world: (f32, f32),
        _yaw: f32,
        _viewport: (u32, u32),
        _tick: u64,
    ) {
    }
}
