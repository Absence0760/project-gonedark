//! Command-view **on-screen touch buttons** — the RTS half's mobile input affordance.
//!
//! The desktop arms build/train/upgrade off the B/R/H/U keys; on a touchscreen there is no
//! keyboard, so the matching `InputFrame` intents (`train_slot` / `upgrade_pressed` /
//! `building_slot`) had no way to be set — the buttons were "deferred" (see the `InputFrame` field
//! docs). This is that deferred surface: a row of labelled buttons along the bottom of the command
//! view. A tap inside a button arms the same intent the desktop key does.
//!
//! Split along the CLAUDE.md *"extract the pure logic to a testable seam"* rule, exactly like the
//! embodied [`touch_controls`](crate::touch_controls): the **geometry + hit-test live here** (plain
//! pixel math, no GPU / no winit / no android types → host-unit-tested below); the engine converts
//! the layout to render's [`CommandBarView`](gonedark_render::command_bar::CommandBarView) (px → NDC)
//! and the renderer just draws it (invariant #2 — render never sees this module). Because both the
//! hit shapes *and* the drawn shapes derive from the **same** layout, the button you see is the
//! button you tap (the no-drift discipline the embodied HUD also keeps).
//!
//! Command view only — never built or drawn while embodied (invariant #6: these are command-layer
//! actions; the embodied view stays dark).

use gonedark_render::command_bar::{CommandBarButton, CommandBarView};

/// Minimum button height as a fraction of the smaller viewport dimension. `0.12·min(w,h)` clears the
/// ~44–48 dp Android/iOS touch-target floor at common phone densities (e.g. `0.12·1080 px ≈ 130 px ≈
/// 43 dp` at 3×, and taller on higher-density panels) — the old `0.085` landed ~30 dp on a 1080-wide
/// phone, well under the floor and easy to fat-finger. Width stays generous (`bw`) for the label.
const BUTTON_H_RATIO: f32 = 0.12;

/// One command-view action a button arms. The engine maps each to an `InputFrame` command intent
/// (Train\* → `train_slot`, `Upgrade` → `upgrade_pressed`); the slot integers live at that boundary,
/// not here, so this seam stays vocabulary-agnostic.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CommandButton {
    /// Queue a Rifleman at the active camp.
    TrainRifleman,
    /// Queue a Heavy at the active camp.
    TrainHeavy,
    /// Upgrade the active camp one tier.
    Upgrade,
}

impl CommandButton {
    /// The on-button label (short, legible at the touch size).
    fn label(self) -> &'static str {
        match self {
            CommandButton::TrainRifleman => "RIFLE",
            CommandButton::TrainHeavy => "HEAVY",
            CommandButton::Upgrade => "UPGRADE",
        }
    }

    /// The fixed left-to-right order the bar lays buttons out in.
    const ORDER: [CommandButton; 3] = [
        CommandButton::TrainRifleman,
        CommandButton::TrainHeavy,
        CommandButton::Upgrade,
    ];
}

/// A screen-space rectangle in pixels — a button's hit shape (and, converted to NDC, its draw shape).
#[derive(Clone, Copy, Debug, PartialEq)]
struct Rect {
    x0: f32,
    y0: f32,
    x1: f32,
    y1: f32,
}

impl Rect {
    #[inline]
    fn cx(&self) -> f32 {
        0.5 * (self.x0 + self.x1)
    }
    #[inline]
    fn cy(&self) -> f32 {
        0.5 * (self.y0 + self.y1)
    }
    #[inline]
    fn hw(&self) -> f32 {
        0.5 * (self.x1 - self.x0)
    }
    #[inline]
    fn hh(&self) -> f32 {
        0.5 * (self.y1 - self.y0)
    }
}

/// The laid-out command bar: each [`CommandButton`] with its pixel hit rect, for the current
/// viewport. Rebuilt cheaply each frame from `(width, height)`; the engine hit-tests taps against it
/// and converts it to the render view, so the two never disagree.
#[derive(Clone, Debug, PartialEq)]
pub struct CommandBarLayout {
    buttons: Vec<(CommandButton, Rect)>,
}

