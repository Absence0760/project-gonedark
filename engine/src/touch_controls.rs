//! On-screen FPS touch controls (the COD-Mobile-style embodied HUD) — the PURE seam that turns a
//! frame's raw multi-touch points into the embodied control intents the rest of `engine` already
//! consumes (`move_axis` / look delta / fire / crouch / reload / surface), plus the screen-space
//! geometry the renderer draws.
//!
//! ## Why it lives here, not in `pal-android`
//! The platform backend can't be unit-tested — an Android `MotionEvent` can't be constructed in a
//! host `cargo test`. So `pal-android` does the dumb part (forward the currently-down pointers as
//! [`TouchSample`](gonedark_pal::TouchSample)s on the `InputFrame`) and ALL the logic — finger
//! ownership, the floating stick math, drag-to-look deltas, button hit-testing + edge detection —
//! lives here as a pure function over `(layout, touches)`, exactly like the `fire`/`locomote`
//! seams extract their logic out of un-constructible window events. The desktop backend leaves
//! `touches` empty, so this seam never runs there (the Android-only GUI, per the design).
//!
//! ## The scheme (D14's validated layout, now the shipping UI)
//! - **Left half** is a *floating* movement joystick: the first finger down in the left zone sets
//!   the stick origin; its offset (clamped to [`STICK_RADIUS_FRAC`]) is `move_axis`.
//! - **Right half** is a free *drag-to-look* region (no visible stick): a finger's per-frame motion
//!   becomes the look delta (same convention as the desktop mouse delta, so it feeds
//!   `integrate_look_yaw`/`_pitch` unchanged).
//! - Floating over the right half: **Fire** (held = auto-fire), **Crouch** (tap = toggle),
//!   **Reload** (tap), **Surface** (tap = eject back to command — this REPLACES the two-finger
//!   gesture while embodied, since two fingers now mean move+look).
//!
//! Floats are fine here: this is host-side presentation/input, the platform side of the PAL seam.
//! Nothing in this module touches the sim — the intents it returns are quantized to `Fixed` *later*
//! by the `fire`/`locomote` seams, exactly as the desktop path is (invariant #1).

use gonedark_pal::TouchSample;

/// Fraction of the min(viewport) dimension a standard round button's radius spans.
const BUTTON_R_FRAC: f32 = 0.072;
/// The Fire button is bigger (the primary action, thumb-reachable without looking).
const FIRE_R_FRAC: f32 = 0.095;
/// The Surface (eject) button is smaller and tucked in the top corner — a deliberate, not a
/// twitch, action.
const SURFACE_R_FRAC: f32 = 0.055;
/// The aim-down-sight (ADS) button is a standard-size disc, sat just above-left of Fire so the
/// right thumb can hold it while a left finger drives the stick (the sniper/zoom, wave-2 W6). Only
/// drawn/honored for a unit that actually has a gun-sight (the host gates the visual + the zoom on
/// `has_scope`, exactly as W2's `scope::zoom_active` does).
const AIM_R_FRAC: f32 = 0.072;
/// Full-deflection radius of the floating move stick, as a fraction of min(viewport).
const STICK_RADIUS_FRAC: f32 = 0.16;
/// Converts a look-region finger's pixel drag this frame into the look-axis units
/// `integrate_look_yaw`/`_pitch` expect (those were tuned for raw mouse-pixel deltas, so ~1.0 keeps
/// a touch drag feeling like a mouse drag; tune for device feel later).
const LOOK_DRAG_SCALE: f32 = 1.0;

/// An axis-aligned screen rectangle in pixels (`x0,y0` top-left, inclusive lower bound).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Rect {
    pub x0: f32,
    pub y0: f32,
    pub x1: f32,
    pub y1: f32,
}

impl Rect {
    #[inline]
    fn contains(&self, x: f32, y: f32) -> bool {
        x >= self.x0 && x < self.x1 && y >= self.y0 && y < self.y1
    }
}

