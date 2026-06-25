//! Command-view **readouts** (W6 — command-view polish).
//!
//! The top-down view drew units but told the player no *numbers* — how many units do I have, how
//! many does the enemy, how many control points are in play? This module derives a small HUD tally
//! from the data the renderer ALREADY holds (the interpolated [`UnitInstance`]s from
//! [`crate::Renderer::prepare`]) and lays it out as corner labels, drawn via the W4
//! [`text`](crate::text) pass. It adds **no new sim read and no engine plumbing** — it is a pure
//! function of the draw set the command pass already has in hand.
//!
//! ## Why count from instances (and the honest limit)
//!
//! The renderer is the float boundary and never calls back into `core`; the only command-view data
//! it is handed is the instance list. So the readouts count what is *visible on the command frame*:
//! player units, enemy units, and control points, classified by the per-instance color/flags the
//! interpolator already baked (`faction_color` / [`crate::FLAG_RING`]). That is exactly the right
//! number to surface in a fog-of-war RTS — it is what the commander can *see*, not omniscient truth.
//!
//! ## Resource / economy readout (the seam, now filled)
//!
//! True resource/economy numbers (banked credits, income) live in the sim and are never sent to
//! the renderer, which is the float boundary and never calls back into `core` at runtime
//! (invariant #4). So the renderer cannot *read* them — but it can *lay them out* once a host
//! hands them in as plain data. [`readout_labels`] takes an optional [`EconomyReadout`] (banked
//! `resources` + an `income_per_tick` rate); when present it appends a `RESOURCES:` line and an
//! `INCOME: <n>/s` line so cost and income are legible at a glance. The integrator supplies those
//! figures from the sim's `economy_system` (the [`Resources`](gonedark_core::economy::Resources)
//! purse + held-point count); render only formats them. A host that has only a held-point count
//! can derive the rate with [`income_per_tick`] (the same `BASE_INCOME + PER_POINT_INCOME * points`
//! shape the sim uses), and [`income_per_second`] converts a per-tick rate to the per-second figure
//! shown to the player (`TICK_HZ` = 60). These reference `core`'s economy/tick CONSTS at compile
//! time only — there is still no runtime sim read and no engine plumbing inside render.
//!
//! ## Fairness (invariant #6)
//!
//! The labels are screen-space NDC chrome (the W4 text pass), carry no world position, and are
//! emitted only for the command view: [`readout_labels`] takes a `world_dark` flag and returns an
//! EMPTY label set while embodied, so the count/economy chrome can NEVER draw over the dark frame
//! (that would hand back exactly the strategic intel "going dark" removes — banked credits and
//! income are pure command-layer information). On the command frame the labels report only
//! counts/credits the commander is entitled to, leaking no intel the player can't already see.
//!
//! ## The pure seam
//!
//! [`tally`] (count the draw set) and [`readout_labels`] (lay out the label strings + NDC anchors)
//! are free fns, unit-testable without a GPU — the `marquee_quads` / `grid_lines` pattern. The host
//! turns each [`ReadoutLabel`] into a [`text::TextRenderer::queue`] call.

use crate::text::Anchor;
use crate::{faction_color, UnitInstance, FLAG_RING};
use gonedark_core::components::Faction;
// Compile-time CONSTS only — the truthful single source of the balance/tick numbers the displayed
// income figure must agree with. Importing a `const` is not a runtime sim read: it inlines to a
// literal, so render still never calls into `core`/`engine` at runtime (invariant #4).
use gonedark_core::economy::{BASE_INCOME, PER_POINT_INCOME};
use gonedark_core::sim::TICK_HZ;

/// A per-faction / objective tally derived from the command-view draw set. Counts only what is on
/// the (fog-filtered) frame — the commander's visible picture, not omniscient truth.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct Tally {
    /// Player (blue) units on the frame.
    pub player_units: u32,
    /// Enemy (red) units on the frame.
    pub enemy_units: u32,
    /// Control-point rings on the frame.
    pub control_points: u32,
}

