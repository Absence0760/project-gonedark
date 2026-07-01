//! Command-panel **glanceability seams** (visual-design WS-C / CP-9): the pure "what can the player
//! read in one second" thresholds the contextual command panel (`command_panel_view` in
//! [`crate`]) leans on so a selection reads at a glance on a small (phone-aspect) screen.
//!
//! Two decisions live here, extracted as free fns so they are unit-testable without a `Game` (the
//! CLAUDE.md "extract the pure logic to a testable seam" rule, exactly like [`crate::command_ui`] /
//! [`crate::command_touch`]):
//!
//! - [`composition_rows`] — collapse a selection's per-kind counts into **at most `budget`** rows,
//!   with a trailing `"+N more"` roll-up when the composition would otherwise overflow the panel on
//!   a short screen. Depth stays in what the player *reads*, never in unit AI (invariant #3).
//! - [`hp_line_style`] — map an average-health percentage to the shared [`LineStyle`] state language
//!   (green when healthy, red when hurt) so a wounded group reads as *hurt* at a glance instead of
//!   as a flat neutral number.
//!
//! PRESENTATION ONLY (invariant #1/#4/#6): these are pure functions of plain host-supplied numbers
//! (kind labels + counts, an HP percent). They read no sim state, touch no `core` type, and never
//! fold into the per-tick checksum — they only shape the screen-space command chrome the host hands
//! to [`gonedark_render::command_panel`], which is itself gated to the command view.

use gonedark_render::command_panel::LineStyle;

/// Average-health percentage at or above which a selected group reads as **healthy** (green).
pub const HP_GOOD_PCT: i32 = 67;
/// Average-health percentage at or below which a selected group reads as **hurt** (red).
pub const HP_BAD_PCT: i32 = 33;

/// Map an average-health percentage (`0..=100`) to the shared command-panel [`LineStyle`] so the
/// "Avg HP" row carries the same green/neutral/red state language the affordability rows use:
/// healthy at [`HP_GOOD_PCT`]+ reads [`LineStyle::Good`], hurt at [`HP_BAD_PCT`]- reads
/// [`LineStyle::Bad`], the band between stays [`LineStyle::Normal`]. Pure — the glanceability seam.
pub fn hp_line_style(avg_hp_pct: i32) -> LineStyle {
    if avg_hp_pct >= HP_GOOD_PCT {
        LineStyle::Good
    } else if avg_hp_pct <= HP_BAD_PCT {
        LineStyle::Bad
    } else {
        LineStyle::Normal
    }
}

/// Collapse a selection's per-kind composition into the command-panel rows, capped at `budget` so a
/// big, mixed selection can't run the panel off the bottom of a phone screen.
///
/// `kinds` is `(display label, count)` in the caller's stable display order; entries with a zero
/// count are skipped (nothing to show). When the non-empty kinds fit within `budget` every kind gets
/// its own `"Nx Label"` row. When they would overflow, the first `budget - 1` kinds are shown and the
/// remainder is rolled up into a single `"+N more"` row, where `N` is the total number of *units* in
/// the collapsed kinds (the more useful glance number than a kind count). A `budget` of 0 yields no
/// rows. Pure (no GPU, no sim) — the testable seam.
pub fn composition_rows(kinds: &[(&str, u32)], budget: usize) -> Vec<String> {
    // Only kinds actually present contribute a row.
    let present: Vec<(&str, u32)> = kinds.iter().copied().filter(|&(_, c)| c > 0).collect();
    if budget == 0 || present.is_empty() {
        return Vec::new();
    }
    if present.len() <= budget {
        return present
            .iter()
            .map(|&(label, count)| format!("{count}x {label}"))
            .collect();
    }
    // Overflow: show the first `budget - 1` kinds, then roll the rest into "+N more" (N = units).
    let shown = budget - 1;
    let mut rows: Vec<String> = present[..shown]
        .iter()
        .map(|&(label, count)| format!("{count}x {label}"))
        .collect();
    let hidden_units: u32 = present[shown..].iter().map(|&(_, c)| c).sum();
    rows.push(format!("+{hidden_units} more"));
    rows
}