/// A screen-space circle in pixels — the hit shape (and draw shape) of an on-screen button.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Circle {
    pub cx: f32,
    pub cy: f32,
    pub r: f32,
}

impl Circle {
    #[inline]
    fn contains(&self, x: f32, y: f32) -> bool {
        let dx = x - self.cx;
        let dy = y - self.cy;
        dx * dx + dy * dy <= self.r * self.r
    }
}

/// The static layout of the touch HUD for a given viewport — every control's screen rect/circle.
/// Pure function of `(width, height)`, recomputed each frame (cheap); both the input seam
/// ([`TouchControls::update`]) and the renderer read it, so the hit shapes and the drawn shapes can
/// never drift.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TouchLayout {
    pub width: f32,
    pub height: f32,
    /// The left region where a finger-down starts the floating move stick.
    pub stick_zone: Rect,
    /// Full-deflection radius of the move stick, in pixels.
    pub stick_radius: f32,
    /// The right region treated as the free drag-to-look area (buttons take hit priority over it).
    pub look_zone: Rect,
    pub fire: Circle,
    pub crouch: Circle,
    pub reload: Circle,
    pub surface: Circle,
    /// Aim-down-sight (ADS) button: HELD = zoom (the sniper scope), like Fire. Its hit shape always
    /// exists in the layout; whether it does anything is gated host-side by `has_scope` (a unit with
    /// an independent turret), so an infantry avatar's press is inert — the W2 turret/tank gate.
    pub aim: Circle,
}

impl TouchLayout {
    /// Lay the HUD out for a `width × height` pixel viewport (a landscape phone). All positions are
    /// fractions of the viewport so the layout scales across screen sizes/DPIs.
    pub fn new(width: u32, height: u32) -> Self {
        let w = width.max(1) as f32;
        let h = height.max(1) as f32;
        let m = w.min(h);
        let br = BUTTON_R_FRAC * m;
        let fire_r = FIRE_R_FRAC * m;
        let surface_r = SURFACE_R_FRAC * m;
        let aim_r = AIM_R_FRAC * m;
        TouchLayout {
            width: w,
            height: h,
            // Lower-left ~40% width, lower ~70% height: a generous thumb area for the floating stick.
            stick_zone: Rect {
                x0: 0.0,
                y0: 0.30 * h,
                x1: 0.42 * w,
                y1: h,
            },
            stick_radius: STICK_RADIUS_FRAC * m,
            // The whole right side is drag-to-look; button circles below win the hit test inside it.
            look_zone: Rect {
                x0: 0.42 * w,
                y0: 0.0,
                x1: w,
                y1: h,
            },
            // Bottom-right thumb cluster.
            fire: Circle {
                cx: 0.84 * w,
                cy: 0.74 * h,
                r: fire_r,
            },
            crouch: Circle {
                cx: 0.70 * w,
                cy: 0.86 * h,
                r: br,
            },
            reload: Circle {
                cx: 0.95 * w,
                cy: 0.50 * h,
                r: br,
            },
            // Top-right corner — out of the way of the aim/fire thumb.
            surface: Circle {
                cx: 0.94 * w,
                cy: 0.08 * h,
                r: surface_r,
            },
            // Just above-left of Fire: the right thumb can hold ADS (zoom) while the left finger
            // drives the stick and a second right-thumb tap hits Fire. Clear of Fire and Crouch.
            aim: Circle {
                cx: 0.70 * w,
                cy: 0.62 * h,
                r: aim_r,
            },
        }
    }
}