/// Classify one instance's RGB against a faction's baked body color. Exact equality is intentional:
/// the interpolator bakes the literal [`faction_color`] bytes (no shading), so a unit's body color
/// is an exact tag. The embodied avatar (amber, [`crate::FLAG_EMBODIED`]) matches no faction here and
/// is counted separately by the caller if needed.
fn is_color(inst: &UnitInstance, faction: Faction) -> bool {
    let [r, g, b] = faction_color(faction);
    inst.r == r && inst.g == g && inst.b == b
}

/// Count the command-view draw set into a [`Tally`]. Pure (no GPU, no sim) — the testable seam.
/// Control points are the [`FLAG_RING`] instances; player/enemy units are the non-ring instances
/// whose body color matches the respective [`faction_color`]. Neutral/avatar instances are not
/// tallied into either side (they are neither the player's nor the opponent's count).
pub fn tally(instances: &[UnitInstance]) -> Tally {
    let mut t = Tally::default();
    for inst in instances {
        if inst.flags & FLAG_RING != 0 {
            t.control_points += 1;
            continue;
        }
        if is_color(inst, Faction::Player) {
            t.player_units += 1;
        } else if is_color(inst, Faction::Enemy) {
            t.enemy_units += 1;
        }
    }
    t
}

/// One laid-out readout label ready to hand to the W4 [`text`](crate::text) pass: the string, its
/// NDC anchor position + [`Anchor`], a size, and a color. Pure data — the host loops these into
/// [`crate::text::TextRenderer::queue`] calls.
#[derive(Clone, PartialEq, Debug)]
pub struct ReadoutLabel {
    pub text: String,
    pub pos: [f32; 2],
    pub px_size: f32,
    pub anchor: Anchor,
    pub color: [f32; 3],
    pub alpha: f32,
}

/// Label glyph height in NDC (cf. `text` module: practical label sizes ~0.03–0.08).
const LABEL_SIZE: f32 = 0.05;
/// Inset from the screen edge for the top-left readout stack, in NDC.
const MARGIN: f32 = 0.04;
/// Vertical step between stacked readout lines, in NDC (a touch more than the glyph height so the
/// lines don't touch).
const LINE_STEP: f32 = 0.075;

/// Label colors — keyed to the faction palette so each count reads as "mine" / "theirs" / "objective".
const PLAYER_LABEL: [f32; 3] = [0.55, 0.78, 1.0];
const ENEMY_LABEL: [f32; 3] = [1.0, 0.55, 0.48];
const NEUTRAL_LABEL: [f32; 3] = [0.85, 0.85, 0.9];
/// Economy lines (banked resources + income) — a credits-gold tint so cost/income read as their
/// own legible class, distinct from the unit counts.
const ECON_LABEL: [f32; 3] = [1.0, 0.86, 0.4];

/// The economy figures a host hands render to fill the resource/income readout. Plain data — the
/// renderer never reads these from the sim (invariant #4); the integrator supplies them from the
/// sim's `economy_system` (the [`Resources`](gonedark_core::economy::Resources) purse + held-point
/// count). `income_per_tick` is the sim's native per-tick rate; the label converts it to a
/// per-second figure ([`income_per_second`]) for legibility.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct EconomyReadout {
    /// Banked resources (current credits) for the local commander.
    pub resources: u32,
    /// Income rate in resources per sim tick (the sim's native unit). Derive it from a held-point
    /// count with [`income_per_tick`], or pass a figure already read from the sim.
    pub income_per_tick: i64,
}

/// Per-tick income for `points` held control points, using the SAME shape the sim's
/// `economy_system` uses: `BASE_INCOME + PER_POINT_INCOME * points`. Pure (no GPU, no runtime sim
/// read — only `core`'s balance consts at compile time), so a host that has just a held-point count
/// can produce a truthful [`EconomyReadout::income_per_tick`] without duplicating the formula.
#[inline]
pub fn income_per_tick(points: u32) -> i64 {
    BASE_INCOME + PER_POINT_INCOME * points as i64
}

