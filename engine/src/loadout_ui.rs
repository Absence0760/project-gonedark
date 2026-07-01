//! Pre-match gunsmith loadout editor — the command-layer customization surface (WS-C, D60,
//! `customization.md` §1).
//!
//! This is the pure UI **state + intent** seam for the gunsmith: it owns the player's current
//! [`Loadout`] and turns a "cycle this slot" input into a new selection. It is the host-testable
//! analogue of `command_ui`/`selection`/`build_ui` — no winit/GPU types, no camera, no platform
//! input structs, so the whole thing is exercised in-process.
//!
//! **It never touches the sim.** The editor only assembles a [`Loadout`] (a small `Copy` value of
//! [`core::gunsmith`](gonedark_core::gunsmith) enums). That loadout is the deterministic match-setup
//! input the scenario seeder applies to the spawned weapon *at match start* via
//! [`Loadout::apply_to_weapon`] — which is where (and the only place) it reaches the deterministic
//! sim. Picking it here is presentation only: choosing a loadout before the dive can't desync
//! anything, and the fairness guarantee (no strictly-dominant build) is proven in `core::gunsmith`,
//! so *every* state this editor can reach is a fair sidegrade by construction.

use gonedark_core::gunsmith::{Grip, Loadout, StatDelta};

/// The editable gunsmith rows, in a fixed on-screen order (the order the UI lays them out and the
/// order [`LoadoutSlot::ALL`] / a numeric slot index follow). Five are **sim** slots (they feed the
/// [`Loadout`] applied to the weapon); [`Grip`](LoadoutSlot::Grip) is the sixth row and is
/// **cosmetic/feel-only** (D85) — it carries no sim effect (invariant #4), tracked separately by
/// the editor and never applied to the weapon.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum LoadoutSlot {
    /// Range ↔ fire-rate (`Optic`).
    Optic,
    /// Damage ↔ reserve (`Barrel`).
    Barrel,
    /// Capacity ↔ handling (`Magazine`).
    Magazine,
    /// Mobility ↔ steadiness (`Stock`) — gunsmith breadth, CP-1 / D85.
    Stock,
    /// Suppression ↔ downrange retention (`Muzzle`) — gunsmith breadth, CP-1 / D85.
    Muzzle,
    /// Recoil / hipfire **feel** (`Grip`) — **cosmetic only**, no sim effect (D85).
    Grip,
}

impl LoadoutSlot {
    /// Every row, in fixed on-screen order (five sim slots + the cosmetic Grip).
    pub const ALL: [LoadoutSlot; 6] = [
        LoadoutSlot::Optic,
        LoadoutSlot::Barrel,
        LoadoutSlot::Magazine,
        LoadoutSlot::Stock,
        LoadoutSlot::Muzzle,
        LoadoutSlot::Grip,
    ];

    /// The slot at on-screen index `i` (`0..6`), or `None` for an out-of-range index — so a stray
    /// UI slot value is a harmless no-op, mirroring `command_ui`'s out-of-range slot handling.
    #[inline]
    pub fn from_index(i: usize) -> Option<LoadoutSlot> {
        LoadoutSlot::ALL.get(i).copied()
    }

    /// A short label for the slot itself (the row heading in the gunsmith UI).
    #[inline]
    pub fn label(self) -> &'static str {
        match self {
            LoadoutSlot::Optic => "Optic",
            LoadoutSlot::Barrel => "Barrel",
            LoadoutSlot::Magazine => "Magazine",
            LoadoutSlot::Stock => "Stock",
            LoadoutSlot::Muzzle => "Muzzle",
            LoadoutSlot::Grip => "Grip",
        }
    }

    /// Is this row a **sim** slot (feeds the applied [`Loadout`])? `false` only for
    /// [`Grip`](LoadoutSlot::Grip), which is cosmetic/feel-only (D85).
    #[inline]
    pub fn is_sim(self) -> bool {
        !matches!(self, LoadoutSlot::Grip)
    }
}