/// The dynamic, per-frame draw state the renderer needs ON TOP of the static [`TouchLayout`]: where
/// the floating stick currently sits and which momentary buttons are pressed (for the press flash).
/// The crouch *toggle* highlight is NOT here — it reflects authoritative sim posture, which the
/// engine passes to the renderer separately.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct TouchHud {
    /// Whether a move-stick finger is down (draw the floating stick).
    pub stick_active: bool,
    /// Stick base center (the captured finger-down origin), pixels.
    pub stick_origin: (f32, f32),
    /// Stick thumb position (origin + clamped offset), pixels.
    pub stick_thumb: (f32, f32),
    pub fire_pressed: bool,
    pub crouch_pressed: bool,
    pub reload_pressed: bool,
    pub surface_pressed: bool,
    /// Aim-down-sight button held this frame (the press flash — held like Fire, not an edge).
    pub aim_pressed: bool,
}

/// The embodied control intents derived from this frame's touches — consumed by `Game::frame`
/// exactly where the desktop `InputFrame` fields are.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct TouchOutput {
    /// Left-stick deflection in the host screen convention (`+x` right, `+y` down), each in
    /// `[-1, 1]` — fed straight to `locomote::locomote_command` like the desktop `move_axis`.
    pub move_axis: (f32, f32),
    /// Right-region drag this frame `(dx, dy)` in look-axis units — fed to
    /// `integrate_look_yaw`/`_pitch` like the desktop `look_axis`.
    pub look_delta: (f32, f32),
    /// Fire button held (auto-fire while down).
    pub fire: bool,
    /// Aim-down-sight button **held** this frame (the zoom is held, not an edge — the level signal
    /// the desktop right-mouse `InputFrame.aim` carries). The host feeds this to `scope::zoom_active`,
    /// which itself gates on `has_scope` (the W2 turret/tank gate), so it is inert for a scope-less
    /// avatar even though the button is always hit-tested here.
    pub aim: bool,
    /// Crouch button press *edge* this frame (the engine flips posture off authoritative sim state).
    pub crouch_edge: bool,
    /// Reload button press edge this frame.
    pub reload_edge: bool,
    /// Surface button press edge this frame (eject to command).
    pub surface_edge: bool,
    /// What to draw this frame.
    pub hud: TouchHud,
}

/// Per-frame-persistent touch state: which finger owns which control, and last frame's button
/// presses (for edge detection). Lives on `Game`; reset on (un)embodiment via [`reset`](Self::reset).
#[derive(Clone, Debug, Default)]
pub struct TouchControls {
    move_id: Option<u64>,
    move_origin: (f32, f32),
    look_id: Option<u64>,
    look_last: (f32, f32),
    prev_fire: bool,
    prev_crouch: bool,
    prev_reload: bool,
    prev_surface: bool,
}

impl TouchControls {
    pub fn new() -> Self {
        Self::default()
    }

    /// Forget all finger ownership + button history. Called when embodiment toggles so a stale
    /// finger from the command view (or a previous possession) never bleeds into a fresh one.
    pub fn reset(&mut self) {
        *self = Self::default();
    }

    /// Is `id` the finger currently driving the stick or the look region? Such a finger is excluded
    /// from button hit-tests and from being re-claimed by the other control.
    #[inline]
    fn owns(&self, id: u64) -> bool {
        self.move_id == Some(id) || self.look_id == Some(id)
    }