/// Convert a per-tick income rate to the per-second figure shown to the player. The sim ticks at a
/// fixed [`TICK_HZ`] (= 60, D21), so per-second = per-tick × 60 — a far more legible "at a glance"
/// number than a raw per-tick drip.
#[inline]
pub fn income_per_second(per_tick: i64) -> i64 {
    per_tick * TICK_HZ as i64
}

/// Lay out the command-view readout labels from a [`Tally`] (and an optional, host-supplied
/// [`EconomyReadout`] — the resource/income seam, now filled). Pure (no GPU, no runtime sim read) —
/// the testable layout seam. The labels stack down the top-left corner:
///
/// - `UNITS: <n>`     — the player's visible unit count (player-blue).
/// - `ENEMY: <n>`     — the visible enemy unit count (enemy-red).
/// - `POINTS: <n>`    — control points on the frame (neutral white).
/// - `RESOURCES: <n>` — banked credits, ONLY when `economy` is `Some` (credits-gold).
/// - `INCOME: <n>/s`  — income converted to per-second ([`income_per_second`]), ONLY when `economy`
///   is `Some` (credits-gold). The renderer has no sim read for the economy, so the host supplies
///   these figures; absent (no fake number) until it does.
///
/// `world_dark` is the fairness gate (invariant #6): while embodied the world goes dark, so this
/// returns an EMPTY set — the command readout (counts AND economy) must never draw over the dark
/// frame, where the alert HUD is the only sanctioned visual thread back to the map. The host need
/// not special-case the call; it passes the embodied/dark state and gets nothing back.
///
/// All positions are NDC ([-1,1], +y up) anchored [`Anchor::TopLeft`], so they hug the screen corner
/// independent of the framing. Screen-space chrome only (invariant #6) — no world position.
pub fn readout_labels(
    t: &Tally,
    economy: Option<EconomyReadout>,
    world_dark: bool,
) -> Vec<ReadoutLabel> {
    // Fairness gate (invariant #6): emit nothing while embodied. The command-layer chrome — visible
    // counts AND banked credits/income — is exactly the strategic intel "going dark" removes.
    if world_dark {
        return Vec::new();
    }
    let top = 1.0 - MARGIN; // top edge, inset
    let left = -1.0 + MARGIN; // left edge, inset
    let mut out = Vec::with_capacity(5);
    let mut row = 0;
    let mut push = |text: String, color: [f32; 3], row: &mut i32| {
        out.push(ReadoutLabel {
            text,
            pos: [left, top - (*row as f32) * LINE_STEP],
            px_size: LABEL_SIZE,
            anchor: Anchor::TopLeft,
            color,
            alpha: 1.0,
        });
        *row += 1;
    };

    push(format!("UNITS: {}", t.player_units), PLAYER_LABEL, &mut row);
    push(format!("ENEMY: {}", t.enemy_units), ENEMY_LABEL, &mut row);
    push(
        format!("POINTS: {}", t.control_points),
        NEUTRAL_LABEL,
        &mut row,
    );
    // Resource/income seam: only emitted when a host hands real economy figures in. The renderer
    // can't read the sim economy, so we never invent one — see the module docs.
    if let Some(econ) = economy {
        push(
            format!("RESOURCES: {}", econ.resources),
            ECON_LABEL,
            &mut row,
        );
        push(
            format!("INCOME: {}/s", income_per_second(econ.income_per_tick)),
            ECON_LABEL,
            &mut row,
        );
    }

    out
}

#[cfg(test)]
mod tests {
    //! `render` is the float boundary, so f32 layout math is fair game. The pure tally + layout
    //! seams are tested here without a GPU.

    use super::*;
    use crate::{AVATAR_COLOR, FLAG_EMBODIED};