#[cfg(test)]
mod tests {
    use super::*;

    // ---- hp_line_style ----

    #[test]
    fn full_health_reads_good_and_low_reads_bad() {
        assert_eq!(hp_line_style(100), LineStyle::Good);
        assert_eq!(hp_line_style(HP_GOOD_PCT), LineStyle::Good, "boundary is inclusive-good");
        assert_eq!(hp_line_style(0), LineStyle::Bad);
        assert_eq!(hp_line_style(HP_BAD_PCT), LineStyle::Bad, "boundary is inclusive-bad");
    }

    #[test]
    fn mid_health_stays_neutral() {
        // Between the two thresholds the row reads as ordinary body text (no false alarm, no
        // false all-clear).
        assert_eq!(hp_line_style(50), LineStyle::Normal);
        assert_eq!(hp_line_style(HP_BAD_PCT + 1), LineStyle::Normal);
        assert_eq!(hp_line_style(HP_GOOD_PCT - 1), LineStyle::Normal);
    }

    // ---- composition_rows ----

    #[test]
    fn under_budget_shows_every_present_kind() {
        let rows = composition_rows(&[("Rifleman", 3), ("Heavy", 1), ("Tank", 2)], 4);
        assert_eq!(rows, vec!["3x Rifleman", "1x Heavy", "2x Tank"]);
    }

    #[test]
    fn zero_count_kinds_are_skipped() {
        // The caller can pass the full kind table; only present kinds get a row.
        let rows = composition_rows(&[("Rifleman", 0), ("Heavy", 2), ("Tank", 0), ("Medic", 1)], 4);
        assert_eq!(rows, vec!["2x Heavy", "1x Medic"]);
    }

    #[test]
    fn empty_or_all_zero_yields_no_rows() {
        assert!(composition_rows(&[], 4).is_empty());
        assert!(composition_rows(&[("Rifleman", 0), ("Heavy", 0)], 4).is_empty());
    }

    #[test]
    fn overflow_rolls_the_remainder_into_plus_n_more_units() {
        // 5 present kinds, budget 4 → show 3 kinds + a "+N more" row summing the hidden units
        // (Medic 4 + AntiTank 5 = 9), so the panel stays a fixed 4 rows tall on a phone.
        let rows = composition_rows(
            &[("Rifleman", 3), ("Heavy", 1), ("Tank", 2), ("Medic", 4), ("AntiTank", 5)],
            4,
        );
        assert_eq!(rows.len(), 4, "capped at the budget");
        assert_eq!(&rows[..3], &["3x Rifleman", "1x Heavy", "2x Tank"]);
        assert_eq!(rows[3], "+9 more", "the tail rolls up as a unit total, not a kind count");
    }

    #[test]
    fn exactly_at_budget_does_not_roll_up() {
        // len == budget must show every kind (no premature "+N more").
        let rows = composition_rows(&[("A", 1), ("B", 1), ("C", 1), ("D", 1)], 4);
        assert_eq!(rows, vec!["1x A", "1x B", "1x C", "1x D"]);
    }

    #[test]
    fn budget_of_one_rolls_everything_past_the_first() {
        let rows = composition_rows(&[("Rifleman", 2), ("Heavy", 3)], 1);
        // budget 1 → 0 explicit kind rows + the roll-up of ALL present units (2 + 3 = 5).
        assert_eq!(rows, vec!["+5 more"]);
    }

    #[test]
    fn zero_budget_yields_nothing() {
        assert!(composition_rows(&[("Rifleman", 3)], 0).is_empty());
    }

    #[test]
    fn preserves_the_callers_display_order() {
        // The rows follow the caller's order (a stable, deterministic kind order), never re-sorted.
        let rows = composition_rows(&[("Zulu", 1), ("Alpha", 1)], 4);
        assert_eq!(rows, vec!["1x Zulu", "1x Alpha"]);
    }
}
