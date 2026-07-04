//! Command-view **multi-touch gesture** seam — the RTS half's pan / zoom / embody grammar on a
//! touchscreen. The PURE, host-testable twin of the embodied [`touch_controls`](crate::touch_controls):
//! it turns the raw two-finger touch set the command view already receives into the three
//! command-layer intents the engine already consumes — a **pan** (`InputFrame::move_axis`), a **zoom**
//! (`InputFrame::scroll`), and a one-shot **embody** edge (`InputFrame::embody_pressed`).
//!
//! ## Why it lives here, not in `pal-android`
//! An Android `MotionEvent` can't be constructed in a host `cargo test`, so — exactly like
//! `touch_controls` — the platform backend does the dumb part (forward the currently-down pointers as
//! [`TouchSample`](gonedark_pal::TouchSample)s + a monotonic timestamp) and ALL the disambiguation
//! logic lives here as a pure state machine over `(touches, now_ms)`. Desktop never feeds it (it uses
//! the mouse-wheel + edge-pan), so this seam never runs there.
//!
//! ## The grammar (the bug it fixes)
//! Before this seam the backend's ONLY multi-touch gesture was "two fingers down = embody", gated on
//! `pointer_count >= 2` alone — so the natural pinch-to-zoom / two-finger-pan a player reaches for on
//! a map **hijacked embody**, and there was no way to pan or zoom the command camera at all. This seam
//! decomposes a two-finger gesture into its orthogonal parts each frame:
//!
//! - **PAN** — the two-finger *centroid* translating drives [`move_axis`](CommandGestureOutput::move_axis)
//!   from the per-frame centroid delta (the same host screen convention the WASD/edge-pan stick uses:
//!   `+x` right, `+y` down), fed to the command camera's `pan_focus`.
//! - **PINCH/ZOOM** — the *inter-finger distance* changing drives
//!   [`scroll`](CommandGestureOutput::scroll) from the per-frame spread delta (fingers apart = zoom IN
//!   = positive scroll, matching the wheel), fed to `zoom_half_extent`.
//! - **EMBODY** — a genuine two-finger **tap** (both fingers down AND back up within
//!   [`TAP_MAX_MS`], with total movement under [`TAP_SLOP_PX`], and never more than two fingers) raises
//!   the one-shot [`embody`](CommandGestureOutput::embody) edge. A pan or a pinch moves too far / holds
//!   too long, so it can NEVER be mistaken for the embody tap (the mis-tap resistance, P1-4).
//!
//! Pan and pinch are emitted together (they are orthogonal — a pure translation changes the centroid
//! but not the spread, a pure pinch the reverse), each behind a small deadzone so finger jitter in one
//! axis can't leak into the other. That decomposition IS the disambiguation.
//!
//! ## Where the outputs go (all host presentation, never the sim)
//! `move_axis` / `scroll` / `embody_pressed` are consumed by the engine **only in the command view**
//! (`Game::frame` gates the camera pan/zoom on `!embodied`, and `map_input_commands` no-ops embody
//! while already embodied), so it is harmless that this seam keeps tracking the twin-stick fingers
//! while embodied — those fields are ignored there. Floats are fine (host-side input/presentation, the
//! platform side of the PAL seam); nothing here touches `core` or the per-tick checksum (invariants
//! #1/#2/#7). The mapping *scales* ([`PAN_GAIN`], [`PINCH_GAIN`]) are a first-cut device feel and are
//! owed an on-device tuning pass; the *classification* (pan vs pinch vs tap) is what this seam locks.

use gonedark_pal::TouchSample;

