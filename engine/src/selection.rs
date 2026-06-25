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
/// This is the world-space *floor*: see [`GestureScale`] for the zoom-aware effective value.
const TAP_SLOP: f32 = 0.6;

/// A TAP picks the nearest candidate within this world radius of the release point; nothing
/// closer means the tap landed on empty ground and deselects. ~1 unit ≈ a unit's footprint.
/// This is the world-space *floor*: see [`GestureScale`] for the zoom-aware effective value.
const PICK_RADIUS: f32 = 1.0;

/// Screen-pixel jitter budget below which a gesture still reads as a TAP. ~22 px is a touch's
/// natural wobble; combined with the ~44 px hit-target floor it keeps taps feeling like taps
/// regardless of zoom. Used as `screen_px * world_per_px` in [`GestureScale::tap_slop`].
const TAP_SLOP_PX: f32 = 22.0;

/// Screen-pixel pick radius around a tap. ~44 px ≈ the platform minimum touch target, so a tap
/// resolves a unit even when the world is zoomed way out. Used in [`GestureScale::pick_radius`].
const PICK_RADIUS_PX: f32 = 44.0;

/// The zoom context for a gesture: how many WORLD units one screen pixel spans this frame. The
/// engine owns the camera/unproject at the input boundary, so it can supply this cheaply (e.g.
/// the world width of the viewport ÷ its pixel width). Pure geometry — no GPU/camera dependency.
///
/// The effective gesture thresholds are `max(world floor, screen_px * world_per_px)`: zoomed out
/// (large `world_per_px`) the px term dominates so a fixed-pixel finger jitter still reads as a
/// tap and the pick radius covers a usable hit target; zoomed in (small `world_per_px`) the world
/// floor dominates so a real drag isn't swallowed as a tap.
#[derive(Clone, Copy, Debug)]
pub struct GestureScale {
    /// World units spanned by one screen pixel this frame. Must be finite and > 0.
    world_per_px: f32,
}

impl GestureScale {
    /// Build a scale from world-units-per-pixel. Non-finite or non-positive input collapses to
    /// the world-floor-only behavior (the px term contributes nothing), so callers can pass an
    /// unknown/degenerate camera without changing the byte-identical legacy feel.
    pub fn new(world_per_px: f32) -> Self {
        let world_per_px = if world_per_px.is_finite() && world_per_px > 0.0 {
            world_per_px
        } else {
            0.0
        };
        GestureScale { world_per_px }
    }

    /// A scale that reproduces the legacy fixed-world-unit thresholds exactly: the px term is zero,
    /// so the effective slop/pick are the world floors `TAP_SLOP` / `PICK_RADIUS`.
    pub fn world_floor() -> Self {
        GestureScale { world_per_px: 0.0 }
    }

    /// Effective TAP/DRAG threshold in world units for this zoom.
    fn tap_slop(self) -> f32 {
        TAP_SLOP.max(TAP_SLOP_PX * self.world_per_px)
    }

    /// Effective tap pick radius in world units for this zoom.
    fn pick_radius(self) -> f32 {
        PICK_RADIUS.max(PICK_RADIUS_PX * self.world_per_px)
    }
}

impl Default for GestureScale {
    fn default() -> Self {
        GestureScale::world_floor()
    }
}

/// Pick the single nearest candidate within `pick_radius` of `release`, in candidate order on
/// ties (first encountered wins via strict `<`). Pure geometry — the tap resolution rule. Returns
/// `None` when the tap landed on empty ground.
fn nearest_within(
    release: (f32, f32),
    pick_radius: f32,
    candidates: &[(Entity, (f32, f32))],
) -> Option<Entity> {
    let (rx, ry) = release;
    let mut best: Option<(Entity, f32)> = None;
    for &(entity, (cx, cy)) in candidates {
        let ex = cx - rx;
        let ey = cy - ry;
        let dist = (ex * ex + ey * ey).sqrt();
        if dist <= pick_radius && best.is_none_or(|(_, b)| dist < b) {
            best = Some((entity, dist));
        }
    }
    best.map(|(entity, _)| entity)
}