/// The pre-match loadout editor: holds the player's current [`Loadout`] and edits it slot-by-slot.
/// Construct with [`LoadoutEditor::new`] (the all-`Standard` baseline) or
/// [`LoadoutEditor::with_loadout`] (a saved selection); read [`LoadoutEditor::current`] to hand the
/// chosen loadout to the scenario seeder at match start.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct LoadoutEditor {
    loadout: Loadout,
    /// The cosmetic Grip selection (D85). Kept **out** of [`Loadout`] on purpose — it has no sim
    /// effect, so it is never applied to the weapon or folded; it exists only so the UI can show and
    /// cycle the sixth gunsmith row.
    grip: Grip,
}

impl LoadoutEditor {
    /// A fresh editor at the neutral all-`Standard` loadout (what a player with no unlocks fields).
    #[inline]
    pub fn new() -> Self {
        LoadoutEditor {
            loadout: Loadout::STANDARD,
            grip: Grip::Standard,
        }
    }

    /// An editor seeded with a previously-saved loadout (cosmetic Grip resets to `Standard`).
    #[inline]
    pub fn with_loadout(loadout: Loadout) -> Self {
        LoadoutEditor {
            loadout,
            grip: Grip::Standard,
        }
    }

    /// The currently-selected loadout — the value the scenario seeder applies at match start. Note
    /// this is the **sim** loadout only; the cosmetic [`grip`](LoadoutEditor::grip) is not part of it.
    #[inline]
    pub fn current(&self) -> Loadout {
        self.loadout
    }

    /// The currently-selected cosmetic Grip (D85) — a feel/presentation choice, never applied to
    /// the sim.
    #[inline]
    pub fn grip(&self) -> Grip {
        self.grip
    }

    /// The net [`StatDelta`] of the current selection — the "net power across the build" the UI can
    /// surface as a readout (it sums the three slot trades; by the sidegrade rule it is never a flat
    /// upgrade over the baseline).
    #[inline]
    pub fn net_delta(&self) -> StatDelta {
        self.loadout.total_delta()
    }

    /// Cycle one slot's option (`forward` = the "next" direction, else "previous"), wrapping through
    /// that slot's three options. This is the single edit primitive the UI calls; it always lands on
    /// a valid, fair selection (every option is a sidegrade).
    pub fn cycle(&mut self, slot: LoadoutSlot, forward: bool) {
        match slot {
            LoadoutSlot::Optic => {
                self.loadout.optic = if forward {
                    self.loadout.optic.next()
                } else {
                    self.loadout.optic.prev()
                };
            }
            LoadoutSlot::Barrel => {
                self.loadout.barrel = if forward {
                    self.loadout.barrel.next()
                } else {
                    self.loadout.barrel.prev()
                };
            }
            LoadoutSlot::Magazine => {
                self.loadout.magazine = if forward {
                    self.loadout.magazine.next()
                } else {
                    self.loadout.magazine.prev()
                };
            }
            LoadoutSlot::Stock => {
                self.loadout.stock = if forward {
                    self.loadout.stock.next()
                } else {
                    self.loadout.stock.prev()
                };
            }
            LoadoutSlot::Muzzle => {
                self.loadout.muzzle = if forward {
                    self.loadout.muzzle.next()
                } else {
                    self.loadout.muzzle.prev()
                };
            }
            // Cosmetic-only (D85): cycles the separate `grip` field, never the sim `Loadout`.
            LoadoutSlot::Grip => {
                self.grip = if forward {
                    self.grip.next()
                } else {
                    self.grip.prev()
                };
            }
        }
    }

    /// Handle a raw UI input: cycle the slot at on-screen index `slot_index` in `forward`/back
    /// direction. Returns whether anything happened — an out-of-range index is a no-op (`false`),
    /// matching `command_ui`'s tolerance of stray slot values.
    pub fn apply_input(&mut self, slot_index: usize, forward: bool) -> bool {
        match LoadoutSlot::from_index(slot_index) {
            Some(slot) => {
                self.cycle(slot, forward);
                true
            }
            None => false,
        }
    }

    /// Reset to the neutral all-`Standard` loadout (the gunsmith "reset" button), including the
    /// cosmetic Grip row.
    #[inline]
    pub fn reset(&mut self) {
        self.loadout = Loadout::STANDARD;
        self.grip = Grip::Standard;
    }

