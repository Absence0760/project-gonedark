---
description: Implement a non-trivial change with a code-reviewer agent loop — coder → review → fix → review → ready-to-commit. Costs ~2-3x a normal edit; use for high-blast-radius changes (sim/netcode, the PAL boundary, embodiment, secrets/infra).
argument-hint: <task description>
---

Implement the task `$ARGUMENTS` with the `code-reviewer` agent in the loop.

## When to use
**Right fit:**
- Anything inside the deterministic sim/core (fixed-point math, ECS systems, the AI
  executor, pathfinding) — invariant #1/#3 are load-bearing and desyncs are silent.
- Netcode / lockstep / checksum changes (invariant #7).
- Anything that crosses the **PAL boundary** (invariant #2) — a core change that risks
  pulling in a platform crate, or a PAL change that risks leaking into core.
- The embodiment / going-dark seam (invariants #5, #6).
- Secrets / Terraform / sops handling (invariant #8).
- Anything you want a second pair of eyes on before commit.

**Wrong fit — refuse, tell the user to edit directly:** typos, one-line doc corrections,
comment edits, diffs under ~10 lines touching no invariant. **Docs-only design changes**
go through `/check-docs` + `/decision`, not here. If trivial, abort and say so — the loop
costs ~2-3x tokens and a round or two of agent latency.

## The loop

1. **Coder pass.** Implement the task. Track multi-step work with TaskCreate. After
   sim/core edits, you may run the `determinism-auditor` agent as part of this pass. Do
   NOT commit yet.

2. **Round 1 review.** Spawn `code-reviewer`:
   > "Review the working diff against the project's documented invariants. The task being
   > implemented is: `$ARGUMENTS`. Output the strict format from your spec."

   - `Status: CLEAN` → step 5.
   - `Status: NEEDS_CHANGES` with concrete file:line findings → step 3.

3. **Apply fixes.** For each Critical/Improvement item: if correct, apply it; if wrong
   (reviewer misread / cited a rule that doesn't apply), state explicitly *why* you're
   not applying it — don't silently skip; if borderline, apply (the reviewer will retract
   on the next round if it was wrong).

4. **Round 2 review.** Spawn `code-reviewer` again, same prompt, **quoting any Round-1
   disagreement** so it can re-evaluate.
   - `CLEAN` (or retracts Round-1 findings) → step 5.
   - `NEEDS_CHANGES` again → **stop the loop.** Surface the remaining findings + what you
     tried. Do not auto-cycle to Round 3 — the user decides.

5. **Ready-to-commit handoff.** Tell the user: what changed (one-line summary), which
   round produced the clean status, any Notes/Out-of-scope observations, and ask whether
   to commit. **Never commit without being asked** (CLAUDE.md).

6. **On user "yes":** stage changed files explicitly (the `git-scope-guard.py` hook
   blocks `git add -A/.` and bare/whole-tree commits — use `git commit -m "…" -- <paths>`),
   write a commit message in the project's style (**no `Co-Authored-By` / "Generated
   with Claude Code" / robot footer** — user-level rule), commit, report. Never `git push`.

## Loop-termination guarantees
- Hard cap: 2 review cycles. Round 3 is forbidden.
- The reviewer can't re-cycle on abstract concerns — its spec requires concrete numbered
  file:line changes for `NEEDS_CHANGES`. Vague "consider X" → treat as `CLEAN`, surface
  the comments.
- `Out-of-scope observations` are informational; they never trigger a cycle.

## What this does NOT replace
- `/check` — the lighter single-pass advisory gate for everyday changes.
- The regular `cargo test` / `cargo clippy` / determinism CI matrix runs.
- `/audit-determinism` — the periodic broad sweep. `/safe-edit` is per-change.

## Tone
Don't narrate the loop. The user sees: normal coder updates, a short "Round 1 found N
findings: […]; applied them" or "Round 1 clean", then "Round 2 clean — ready to commit.
Want me to?" — or, if no consensus, a clear summary of what's still contested.