/// Collect every candidate inside the axis-aligned `anchor`→`release` rectangle, in candidate
/// (stable index) order. Pure geometry — the band-select rule.
fn within_rect(
    anchor: (f32, f32),
    release: (f32, f32),
    candidates: &[(Entity, (f32, f32))],
) -> Vec<Entity> {
    let (ax, ay) = anchor;
    let (rx, ry) = release;
    let (min_x, max_x) = (ax.min(rx), ax.max(rx));
    let (min_y, max_y) = (ay.min(ry), ay.max(ry));
    let mut out = Vec::new();
    for &(entity, (cx, cy)) in candidates {
        if cx >= min_x && cx <= max_x && cy >= min_y && cy <= max_y {
            out.push(entity);
        }
    }
    out
}

/// Union `incoming` into `existing` in place, preserving `existing`'s order and appending only the
/// entities not already present (dedup by `Entity`), `incoming` keeping its candidate order. Pure
/// — the additive-drag merge rule. O(n·m) is fine: selections are tens of units, not thousands.
fn union_into(existing: &mut Vec<Entity>, incoming: &[Entity]) {
    for &e in incoming {
        if !existing.contains(&e) {
            existing.push(e);
        }
    }
}

/// Toggle `entity`'s membership in `existing`: remove it if present (preserving the order of the
/// rest), else append it. Pure — the additive-tap rule.
fn toggle(existing: &mut Vec<Entity>, entity: Entity) {
    if let Some(i) = existing.iter().position(|&e| e == entity) {
        existing.remove(i);
    } else {
        existing.push(entity);
    }
}

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

    /// The world-space anchor of the band-drag currently in flight, or `None` between gestures. Set
    /// on the press edge and cleared on release — the host pairs it with the live pointer to draw the
    /// selection marquee while dragging. Presentation only; reading it never changes the selection.
    pub fn drag_anchor(&self) -> Option<(f32, f32)> {
        self.anchor
    }

    /// Fold pointer activity into the selection with the full gesture grammar.
    ///
    /// The single command-view selection entry point. Two presentation knobs the PAL carries ride
    /// alongside the pointer edges:
    ///
    /// - `additive`: when true (a held modifier on the down/up edges) the gesture *grows* the set
    ///   instead of replacing it — a TAP toggles the nearest candidate's membership (in then out
    ///   on a re-tap) and a DRAG unions its rectangle into the existing set (dedup by `Entity`,
    ///   preserving stable candidate order). When false the result is the legacy clear-then-select.
    /// - `scale`: the zoom context (world-units-per-pixel) used to derive the effective TAP_SLOP /
    ///   PICK_RADIUS so the gesture feel is stable across zoom (see [`GestureScale`]).
    ///
    /// Still pure presentation geometry: no sim mutation, no GPU dependency.
    #[allow(clippy::too_many_arguments)]
    pub fn update_ex(
        &mut self,
        pointer_world: Option<(f32, f32)>,
        pointer_down: bool,
        pointer_up: bool,
        embodied: bool,
        additive: bool,
        scale: GestureScale,
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

                // Zoom-aware effective thresholds (floor in world units, grown by the px term when
                // zoomed out). At `world_floor()` these are exactly TAP_SLOP / PICK_RADIUS.
                let tap_slop = scale.tap_slop();
                let pick_radius = scale.pick_radius();

                if moved < tap_slop {
                    // TAP: the single nearest candidate within the pick radius of the release.
                    let hit = nearest_within(release, pick_radius, candidates);
                    if additive {
                        // Additive tap toggles that unit's membership; a tap on empty ground (no
                        // hit) leaves the existing set alone rather than clearing it.
                        if let Some(entity) = hit {
                            toggle(&mut self.units, entity);
                        }
                    } else {
                        // Legacy: clear, then select the nearest (or nothing on empty ground).
                        self.units.clear();
                        if let Some(entity) = hit {
                            self.units.push(entity);
                        }
                    }
                } else {
                    // DRAG: band-select inside the axis-aligned anchor→release rectangle, in
                    // stable candidate order.
                    let rect = within_rect((ax, ay), (rx, ry), candidates);
                    if additive {
                        // Additive drag unions the rectangle into the existing set (dedup, order
                        // preserved).
                        union_into(&mut self.units, &rect);
                    } else {
                        // Legacy: replace the set with the rectangle's contents.
                        self.units = rect;
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
    fn tap(
        sel: &mut Selection,
        down: (f32, f32),
        up: (f32, f32),
        candidates: &[(Entity, (f32, f32))],
    ) {
        sel.update_ex(
            Some(down),
            true,
            false,
            false,
            false,
            GestureScale::world_floor(),
            candidates,
        );
        sel.update_ex(
            Some(up),
            false,
            true,
            false,
            false,
            GestureScale::world_floor(),
            candidates,
        );
    }

    /// A press→release gesture with explicit `additive` and `scale`.
    fn gesture_ex(
        sel: &mut Selection,
        down: (f32, f32),
        up: (f32, f32),
        additive: bool,
        scale: GestureScale,
        candidates: &[(Entity, (f32, f32))],
    ) {
        sel.update_ex(Some(down), true, false, false, additive, scale, candidates);
        sel.update_ex(Some(up), false, true, false, additive, scale, candidates);
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
        sel.update_ex(
            Some((0.0, 0.0)),
            true,
            false,
            false,
            false,
            GestureScale::world_floor(),
            &candidates,
        );
        sel.update_ex(
            Some((5.0, 5.0)),
            false,
            true,
            false,
            false,
            GestureScale::world_floor(),
            &candidates,
        );
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
        sel.update_ex(
            Some((0.0, 0.0)),
            true,
            false,
            false,
            false,
            GestureScale::world_floor(),
            &candidates,
        );
        sel.update_ex(
            Some((5.0, 5.0)),
            false,
            true,
            false,
            false,
            GestureScale::world_floor(),
            &candidates,
        );
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
        sel.update_ex(
            Some((1.0, 1.0)),
            true,
            false,
            true,
            false,
            GestureScale::world_floor(),
            &candidates,
        );
        sel.update_ex(
            Some((1.0, 1.0)),
            false,
            true,
            true,
            false,
            GestureScale::world_floor(),
            &candidates,
        );
        assert_eq!(sel.units, vec![a]);
        // A drag while embodied is likewise a no-op.
        sel.update_ex(
            Some((0.0, 0.0)),
            true,
            false,
            true,
            false,
            GestureScale::world_floor(),
            &candidates,
        );
        sel.update_ex(
            Some((5.0, 5.0)),
            false,
            true,
            true,
            false,
            GestureScale::world_floor(),
            &candidates,
        );
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
        sel.update_ex(
            Some((3.0, 3.0)),
            true,
            false,
            false,
            false,
            GestureScale::world_floor(),
            &candidates,
        );
        sel.update_ex(
            None,
            false,
            true,
            false,
            false,
            GestureScale::world_floor(),
            &candidates,
        );
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

    // ----- additive selection -----

    #[test]
    fn additive_tap_toggles_a_unit_in_then_out() {
        let a = ent(0);
        let b = ent(1);
        let candidates = vec![(a, (0.0, 0.0)), (b, (10.0, 10.0))];
        let mut sel = Selection::new();
        let s = GestureScale::world_floor();
        // First additive tap on `a` → adds it.
        gesture_ex(&mut sel, (0.0, 0.0), (0.0, 0.0), true, s, &candidates);
        assert_eq!(sel.units, vec![a]);
        // Additive tap on `b` → adds it without dropping `a`.
        gesture_ex(&mut sel, (10.0, 10.0), (10.0, 10.0), true, s, &candidates);
        assert_eq!(sel.units, vec![a, b]);
        // Re-tap `a` additively → toggles it back out, leaving `b` (order preserved).
        gesture_ex(&mut sel, (0.0, 0.0), (0.0, 0.0), true, s, &candidates);
        assert_eq!(sel.units, vec![b]);
    }

    #[test]
    fn additive_tap_on_empty_ground_keeps_selection() {
        let a = ent(0);
        let candidates = vec![(a, (0.0, 0.0))];
        let mut sel = Selection::new();
        let s = GestureScale::world_floor();
        gesture_ex(&mut sel, (0.0, 0.0), (0.0, 0.0), true, s, &candidates);
        assert_eq!(sel.units, vec![a]);
        // Additive tap far from any candidate must NOT clear (unlike the non-additive path).
        gesture_ex(&mut sel, (50.0, 50.0), (50.0, 50.0), true, s, &candidates);
        assert_eq!(sel.units, vec![a]);
    }

    #[test]
    fn additive_drag_unions_without_duplicates_and_keeps_order() {
        let a = ent(0);
        let b = ent(1);
        let c = ent(2);
        // Candidates given out of index order so we can prove stable candidate order survives.
        // c sits at (8,8) — outside the first box, inside the second.
        let candidates = vec![(c, (8.0, 8.0)), (a, (1.0, 1.0)), (b, (2.0, 3.0))];
        let mut sel = Selection::new();
        let s = GestureScale::world_floor();
        // First (non-additive) drag over (0,0)-(3,3) selects a, b (candidate order); c is outside.
        gesture_ex(&mut sel, (0.0, 0.0), (3.0, 3.0), false, s, &candidates);
        assert_eq!(sel.units, vec![a, b]);
        // Additive drag over (0,0)-(10,10) covers a, b (already in) and c → c appended, no dups,
        // existing order preserved.
        gesture_ex(&mut sel, (0.0, 0.0), (10.0, 10.0), true, s, &candidates);
        assert_eq!(sel.units, vec![a, b, c]);
    }

    #[test]
    fn non_additive_path_unchanged_via_update_ex() {
        // update_ex with additive=false + world_floor must match the legacy clear-then-select.
        let a = ent(0);
        let b = ent(1);
        let candidates = vec![(a, (0.0, 0.0)), (b, (10.0, 10.0))];
        let mut sel = Selection::new();
        let s = GestureScale::world_floor();
        gesture_ex(&mut sel, (0.0, 0.0), (0.0, 0.0), false, s, &candidates);
        assert_eq!(sel.units, vec![a]);
        // Non-additive tap on `b` replaces (does not add).
        gesture_ex(&mut sel, (10.0, 10.0), (10.0, 10.0), false, s, &candidates);
        assert_eq!(sel.units, vec![b]);
        // Non-additive tap on empty ground clears.
        gesture_ex(&mut sel, (50.0, 50.0), (50.0, 50.0), false, s, &candidates);
        assert!(sel.is_empty());
    }

    // ----- zoom-stable thresholds -----

    #[test]
    fn fixed_pixel_jitter_reads_as_tap_when_zoomed_out() {
        // Zoomed out: 1 px = 0.5 world units. A 20 px finger jitter is 10 world units — far past
        // the 0.6 world floor, yet under the 22 px (= 11 world unit) px-scaled slop → still a TAP.
        let a = ent(0);
        let candidates = vec![(a, (0.0, 0.0))];
        let mut sel = Selection::new();
        let zoomed_out = GestureScale::new(0.5);
        // Down at origin, up 10 world units away (= 20 px). Tap picks `a` near the down origin
        // because pick radius is also px-scaled (44 px = 22 world units >> 10).
        gesture_ex(
            &mut sel,
            (0.0, 0.0),
            (10.0, 0.0),
            false,
            zoomed_out,
            &candidates,
        );
        assert_eq!(sel.units, vec![a]);
    }

    #[test]
    fn same_world_delta_reads_as_drag_when_zoomed_in() {
        // Zoomed in: 1 px = 0.01 world units. The px-scaled slop is 22*0.01 = 0.22 world units,
        // below the 0.6 world floor, so the floor governs: a 10 world-unit move is a clear DRAG.
        let a = ent(0);
        let b = ent(1);
        let candidates = vec![(a, (1.0, 0.0)), (b, (20.0, 0.0))];
        let mut sel = Selection::new();
        let zoomed_in = GestureScale::new(0.01);
        // Drag from (0,0) to (10,0): a 10 world-unit move → DRAG, band-selects `a` (in the box),
        // not `b` (outside). If it had misread as a tap it would pick the single nearest instead.
        gesture_ex(
            &mut sel,
            (0.0, 0.0),
            (10.0, 0.0),
            false,
            zoomed_in,
            &candidates,
        );
        assert_eq!(sel.units, vec![a]);
    }

    #[test]
    fn effective_pick_radius_grows_when_zoomed_out() {
        // A unit 5 world units from the tap is outside the 1.0 world floor at floor zoom, but the
        // px-scaled radius (44 px) at world_per_px = 0.5 is 22 world units → it gets picked.
        let a = ent(0);
        let candidates = vec![(a, (5.0, 0.0))];

        // Floor zoom: pick radius is just 1.0 world unit → the tap finds nothing, clears.
        let mut sel_floor = Selection::new();
        gesture_ex(
            &mut sel_floor,
            (0.0, 0.0),
            (0.0, 0.0),
            false,
            GestureScale::world_floor(),
            &candidates,
        );
        assert!(sel_floor.is_empty());

        // Zoomed out: pick radius is 22 world units → the same tap resolves `a`.
        let mut sel_out = Selection::new();
        gesture_ex(
            &mut sel_out,
            (0.0, 0.0),
            (0.0, 0.0),
            false,
            GestureScale::new(0.5),
            &candidates,
        );
        assert_eq!(sel_out.units, vec![a]);
    }

    #[test]
    fn gesture_scale_clamps_degenerate_input_to_world_floor() {
        // Non-finite / non-positive world_per_px must collapse to the world floor (px term zero),
        // so a degenerate camera can't blow up the thresholds.
        for bad in [0.0, -1.0, f32::NAN, f32::INFINITY] {
            let s = GestureScale::new(bad);
            assert_eq!(s.tap_slop(), TAP_SLOP);
            assert_eq!(s.pick_radius(), PICK_RADIUS);
        }
    }

    // ----- pure free-fn geometry -----

    #[test]
    fn nearest_within_picks_closest_and_respects_radius() {
        let a = ent(0);
        let b = ent(1);
        let cands = vec![(a, (0.8, 0.0)), (b, (0.3, 0.0))];
        assert_eq!(nearest_within((0.0, 0.0), 1.0, &cands), Some(b));
        // Nothing within a tiny radius.
        assert_eq!(nearest_within((0.0, 0.0), 0.1, &cands), None);
    }

    #[test]
    fn union_into_dedups_and_appends_in_order() {
        let a = ent(0);
        let b = ent(1);
        let c = ent(2);
        let mut existing = vec![a, b];
        union_into(&mut existing, &[b, c, a]);
        assert_eq!(existing, vec![a, b, c]);
    }

    #[test]
    fn toggle_adds_then_removes() {
        let a = ent(0);
        let b = ent(1);
        let mut s = vec![a];
        toggle(&mut s, b);
        assert_eq!(s, vec![a, b]);
        toggle(&mut s, a);
        assert_eq!(s, vec![b]);
    }

    #[test]
    fn drag_anchor_tracks_the_in_flight_gesture() {
        let a = ent(0);
        let candidates = vec![(a, (0.0, 0.0))];
        let floor = GestureScale::world_floor();
        let mut sel = Selection::new();
        assert_eq!(sel.drag_anchor(), None, "no gesture in flight");
        // Press: the anchor is recorded so the host can draw the marquee from it.
        sel.update_ex(
            Some((2.0, 3.0)),
            true,
            false,
            false,
            false,
            floor,
            &candidates,
        );
        assert_eq!(sel.drag_anchor(), Some((2.0, 3.0)), "anchor set on press");
        // Still held (pointer moves, no release): the anchor persists across drag frames.
        sel.update_ex(
            Some((5.0, 6.0)),
            true,
            false,
            false,
            false,
            floor,
            &candidates,
        );
        assert_eq!(
            sel.drag_anchor(),
            Some((2.0, 3.0)),
            "anchor held during the drag"
        );
        // Release: the gesture resolves and the anchor clears (marquee stops drawing).
        sel.update_ex(
            Some((5.0, 6.0)),
            false,
            true,
            false,
            false,
            floor,
            &candidates,
        );
        assert_eq!(sel.drag_anchor(), None, "anchor cleared on release");
    }
}