impl CommandBarLayout {
    /// Lay the buttons out as a left-aligned row hugging the bottom edge. Sizes scale with the
    /// smaller viewport dimension so the bar reads the same on any aspect/DPI (the embodied HUD
    /// scales the same way). The right side of the screen is left clear (it is the drag/aim zone
    /// while embodied — keeping the command bar off it avoids a muscle-memory clash).
    pub fn new(width: u32, height: u32) -> Self {
        let w = width as f32;
        let h = height as f32;
        let m = w.min(h).max(1.0);
        let bw = m * 0.20; // button width (wide enough for the longest label, "UPGRADE")
        let bh = m * BUTTON_H_RATIO; // button height (clears the mobile touch-target floor)
        let gap = m * 0.02;
        let margin = m * 0.03;
        let y1 = (h - margin).max(bh);
        let y0 = y1 - bh;
        let buttons = CommandButton::ORDER
            .iter()
            .enumerate()
            .map(|(i, &kind)| {
                let x0 = margin + i as f32 * (bw + gap);
                (
                    kind,
                    Rect {
                        x0,
                        y0,
                        x1: x0 + bw,
                        y1,
                    },
                )
            })
            .collect();
        CommandBarLayout { buttons }
    }

    /// Which button (if any) a tap at pixel `(x, y)` lands in. The first containing rect wins; the
    /// layout never overlaps buttons, so the result is unambiguous.
    pub fn button_at(&self, x: f32, y: f32) -> Option<CommandButton> {
        self.button_at_scaled(x, y, 1.0)
    }

    /// [`button_at`] with the physical `ui_scale` applied — the hit-test twin of the renderer's
    /// scaled DRAW (`render::command_bar::command_bar_quads_scaled`, which inflates each button's
    /// half-extents by `ui_scale` about its center). Scaling the hit rect the same way keeps the
    /// tappable region on the button as drawn; without it, at `ui_scale != 1` (dense phone, retina
    /// desktop) the caption + box draw large but the tap target stays at its 1.0 size and a tap on
    /// the visible button misses. `ui_scale == 1.0` is byte-identical to the legacy hit-test.
    pub fn button_at_scaled(&self, x: f32, y: f32, ui_scale: f32) -> Option<CommandButton> {
        let s = ui_scale.max(0.0);
        self.buttons
            .iter()
            .find(|(_, r)| {
                let hw = r.hw() * s;
                let hh = r.hh() * s;
                (x - r.cx()).abs() <= hw && (y - r.cy()).abs() <= hh
            })
            .map(|(k, _)| *k)
    }

