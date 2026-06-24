//! Command-layer unit selection (the touch-UI depth layer, roadmap Phase 2 / game-design §8).
//!
//! Selection is pure *presentation* state — which of the player's units the next order applies
//! to. It is computed from the command-view pointer (tap to pick the nearest unit; drag a
//! rectangle to band-select several) and never touches sim state (invariant #1) — it only ever
//! produces, downstream in [`crate::command_ui`], `Command`s the sim already understands.
//!
//! The engine does the camera unprojection at the input boundary and hands this layer
//! WORLD-space points + the candidate units' world positions, so the logic here is float-only
//! geometry with no GPU/camera dependency — hence unit-testable.
//!
//! IMPLEMENTATION OWNER: worker 4 (touch selection). Compiling stub: selects nothing, so the
//! command layer keeps its Phase-1 single-`player` behavior until you fill `update` (tap-pick +
//! drag-rectangle band-select) + inline tests. KEEP the two public signatures intact; you own
//! the internal drag state.

use gonedark_core::ecs::Entity;

/// The set of player units the next command targets. Empty = nothing selected (the engine falls
/// back to its legacy single-avatar tap-to-move so existing behavior is preserved).
#[derive(Clone, Debug, Default)]
pub struct Selection {
    /// Currently selected player-unit handles, in stable selection order.
    pub units: Vec<Entity>,
    // worker 4: add drag-rectangle anchor state here (the world-space pointer-down origin).
}

impl Selection {
    pub fn new() -> Self {
        Selection::default()
    }

    /// True when at least one unit is selected.
    pub fn is_empty(&self) -> bool {
        self.units.is_empty()
    }

    /// Fold this frame's command-view pointer activity into the selection.
    ///
    /// - `pointer_world`: the pointer unprojected onto the ground plane this frame, if known.
    /// - `pointer_down` / `pointer_up`: the press / release edges (a drag spans down→up).
    /// - `embodied`: when true the command layer is hidden — selection must not change.
    /// - `candidates`: every selectable player unit as `(handle, world_xy)`.
    ///
    /// Implementation (worker 4): a quick down→up with little movement picks the nearest unit
    /// within a small radius; a drag selects all candidates inside the rectangle. KEEP this
    /// signature; the body is yours.
    pub fn update(
        &mut self,
        _pointer_world: Option<(f32, f32)>,
        _pointer_down: bool,
        _pointer_up: bool,
        _embodied: bool,
        _candidates: &[(Entity, (f32, f32))],
    ) {
    }
}
