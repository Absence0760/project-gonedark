---
description: Go wide hunting for real correctness bugs across the engine — reproduce each with a probe, confirm it's real, fix at the root, lock it with a regression test, then sweep sibling paths. Multi-round; commits scoped; never pushes. (Requires engine code — until then, there's nothing to hunt.)
argument-hint: "[optional scope — a system, feature, or path, e.g. 'pathfinding', 'the lockstep step', crates/sim/src/movement.rs; omit to let it choose high-yield targets]"
---

Hunt for genuine correctness bugs and land the fixes. The cross-cutting, multi-round
companion to the targeted `/audit-determinism` sweep: `/bug-hunt` ranks high-yield
targets, finds bugs, **proves each with a runnable probe before believing it**, fixes the
root cause, ships a regression test that would fail on the old code, then sweeps the
sibling paths that share the pattern.

`$ARGUMENTS` is an optional scope. If empty, you pick targets (step 1).

**Precondition:** this needs engine code. The repo is pre-production/design-only
(decisions.md D10) — if there are no `crates/`/Rust sources yet, say so and stop; there's
nothing to hunt. (Design-doc inconsistencies go through `/check-docs`.)

## Operating rules (non-negotiable — CLAUDE.md invariants)
- **Prove it before you believe it.** A bug isn't real until reproduced — a failing probe
  (a throwaway `#[test]`, a golden-tick comparison, a small `cargo run` harness). No
  probe, no finding. Delete throwaway probes before committing.
- **Fix the root cause — never mask.** No widened tolerances, no swallowed desyncs, no
  silenced checksums. If you can't fix it now, surface it and file a roadmap follow-up.
- **Be honest when there's no bug.** If a target is sound, say so and make the deliverable
  the coverage gap you closed — never invent a "fix" to justify the command.
- **Determinism is sacred.** Never "fix" a desync by relaxing the checksum, dropping a CI
  platform, or introducing a float into the sim. The fix makes the math bit-identical.
- **Docs-as-code.** A behaviour/command/convention change updates its doc in the same
  commit.
- **Commit each logical unit, path-scoped; never push.** Fix and tests can be separate
  commits (`git commit -m "…" -- <paths>`; `git-scope-guard.py` blocks bare/whole-tree
  commits and `git add -A/.`).

## Where bugs will live here
Bias the hunt toward the classes that bite a deterministic RTS/FPS engine:
- **Determinism leaks** (the #1 class — desyncs are silent). A float laundered into sim
  math; a non-deterministic transcendental; unstable `HashMap`/`HashSet` iteration driving
  spawn/tick/AI order; unseeded or out-of-sequence RNG; a width-dependent integer
  (`usize`) in checksummed state; wall-clock/frame-time feeding sim. (Run
  `determinism-auditor` to seed candidates.)
- **Inconsistent logic across paths that should agree.** Two places computing "the same"
  thing differently — fixed-point rounding in one site but not its sibling, an order
  applied differently in the predicted vs authoritative path. Find the canonical version;
  diff the others against it.
- **Order/stance executor edge cases (invariant #3).** A unit with no order, an order
  arriving the same tick as embodiment hand-off, a stance transition that silently drops
  the last order — and the inverse failure: a unit that starts making *autonomous*
  decisions it shouldn't (that's a design violation, flag it).
- **Embodiment / going-dark seam (invariants #5, #6).** Input-source swap races, death-
  ejects-to-command edge cases, and **fairness leaks** — any path that hands the embodied
  player map intel instead of an alert is a bug.
- **Sim/render coupling (invariant #4).** Render reading mid-tick sim state without
  interpolation; sim mutating from a render callback.
- **PAL boundary leaks (invariant #2).** A platform crate reachable from core; game logic
  forked per platform.
- **Edge cases:** fixed-point overflow/underflow, divide-by-zero, empty/zero quantities,
  pathfinding on unreachable/cornered targets, pagination/grid boundaries, concurrent
  order application, out-of-order order arrival.

## Procedure
1. **Pick targets.** With `$ARGUMENTS`: resolve to concrete paths and hunt within.
   Empty: rank by logic-density × under-coverage × recent churn (`git log --oneline -20
   <file>`), skipping generated/asset files. Favour sim math, the lockstep step,
   pathfinding, the AI executor, and shared helpers (blast radius). State each pick + why
   in one line; prefer targets not hit in a recent session.
2. **Map before judging.** Recon the target's contract — data model, call sites, the
   invariant it must hold. For anything non-trivial spawn an `Explore` agent to map
   callers/siblings. Note the canonical version of any duplicated logic.
3. **Hunt + reproduce.** Trace the mechanism, then **write a probe that fails on current
   code.** For determinism bugs the probe is a two-run / two-target checksum comparison
   that diverges. Keep probes throwaway and named (`probe_*`).
4. **Fix at the root.** Durable fix, matched to surrounding idiom, tightly scoped. If a
   quick patch and the durable fix diverge, name the durable fix even if you ship the
   patch.
5. **Lock it with a regression test.** Promote the probe to a real test at the right
   layer (unit `#[test]` for pure logic; golden-tick/checksum test for sim-state
   evolution). It must fail on the old code, pass on the fix, and assert the invariant the
   bug violated.
6. **Sweep the siblings.** Grep for the same shape elsewhere (other sim systems, other
   order handlers, other render-read sites) and fix-and-test them too, or state they're
   correct (one-line reason). This sweep is where `/bug-hunt` earns its keep.
7. **Verify + review.** Run `cargo clippy` + `cargo test` (scoped where possible) and the
   new tests; run nearby suites to prove no regression — report counts faithfully. For
   determinism/netcode/PAL/embodiment diffs, run `code-reviewer` before committing.
8. **Commit (scoped) — never push.** Conventional-commit style, no AI/co-author trailer;
   docs ride with the behaviour change. Loop to step 3 for the next target until the scope
   (or the user's round budget) is covered.

## Report
```
## /bug-hunt — <scope or "self-selected">

**Targets:** <each pick + one-line why>

**Bugs found & fixed:**
- <file:line> — <what was wrong> → <root-cause fix> | repro: <how> | test: <file (layer)>
- … (or "none — targets were sound; coverage backfilled where thin")

**Sibling sweep:** <same-shape paths — fixed / confirmed correct + why>

**Verification:** <clippy/test gate; new tests N/N; nearby suites N/N; review verdict>

**Commits:** <hash + subject, one per line>

**Deferred / recommended:** <out-of-scope leads + the long-term fix named + follow-up — or "nothing outstanding">
```

## Tone
Lead with the verdict, not the process. State a real bug plainly with its repro; if a
target was sound, say so and point at the coverage you added. Don't dress up a non-finding
as a fix.