/// Max duration (ms) a two-finger contact may last and still count as an embody **tap** — longer is a
/// deliberate pan/pinch/hold, not a tap. Android's `ViewConfiguration` long-press is ~500 ms and a
/// tap ~100–180 ms; 200 ms is a comfortable tap ceiling that a pan/pinch blows past immediately.
const TAP_MAX_MS: u64 = 200;
/// Max total finger travel (px, centroid motion + spread change summed over the gesture) a two-finger
/// tap may accumulate and still embody. A pan or a pinch exceeds this within a frame or two, so it can
/// never fire embody; a real tap's micro-jitter stays well under it.
const TAP_SLOP_PX: f32 = 24.0;
/// Per-frame centroid-motion deadzone (px): below this the pan output is zero, so a still or pinching
/// two-finger hold doesn't drift the camera on sub-pixel jitter.
const PAN_DEADZONE_PX: f32 = 1.0;
/// Per-frame spread-change deadzone (px): below this the zoom output is zero, so a pure pan (centroid
/// moving, fingers a fixed distance apart) doesn't leak into zoom on jitter.
const PINCH_DEADZONE_PX: f32 = 1.5;
/// Pixels of per-frame centroid motion → one unit of `move_axis` deflection. `move_axis` is clamped to
/// the `[-1, 1]` stick contract `pan_focus` expects, so a fast drag simply saturates the pan velocity.
/// First-cut feel — the exact rate is owed a device pass (the `LOOK_DRAG_SCALE` analogue).
const PAN_GAIN: f32 = 1.0 / 16.0;
/// Pixels of per-frame inter-finger spread change → one `scroll` notch (the wheel unit `zoom_half_extent`
/// consumes). ~120 px of pinch ≈ one full notch. First-cut feel — owed a device pass.
const PINCH_GAIN: f32 = 1.0 / 120.0;

/// The command-layer intents a two-finger gesture produced THIS frame — mapped straight onto the
/// matching [`InputFrame`](gonedark_pal::InputFrame) fields by the platform backend. All three are
/// command-view presentation only; none reaches the sim (invariants #1/#2).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct CommandGestureOutput {
    /// PAN: host screen-convention stick deflection (`+x` right, `+y` down), each clamped to `[-1, 1]`
    /// — fed to `InputFrame::move_axis`, which the command camera's `pan_focus` already consumes.
    pub move_axis: (f32, f32),
    /// ZOOM: wheel-equivalent notches this frame (positive = zoom IN, fingers apart) — fed to
    /// `InputFrame::scroll`, which `zoom_half_extent` already consumes.
    pub scroll: f32,
    /// EMBODY: a one-shot edge, `true` only on the frame a genuine two-finger **tap** completed — fed
    /// to `InputFrame::embody_pressed` (a no-op while already embodied, so it is self-gating).
    pub embody: bool,
}

/// The in-flight two-finger gesture's running state (centroid/spread history + tap qualifiers).
#[derive(Clone, Copy, Debug)]
struct Tracking {
    /// The two finger ids the gesture is anchored to. The pair is looked up **by id** each frame —
    /// never by array position, because the Android backend rebuilds the touch slice index-compacted
    /// every event, so positions silently re-shuffle when a finger joins or lifts (the exact bug
    /// `touch_controls` already avoids by owning fingers by id).
    ids: (u64, u64),
    /// Monotonic ms the gesture began (second finger landed) — the tap-duration clock.
    start_ms: u64,
    /// Previous frame's two-finger centroid (px), for the pan delta.
    prev_centroid: (f32, f32),
    /// Previous frame's inter-finger distance (px), for the pinch delta.
    prev_spread: f32,
    /// Accumulated |centroid motion| + |spread change| over the gesture (px) — the tap-slop budget.
    moved: f32,
    /// Largest finger count seen during the gesture: a tap must never have exceeded two fingers.
    max_pointers: usize,
}

/// Per-frame-persistent command-gesture state. Lives on the platform backend (Android), fed the raw
/// down-finger set + a monotonic timestamp each poll; desktop never constructs one.
#[derive(Clone, Debug, Default)]
pub struct CommandGesture {
    tracking: Option<Tracking>,
}

impl CommandGesture {
    pub fn new() -> Self {
        Self::default()
    }

