# Combat rebalance plan — inter-unit balance + suppression at lethal speed

> **Status: COMPLETE — both workstreams landed, [Q18](../open-questions.md) closed.** WS-A
> ([D69](../decisions.md): Heavy HP 280→300, damage 90→100; Rifleman/Heavy RPS restored) and WS-B
> ([D70](../decisions.md): area suppression + `SUPPRESSION_PIN` 1/2→3/8; concentrated fire pins a
> cluster before the kill, lone shooter never pins). Both harness-measured; metrics re-pinned to the
> intended properties (`heavy_wins_close_rifle_wins_at_range`,
> `focus_fire_pins_before_kill_but_lone_shooter_never_pins`). Addresses [Q18](../open-questions.md). Follows the
> modern-combat-realism arc: [D66](../decisions.md) (×5 lethality) and [D67](../decisions.md)
> (all-unit ammo + resupply) shipped. This doc is the build sequencing for the *re-tune* those
> changes made necessary — the analogue of [`playability-plan.md`](playability-plan.md), scoped to
> balance, not engine risk. It is **faction-independent** ([`factions.md`](../factions.md) build
> depends on the full rebalance landing first — [`factions-plan.md`](factions-plan.md) WS-0).

---

## Why this exists

[D66](../decisions.md) scaled per-shot damage ×5 so a hit kills like a real rifle round (rifle 1v1
~1.5 s / 4 hits, down from ~8 s). Uniform scaling preserved the [D30](../decisions.md) DPS *ratios* on
paper, but at 1–2-volley kill speed two **emergent** balance properties measurably broke — and the
`--metrics` tests were honestly **re-pinned to lock the regression** rather than assert the now-false
properties (so it can't drift silently). This plan un-breaks them:

1. **The Rifleman-vs-Heavy rock-paper-scissors flattened.** At lethal speed the rifle mass's
   body-count + faster cadence win at *every* range (heavies wiped 0-for), not just at range. The
   intended matchup — *heavies win close, rifles kite at range* ([D30](../decisions.md)) — is gone.
2. **Per-hit suppression stopped pinning before the kill.** The target dies before it accumulates
   `SUPPRESSION_PIN`, so focus-fire's "concentrate fire to pin" lever is vestigial. This is the
   worst loss: **suppression + maneuver is the core of modern infantry doctrine** — exactly the
   fantasy the US-vs-France direction ([D68](../decisions.md)) leans into.

Measured baseline this plan moves off of (from `--metrics summary`, the [D30](../decisions.md) harness):

```
rifle 1v1 TTK: 91 ticks (~1.5s)              ← keep (D66 target)
equal-cost 500 sep5:  rifle 2, heavy 0       ← WANT: heavy wins close
equal-cost 1000 sep9: rifle 6, heavy 0       ← WANT: rifle wins at range
4-on-1 focus: pin at 0 (never), kill at 1    ← WANT: pin before kill
```

---

## The load-bearing call

**Measure, don't feel.** Every change is dialed against the deterministic balance-metrics harness
(`sim-runner --metrics`, the same objective signal the [D30](../decisions.md) re-tune used), not by
vibe. Everything stays **fixed-point** (invariant #1) and **checksum-folded** where it touches sim
state (suppression and the weapon stats already fold), so the cross-arch + 2-peer lockstep runners
remain the safety net (invariant #7). The two workstreams are independent and can land separately.

---

## Workstreams

### WS-A — Restore the Rifleman / Heavy rock-paper-scissors

Re-tune the Heavy (and only the Heavy, to start) so it wins the equal-cost trade **close** while the
longer-ranged Rifleman still kites it **at range** — at the new lethal kill speed.

- **Levers (in `economy::unit_stats`, fixed-point):** Heavy durability (HP) and/or close-range
  throughput (damage, cooldown). The Heavy's shorter range (11 vs 14) is the load-bearing weakness
  that *must* stay (it's what makes the matchup range-dependent — [D30](../decisions.md) notes this);
  do not touch rifle range. Iterate against `equal_cost_outcome(500, 5)` (close) and
  `equal_cost_outcome(1000, 9)` (range) until close→heavy-wins and range→rifle-wins both hold.
- **Re-pin the metrics tests to the *intended* properties**, reversing the [D66](../decisions.md)
  regression-lock: `equal_cost_outcomes_locked_at_lethal_baseline` →
  `heavy_wins_close_rifle_wins_at_range` (or restore the original two-test split). Keep
  `rifle_ttk_in_lethal_band` (the D66 TTK target is correct and stays).

**Tests (same commit):** the metrics tests assert the restored RPS (heavy survivors > 0 close, rifle
survivors > 0 at range, each 0-for the other); `rifle_ttk_in_lethal_band` still green; regenerate any
golden checksum the stat change moves (the embodied-scene golden is rifle-only, so likely only
heavy-bearing goldens move — confirm). Green **dev + release**; `determinism.yml` matrix green.

### WS-B — Suppression that bites at lethal speed (proximity/area suppression)

Make suppression a real fire-and-maneuver lever again. Today suppression is added **only to the unit
a shot lands on** (`SUPPRESSION_PER_HIT`), so at lethal speed a target dies before it pins. The fix
that matches doctrine: **a shot suppresses the area, not just the body it hits** — rounds cracking
past a position pin the soldiers near the impact, even the ones not being shot.

- **In `core::combat`:** when a shot resolves (both the auto engage pass and `resolve_fire`), add a
  reduced suppression increment to **enemy units within `SUPPRESSION_RADIUS` of the target**, in
  addition to the full `SUPPRESSION_PER_HIT` on the target itself. New fixed-point constants
  (`SUPPRESSION_RADIUS`, `SUPPRESSION_SPLASH_PER_HIT`); index-ordered radius scan (or reuse the
  per-tick `SpatialHash`), no float, no RNG. This makes concentrated fire pin a *cluster* before it
  wipes it one-by-one — the property `focus_fire_pin_kill` wants.
- **Tune** `SUPPRESSION_RADIUS` / splash magnitude (and revisit `SUPPRESSION_PER_HIT` /
  `SUPPRESSION_PIN` if needed) against `focus_fire_pin_kill(m)` so a multi-shooter focus pins **before**
  the kill while a lone shooter still resolves by damage (never pins) — the [D26](../decisions.md)/
  [D30](../decisions.md) goal, now achievable because area suppression outruns single-target death.

**Tests (same commit):** `suppression_no_longer_pins_before_kill_at_lethal_speed` →
`focus_fire_pins_before_kill_but_lone_shooter_never_pins` restored (pin4 > 0 and pin4 < kill4; pin1 ==
0); new `core::combat` unit tests for area suppression (a shot at A suppresses a nearby B; out of
radius does not; deterministic index order). Suppression is checksum-folded → regenerate the goldens
the new writes move; full determinism audit (this changes the per-tick sim) — run under
[`/safe-edit`](../../.claude/commands).

---

## Sequencing

```
   WS-A  (Heavy RPS re-tune)   ─┐
                                ├─►  (ship the rebalance; unblocks factions-plan WS-0)
   WS-B  (area suppression)    ─┘
```

Independent — either can land first. Both are measured against `--metrics` and re-pin the tests that
[D66](../decisions.md) parked at the regression baseline. Landing both closes [Q18](../open-questions.md).

## Determinism & fairness guardrails

- **Fixed-point only**, no floats in any stat or suppression math (invariant #1; the determinism guard
  greps it).
- **Suppression + weapon stats are checksum-folded** → behavior changes move the stream *by design*;
  regenerate goldens, never narrow the matrix (invariant #7).
- **Measured, not felt** — every number dialed against `sim-runner --metrics`, the objective signal
  ([D30](../decisions.md)).
- **No power creep** — this is a *relative* re-tune (restore the matchup), not a numbers-up arms race;
  the cost-parity discipline ([D30](../decisions.md), pillar 4) holds.

## Verification

- Per WS: tests **in the same commit**, green `cargo test` **dev + release**; the determinism + 2-peer
  lockstep runners agree; [`/check`](../../.claude/commands) before commit, [`/safe-edit`](../../.claude/commands)
  for WS-B (it changes the per-tick combat resolution).
- End-to-end: `sim-runner --metrics summary` shows the restored RPS + pin-before-kill numbers, and a
  by-hand skirmish read confirms a Heavy push *feels* like it trades, and a squad caught in the open
  *feels* pinned — the human-confirmation layer (same honesty bar as D31 / playability-plan).
