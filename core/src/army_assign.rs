//! Deterministic ranked-1v1 army assignment (D130).
//!
//! This is **host-side match-setup** — the army each side fields, fed to the sim through the
//! `core::shell` SelectArmy seam (`Game::select_army`) at match start, exactly like the persisted
//! army-select pick. Like that pick it is *not* folded into the per-tick checksum (invariant #7):
//! its gameplay effect (the per-army stat table, WS-B) is what folds, so a peer that fielded a
//! different army diverges in spawned-unit stats and the desync is caught there.
//!
//! But unlike a human's manual pick, a **ranked** assignment is randomized — and every peer must
//! arrive at the *same* pair without exchanging it. So it is a pure deterministic function of a
//! **shared match seed** ([`Rng::new`](crate::rng::Rng::new)), computed identically on every peer.
//! Same seed and roster in, same `(Army, Army)` out, on every device and arch (no floats —
//! invariant #1; the draw is integer-only via [`Rng::below`](crate::rng::Rng::below)).
//!
//! The **anti-mirror guard** (D130): in a real 1v1 the two sides must field *distinct* armies, so
//! a random assignment never accidentally produces a mirror match.

use crate::components::Army;
use crate::rng::Rng;

/// Assign the two armies for a ranked 1v1 as a pure deterministic function of the shared match
/// `seed` and the available `roster` (D130).
///
/// - **Deterministic:** the same `(seed, roster)` yields the same `(Army, Army)` on every peer and
///   architecture — the invariant that lets ranked randomize without exchanging the result over the
///   wire. Integer-only (invariant #1).
/// - **Anti-mirror:** when `roster.len() >= 2` the two returned armies are guaranteed **distinct**
///   (side B is drawn from the roster with side A removed), so ranked never rolls a mirror match.
/// - **Both orderings reachable:** side A is drawn uniformly across the whole roster (side A is not
///   hardcoded to `roster[0]`), so across seeds every ordered distinct pair can occur.
/// - **Degenerate rosters:** with `roster.len() == 1` there is no second army to field, so both
///   sides are forced to `roster[0]` (the only option — the caller owns whether a single-army
///   roster is even legal for ranked). With an **empty** roster there is no army at all, so both
///   sides fall back to [`Army::Neutral`], the non-aligned default.
pub fn assign_ranked_1v1(seed: u64, roster: &[Army]) -> (Army, Army) {
    let n = roster.len();
    if n == 0 {
        // No army to field — fall back to the non-aligned default rather than panic.
        return (Army::Neutral, Army::Neutral);
    }
    if n == 1 {
        // Only one army available: a mirror is the sole option (documented above).
        return (roster[0], roster[0]);
    }

    let mut rng = Rng::new(seed);
    // Warm up one draw before deciding. A PCG generator's *first* output from a freshly seeded
    // state is poorly distributed across sequential seeds (`Rng::new` only mixes the seed into the
    // low state bits, which the first output's shifts largely discard) — so drawing side A straight
    // off `new` pins it to roster[0] for whole runs of nearby seeds. Discarding one output lets the
    // multiply diffuse the seed into the high bits first. Deterministic, so still identical per peer.
    rng.next_u32();
    // Side A: uniform over the whole roster, so A is not pinned to roster[0].
    let a = rng.below(n as u32) as usize;
    // Side B: uniform over the roster with A removed, so B != A (anti-mirror). Draw an index in
    // [0, n-1), then skip past A to map it back onto the full roster.
    let mut b = rng.below((n - 1) as u32) as usize;
    if b >= a {
        b += 1;
    }
    (roster[a], roster[b])
}

#[cfg(test)]
mod tests {
    use super::*;

    const ROSTER: [Army; 2] = [Army::Us, Army::Fr];

    #[test]
    fn deterministic_same_seed_same_result() {
        for seed in 0u64..500 {
            assert_eq!(
                assign_ranked_1v1(seed, &ROSTER),
                assign_ranked_1v1(seed, &ROSTER),
                "seed {seed} produced different assignments on repeat"
            );
        }
    }

    #[test]
    fn anti_mirror_never_matches_for_two_army_roster() {
        for seed in 0u64..2000 {
            let (a, b) = assign_ranked_1v1(seed, &ROSTER);
            assert_ne!(a, b, "seed {seed} produced a mirror match {a:?} vs {b:?}");
        }
    }

    #[test]
    fn both_orderings_reachable_across_seeds() {
        let mut saw_us_fr = false;
        let mut saw_fr_us = false;
        for seed in 0u64..2000 {
            match assign_ranked_1v1(seed, &ROSTER) {
                (Army::Us, Army::Fr) => saw_us_fr = true,
                (Army::Fr, Army::Us) => saw_fr_us = true,
                other => panic!("seed {seed} produced an out-of-roster pair {other:?}"),
            }
        }
        assert!(
            saw_us_fr,
            "ordering (Us, Fr) never occurred — side A may be hardcoded"
        );
        assert!(
            saw_fr_us,
            "ordering (Fr, Us) never occurred — side A may be hardcoded"
        );
    }

    #[test]
    fn anti_mirror_holds_for_larger_rosters() {
        let roster = [Army::Us, Army::Fr, Army::UsWw2, Army::Germany];
        for seed in 0u64..2000 {
            let (a, b) = assign_ranked_1v1(seed, &roster);
            assert!(roster.contains(&a) && roster.contains(&b));
            assert_ne!(
                a, b,
                "seed {seed} produced a mirror match on a 4-army roster"
            );
        }
    }

    #[test]
    fn single_army_roster_is_forced_mirror() {
        // len == 1: a mirror is the only option.
        let (a, b) = assign_ranked_1v1(7, &[Army::Us]);
        assert_eq!((a, b), (Army::Us, Army::Us));
    }

    #[test]
    fn empty_roster_falls_back_to_neutral() {
        let (a, b) = assign_ranked_1v1(7, &[]);
        assert_eq!((a, b), (Army::Neutral, Army::Neutral));
    }
}