    /// Advance the gesture state machine one frame from this frame's currently-down `touches` and a
    /// monotonic `now_ms`, returning the pan / zoom / embody intents. The backend passes the SAME
    /// touch set it puts on `InputFrame::touches`; the gesture anchors to two fingers and tracks that
    /// pair **by id** thereafter (a 3rd+ finger is ignored for the deltas and disqualifies the embody
    /// tap).
    ///
    /// Contract: on the frame the second finger first lands there is NO delta (a capture frame, so the
    /// camera never jumps to the initial contact); when the pair composition changes with two-plus
    /// fingers still down (a tracked finger lifted, a survivor + another remain) the gesture
    /// re-anchors to the current fingers with the same no-delta capture-frame contract; on the frame
    /// the gesture drops below two fingers the pan/zoom are zero and `embody` reflects whether it
    /// qualified as a tap.
    pub fn update(&mut self, touches: &[TouchSample], now_ms: u64) -> CommandGestureOutput {
        let n = touches.len();
        if n >= 2 {
            match self.tracking.as_mut() {
                None => {
                    // Second finger just landed: begin tracking, emit no delta this capture frame.
                    let (centroid, spread) = centroid_spread(touches[0], touches[1]);
                    self.tracking = Some(Tracking {
                        ids: (touches[0].id, touches[1].id),
                        start_ms: now_ms,
                        prev_centroid: centroid,
                        prev_spread: spread,
                        moved: 0.0,
                        max_pointers: n,
                    });
                    CommandGestureOutput::default()
                }
                Some(tr) => {
                    tr.max_pointers = tr.max_pointers.max(n);
                    match find_pair(touches, tr.ids) {
                        Some((a, b)) => {
                            let (centroid, spread) = centroid_spread(a, b);
                            let dcx = centroid.0 - tr.prev_centroid.0;
                            let dcy = centroid.1 - tr.prev_centroid.1;
                            let dspread = spread - tr.prev_spread;
                            tr.prev_centroid = centroid;
                            tr.prev_spread = spread;
                            tr.moved += (dcx * dcx + dcy * dcy).sqrt() + dspread.abs();
                            CommandGestureOutput {
                                move_axis: pan_axis(dcx, dcy),
                                scroll: pinch_scroll(dspread),
                                embody: false,
                            }
                        }
                        None => {
                            // A tracked finger lifted while two-plus remain: the pair composition
                            // changed, so re-anchor to the current fingers and emit NO delta — the
                            // capture-frame contract again, so the identity swap can never jerk the
                            // camera with a spurious pan/zoom.
                            let (centroid, spread) = centroid_spread(touches[0], touches[1]);
                            tr.ids = (touches[0].id, touches[1].id);
                            tr.prev_centroid = centroid;
                            tr.prev_spread = spread;
                            CommandGestureOutput::default()
                        }
                    }
                }
            }
        } else {
            // Below two fingers: the gesture is ending (or none is in flight). Classify the finished
            // gesture as an embody tap iff it was quick, barely moved, and never grew past two fingers.
            let embody = match self.tracking.take() {
                Some(tr) => {
                    let dur = now_ms.saturating_sub(tr.start_ms);
                    tr.max_pointers == 2 && dur < TAP_MAX_MS && tr.moved < TAP_SLOP_PX
                }
                None => false,
            };
            CommandGestureOutput {
                move_axis: (0.0, 0.0),
                scroll: 0.0,
                embody,
            }
        }
    }
}

/// Look the tracked finger pair up **by id** in this frame's touch slice, in tracked order. `None`
/// when either finger is gone — array positions are meaningless (the Android backend index-compacts
/// the slice on every event), so this lookup is what keeps the pair's identity stable.
#[inline]
fn find_pair(touches: &[TouchSample], ids: (u64, u64)) -> Option<(TouchSample, TouchSample)> {
    let a = touches.iter().find(|t| t.id == ids.0)?;
    let b = touches.iter().find(|t| t.id == ids.1)?;
    Some((*a, *b))
}

/// The centroid (midpoint) and spread (distance) of two touch points, in px.
#[inline]
fn centroid_spread(a: TouchSample, b: TouchSample) -> ((f32, f32), f32) {
    let centroid = ((a.x + b.x) * 0.5, (a.y + b.y) * 0.5);
    let dx = a.x - b.x;
    let dy = a.y - b.y;
    let spread = (dx * dx + dy * dy).sqrt();
    (centroid, spread)
}