    /// Convert the pixel layout to the renderer's NDC [`CommandBarView`]. `(+y` up, origin center).
    /// The same `(width, height)` used to build the layout MUST be passed so the NDC boxes line up
    /// with the hit rects exactly (no drift).
    pub fn to_view(&self, width: u32, height: u32) -> CommandBarView {
        let w = (width as f32).max(1.0);
        let h = (height as f32).max(1.0);
        let buttons = self
            .buttons
            .iter()
            .map(|(kind, r)| CommandBarButton {
                ndc_x: 2.0 * r.cx() / w - 1.0,
                ndc_y: 1.0 - 2.0 * r.cy() / h,
                half_x: r.hw() / w * 2.0,
                half_y: r.hh() / h * 2.0,
                label: kind.label().to_string(),
            })
            .collect();
        CommandBarView { buttons }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const W: u32 = 2340;
    const H: u32 = 1080;

    #[test]
    fn a_tap_in_each_button_center_resolves_to_that_button() {
        let bar = CommandBarLayout::new(W, H);
        for (kind, r) in &bar.buttons {
            assert_eq!(
                bar.button_at(r.cx(), r.cy()),
                Some(*kind),
                "center of {kind:?} should hit it"
            );
        }
    }

    #[test]
    fn scaled_hit_test_matches_the_scaled_draw_and_is_identity_at_one() {
        // `button_at_scaled` grows the hit region about each button's center to track the renderer's
        // scaled DRAW. NOTE: scaling a *row* of buttons about their centers makes neighbours overlap
        // past ~1.5×, which is exactly why the raw display scale is NOT forwarded to the command bar
        // (see `Game::render_ui_scale` — the raw density overflows tiling rows; a device-tuned scale
        // is owed). We only assert the two safe guarantees the seam must hold: identity at 1.0, and
        // that a button's own center keeps hitting it when the region grows.
        let bar = CommandBarLayout::new(W, H);
        let (kind, r) = bar.buttons[0];
        assert_eq!(
            bar.button_at_scaled(r.cx(), r.cy(), 1.0),
            bar.button_at(r.cx(), r.cy()),
            "1× is byte-identical to the legacy hit-test"
        );
        assert_eq!(
            bar.button_at_scaled(r.cx(), r.cy(), 1.8),
            Some(kind),
            "a button's own center keeps hitting it as the region grows"
        );
    }

    #[test]
    fn taps_outside_the_bar_hit_nothing() {
        let bar = CommandBarLayout::new(W, H);
        // Top-center (the map), and far right (the embodied drag zone) — both clear of the bar.
        assert_eq!(bar.button_at(W as f32 * 0.5, H as f32 * 0.2), None);
        assert_eq!(bar.button_at(W as f32 - 1.0, H as f32 * 0.5), None);
        // Just above the row.
        let top = bar.buttons[0].1.y0;
        assert_eq!(bar.button_at(bar.buttons[0].1.cx(), top - 5.0), None);
    }

    #[test]
    fn the_gap_between_two_buttons_is_dead_space() {
        let bar = CommandBarLayout::new(W, H);
        let (_, a) = bar.buttons[0];
        let (_, b) = bar.buttons[1];
        let mid_x = 0.5 * (a.x1 + b.x0);
        assert!(a.x1 < b.x0, "buttons don't overlap");
        assert_eq!(bar.button_at(mid_x, a.cy()), None, "the gap hits nothing");
    }

    #[test]
    fn the_three_actions_lay_out_left_to_right_in_order() {
        let bar = CommandBarLayout::new(W, H);
        let kinds: Vec<_> = bar.buttons.iter().map(|(k, _)| *k).collect();
        assert_eq!(
            kinds,
            vec![
                CommandButton::TrainRifleman,
                CommandButton::TrainHeavy,
                CommandButton::Upgrade,
            ]
        );
        // Strictly increasing left edge.
        assert!(bar.buttons[0].1.x0 < bar.buttons[1].1.x0);
        assert!(bar.buttons[1].1.x0 < bar.buttons[2].1.x0);
    }

    #[test]
    fn the_view_has_one_labelled_button_per_action_inside_ndc() {
        let bar = CommandBarLayout::new(W, H);
        let view = bar.to_view(W, H);
        assert_eq!(view.buttons.len(), 3);
        assert_eq!(view.buttons[0].label, "RIFLE");
        assert_eq!(view.buttons[1].label, "HEAVY");
        assert_eq!(view.buttons[2].label, "UPGRADE");
        for b in &view.buttons {
            assert!((-1.0..=1.0).contains(&b.ndc_x), "x in NDC");
            assert!((-1.0..=1.0).contains(&b.ndc_y), "y in NDC");
            assert!(b.half_x > 0.0 && b.half_y > 0.0);
            assert!(b.ndc_y < 0.0, "the bar sits in the bottom half");
        }
    }

    #[test]
    fn hit_rect_and_drawn_box_agree_no_drift() {
        // The NDC box the renderer draws must map back to the same pixel rect the hit-test uses.
        let bar = CommandBarLayout::new(W, H);
        let view = bar.to_view(W, H);
        for ((_, r), b) in bar.buttons.iter().zip(view.buttons.iter()) {
            let px_cx = (b.ndc_x + 1.0) * 0.5 * W as f32;
            let px_cy = (1.0 - b.ndc_y) * 0.5 * H as f32;
            assert!((px_cx - r.cx()).abs() < 0.5, "center x round-trips");
            assert!((px_cy - r.cy()).abs() < 0.5, "center y round-trips");
        }
    }

    #[test]
    fn button_height_clears_the_touch_target_floor() {
        // Regression (mobile touch floor): the button height must stay >= BUTTON_H_RATIO·min(w,h) so
        // it clears ~44–48 dp at common phone densities — the old 0.085·min(w,h) landed ~30 dp on a
        // 1080-wide phone, under the floor. Checked on representative portrait phones.
        for (w, h) in [(1080u32, 2340u32), (750, 1334), (1440, 3200)] {
            let bar = CommandBarLayout::new(w, h);
            let m = w.min(h) as f32;
            for (kind, r) in &bar.buttons {
                let ratio = (r.y1 - r.y0) / m;
                assert!(
                    ratio >= BUTTON_H_RATIO - 1e-6,
                    "{w}x{h}: {kind:?} height ratio {ratio} is below the {BUTTON_H_RATIO} touch floor"
                );
            }
        }
    }

    #[test]
    fn layout_scales_with_viewport_and_stays_on_screen() {
        for (w, h) in [(1280u32, 720u32), (2340, 1080), (800, 1600)] {
            let bar = CommandBarLayout::new(w, h);
            for (_, r) in &bar.buttons {
                assert!(r.x0 >= 0.0 && r.x1 <= w as f32, "within width {w}");
                assert!(r.y0 >= 0.0 && r.y1 <= h as f32, "within height {h}");
            }
        }
    }
}