    /// Translate this frame's currently-down `touches` into embodied intents + HUD geometry.
    ///
    /// Ordering is deliberate: resolve the move finger, then the look finger (so a freshly claimed
    /// look finger can't also be the move finger), then hit-test buttons against the *unowned*
    /// fingers (so a dragging aim finger crossing a button doesn't fire it). A finger that lifts
    /// releases its control; the next finger down in that zone re-captures it.
    pub fn update(&mut self, layout: &TouchLayout, touches: &[TouchSample]) -> TouchOutput {
        let find = |id: u64| {
            touches
                .iter()
                .find(|t| t.id == id)
                .map(|t| (t.x, t.y))
        };

        // --- Move stick (floating): claim a finger-down in the left zone; offset → axis. ---
        if let Some(id) = self.move_id {
            if find(id).is_none() {
                self.move_id = None;
            }
        }
        if self.move_id.is_none() {
            if let Some(t) = touches
                .iter()
                .find(|t| !self.owns(t.id) && layout.stick_zone.contains(t.x, t.y))
            {
                self.move_id = Some(t.id);
                self.move_origin = (t.x, t.y);
            }
        }
        let mut move_axis = (0.0, 0.0);
        let mut stick_thumb = self.move_origin;
        let stick_active = self.move_id.is_some();
        if let Some(id) = self.move_id {
            if let Some((x, y)) = find(id) {
                let r = layout.stick_radius.max(1.0);
                let dx = x - self.move_origin.0;
                let dy = y - self.move_origin.1;
                let len = (dx * dx + dy * dy).sqrt();
                let (cdx, cdy) = if len > r {
                    (dx * r / len, dy * r / len)
                } else {
                    (dx, dy)
                };
                stick_thumb = (self.move_origin.0 + cdx, self.move_origin.1 + cdy);
                move_axis = (cdx / r, cdy / r);
            }
        }

        // --- Look region (drag): per-frame motion of the owning finger → look delta. ---
        let mut look_delta = (0.0, 0.0);
        if let Some(id) = self.look_id {
            match find(id) {
                Some((x, y)) => {
                    look_delta = (
                        (x - self.look_last.0) * LOOK_DRAG_SCALE,
                        (y - self.look_last.1) * LOOK_DRAG_SCALE,
                    );
                    self.look_last = (x, y);
                }
                None => self.look_id = None,
            }
        }
        if self.look_id.is_none() {
            if let Some(t) = touches.iter().find(|t| {
                !self.owns(t.id)
                    && layout.look_zone.contains(t.x, t.y)
                    && !self.on_any_button(layout, t.x, t.y)
            }) {
                self.look_id = Some(t.id);
                self.look_last = (t.x, t.y);
                // No delta on the capture frame (avoids a jump from the first contact point).
            }
        }

        // --- Buttons: pressed = an UNOWNED finger inside the circle; edges vs last frame. ---
        let fire = self.button_pressed(&layout.fire, touches);
        // ADS is HELD (the zoom level signal), exactly like Fire — no edge detection.
        let aim = self.button_pressed(&layout.aim, touches);
        let crouch = self.button_pressed(&layout.crouch, touches);
        let reload = self.button_pressed(&layout.reload, touches);
        let surface = self.button_pressed(&layout.surface, touches);

        let out = TouchOutput {
            move_axis,
            look_delta,
            fire,
            aim,
            crouch_edge: crouch && !self.prev_crouch,
            reload_edge: reload && !self.prev_reload,
            surface_edge: surface && !self.prev_surface,
            hud: TouchHud {
                stick_active,
                stick_origin: self.move_origin,
                stick_thumb,
                fire_pressed: fire,
                crouch_pressed: crouch,
                reload_pressed: reload,
                surface_pressed: surface,
                aim_pressed: aim,
            },
        };

        self.prev_fire = fire;
        self.prev_crouch = crouch;
        self.prev_reload = reload;
        self.prev_surface = surface;
        out
    }

    /// Any **unowned** finger inside `c` (a finger driving the stick/look never also taps a button).
    #[inline]
    fn button_pressed(&self, c: &Circle, touches: &[TouchSample]) -> bool {
        touches
            .iter()
            .any(|t| !self.owns(t.id) && c.contains(t.x, t.y))
    }

