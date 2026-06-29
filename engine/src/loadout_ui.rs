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

use gonedark_core::gunsmith::{Barrel, Loadout, Magazine, Optic, StatDelta};

/// The editable attachment slots, in a fixed on-screen order (the order the UI lays them out and
/// the order [`LoadoutSlot::ALL`] / a numeric slot index follow).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum LoadoutSlot {
    /// Range ↔ fire-rate ([`Optic`]).
    Optic,
    /// Damage ↔ reserve ([`Barrel`]).
    Barrel,
    /// Capacity ↔ handling ([`Magazine`]).
    Magazine,
}

impl LoadoutSlot {
    /// Every slot, in fixed on-screen order.
    pub const ALL: [LoadoutSlot; 3] = [
        LoadoutSlot::Optic,
        LoadoutSlot::Barrel,
        LoadoutSlot::Magazine,
    ];

    /// The slot at on-screen index `i` (`0..3`), or `None` for an out-of-range index — so a stray
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
        }
    }
}

/// The pre-match loadout editor: holds the player's current [`Loadout`] and edits it slot-by-slot.
/// Construct with [`LoadoutEditor::new`] (the all-`Standard` baseline) or
/// [`LoadoutEditor::with_loadout`] (a saved selection); read [`LoadoutEditor::current`] to hand the
/// chosen loadout to the scenario seeder at match start.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct LoadoutEditor {
    loadout: Loadout,
}

impl LoadoutEditor {
    /// A fresh editor at the neutral all-`Standard` loadout (what a player with no unlocks fields).
    #[inline]
    pub fn new() -> Self {
        LoadoutEditor {
            loadout: Loadout::STANDARD,
        }
    }

    /// An editor seeded with a previously-saved loadout.
    #[inline]
    pub fn with_loadout(loadout: Loadout) -> Self {
        LoadoutEditor { loadout }
    }

    /// The currently-selected loadout — the value the scenario seeder applies at match start.
    #[inline]
    pub fn current(&self) -> Loadout {
        self.loadout
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

    /// Reset to the neutral all-`Standard` loadout (the gunsmith "reset" button).
    #[inline]
    pub fn reset(&mut self) {
        self.loadout = Loadout::STANDARD;
    }

    /// The currently-selected option label for `slot` (the cell text in the gunsmith UI).
    pub fn option_label(&self, slot: LoadoutSlot) -> &'static str {
        match slot {
            LoadoutSlot::Optic => self.loadout.optic.label(),
            LoadoutSlot::Barrel => self.loadout.barrel.label(),
            LoadoutSlot::Magazine => self.loadout.magazine.label(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
        // Out of range: no-op, nothing changes.
        let before = ed.current();
        assert!(!ed.apply_input(3, true));
        assert!(!ed.apply_input(99, false));
        assert_eq!(ed.current(), before);
    }

    #[test]
    fn reset_returns_to_baseline() {
        let mut ed = LoadoutEditor::with_loadout(Loadout {
            optic: Optic::CloseQuarters,
            barrel: Barrel::Light,
            magazine: Magazine::Quickdraw,
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
        })
        .net_delta();
        let runner = LoadoutEditor::with_loadout(Loadout {
            optic: Optic::CloseQuarters,
            barrel: Barrel::Light,
            magazine: Magazine::Quickdraw,
        })
        .net_delta();
        assert!(!sniper.strictly_dominates(&runner));
        assert!(!runner.strictly_dominates(&sniper));
    }
}
