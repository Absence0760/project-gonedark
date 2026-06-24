//! Renderer — consumes a READ-ONLY core snapshot and draws it (invariant #4).
//!
//! This is the float boundary: Q16.16 sim positions become `f32` HERE, never in `core`.
//! The actual wgpu device/pipeline is wired in Phase 1 build-order step 4; this scaffold
//! fixes the *boundary* — snapshot in, interpolation, fixed→float — so the GPU code drops
//! in behind a stable interface.

use gonedark_core::fixed::Fixed;
use gonedark_core::snapshot::Snapshot;

/// Convert a Q16.16 fixed value to `f32` for the GPU. The ONLY sanctioned fixed→float hop.
#[inline]
pub fn fixed_to_f32(v: Fixed) -> f32 {
    v.to_bits() as f32 / Fixed::SCALE as f32
}

/// A renderable unit instance in float space (render-only).
#[derive(Clone, Copy, Debug, Default)]
pub struct UnitInstance {
    pub x: f32,
    pub y: f32,
    pub embodied: bool,
}

/// The renderer. Holds the prepared instance set; the GPU resources land in step 4.
#[derive(Default)]
pub struct Renderer {
    instances: Vec<UnitInstance>,
    // TODO(phase1-step4): wgpu device/queue/surface, instanced pipeline, camera UBO.
}

impl Renderer {
    pub fn new() -> Self {
        Renderer::default()
    }

    /// Build render instances by interpolating between the previous and current sim
    /// snapshots by `alpha` in `[0,1]` (invariant #4 — interpolation lives here, not in
    /// the sim). Units are matched by index; this scaffold assumes a stable unit set.
    pub fn prepare(&mut self, prev: &Snapshot, curr: &Snapshot, alpha: f32) {
        self.instances.clear();
        let n = prev.units.len().min(curr.units.len());
        for i in 0..n {
            let a = &prev.units[i];
            let b = &curr.units[i];
            let (ax, ay) = (fixed_to_f32(a.pos.x), fixed_to_f32(a.pos.y));
            let (bx, by) = (fixed_to_f32(b.pos.x), fixed_to_f32(b.pos.y));
            self.instances.push(UnitInstance {
                x: ax + (bx - ax) * alpha,
                y: ay + (by - ay) * alpha,
                embodied: b.embodied,
            });
        }
    }

    pub fn instances(&self) -> &[UnitInstance] {
        &self.instances
    }

    /// Submit the prepared frame. Stub until the wgpu backend lands (step 4).
    pub fn draw(&mut self) {
        // TODO(phase1-step4): record + submit a wgpu command buffer for `self.instances`.
    }
}