    /// Is `(x, y)` inside any button circle? Keeps the drag-look claim from stealing a button tap.
    #[inline]
    fn on_any_button(&self, layout: &TouchLayout, x: f32, y: f32) -> bool {
        layout.fire.contains(x, y)
            || layout.aim.contains(x, y)
            || layout.crouch.contains(x, y)
            || layout.reload.contains(x, y)
            || layout.surface.contains(x, y)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const W: u32 = 1280;
    const H: u32 = 720;

    fn layout() -> TouchLayout {
        TouchLayout::new(W, H)
    }

    fn t(id: u64, x: f32, y: f32) -> TouchSample {
        TouchSample { id, x, y }
    }

    /// A point at the center of a circle (a guaranteed hit).
    fn center(c: &Circle) -> (f32, f32) {
        (c.cx, c.cy)
    }

    #[test]
    fn left_zone_finger_drives_move_axis_from_its_origin() {
        let l = layout();
        let mut tc = TouchControls::new();
        // Finger down inside the stick zone establishes the origin (axis ~0 on the first frame).
        let origin = (l.stick_zone.x0 + 100.0, l.stick_zone.y0 + 100.0);
        let out = tc.update(&l, &[t(1, origin.0, origin.1)]);
        assert_eq!(out.move_axis, (0.0, 0.0), "first contact is the neutral origin");
        assert!(out.hud.stick_active);

        // Push it right + up by less than the radius: axis is the normalized offset.
        let dx = l.stick_radius * 0.5;
        let out = tc.update(&l, &[t(1, origin.0 + dx, origin.1 - dx)]);
        assert!((out.move_axis.0 - 0.5).abs() < 1e-5, "right half-deflection");
        assert!((out.move_axis.1 + 0.5).abs() < 1e-5, "up is -y (screen convention)");
    }

    #[test]
    fn move_axis_clamps_to_unit_magnitude_beyond_the_radius() {
        let l = layout();
        let mut tc = TouchControls::new();
        let origin = (50.0, l.stick_zone.y0 + 50.0);
        tc.update(&l, &[t(1, origin.0, origin.1)]);
        // Shove far past the radius straight down: magnitude clamps to 1.
        let out = tc.update(&l, &[t(1, origin.0, origin.1 + l.stick_radius * 10.0)]);
        let mag = (out.move_axis.0 * out.move_axis.0 + out.move_axis.1 * out.move_axis.1).sqrt();
        assert!((mag - 1.0).abs() < 1e-5, "deflection clamps to the unit circle, got {mag}");
        assert!(out.move_axis.1 > 0.99, "straight down is +y");
    }

    #[test]
    fn right_region_drag_produces_look_delta_then_resets_each_frame() {
        let l = layout();
        let mut tc = TouchControls::new();
        let start = (l.look_zone.x0 + 200.0, 200.0);
        // Capture frame: no delta.
        let out = tc.update(&l, &[t(7, start.0, start.1)]);
        assert_eq!(out.look_delta, (0.0, 0.0), "no jump on first contact");
        // Drag right by 30 px: look delta tracks the per-frame motion.
        let out = tc.update(&l, &[t(7, start.0 + 30.0, start.1)]);
        assert!((out.look_delta.0 - 30.0 * LOOK_DRAG_SCALE).abs() < 1e-4);
        // Holding still next frame yields zero delta again (it is a per-frame delta, not absolute).
        let out = tc.update(&l, &[t(7, start.0 + 30.0, start.1)]);
        assert_eq!(out.look_delta, (0.0, 0.0));
    }

    #[test]
    fn fire_button_is_held_not_edge() {
        let l = layout();
        let mut tc = TouchControls::new();
        let (fx, fy) = center(&l.fire);
        let out = tc.update(&l, &[t(3, fx, fy)]);
        assert!(out.fire, "fire is true while held");
        assert!(out.hud.fire_pressed);
        let out = tc.update(&l, &[t(3, fx, fy)]);
        assert!(out.fire, "still firing on the next held frame");
    }

    #[test]
    fn aim_button_is_held_not_edge_like_fire() {
        // ADS is the zoom level signal — true for as long as the finger is down, never a one-shot.
        let l = layout();
        let mut tc = TouchControls::new();
        let (ax, ay) = center(&l.aim);
        let out = tc.update(&l, &[t(8, ax, ay)]);
        assert!(out.aim, "aim is true while the ADS button is held");
        assert!(out.hud.aim_pressed, "the press flash tracks the held button");
        let out = tc.update(&l, &[t(8, ax, ay)]);
        assert!(out.aim, "still aiming on the next held frame (no edge dropout)");
        // Releasing drops ADS the same frame.
        let out = tc.update(&l, &[]);
        assert!(!out.aim, "releasing the button releases the zoom");
        assert!(!out.hud.aim_pressed);
    }

    #[test]
    fn a_touch_outside_the_aim_button_does_not_hold_aim() {
        let l = layout();
        let mut tc = TouchControls::new();
        // A finger in the bare look region (clear of every button) never holds ADS.
        let p = (l.look_zone.x0 + 250.0, 90.0);
        assert!(!l.aim.contains(p.0, p.1), "precondition: point is outside the ADS button");
        let out = tc.update(&l, &[t(1, p.0, p.1)]);
        assert!(!out.aim, "only a touch inside the ADS circle holds aim");
    }

    #[test]
    fn aim_finger_is_not_claimed_as_look_and_does_not_steal_fire_or_move() {
        // The load-bearing isolation case: hold ADS with one right-thumb finger while the OTHER
        // controls keep working — the ADS finger must not also drive the look drag, and it must not
        // swallow the move stick or the Fire button.
        let l = layout();
        let mut tc = TouchControls::new();
        let (ax, ay) = center(&l.aim);
        let (fx, fy) = center(&l.fire);
        let stick_o = (60.0, l.stick_zone.y0 + 60.0);

        // Establish stick + an ADS hold (frame 1).
        tc.update(&l, &[t(1, stick_o.0, stick_o.1), t(2, ax, ay)]);
        // Frame 2: deflect the stick, drag the ADS finger, and add a Fire finger — all independent.
        let out = tc.update(
            &l,
            &[
                t(1, stick_o.0, stick_o.1 + l.stick_radius), // full down
                t(2, ax + 40.0, ay),                         // ADS finger dragged
                t(3, fx, fy),                                // fire
            ],
        );
        assert!(out.aim, "ADS stays held across the drag");
        assert_eq!(out.look_delta, (0.0, 0.0), "the ADS finger never drives the look region");
        assert!(out.move_axis.1 > 0.99, "the move stick still reads full forward");
        assert!(out.fire, "and the trigger still fires — ADS didn't swallow it");
    }

    #[test]
    fn crouch_reload_surface_are_one_shot_press_edges() {
        let l = layout();
        let mut tc = TouchControls::new();
        let (cx, cy) = center(&l.crouch);
        // Hold the crouch button across two frames: edge fires once, on the press frame.
        let out = tc.update(&l, &[t(2, cx, cy)]);
        assert!(out.crouch_edge, "press edge on first contact");
        let out = tc.update(&l, &[t(2, cx, cy)]);
        assert!(!out.crouch_edge, "no repeat edge while held");
        // Release, then press again: a new edge.
        tc.update(&l, &[]);
        let out = tc.update(&l, &[t(2, cx, cy)]);
        assert!(out.crouch_edge, "re-press is a fresh edge");

        // Reload + Surface behave identically (edge on press).
        let (rx, ry) = center(&l.reload);
        let (sx, sy) = center(&l.surface);
        let out = tc.update(&l, &[t(4, rx, ry), t(5, sx, sy)]);
        assert!(out.reload_edge && out.surface_edge);
        let out = tc.update(&l, &[t(4, rx, ry), t(5, sx, sy)]);
        assert!(!out.reload_edge && !out.surface_edge, "held buttons don't re-edge");
    }

    #[test]
    fn simultaneous_move_look_and_fire_with_three_fingers() {
        // The load-bearing case: walk + aim + shoot at once, the whole point of the layout.
        let l = layout();
        let mut tc = TouchControls::new();
        let stick_o = (60.0, l.stick_zone.y0 + 60.0);
        let look_o = (l.look_zone.x0 + 150.0, 120.0);
        let (firex, firey) = center(&l.fire);

        // Establish stick + look fingers (frame 1).
        tc.update(&l, &[t(1, stick_o.0, stick_o.1), t(2, look_o.0, look_o.1)]);
        // Frame 2: deflect stick down, drag look right, hold fire — all three resolve independently.
        let out = tc.update(
            &l,
            &[
                t(1, stick_o.0, stick_o.1 + l.stick_radius), // full down
                t(2, look_o.0 + 25.0, look_o.1),             // look right 25 px
                t(3, firex, firey),                          // fire
            ],
        );
        assert!(out.move_axis.1 > 0.99, "stick reads full forward/down");
        assert!((out.look_delta.0 - 25.0 * LOOK_DRAG_SCALE).abs() < 1e-4, "look tracks the drag");
        assert!(out.fire, "and the trigger is held");
    }

    #[test]
    fn lifting_a_finger_releases_its_control_and_recaptures_on_next_touch() {
        let l = layout();
        let mut tc = TouchControls::new();
        let a = (40.0, l.stick_zone.y0 + 40.0);
        tc.update(&l, &[t(1, a.0, a.1)]);
        let out = tc.update(&l, &[t(1, a.0 + l.stick_radius, a.1)]);
        assert!(out.move_axis.0 > 0.99, "finger 1 drives the stick right");
        // Lift finger 1: stick goes neutral/inactive.
        let out = tc.update(&l, &[]);
        assert!(!out.hud.stick_active);
        assert_eq!(out.move_axis, (0.0, 0.0));
        // A NEW finger down at a different origin re-captures the stick from there.
        let b = (300.0, l.stick_zone.y0 + 10.0);
        let out = tc.update(&l, &[t(9, b.0, b.1)]);
        assert!(out.hud.stick_active);
        assert_eq!(out.hud.stick_origin, b, "new origin is the new finger-down point");
    }

    #[test]
    fn a_finger_on_the_fire_button_is_not_also_claimed_as_look() {
        // Fire sits inside the look zone; the button must win so holding fire never also aims.
        let l = layout();
        let mut tc = TouchControls::new();
        let (fx, fy) = center(&l.fire);
        assert!(l.look_zone.contains(fx, fy), "precondition: fire is within the look region");
        let out = tc.update(&l, &[t(1, fx, fy)]);
        assert!(out.fire);
        assert_eq!(out.look_delta, (0.0, 0.0), "the fire finger never drives look");
        // Even dragging the held fire finger produces no look delta (it was never the look owner).
        let out = tc.update(&l, &[t(1, fx + 40.0, fy)]);
        assert_eq!(out.look_delta, (0.0, 0.0));
        assert!(out.fire);
    }

    #[test]
    fn reset_clears_finger_ownership_and_button_history() {
        let l = layout();
        let mut tc = TouchControls::new();
        let (cx, cy) = center(&l.crouch);
        tc.update(&l, &[t(2, cx, cy)]); // crouch held → prev_crouch = true
        tc.reset();
        // After reset the same held button reads as a fresh press edge (history cleared).
        let out = tc.update(&l, &[t(2, cx, cy)]);
        assert!(out.crouch_edge, "reset forgets the prior press so re-entry is a clean edge");
        assert!(tc.move_id.is_none() && tc.look_id.is_none());
    }

    #[test]
    fn layout_buttons_sit_inside_the_viewport() {
        let l = TouchLayout::new(1920, 1080);
        for c in [l.fire, l.aim, l.crouch, l.reload, l.surface] {
            assert!(c.cx - c.r >= 0.0 && c.cx + c.r <= l.width, "button within width");
            assert!(c.cy - c.r >= 0.0 && c.cy + c.r <= l.height, "button within height");
        }
        // Stick zone is on the left, look zone on the right, and they partition horizontally.
        assert!(l.stick_zone.x1 <= l.look_zone.x0 + 1.0);
    }
}