    fn unit_of(faction: Faction) -> UnitInstance {
        let [r, g, b] = faction_color(faction);
        UnitInstance {
            r,
            g,
            b,
            ..Default::default()
        }
    }

    fn ring(faction: Faction) -> UnitInstance {
        let mut u = unit_of(faction);
        u.flags = FLAG_RING;
        u
    }

    // ---- tally ----

    #[test]
    fn tally_counts_player_enemy_and_points() {
        let set = vec![
            unit_of(Faction::Player),
            unit_of(Faction::Player),
            unit_of(Faction::Enemy),
            ring(Faction::Player),
            ring(Faction::Enemy),
        ];
        let t = tally(&set);
        assert_eq!(t.player_units, 2);
        assert_eq!(t.enemy_units, 1);
        assert_eq!(t.control_points, 2, "both rings counted regardless of owner");
    }

    #[test]
    fn rings_are_points_not_units() {
        // A ring carries a faction body color but is a control point, not a unit — it must not be
        // double-counted into the unit tallies.
        let t = tally(&[ring(Faction::Player), ring(Faction::Enemy)]);
        assert_eq!(t.player_units, 0);
        assert_eq!(t.enemy_units, 0);
        assert_eq!(t.control_points, 2);
    }

    #[test]
    fn neutral_and_avatar_are_not_tallied_into_a_side() {
        // Neutral grey and the amber avatar are neither the player's nor the enemy's count.
        let neutral = unit_of(Faction::Neutral);
        let avatar = UnitInstance {
            r: AVATAR_COLOR[0],
            g: AVATAR_COLOR[1],
            b: AVATAR_COLOR[2],
            flags: FLAG_EMBODIED,
            ..Default::default()
        };
        let t = tally(&[neutral, avatar]);
        assert_eq!(t.player_units, 0);
        assert_eq!(t.enemy_units, 0);
        assert_eq!(t.control_points, 0);
    }

    #[test]
    fn empty_set_tallies_zero() {
        assert_eq!(tally(&[]), Tally::default());
    }

    // ---- readout_labels ----

    fn econ(resources: u32, income_per_tick: i64) -> EconomyReadout {
        EconomyReadout {
            resources,
            income_per_tick,
        }
    }

    #[test]
    fn labels_report_the_tally_counts() {
        let t = Tally {
            player_units: 5,
            enemy_units: 3,
            control_points: 2,
        };
        let labels = readout_labels(&t, None, false);
        // Three lines without the economy seam.
        assert_eq!(labels.len(), 3);
        assert!(labels[0].text.contains('5'), "player count in the units line");
        assert!(labels[1].text.contains('3'), "enemy count in the enemy line");
        assert!(labels[2].text.contains('2'), "point count in the points line");
        assert!(labels[0].text.starts_with("UNITS"));
        assert!(labels[1].text.starts_with("ENEMY"));
        assert!(labels[2].text.starts_with("POINTS"));
    }

    #[test]
    fn economy_seam_only_appears_when_supplied() {
        let t = Tally::default();
        assert_eq!(
            readout_labels(&t, None, false).len(),
            3,
            "no economy lines by default"
        );
        let with = readout_labels(&t, Some(econ(250, income_per_tick(0))), false);
        assert_eq!(
            with.len(),
            5,
            "RESOURCES + INCOME lines appear when a host supplies economy"
        );
        assert!(with[3].text.starts_with("RESOURCES"));
        assert!(with[4].text.starts_with("INCOME"));
    }

    #[test]
    fn resource_line_shows_the_banked_credits() {
        // The banked figure the host hands in is the exact number shown.
        let labels = readout_labels(&Tally::default(), Some(econ(1337, income_per_tick(0))), false);
        assert!(labels[3].text.contains("1337"), "banked credits verbatim");
    }

