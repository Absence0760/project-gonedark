//! Embodied **jump** seam — the PURE, GPU-free math for the standard-FPS Space hop.
//!
//! **Presentation/input only (invariant #4/#5).** The deterministic sim is 2-D fixed-point with no
//! vertical axis (units live on the ground plane), so a jump is NOT a sim state change and NOT a
//! character system — it is a transient upward offset the host adds to the embodied camera eye (and
//! the viewmodel) for a fraction of a second, exactly like the recoil view-kick. It is stepped off
//! the **wall-clock `dt`** (frame-rate independent), lives entirely on `Game` as host state, and can
//! never reach `&mut Sim` — so it adds no desync surface (invariant #7). Floats are fine here: this
//! is the render/input boundary, never the sim.
//!
//! The state is a single `jump_t` timer counting DOWN from [`JUMP_DURATION`] to `0` (grounded). A
//! new jump is only accepted while grounded (`jump_t <= 0`), so holding Space can't pogo mid-air.

/// How long one hop lasts, in seconds (launch → apex → land).
pub const JUMP_DURATION: f32 = 0.45;

/// Peak height of the hop, in world units (the eye rises this far at the apex). Kept modest — a
/// readable bob, not a moon-jump — since it moves the camera, not a real physics body.
pub const JUMP_HEIGHT: f32 = 0.85;

/// Start a hop on the Space press `edge`, but ONLY while grounded (`jump_t <= 0`) — a mid-air press
/// is ignored, so there is no double-jump / pogo. Returns the (possibly re-armed) timer. Pure.
#[inline]
pub fn start_jump(jump_t: f32, edge: bool) -> f32 {
    if edge && grounded(jump_t) {
        JUMP_DURATION
    } else {
        jump_t
    }
}

/// Advance the hop timer by one frame of `dt` seconds toward `0` (grounded), clamped so it never
/// goes negative and a non-positive `dt` is a no-op. Frame-rate independent (same wall-clock arc at
/// 30/60/120 fps), exactly like `recoil::decay_recoil`. Pure.
#[inline]
pub fn step_jump(jump_t: f32, dt: f32) -> f32 {
    (jump_t - dt.max(0.0)).max(0.0)
}

/// Whether the avatar is on the ground (no hop in progress) — the gate `start_jump` uses.
#[inline]
pub fn grounded(jump_t: f32) -> bool {
    jump_t <= 0.0
}

/// The current hop height (world units) for the timer — a parabolic arc that is `0` at launch and
/// land and peaks at [`JUMP_HEIGHT`] at the apex (mid-flight). `0` while grounded. The host adds this
/// to the embodied eye's Z (and a fraction to the viewmodel). Pure → host-tested.
#[inline]
pub fn jump_height(jump_t: f32) -> f32 {
    if jump_t <= 0.0 {
        return 0.0;
    }
    // u: 0 at launch → 1 at land. 4·u·(1−u) is the standard 0→1→0 parabola, peaking at u = 0.5.
    let u = 1.0 - (jump_t / JUMP_DURATION).clamp(0.0, 1.0);
    JUMP_HEIGHT * 4.0 * u * (1.0 - u)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_press_launches_only_from_the_ground() {
        // Grounded + edge → arm the full timer.
        assert_eq!(start_jump(0.0, true), JUMP_DURATION);
        // Mid-air press is ignored (no double jump).
        assert_eq!(start_jump(0.3, true), 0.3, "a press mid-hop does not re-launch");
        // No edge → unchanged.
        assert_eq!(start_jump(0.0, false), 0.0);
    }

    #[test]
    fn the_timer_decays_to_the_ground_and_stops() {
        let t = start_jump(0.0, true);
        assert!(!grounded(t));
        let landed = step_jump(t, JUMP_DURATION + 1.0); // overshoot clamps at 0
        assert_eq!(landed, 0.0);
        assert!(grounded(landed));
        // A negative/zero dt never bumps the timer.
        assert_eq!(step_jump(0.2, 0.0), 0.2);
        assert_eq!(step_jump(0.2, -1.0), 0.2);
    }

    #[test]
    fn height_is_a_zero_apex_zero_parabola() {
        assert_eq!(jump_height(0.0), 0.0, "grounded → no rise");
        // Launch (timer full) and land (timer ~0) are both near the ground.
        assert!(jump_height(JUMP_DURATION) < 1e-4, "just launched, still low");
        // The apex (timer at half) is the peak.
        let apex = jump_height(JUMP_DURATION * 0.5);
        assert!((apex - JUMP_HEIGHT).abs() < 1e-4, "apex reaches JUMP_HEIGHT (got {apex})");
        // Monotone up on the way to the apex.
        assert!(jump_height(JUMP_DURATION * 0.75) < apex, "rising toward the apex");
        assert!(jump_height(JUMP_DURATION * 0.25) < apex, "falling from the apex");
    }
}