    /// The currently-selected option label for `slot` (the cell text in the gunsmith UI).
    pub fn option_label(&self, slot: LoadoutSlot) -> &'static str {
        match slot {
            LoadoutSlot::Optic => self.loadout.optic.label(),
            LoadoutSlot::Barrel => self.loadout.barrel.label(),
            LoadoutSlot::Magazine => self.loadout.magazine.label(),
            LoadoutSlot::Stock => self.loadout.stock.label(),
            LoadoutSlot::Muzzle => self.loadout.muzzle.label(),
            LoadoutSlot::Grip => self.grip.label(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gonedark_core::gunsmith::{Barrel, Magazine, Muzzle, Optic, Stock};

    #[test]
    fn new_editor_is_the_neutral_baseline() {
        let ed = LoadoutEditor::new();
        assert_eq!(ed.current(), Loadout::STANDARD);
        assert_eq!(
            ed.net_delta(),
            StatDelta::ZERO,
            "the baseline moves no stat"
        );
        assert_eq!(ed.option_label(LoadoutSlot::Optic), "Standard");
        assert_eq!(ed.option_label(LoadoutSlot::Barrel), "Standard");
        assert_eq!(ed.option_label(LoadoutSlot::Magazine), "Standard");
        assert_eq!(ed.option_label(LoadoutSlot::Stock), "Standard");
        assert_eq!(ed.option_label(LoadoutSlot::Muzzle), "Standard");
        assert_eq!(ed.option_label(LoadoutSlot::Grip), "Standard");
    }

    #[test]
    fn there_are_six_rows_five_sim_plus_cosmetic_grip() {
        assert_eq!(LoadoutSlot::ALL.len(), 6);
        let sim: Vec<_> = LoadoutSlot::ALL.iter().filter(|s| s.is_sim()).collect();
        assert_eq!(sim.len(), 5, "five functional sim slots");
        assert!(!LoadoutSlot::Grip.is_sim(), "Grip is cosmetic-only (D85)");
        assert_eq!(LoadoutSlot::from_index(5), Some(LoadoutSlot::Grip));
        assert_eq!(LoadoutSlot::from_index(6), None);
    }

    #[test]
    fn cycling_the_grip_row_never_touches_the_sim_loadout() {
        // The load-bearing D85 property: the Grip row is cosmetic — cycling it changes only the
        // editor's grip feel, never the sim `Loadout` the seeder applies.
        let mut ed = LoadoutEditor::new();
        let loadout_before = ed.current();
        ed.cycle(LoadoutSlot::Grip, true);
        assert_eq!(ed.grip(), gonedark_core::gunsmith::Grip::Vertical);
        assert_eq!(
            ed.current(),
            loadout_before,
            "Grip is cosmetic: the applied sim loadout is unchanged"
        );
        assert_eq!(ed.net_delta(), StatDelta::ZERO, "Grip contributes no stat delta");
    }

    #[test]
    fn cycling_stock_and_muzzle_edits_the_sim_loadout() {
        let mut ed = LoadoutEditor::new();
        ed.cycle(LoadoutSlot::Stock, true);
        assert_eq!(ed.current().stock, Stock::Agile);
        ed.cycle(LoadoutSlot::Muzzle, true);
        assert_eq!(ed.current().muzzle, Muzzle::Brake);
        assert_ne!(ed.net_delta(), StatDelta::ZERO, "sim slots move the net delta");
    }

    #[test]
    fn cycling_a_slot_advances_only_that_slot() {
        let mut ed = LoadoutEditor::new();
        ed.cycle(LoadoutSlot::Optic, true);
        assert_eq!(ed.current().optic, Optic::Marksman);
        // The other slots are untouched.
        assert_eq!(ed.current().barrel, Barrel::Standard);
        assert_eq!(ed.current().magazine, Magazine::Standard);
        assert_eq!(ed.option_label(LoadoutSlot::Optic), "Marksman");
    }

    #[test]
    fn forward_then_back_is_identity_on_every_slot() {
        for slot in LoadoutSlot::ALL {
            let mut ed = LoadoutEditor::new();
            ed.cycle(slot, true);
            ed.cycle(slot, false);
            assert_eq!(
                ed.current(),
                Loadout::STANDARD,
                "{} round-trips",
                slot.label()
            );
        }
    }

    #[test]
    fn cycling_a_slot_three_times_returns_to_start() {
        for slot in LoadoutSlot::ALL {
            let mut ed = LoadoutEditor::new();
            for _ in 0..3 {
                ed.cycle(slot, true);
            }
            assert_eq!(
                ed.current(),
                Loadout::STANDARD,
                "{} wraps after 3",
                slot.label()
            );
        }
    }

    #[test]
    fn apply_input_routes_by_index_and_ignores_out_of_range() {
        let mut ed = LoadoutEditor::new();
        assert!(ed.apply_input(0, true), "index 0 = Optic");
        assert_eq!(ed.current().optic, Optic::Marksman);
        assert!(ed.apply_input(1, true), "index 1 = Barrel");
        assert_eq!(ed.current().barrel, Barrel::Heavy);
        assert!(ed.apply_input(2, true), "index 2 = Magazine");
        assert_eq!(ed.current().magazine, Magazine::Extended);
        assert!(ed.apply_input(3, true), "index 3 = Stock");
        assert_eq!(ed.current().stock, Stock::Agile);
        assert!(ed.apply_input(4, true), "index 4 = Muzzle");
        assert_eq!(ed.current().muzzle, Muzzle::Brake);
        assert!(ed.apply_input(5, true), "index 5 = Grip (cosmetic)");
        assert_eq!(ed.grip(), gonedark_core::gunsmith::Grip::Vertical);
        // Out of range: no-op, nothing changes. (Six rows now, so 6 is the first invalid index.)
        let before = ed.current();
        let grip_before = ed.grip();
        assert!(!ed.apply_input(6, true));
        assert!(!ed.apply_input(99, false));
        assert_eq!(ed.current(), before);
        assert_eq!(ed.grip(), grip_before);
    }

    #[test]
    fn reset_returns_to_baseline() {
        let mut ed = LoadoutEditor::with_loadout(Loadout {
            optic: Optic::CloseQuarters,
            barrel: Barrel::Light,
            magazine: Magazine::Quickdraw,
            ..Loadout::STANDARD
        });
        assert_ne!(ed.current(), Loadout::STANDARD);
        ed.reset();
        assert_eq!(ed.current(), Loadout::STANDARD);
    }

    #[test]
    fn net_delta_reflects_the_selected_trades() {
        let ed = LoadoutEditor::with_loadout(Loadout {
            optic: Optic::Marksman,
            barrel: Barrel::Heavy,
            magazine: Magazine::Extended,
            ..Loadout::STANDARD
        });
        let d = ed.net_delta();
        // The summed trades: +range/+cooldown (Marksman), +damage/-reserve (Heavy),
        // +mag/+reload (Extended).
        assert_eq!(
            d,
            Optic::Marksman
                .delta()
                .add(Barrel::Heavy.delta())
                .add(Magazine::Extended.delta())
        );
    }

    /// Every selection the editor can reach is, by `core::gunsmith`'s proof, a fair sidegrade — no
    /// reachable build strictly dominates another. We spot-check the editor never desyncs that
    /// guarantee by confirming a couple of opposed full builds neither dominate.
    #[test]
    fn reachable_builds_are_fair_sidegrades() {
        let sniper = LoadoutEditor::with_loadout(Loadout {
            optic: Optic::Marksman,
            barrel: Barrel::Heavy,
            magazine: Magazine::Extended,
            ..Loadout::STANDARD
        })
        .net_delta();
        let runner = LoadoutEditor::with_loadout(Loadout {
            optic: Optic::CloseQuarters,
            barrel: Barrel::Light,
            magazine: Magazine::Quickdraw,
            ..Loadout::STANDARD
        })
        .net_delta();
        assert!(!sniper.strictly_dominates(&runner));
        assert!(!runner.strictly_dominates(&sniper));
    }
}
