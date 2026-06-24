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
//! IMPLEMENTATION OWNER: worker 4 (touch selection).
//!
//! Gesture grammar (all in command-view WORLD space):
//!   press → record an anchor at the pointer-down origin.
//!   release with little movement (< `TAP_SLOP`)  → TAP: select the single nearest candidate
//!       within `PICK_RADIUS` of the release point, or clear if the ground was empty.
//!   release after moving farther                  → DRAG: band-select every candidate inside the
//!       axis-aligned rectangle spanned by anchor→release.
//! Selection order follows candidate (stable index) order so the result is deterministic.

use gonedark_core::ecs::Entity;

/// Below this anchor→release world-distance a gesture reads as a TAP (point pick) rather than a
/// DRAG (band select). 0.6 world units ≈ a fat-finger jitter budget that still feels like a tap.
const TAP_SLOP: f32 = 0.6;

/// A TAP picks the nearest candidate within this world radius of the release point; nothing
/// closer means the tap landed on empty ground and deselects. ~1 unit ≈ a unit's footprint.
const PICK_RADIUS: f32 = 1.0;

/// The set of player units the next command targets. Empty = nothing selected (the engine falls
/// back to its legacy single-avatar tap-to-move so existing behavior is preserved).
#[derive(Clone, Debug, Default)]
pub struct Selection {
    /// Currently selected player-unit handles, in stable selection order.
    pub units: Vec<Entity>,
    /// World-space pointer-down origin of the gesture in progress, `None` between gestures.
    /// Set on the press edge, consumed (and cleared) on release.
    anchor: Option<(f32, f32)>,
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
    pub fn update(
        &mut self,
        pointer_world: Option<(f32, f32)>,
        pointer_down: bool,
        pointer_up: bool,
        embodied: bool,
        candidates: &[(Entity, (f32, f32))],
    ) {
        // Command layer hidden while embodied (invariant #5): leave selection untouched. We also
        // drop any in-flight anchor so a gesture interrupted by embodiment can't resolve later.
        if embodied {
            self.anchor = None;
            return;
        }

        // Press edge: begin a gesture by recording where the pointer went down. We only arm a new
        // gesture when there isn't one already in flight and we actually know the world point.
        if pointer_down && self.anchor.is_none() {
            if let Some(p) = pointer_world {
                self.anchor = Some(p);
            }
        }

        // Release edge: resolve the gesture (if one was armed) into a new selection.
        if pointer_up {
            let anchor = self.anchor.take();
            // A release with no known world point (pointer left the window mid-drag) just cancels
            // the gesture — anchor already taken above, nothing else to do.
            if let (Some(anchor), Some(release)) = (anchor, pointer_world) {
                let (ax, ay) = anchor;
                let (rx, ry) = release;
                let dx = rx - ax;
                let dy = ry - ay;
                let moved = (dx * dx + dy * dy).sqrt();

                if moved < TAP_SLOP {
                    // TAP: pick the single nearest candidate within PICK_RADIUS of the release.
                    self.units.clear();
                    let mut best: Option<(Entity, f32)> = None;
                    for &(entity, (cx, cy)) in candidates {
                        let ex = cx - rx;
                        let ey = cy - ry;
                        let dist = (ex * ex + ey * ey).sqrt();
                        if dist <= PICK_RADIUS && best.is_none_or(|(_, b)| dist < b) {
                            best = Some((entity, dist));
                        }
                    }
                    // None close enough → tap on empty ground clears the selection (units already
                    // cleared above); otherwise select exactly the nearest one.
                    if let Some((entity, _)) = best {
                        self.units.push(entity);
                    }
                } else {
                    // DRAG: band-select every candidate inside the axis-aligned anchor→release
                    // rectangle. Iterate candidates in their stable (index) order for determinism.
                    let (min_x, max_x) = (ax.min(rx), ax.max(rx));
                    let (min_y, max_y) = (ay.min(ry), ay.max(ry));
                    self.units.clear();
                    for &(entity, (cx, cy)) in candidates {
                        if cx >= min_x && cx <= max_x && cy >= min_y && cy <= max_y {
                            self.units.push(entity);
                        }
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ent(index: u32) -> Entity {
        Entity {
            index,
            generation: 0,
        }
    }

    /// Run a press frame at `down`, then a release frame at `up`, against `candidates`.
    fn tap(sel: &mut Selection, down: (f32, f32), up: (f32, f32), candidates: &[(Entity, (f32, f32))]) {
        sel.update(Some(down), true, false, false, candidates);
        sel.update(Some(up), false, true, false, candidates);
    }

    #[test]
    fn tap_near_candidate_selects_exactly_that_one() {
        let a = ent(0);
        let b = ent(1);
        let candidates = vec![(a, (0.0, 0.0)), (b, (10.0, 10.0))];
        let mut sel = Selection::new();
        // Tap right on top of `a` (no movement → TAP).
        tap(&mut sel, (0.1, 0.1), (0.1, 0.1), &candidates);
        assert_eq!(sel.units, vec![a]);
        assert!(!sel.is_empty());
    }

    #[test]
    fn tap_picks_nearest_when_two_are_in_radius() {
        let a = ent(0);
        let b = ent(1);
        // Both within PICK_RADIUS of (0,0); `b` is closer.
        let candidates = vec![(a, (0.8, 0.0)), (b, (0.3, 0.0))];
        let mut sel = Selection::new();
        tap(&mut sel, (0.0, 0.0), (0.0, 0.0), &candidates);
        assert_eq!(sel.units, vec![b]);
    }

    #[test]
    fn tap_on_empty_ground_clears_selection() {
        let a = ent(0);
        let candidates = vec![(a, (0.0, 0.0))];
        let mut sel = Selection::new();
        // First select `a`.
        tap(&mut sel, (0.0, 0.0), (0.0, 0.0), &candidates);
        assert_eq!(sel.units, vec![a]);
        // Now tap far from any candidate (> PICK_RADIUS) → clears.
        tap(&mut sel, (50.0, 50.0), (50.0, 50.0), &candidates);
        assert!(sel.is_empty());
    }

    #[test]
    fn drag_rectangle_selects_all_inside_and_excludes_outside() {
        let a = ent(0);
        let b = ent(1);
        let c = ent(2);
        let outside = ent(3);
        let candidates = vec![
            (a, (1.0, 1.0)),
            (b, (2.0, 3.0)),
            (c, (4.0, 4.0)),
            (outside, (20.0, 20.0)),
        ];
        let mut sel = Selection::new();
        // Drag from (0,0) to (5,5): well past TAP_SLOP, encloses a, b, c — not `outside`.
        sel.update(Some((0.0, 0.0)), true, false, false, &candidates);
        sel.update(Some((5.0, 5.0)), false, true, false, &candidates);
        assert_eq!(sel.units, vec![a, b, c]);
    }

    #[test]
    fn drag_preserves_candidate_order() {
        // Candidates given out of spatial order; band-select must keep stable input order.
        let e2 = ent(2);
        let e0 = ent(0);
        let e1 = ent(1);
        let candidates = vec![(e2, (3.0, 1.0)), (e0, (1.0, 1.0)), (e1, (2.0, 1.0))];
        let mut sel = Selection::new();
        sel.update(Some((0.0, 0.0)), true, false, false, &candidates);
        sel.update(Some((5.0, 5.0)), false, true, false, &candidates);
        assert_eq!(sel.units, vec![e2, e0, e1]);
    }

    #[test]
    fn embodied_is_a_no_op() {
        let a = ent(0);
        let b = ent(1);
        let candidates = vec![(a, (0.0, 0.0)), (b, (1.0, 1.0))];
        let mut sel = Selection::new();
        // Pre-seed a selection while NOT embodied.
        tap(&mut sel, (0.0, 0.0), (0.0, 0.0), &candidates);
        assert_eq!(sel.units, vec![a]);
        // A tap on `b` while embodied must leave the selection unchanged.
        sel.update(Some((1.0, 1.0)), true, false, true, &candidates);
        sel.update(Some((1.0, 1.0)), false, true, true, &candidates);
        assert_eq!(sel.units, vec![a]);
        // A drag while embodied is likewise a no-op.
        sel.update(Some((0.0, 0.0)), true, false, true, &candidates);
        sel.update(Some((5.0, 5.0)), false, true, true, &candidates);
        assert_eq!(sel.units, vec![a]);
    }

    #[test]
    fn missing_pointer_world_on_release_cancels_without_panic() {
        let a = ent(0);
        let candidates = vec![(a, (0.0, 0.0)), (ent(1), (1.0, 1.0))];
        let mut sel = Selection::new();
        // Select `a` first.
        tap(&mut sel, (0.0, 0.0), (0.0, 0.0), &candidates);
        assert_eq!(sel.units, vec![a]);
        // Press, then release with the pointer outside the window (None) → gesture cancels,
        // selection unchanged, no panic.
        sel.update(Some((3.0, 3.0)), true, false, false, &candidates);
        sel.update(None, false, true, false, &candidates);
        assert_eq!(sel.units, vec![a]);
    }

    #[test]
    fn is_empty_reflects_state() {
        let mut sel = Selection::new();
        assert!(sel.is_empty());
        let a = ent(0);
        let candidates = vec![(a, (0.0, 0.0))];
        tap(&mut sel, (0.0, 0.0), (0.0, 0.0), &candidates);
        assert!(!sel.is_empty());
        tap(&mut sel, (99.0, 99.0), (99.0, 99.0), &candidates);
        assert!(sel.is_empty());
    }
}