    #[test]
    fn income_label_converts_per_tick_to_per_second() {
        // Base income (no points) is 1/tick -> 60/s at TICK_HZ = 60.
        let base = readout_labels(&Tally::default(), Some(econ(0, income_per_tick(0))), false);
        assert!(base[4].text.contains("60/s"), "1/tick reads as 60/s, got {:?}", base[4].text);
        // Holding two points: 1 + 2*2 = 5/tick -> 300/s.
        let held = readout_labels(&Tally::default(), Some(econ(0, income_per_tick(2))), false);
        assert!(held[4].text.contains("300/s"), "5/tick reads as 300/s, got {:?}", held[4].text);
    }

    #[test]
    fn income_per_tick_matches_the_sim_economy_shape() {
        // Mirror of economy_system: BASE_INCOME + PER_POINT_INCOME * points.
        assert_eq!(income_per_tick(0), BASE_INCOME);
        assert_eq!(income_per_tick(3), BASE_INCOME + PER_POINT_INCOME * 3);
    }

    #[test]
    fn income_per_second_scales_by_tick_hz() {
        assert_eq!(income_per_second(1), TICK_HZ as i64);
        assert_eq!(income_per_second(5), 5 * TICK_HZ as i64);
    }

    #[test]
    fn nothing_is_emitted_for_the_dark_embodied_frame() {
        // Fairness (invariant #6): while embodied the world goes dark — no command/economy chrome,
        // even when counts and a fat purse are available. The readout must stay off that frame.
        let t = Tally {
            player_units: 9,
            enemy_units: 9,
            control_points: 4,
        };
        assert!(
            readout_labels(&t, Some(econ(9999, income_per_tick(4))), true).is_empty(),
            "no labels at all over the dark embodied frame"
        );
        // And with no economy either — the count chrome is also withheld.
        assert!(readout_labels(&t, None, true).is_empty());
    }

    #[test]
    fn economy_lines_carry_the_credits_color() {
        let labels = readout_labels(&Tally::default(), Some(econ(100, income_per_tick(1))), false);
        // The two economy lines share the credits-gold tint, distinct from the white point line.
        assert_eq!(labels[3].color, ECON_LABEL, "RESOURCES line is credits-gold");
        assert_eq!(labels[4].color, ECON_LABEL, "INCOME line is credits-gold");
        assert_ne!(labels[2].color, ECON_LABEL, "points line is not the economy color");
    }

    #[test]
    fn labels_stack_down_the_top_left_corner() {
        let labels = readout_labels(&Tally::default(), None, false);
        for w in labels.windows(2) {
            // Each line is left-aligned at the same x and steps DOWN (smaller y) from the last.
            assert_eq!(w[0].pos[0], w[1].pos[0], "same left x");
            assert!(w[1].pos[1] < w[0].pos[1], "next line is lower");
            assert_eq!(w[0].anchor, Anchor::TopLeft);
        }
        // The stack hugs the top-left corner (inside the screen).
        assert!(labels[0].pos[0] < 0.0 && labels[0].pos[0] > -1.0);
        assert!(labels[0].pos[1] > 0.0 && labels[0].pos[1] < 1.0);
    }

    #[test]
    fn labels_are_screen_space_chrome() {
        // Fairness guard (invariant #6): every label is NDC chrome, never a world position.
        let t = Tally {
            player_units: 99,
            enemy_units: 99,
            control_points: 9,
        };
        for l in readout_labels(&t, Some(econ(9999, income_per_tick(9))), false) {
            assert!(l.pos[0] >= -1.0 && l.pos[0] <= 1.0, "x in NDC");
            assert!(l.pos[1] >= -1.0 && l.pos[1] <= 1.0, "y in NDC");
            assert!(l.px_size > 0.0 && l.alpha > 0.0);
        }
    }

    #[test]
    fn each_side_label_carries_its_faction_color() {
        let labels = readout_labels(&Tally::default(), None, false);
        // The player line leans blue, the enemy line leans red (so each reads as its side).
        assert!(labels[0].color[2] > labels[0].color[0], "player label is blue-leaning");
        assert!(labels[1].color[0] > labels[1].color[2], "enemy label is red-leaning");
    }
}