/// Per-frame centroid delta → clamped `move_axis`, with a deadzone below [`PAN_DEADZONE_PX`] so a
/// still / pinching hold doesn't drift the camera. Each axis is independent (a diagonal drag pans both).
#[inline]
fn pan_axis(dcx: f32, dcy: f32) -> (f32, f32) {
    let axis = |d: f32| {
        if d.abs() < PAN_DEADZONE_PX {
            0.0
        } else {
            (d * PAN_GAIN).clamp(-1.0, 1.0)
        }
    };
    (axis(dcx), axis(dcy))
}

/// Per-frame spread delta → `scroll` notches, with a deadzone below [`PINCH_DEADZONE_PX`] so a pure
/// pan doesn't leak into zoom. Fingers apart (positive `dspread`) = zoom IN = positive scroll.
#[inline]
fn pinch_scroll(dspread: f32) -> f32 {
    if dspread.abs() < PINCH_DEADZONE_PX {
        0.0
    } else {
        dspread * PINCH_GAIN
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn t(id: u64, x: f32, y: f32) -> TouchSample {
        TouchSample { id, x, y }
    }

    /// A pure two-finger translation (both fingers shift by the same vector, so the spread is
    /// unchanged) drives PAN only: `move_axis` tracks the centroid delta, `scroll` stays ~0, no embody.
    #[test]
    fn two_finger_translation_pans_not_zooms_or_embodies() {
        let mut g = CommandGesture::new();
        // Capture frame: two fingers land, no delta.
        let out = g.update(&[t(1, 100.0, 100.0), t(2, 200.0, 100.0)], 0);
        assert_eq!(out, CommandGestureOutput::default(), "no jump on the capture frame");
        // Both fingers slide +48 px right (spread fixed at 100 px): pans +x, no zoom.
        let out = g.update(&[t(1, 148.0, 100.0), t(2, 248.0, 100.0)], 16);
        assert!(out.move_axis.0 > 0.0, "centroid moved right → +x pan: {:?}", out.move_axis);
        assert!((out.move_axis.1).abs() < 1e-6, "no vertical motion");
        assert_eq!(out.scroll, 0.0, "a rigid translation must not zoom");
        assert!(!out.embody);
        // A 48 px delta at PAN_GAIN=1/16 → 3.0, clamped to 1.0 (saturated fast pan).
        assert_eq!(out.move_axis.0, 1.0, "fast drag saturates to the stick max");
    }

    /// A pure pinch (centroid fixed, fingers spreading apart) drives ZOOM only: positive `scroll`
    /// (zoom IN), `move_axis` ~0, no embody. Pinching together gives negative scroll (zoom OUT).
    #[test]
    fn pinch_zooms_with_the_right_sign_not_pans_or_embodies() {
        let mut g = CommandGesture::new();
        // Land symmetric about x=150 (centroid fixed), spread 60 px.
        g.update(&[t(1, 120.0, 100.0), t(2, 180.0, 100.0)], 0);
        // Spread grows to 120 px (each finger 30 px further out), centroid unchanged: zoom IN.
        let out = g.update(&[t(1, 90.0, 100.0), t(2, 210.0, 100.0)], 16);
        assert!(out.scroll > 0.0, "fingers apart = zoom IN = positive scroll: {}", out.scroll);
        assert_eq!(out.move_axis, (0.0, 0.0), "a symmetric pinch must not pan");
        assert!(!out.embody);
        // Now pinch back together to 40 px: zoom OUT (negative scroll).
        let out = g.update(&[t(1, 130.0, 100.0), t(2, 170.0, 100.0)], 32);
        assert!(out.scroll < 0.0, "fingers together = zoom OUT = negative scroll: {}", out.scroll);
    }

    /// A genuine two-finger TAP — both down, then up, quickly and near-motionless — raises exactly one
    /// embody edge, on the lift frame (and never while the fingers are still down).
    #[test]
    fn quick_still_two_finger_tap_embodies_on_lift() {
        let mut g = CommandGesture::new();
        let down = [t(1, 100.0, 100.0), t(2, 200.0, 100.0)];
        let out = g.update(&down, 0);
        assert!(!out.embody, "no embody while the fingers are still down (capture frame)");
        // A tiny bit of jitter, still within slop and the tap window.
        let out = g.update(&[t(1, 101.0, 100.0), t(2, 200.0, 101.0)], 40);
        assert!(!out.embody, "still down → still no embody");
        // Lift both fingers within the window: embody fires once.
        let out = g.update(&[], 80);
        assert!(out.embody, "a quick, still two-finger tap embodies on lift");
        assert_eq!(out.move_axis, (0.0, 0.0));
        assert_eq!(out.scroll, 0.0);
        // And it is a one-shot: a subsequent empty frame does not re-fire.
        let out = g.update(&[], 96);
        assert!(!out.embody, "embody is a one-shot edge, not a level");
    }

    /// A PAN (moved past the slop budget) must NOT be mistaken for the embody tap on lift (P1-4).
    #[test]
    fn a_pan_does_not_fire_embody() {
        let mut g = CommandGesture::new();
        g.update(&[t(1, 100.0, 100.0), t(2, 200.0, 100.0)], 0);
        // Drag well past the slop budget (both fingers +40 px), still quick.
        g.update(&[t(1, 140.0, 100.0), t(2, 240.0, 100.0)], 16);
        let out = g.update(&[], 32);
        assert!(!out.embody, "a two-finger PAN is not an embody tap");
    }

    /// A PINCH (spread changed past the slop budget) must NOT fire embody on lift (P1-4).
    #[test]
    fn a_pinch_does_not_fire_embody() {
        let mut g = CommandGesture::new();
        g.update(&[t(1, 120.0, 100.0), t(2, 180.0, 100.0)], 0); // spread 60
        g.update(&[t(1, 60.0, 100.0), t(2, 240.0, 100.0)], 16); // spread 180: +120 > slop
        let out = g.update(&[], 32);
        assert!(!out.embody, "a two-finger PINCH is not an embody tap");
    }

    /// A tap held too long (past the tap window) is a deliberate hold, not an embody tap.
    #[test]
    fn a_slow_two_finger_hold_does_not_embody() {
        let mut g = CommandGesture::new();
        g.update(&[t(1, 100.0, 100.0), t(2, 200.0, 100.0)], 0);
        g.update(&[t(1, 100.0, 100.0), t(2, 200.0, 100.0)], 100); // still, but the clock runs
        let out = g.update(&[], TAP_MAX_MS + 50); // lifted after the window
        assert!(!out.embody, "held past TAP_MAX_MS → not a tap");
    }

    /// Three fingers down at any point disqualifies the embody tap (it was a different gesture), even
    /// if it was quick and still.
    #[test]
    fn a_three_finger_gesture_never_embodies() {
        let mut g = CommandGesture::new();
        g.update(&[t(1, 100.0, 100.0), t(2, 200.0, 100.0)], 0);
        // A third finger joins (still, quick).
        g.update(&[t(1, 100.0, 100.0), t(2, 200.0, 100.0), t(3, 150.0, 150.0)], 20);
        let out = g.update(&[], 40);
        assert!(!out.embody, "more than two fingers → not the two-finger embody tap");
    }

    /// A single finger never engages the gesture seam at all (that is the command-layer tap/drag
    /// scheme's job) — no pan, no zoom, no embody.
    #[test]
    fn a_single_finger_is_inert_here() {
        let mut g = CommandGesture::new();
        let out = g.update(&[t(1, 100.0, 100.0)], 0);
        assert_eq!(out, CommandGestureOutput::default());
        let out = g.update(&[t(1, 140.0, 120.0)], 16);
        assert_eq!(out, CommandGestureOutput::default(), "a one-finger drag is not a command gesture");
        let out = g.update(&[], 32);
        assert!(!out.embody, "lifting a single finger never embodies");
    }

    /// REGRESSION (id-tracking): a 3rd finger joins, then an ORIGINAL finger lifts — the backend
    /// index-compacts the slice, so positions `[0]`/`[1]` now name a different pair. Tracking by id,
    /// the frame the pair composition changes is a re-anchor frame with NO spurious pan/zoom delta
    /// (the by-position bug jerked the camera here), and the new pair drives the gesture normally
    /// from the next frame.
    #[test]
    fn original_finger_lifting_under_a_third_re_anchors_without_a_delta() {
        let mut g = CommandGesture::new();
        // Pair (1, 2) lands.
        g.update(&[t(1, 100.0, 100.0), t(2, 200.0, 100.0)], 0);
        // A 3rd finger joins far away; the tracked pair is motionless → no delta.
        let out = g.update(
            &[t(1, 100.0, 100.0), t(2, 200.0, 100.0), t(3, 500.0, 400.0)],
            16,
        );
        assert_eq!(out.move_axis, (0.0, 0.0), "a joining 3rd finger must not pan");
        assert_eq!(out.scroll, 0.0, "a joining 3rd finger must not zoom");
        // Finger 1 lifts; the compacted slice is [2, 3], whose centroid/spread differ wildly from
        // (1, 2)'s. Pair composition changed → re-anchor, NO delta this frame.
        let out = g.update(&[t(2, 200.0, 100.0), t(3, 500.0, 400.0)], 32);
        assert_eq!(out, CommandGestureOutput::default(), "pair change re-anchors without a delta");
        // From the next frame the re-anchored pair (2, 3) drives pan/zoom normally.
        let out = g.update(&[t(2, 248.0, 100.0), t(3, 548.0, 400.0)], 48);
        assert_eq!(out.move_axis.0, 1.0, "the new pair pans after the re-anchor frame");
        assert_eq!(out.scroll, 0.0, "a rigid translation of the new pair must not zoom");
    }

    /// REGRESSION (id-tracking): the tracked pair is found BY ID, not by slice position — a 3rd
    /// finger landing at index 0 (the identity-swap the by-position bug produced) neither jerks the
    /// camera while the true pair is still, nor pollutes the true pair's deltas when it moves.
    #[test]
    fn tracked_pair_is_found_by_id_regardless_of_slice_order() {
        let mut g = CommandGesture::new();
        g.update(&[t(1, 100.0, 100.0), t(2, 200.0, 100.0)], 0);
        // The slice is handed over re-ordered with an interloper first: touches[0]/[1] = (3, 1),
        // which the by-position code would have read as a huge spurious pan + pinch.
        let out = g.update(
            &[t(3, 500.0, 400.0), t(1, 100.0, 100.0), t(2, 200.0, 100.0)],
            16,
        );
        assert_eq!(out.move_axis, (0.0, 0.0), "a still tracked pair must not pan on re-order");
        assert_eq!(out.scroll, 0.0, "a still tracked pair must not zoom on re-order");
        // The true pair sliding +48 px still saturates the pan, unaffected by the interloper.
        let out = g.update(
            &[t(3, 500.0, 400.0), t(1, 148.0, 100.0), t(2, 248.0, 100.0)],
            32,
        );
        assert_eq!(out.move_axis, (1.0, 0.0), "the tracked pair's motion drives the pan");
        assert_eq!(out.scroll, 0.0, "the tracked pair's rigid translation must not zoom");
    }

    /// Sub-deadzone jitter on a held two-finger contact produces neither pan nor zoom (so a resting
    /// pinch/hold doesn't drift the camera), yet the tiny motion still counts toward the tap slop.
    #[test]
    fn sub_deadzone_jitter_is_zero_pan_and_zoom() {
        let mut g = CommandGesture::new();
        g.update(&[t(1, 100.0, 100.0), t(2, 200.0, 100.0)], 0);
        // Half-pixel wobble on both fingers: below both deadzones.
        let out = g.update(&[t(1, 100.4, 100.0), t(2, 200.4, 100.0)], 16);
        assert_eq!(out.move_axis, (0.0, 0.0), "jitter under the pan deadzone → no pan");
        assert_eq!(out.scroll, 0.0, "jitter under the pinch deadzone → no zoom");
    }
}
