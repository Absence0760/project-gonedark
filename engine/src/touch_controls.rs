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
//! ## The scheme (COD-Mobile "fixed joystick", D14's validated split)
//! - **Move stick** is a *fixed*, always-drawn ring anchored in the lower-left ([`stick_base`]): a
//!   finger-down **inside the visible ring** claims the stick, and deflection is measured from the
//!   ring's (fixed) centre, clamped to its radius, to give `move_axis`. Fixed — not floating — so
//!   there is a discoverable target and a touch can never be silently mis-classified as look
//!   (the bug the fixed ring fixes: a touch just outside the ring does *nothing* rather than
//!   panning the camera).
//! - **Right half** is a free *drag-to-look* region: a finger's per-frame motion becomes the look
//!   delta (same convention as the desktop mouse delta, so it feeds `integrate_look_yaw`/`_pitch`
//!   unchanged).
//! - Floating over the right half: **Fire** (held = auto-fire), **Crouch** (tap = toggle),
//!   **Reload** (tap), **Surface** (tap = eject back to command — this REPLACES the two-finger
//!   gesture while embodied, since two fingers now mean move+look).
//! - **Buttons always win** (COD-Mobile): any finger inside a button circle presses it, even one
//!   that was mid-look-drag and slid onto it — a dragging look finger *releases* look the moment it
//!   crosses a button, so Fire/Crouch/etc. fire without lifting first.
//!
//! [`stick_base`]: TouchLayout::stick_base
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
/// Full-deflection radius of the move stick — and the radius of its visible ring — as a fraction
/// of min(viewport). Generous so the fixed ring is an easy thumb target (COD-Mobile forgiveness).
const STICK_RADIUS_FRAC: f32 = 0.16;
/// Fixed anchor of the move-stick ring centre: a fraction of width (x) / height (y), lower-left.
const STICK_ANCHOR_X_FRAC: f32 = 0.15;
const STICK_ANCHOR_Y_FRAC: f32 = 0.72;
/// Left edge of the drag-to-look region, as a fraction of width. Everything to the right is look
/// (buttons take hit priority); the move ring sits well left of this, and the narrow inert band
/// between them means a slightly-missed move touch does *nothing* rather than swinging the camera.
const LOOK_SPLIT_FRAC: f32 = 0.42;
/// Converts a look-region finger's pixel drag this frame into the look-axis units
/// `integrate_look_yaw`/`_pitch` expect (those were tuned for raw mouse-pixel deltas, so ~1.0 keeps
/// a touch drag feeling like a mouse drag; tune for device feel later).
const LOOK_DRAG_SCALE: f32 = 1.0;
/// The Jump button is a standard-size disc (the touch twin of the desktop Space hop). A one-shot
/// press edge — a mid-air re-press is ignored sim-height-side by `jump::start_jump`, so a held
/// finger can't pogo.
const JUMP_R_FRAC: f32 = 0.072;
/// The fire-mode (select-fire) button is standard-size too — a deliberate tap that toggles
/// semi ⇄ auto. It doubles as the on-screen fire-mode READOUT: the renderer draws its glyph from the
/// CURRENT mode (the host passes it), so the Android player can read semi-vs-auto at a glance (the
/// desktop player infers it; there is no other on-screen indicator).
const FIREMODE_R_FRAC: f32 = 0.072;

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
    /// The fixed move-stick ring: a finger-down **inside this circle** claims the stick; deflection
    /// is measured from its (fixed) centre and clamped to `r`. Drawn at the anchor every frame
    /// while embodied (discoverability), whether or not a finger is down.
    pub stick_base: Circle,
    /// The right region treated as the free drag-to-look area (buttons take hit priority over it).
    pub look_zone: Rect,
    pub fire: Circle,
    pub crouch: Circle,
    pub reload: Circle,
    pub surface: Circle,
    /// Jump button: a one-shot press edge that launches the cosmetic first-person hop (the touch twin
    /// of the desktop Space binding). Presentation only — the host feeds its edge to `jump::start_jump`
    /// (invariant #4/#5), never the sim.
    pub jump: Circle,
    /// Select-fire button: a one-shot press edge that toggles the embodied weapon between semi and
    /// full-auto (`fire::FireMode::toggled`). Its drawn glyph reflects the CURRENT mode, so it is also
    /// the on-screen fire-mode readout. Input preference only — never sim state (invariant #4/#5).
    pub fire_mode: Circle,
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
        let jump_r = JUMP_R_FRAC * m;
        let fire_mode_r = FIREMODE_R_FRAC * m;
        TouchLayout {
            width: w,
            height: h,
            // Fixed move-stick ring anchored in the lower-left. Its radius is both the visible ring
            // and the max-deflection distance, so what you see is exactly what activates + clamps.
            stick_base: Circle {
                cx: STICK_ANCHOR_X_FRAC * w,
                cy: STICK_ANCHOR_Y_FRAC * h,
                r: STICK_RADIUS_FRAC * m,
            },
            // The right side is drag-to-look; button circles below win the hit test inside it.
            look_zone: Rect {
                x0: LOOK_SPLIT_FRAC * w,
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
            // Lower-middle, left of Crouch and clear of the move ring: a right-hand thumb-reach hop.
            jump: Circle {
                cx: 0.58 * w,
                cy: 0.86 * h,
                r: jump_r,
            },
            // Upper-right of the thumb cluster, above Fire and clear of Reload: a deliberate mode tap
            // that also reads as the current-mode indicator.
            fire_mode: Circle {
                cx: 0.84 * w,
                cy: 0.46 * h,
                r: fire_mode_r,
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
    /// Jump button pressed this frame (the momentary press flash).
    pub jump_pressed: bool,
    /// Fire-mode button pressed this frame (the momentary press flash). The current-mode glyph is
    /// chosen host-side (the seam doesn't hold the `FireMode`).
    pub fire_mode_pressed: bool,
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
    /// Jump button press *edge* this frame — the host feeds it to `jump::start_jump` (the touch twin
    /// of `InputFrame.jump_pressed`).
    pub jump_edge: bool,
    /// Select-fire (fire-mode) button press *edge* this frame — the host feeds it to
    /// `fire::FireMode::toggled` (the touch twin of `InputFrame.select_fire_pressed`).
    pub fire_mode_edge: bool,
    /// What to draw this frame.
    pub hud: TouchHud,
}

/// Per-frame-persistent touch state: which finger owns which control, and last frame's button
/// presses (for edge detection). Lives on `Game`; reset on (un)embodiment via [`reset`](Self::reset).
#[derive(Clone, Debug)]
pub struct TouchControls {
    move_id: Option<u64>,
    look_id: Option<u64>,
    look_last: (f32, f32),
    prev_fire: bool,
    prev_crouch: bool,
    prev_reload: bool,
    prev_surface: bool,
    prev_jump: bool,
    prev_fire_mode: bool,
    /// Player look-sensitivity multiplier (`1.0` = stock) and pitch-invert, from the Compose shell's
    /// Settings — the touch twin of `pal-desktop::scale_look`. Applied to the drag-look `look_delta`
    /// inside [`update`](Self::update) (the raw finger *positions* are never scaled — that would
    /// corrupt the stick centre + button hit-tests). Presentation/input only, above `core`
    /// (invariant #1): floats are fine here. Persist across [`reset`](Self::reset) — a match-setup
    /// pref must survive every embody/surface toggle.
    look_sensitivity: f32,
    invert_y: bool,
}

impl Default for TouchControls {
    fn default() -> Self {
        Self {
            move_id: None,
            look_id: None,
            look_last: (0.0, 0.0),
            prev_fire: false,
            prev_crouch: false,
            prev_reload: false,
            prev_surface: false,
            prev_jump: false,
            prev_fire_mode: false,
            // Stock pass-through until the host pushes the player's Settings prefs.
            look_sensitivity: 1.0,
            invert_y: false,
        }
    }
}

impl TouchControls {
    pub fn new() -> Self {
        Self::default()
    }

    /// Forget all finger ownership + button history. Called when embodiment toggles so a stale
    /// finger from the command view (or a previous possession) never bleeds into a fresh one. The
    /// look prefs are a match-setup preference, not per-possession state, so they PERSIST across a
    /// reset (a fresh embodiment must keep the player's sensitivity / invert-Y).
    pub fn reset(&mut self) {
        let look_sensitivity = self.look_sensitivity;
        let invert_y = self.invert_y;
        *self = Self::default();
        self.look_sensitivity = look_sensitivity;
        self.invert_y = invert_y;
    }

    /// Set the player's embodied look prefs (Compose shell Settings). `sensitivity` multiplies the
    /// drag-look delta (`1.0` = stock); `invert_y` flips the pitch axis. The touch twin of
    /// `pal-desktop::DesktopInput::set_look_prefs` — desktop scales the raw mouse delta at the PAL
    /// boundary, but the Android look delta is produced *inside* this seam, so the scaling lives here.
    /// Applied in [`update`](Self::update); survives [`reset`](Self::reset).
    pub fn set_look_prefs(&mut self, sensitivity: f32, invert_y: bool) {
        self.look_sensitivity = sensitivity;
        self.invert_y = invert_y;
    }

    /// Is `id` the finger currently driving the stick or the look region? Such a finger can't be
    /// re-claimed by the *other* control (move ↔ look). Buttons are exempt — they win over an
    /// owning finger (COD-Mobile), so a look finger sliding onto Fire still fires.
    #[inline]
    fn owns(&self, id: u64) -> bool {
        self.move_id == Some(id) || self.look_id == Some(id)
    }

    /// Translate this frame's currently-down `touches` into embodied intents + HUD geometry.
    ///
    /// Ordering is deliberate: resolve the move finger, then the look finger (so a freshly claimed
    /// look finger can't also be the move finger), then hit-test buttons. **Buttons always win**
    /// (COD-Mobile): any finger inside a button circle presses it, and a look finger that slides
    /// onto a button releases look *this frame* so the button fires without a lift. A finger that
    /// lifts releases its control; the next finger down in that zone/ring re-captures it.
    pub fn update(&mut self, layout: &TouchLayout, touches: &[TouchSample]) -> TouchOutput {
        let find = |id: u64| {
            touches
                .iter()
                .find(|t| t.id == id)
                .map(|t| (t.x, t.y))
        };
        // A button press is ANY finger inside the circle — buttons win over look/move (COD-Mobile).
        let pressed = |c: &Circle| touches.iter().any(|t| c.contains(t.x, t.y));
        // The stick's neutral origin IS the fixed ring centre (this is a fixed, not floating, stick).
        let base = (layout.stick_base.cx, layout.stick_base.cy);

        // --- Move stick (fixed ring): claim a finger-down inside the ring; deflect from centre. ---
        if let Some(id) = self.move_id {
            if find(id).is_none() {
                self.move_id = None;
            }
        }
        if self.move_id.is_none() {
            if let Some(t) = touches
                .iter()
                .find(|t| !self.owns(t.id) && layout.stick_base.contains(t.x, t.y))
            {
                self.move_id = Some(t.id);
            }
        }
        let mut move_axis = (0.0, 0.0);
        let mut stick_thumb = base;
        let stick_active = self.move_id.is_some();
        if let Some(id) = self.move_id {
            if let Some((x, y)) = find(id) {
                let r = layout.stick_base.r.max(1.0);
                let dx = x - base.0;
                let dy = y - base.1;
                let len = (dx * dx + dy * dy).sqrt();
                let (cdx, cdy) = if len > r {
                    (dx * r / len, dy * r / len)
                } else {
                    (dx, dy)
                };
                stick_thumb = (base.0 + cdx, base.1 + cdy);
                move_axis = (cdx / r, cdy / r);
            }
        }

        // --- Look region (drag): per-frame motion of the owning finger → look delta. A look finger
        //     that lifts OR slides onto a button gives up look ownership (buttons win). ---
        let mut look_delta = (0.0, 0.0);
        if let Some(id) = self.look_id {
            match find(id) {
                Some((x, y)) if !Self::on_any_button(layout, x, y) => {
                    look_delta = (
                        (x - self.look_last.0) * LOOK_DRAG_SCALE,
                        (y - self.look_last.1) * LOOK_DRAG_SCALE,
                    );
                    self.look_last = (x, y);
                }
                // Finger gone, or it crossed onto a button: release look so the button can fire.
                _ => self.look_id = None,
            }
        }
        if self.look_id.is_none() {
            if let Some(t) = touches.iter().find(|t| {
                !self.owns(t.id)
                    && layout.look_zone.contains(t.x, t.y)
                    && !Self::on_any_button(layout, t.x, t.y)
            }) {
                self.look_id = Some(t.id);
                self.look_last = (t.x, t.y);
                // No delta on the capture frame (avoids a jump from the first contact point).
            }
        }

        // Apply the player's look prefs to the drag delta — the touch twin of
        // `pal-desktop::scale_look`: scale BOTH axes by `look_sensitivity` and flip the PITCH (y) when
        // `invert_y`. The raw finger *positions* are never scaled (that would corrupt the stick centre
        // + button hit-tests, which are position-based); only this per-frame delta is. A zero delta
        // (holding still / no look finger) stays zero. Presentation/input only, above `core`.
        let look_delta = (
            look_delta.0 * self.look_sensitivity,
            (if self.invert_y { -look_delta.1 } else { look_delta.1 }) * self.look_sensitivity,
        );

        // --- Buttons: pressed = ANY finger inside the circle (buttons win); edges vs last frame. ---
        let fire = pressed(&layout.fire);
        // ADS is HELD (the zoom level signal), exactly like Fire — no edge detection.
        let aim = pressed(&layout.aim);
        let crouch = pressed(&layout.crouch);
        let reload = pressed(&layout.reload);
        let surface = pressed(&layout.surface);
        // Jump + select-fire are one-shot press EDGES (like Crouch/Reload/Surface): the host feeds
        // them to `jump::start_jump` / `fire::FireMode::toggled`, the touch twins of the desktop
        // `InputFrame.jump_pressed` / `.select_fire_pressed` keys.
        let jump = pressed(&layout.jump);
        let fire_mode = pressed(&layout.fire_mode);

        let out = TouchOutput {
            move_axis,
            look_delta,
            fire,
            aim,
            crouch_edge: crouch && !self.prev_crouch,
            reload_edge: reload && !self.prev_reload,
            surface_edge: surface && !self.prev_surface,
            jump_edge: jump && !self.prev_jump,
            fire_mode_edge: fire_mode && !self.prev_fire_mode,
            hud: TouchHud {
                stick_active,
                stick_origin: base,
                stick_thumb,
                fire_pressed: fire,
                crouch_pressed: crouch,
                reload_pressed: reload,
                surface_pressed: surface,
                jump_pressed: jump,
                fire_mode_pressed: fire_mode,
                aim_pressed: aim,
            },
        };

        self.prev_fire = fire;
        self.prev_crouch = crouch;
        self.prev_reload = reload;
        self.prev_surface = surface;
        self.prev_jump = jump;
        self.prev_fire_mode = fire_mode;
        out
    }

    /// Is `(x, y)` inside any button circle? Keeps the drag-look claim from stealing a button tap,
    /// and releases a look finger that slides onto a button so the button wins.
    #[inline]
    fn on_any_button(layout: &TouchLayout, x: f32, y: f32) -> bool {
        layout.fire.contains(x, y)
            || layout.aim.contains(x, y)
            || layout.crouch.contains(x, y)
            || layout.reload.contains(x, y)
            || layout.surface.contains(x, y)
            || layout.jump.contains(x, y)
            || layout.fire_mode.contains(x, y)
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

    /// The fixed ring centre (a guaranteed neutral-origin hit).
    fn stick_center(l: &TouchLayout) -> (f32, f32) {
        (l.stick_base.cx, l.stick_base.cy)
    }

    #[test]
    fn ring_finger_drives_move_axis_from_the_fixed_centre() {
        let l = layout();
        let mut tc = TouchControls::new();
        // Finger down at the ring centre: neutral (deflection measured from the FIXED centre).
        let (cx, cy) = stick_center(&l);
        let out = tc.update(&l, &[t(1, cx, cy)]);
        assert_eq!(out.move_axis, (0.0, 0.0), "a touch at the centre is neutral");
        assert!(out.hud.stick_active);
        assert_eq!(out.hud.stick_origin, (cx, cy), "the origin is the fixed ring centre");

        // Push right + up by half the radius: axis is the normalized offset from the centre.
        let dx = l.stick_base.r * 0.5;
        let out = tc.update(&l, &[t(1, cx + dx, cy - dx)]);
        assert!((out.move_axis.0 - 0.5).abs() < 1e-5, "right half-deflection");
        assert!((out.move_axis.1 + 0.5).abs() < 1e-5, "up is -y (screen convention)");
    }

    #[test]
    fn move_axis_clamps_to_unit_magnitude_beyond_the_radius() {
        let l = layout();
        let mut tc = TouchControls::new();
        let (cx, cy) = stick_center(&l);
        tc.update(&l, &[t(1, cx, cy)]);
        // Shove far past the radius straight down: magnitude clamps to 1.
        let out = tc.update(&l, &[t(1, cx, cy + l.stick_base.r * 10.0)]);
        let mag = (out.move_axis.0 * out.move_axis.0 + out.move_axis.1 * out.move_axis.1).sqrt();
        assert!((mag - 1.0).abs() < 1e-5, "deflection clamps to the unit circle, got {mag}");
        assert!(out.move_axis.1 > 0.99, "straight down is +y");
    }

    #[test]
    fn a_touch_just_outside_the_ring_does_nothing_not_look() {
        // THE reported bug: a move-intent touch that lands just outside the ring must NOT be
        // re-classified as a look drag (which would swing the camera). It is simply inert — the
        // player re-touches the visible ring. This is the whole reason the stick is fixed.
        let l = layout();
        let mut tc = TouchControls::new();
        let (cx, cy) = stick_center(&l);
        // A point just past the ring, still on the left (well clear of the look zone).
        let p = (cx + l.stick_base.r + 8.0, cy);
        assert!(!l.stick_base.contains(p.0, p.1), "precondition: outside the ring");
        assert!(!l.look_zone.contains(p.0, p.1), "precondition: not in the look zone either");
        let out = tc.update(&l, &[t(1, p.0, p.1)]);
        assert!(!out.hud.stick_active, "no stick claim outside the ring");
        assert_eq!(out.move_axis, (0.0, 0.0), "and no movement");
        // Dragging that inert finger produces NO look delta — it never became a look finger.
        let out = tc.update(&l, &[t(1, p.0 + 40.0, p.1)]);
        assert_eq!(out.look_delta, (0.0, 0.0), "an off-ring left touch never pans the camera");
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
        let stick_o = stick_center(&l);

        // Establish stick + an ADS hold (frame 1).
        tc.update(&l, &[t(1, stick_o.0, stick_o.1), t(2, ax, ay)]);
        // Frame 2: deflect the stick, drag the ADS finger, and add a Fire finger — all independent.
        let out = tc.update(
            &l,
            &[
                t(1, stick_o.0, stick_o.1 + l.stick_base.r), // full down
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
        let stick_o = stick_center(&l);
        let look_o = (l.look_zone.x0 + 150.0, 120.0);
        let (firex, firey) = center(&l.fire);

        // Establish stick + look fingers (frame 1).
        tc.update(&l, &[t(1, stick_o.0, stick_o.1), t(2, look_o.0, look_o.1)]);
        // Frame 2: deflect stick down, drag look right, hold fire — all three resolve independently.
        let out = tc.update(
            &l,
            &[
                t(1, stick_o.0, stick_o.1 + l.stick_base.r), // full down
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
        let (cx, cy) = stick_center(&l);
        tc.update(&l, &[t(1, cx, cy)]);
        let out = tc.update(&l, &[t(1, cx + l.stick_base.r, cy)]);
        assert!(out.move_axis.0 > 0.99, "finger 1 drives the stick right");
        // Lift finger 1: stick goes neutral/inactive, thumb snaps back to the fixed centre.
        let out = tc.update(&l, &[]);
        assert!(!out.hud.stick_active);
        assert_eq!(out.move_axis, (0.0, 0.0));
        assert_eq!(out.hud.stick_thumb, (cx, cy), "released thumb rests at the fixed centre");
        // A NEW finger anywhere in the ring re-captures the stick — origin is ALWAYS the fixed
        // centre (fixed stick), and deflection is measured from there.
        let out = tc.update(&l, &[t(9, cx - l.stick_base.r * 0.5, cy)]);
        assert!(out.hud.stick_active);
        assert_eq!(out.hud.stick_origin, (cx, cy), "origin stays the fixed ring centre");
        assert!((out.move_axis.0 + 0.5).abs() < 1e-5, "left half-deflection from the centre");
    }

    #[test]
    fn a_look_finger_sliding_onto_fire_fires_without_lifting() {
        // COD-Mobile "buttons win": a right-thumb finger that is drag-looking and then slides onto
        // the Fire button must FIRE — not keep panning, and not require a lift-and-retap. This is
        // the "fire doesn't work immediately" half of the report.
        let l = layout();
        let mut tc = TouchControls::new();
        let (fx, fy) = center(&l.fire);
        // Start looking in the bare right region (clear of every button).
        let start = (l.look_zone.x0 + 250.0, 90.0);
        assert!(!TouchControls::on_any_button(&l, start.0, start.1), "precondition: not on a button");
        tc.update(&l, &[t(5, start.0, start.1)]);
        let out = tc.update(&l, &[t(5, start.0 + 20.0, start.1)]);
        assert!(out.look_delta.0 > 0.0, "it is looking first");
        assert!(!out.fire, "and not yet firing");
        // Same finger slides onto Fire: it fires, and look stops the moment it crosses the button.
        let out = tc.update(&l, &[t(5, fx, fy)]);
        assert!(out.fire, "the look finger now fires (buttons win)");
        assert_eq!(out.look_delta, (0.0, 0.0), "and it stops panning the camera");
        assert!(tc.look_id.is_none(), "look ownership was released to the button");
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
        for c in [l.fire, l.aim, l.crouch, l.reload, l.surface, l.jump, l.fire_mode] {
            assert!(c.cx - c.r >= 0.0 && c.cx + c.r <= l.width, "button within width");
            assert!(c.cy - c.r >= 0.0 && c.cy + c.r <= l.height, "button within height");
        }
        // The new Jump + fire-mode buttons must not overlap any existing control (a shared touch
        // point would fire two intents at once).
        let no_overlap = |a: &Circle, b: &Circle| {
            let dx = a.cx - b.cx;
            let dy = a.cy - b.cy;
            (dx * dx + dy * dy).sqrt() >= a.r + b.r
        };
        for other in [l.fire, l.crouch, l.reload, l.surface, l.aim] {
            assert!(no_overlap(&l.jump, &other), "jump overlaps another button");
            assert!(no_overlap(&l.fire_mode, &other), "fire-mode overlaps another button");
        }
        assert!(no_overlap(&l.jump, &l.fire_mode), "jump overlaps fire-mode");
        // Jump sits clear of the move ring too (it lives in/near the look zone, not on the stick).
        assert!(
            l.jump.cx - l.jump.r > l.stick_base.cx + l.stick_base.r,
            "jump is clear of the move ring"
        );
        // The move ring sits on the left, entirely clear of the look zone (its right edge is left
        // of the look split) — so the two controls can never claim the same touch.
        assert!(l.stick_base.cx + l.stick_base.r < l.look_zone.x0, "ring is clear of the look zone");
        assert!(l.stick_base.cx - l.stick_base.r >= 0.0, "ring stays on-screen");
        assert!(l.stick_base.cy + l.stick_base.r <= l.height, "ring stays on-screen");
    }

    #[test]
    fn jump_button_is_a_one_shot_press_edge() {
        // Parity with the desktop Space hop: a Jump tap emits exactly one edge on the press frame and
        // does NOT re-edge while held (a held finger can't pogo — `jump::start_jump` also gates on
        // grounded, but the seam already refuses the repeat here).
        let l = layout();
        let mut tc = TouchControls::new();
        let (jx, jy) = center(&l.jump);
        let out = tc.update(&l, &[t(1, jx, jy)]);
        assert!(out.jump_edge, "press edge on first contact");
        assert!(out.hud.jump_pressed, "the press flash tracks the held button");
        let out = tc.update(&l, &[t(1, jx, jy)]);
        assert!(!out.jump_edge, "no repeat edge while held");
        // Release then re-press: a fresh edge.
        tc.update(&l, &[]);
        let out = tc.update(&l, &[t(1, jx, jy)]);
        assert!(out.jump_edge, "re-press is a fresh edge");
    }

    #[test]
    fn fire_mode_button_is_a_one_shot_press_edge() {
        // The select-fire toggle: one edge per tap, so a single tap flips semi ⇄ auto exactly once
        // (the host consumes the edge via `fire::FireMode::toggled`).
        let l = layout();
        let mut tc = TouchControls::new();
        let (fx, fy) = center(&l.fire_mode);
        let out = tc.update(&l, &[t(2, fx, fy)]);
        assert!(out.fire_mode_edge, "press edge on first contact");
        assert!(out.hud.fire_mode_pressed);
        let out = tc.update(&l, &[t(2, fx, fy)]);
        assert!(!out.fire_mode_edge, "held does not re-toggle");
        tc.update(&l, &[]);
        let out = tc.update(&l, &[t(2, fx, fy)]);
        assert!(out.fire_mode_edge, "re-press is a fresh edge");
    }

    #[test]
    fn jump_and_fire_mode_buttons_win_over_look() {
        // Both sit inside the look zone; like every other button they must WIN the hit test so a
        // right-thumb finger on them never also drives the camera (COD-Mobile "buttons win").
        let l = layout();
        assert!(l.look_zone.contains(l.jump.cx, l.jump.cy), "precondition: jump is in the look zone");
        assert!(
            l.look_zone.contains(l.fire_mode.cx, l.fire_mode.cy),
            "precondition: fire-mode is in the look zone"
        );
        let mut tc = TouchControls::new();
        let (jx, jy) = center(&l.jump);
        let out = tc.update(&l, &[t(1, jx, jy)]);
        assert!(out.jump_edge);
        assert_eq!(out.look_delta, (0.0, 0.0), "the jump finger never drives look");
        // Even dragging the held jump finger produces no look delta.
        let out = tc.update(&l, &[t(1, jx + 40.0, jy)]);
        assert_eq!(out.look_delta, (0.0, 0.0));
    }

    #[test]
    fn look_delta_scales_with_sensitivity() {
        // The touch twin of `pal-desktop::scale_look`: the drag delta multiplies by the sensitivity.
        let l = layout();
        let mut tc = TouchControls::new();
        tc.set_look_prefs(2.0, false);
        let start = (l.look_zone.x0 + 200.0, 200.0);
        tc.update(&l, &[t(7, start.0, start.1)]); // capture frame (no delta)
        // Drag right 30 px, up 10 px → each axis scaled by 2.0 (pitch NOT inverted).
        let out = tc.update(&l, &[t(7, start.0 + 30.0, start.1 - 10.0)]);
        assert!((out.look_delta.0 - 60.0 * LOOK_DRAG_SCALE).abs() < 1e-4, "x scaled ×2");
        assert!((out.look_delta.1 - (-20.0) * LOOK_DRAG_SCALE).abs() < 1e-4, "y scaled ×2, sign kept");
    }

    #[test]
    fn look_delta_inverts_pitch_only_when_invert_y() {
        // invert_y flips the PITCH (y) sign and leaves yaw (x) alone — again mirroring `scale_look`.
        let l = layout();
        let mut tc = TouchControls::new();
        tc.set_look_prefs(1.0, true);
        let start = (l.look_zone.x0 + 200.0, 200.0);
        tc.update(&l, &[t(7, start.0, start.1)]);
        let out = tc.update(&l, &[t(7, start.0 + 30.0, start.1 - 10.0)]);
        assert!((out.look_delta.0 - 30.0 * LOOK_DRAG_SCALE).abs() < 1e-4, "yaw unaffected by invert");
        assert!((out.look_delta.1 - 10.0 * LOOK_DRAG_SCALE).abs() < 1e-4, "pitch flipped: -10 → +10");
    }

    #[test]
    fn look_prefs_combine_scale_and_invert() {
        // Sensitivity AND invert together: y = -raw.y * sensitivity, x = raw.x * sensitivity.
        let l = layout();
        let mut tc = TouchControls::new();
        tc.set_look_prefs(0.5, true);
        let start = (l.look_zone.x0 + 200.0, 200.0);
        tc.update(&l, &[t(7, start.0, start.1)]);
        let out = tc.update(&l, &[t(7, start.0 + 40.0, start.1 + 20.0)]);
        assert!((out.look_delta.0 - 20.0).abs() < 1e-4, "x: 40 × 0.5 = 20");
        assert!((out.look_delta.1 - (-10.0)).abs() < 1e-4, "y: -(20) × 0.5 = -10");
    }

    #[test]
    fn look_prefs_survive_a_reset() {
        // The prefs are match-setup, not per-possession — an embody/surface reset must keep them.
        let l = layout();
        let mut tc = TouchControls::new();
        tc.set_look_prefs(2.5, true);
        tc.reset();
        let start = (l.look_zone.x0 + 200.0, 200.0);
        tc.update(&l, &[t(7, start.0, start.1)]);
        let out = tc.update(&l, &[t(7, start.0 + 10.0, start.1 + 10.0)]);
        assert!((out.look_delta.0 - 25.0).abs() < 1e-4, "sensitivity persisted through reset");
        assert!((out.look_delta.1 - (-25.0)).abs() < 1e-4, "invert persisted through reset");
    }

    #[test]
    fn default_look_prefs_are_a_stock_pass_through() {
        // Without any host push the seam must behave exactly as before (×1.0, no invert) so the
        // shipped feel is unchanged.
        let l = layout();
        let mut tc = TouchControls::new();
        let start = (l.look_zone.x0 + 200.0, 200.0);
        tc.update(&l, &[t(7, start.0, start.1)]);
        let out = tc.update(&l, &[t(7, start.0 + 30.0, start.1 - 10.0)]);
        assert!((out.look_delta.0 - 30.0 * LOOK_DRAG_SCALE).abs() < 1e-4);
        assert!((out.look_delta.1 - (-10.0) * LOOK_DRAG_SCALE).abs() < 1e-4);
    }
}
