---
description: Pre-commit gate — runs code-reviewer + test-gap-checker + design-doc-keeper in parallel against the working diff. Advisory output. Cheaper than /safe-edit; use before every non-trivial commit. (For a docs-only diff this is mostly /check-docs.)
allowed-tools: Task, Bash, Read, Grep, Glob
---

Run a parallel audit on the working diff, aggregate findings, and report. Advisory only —
you don't apply fixes here; the user decides which to land.

## When to use
**Right fit:** right before a non-trivial commit; after a bug fix, to confirm a
regression test went with it; after an engine change, to confirm coverage + determinism
tests went with it.

**Wrong fit — refuse:** trivial diffs (typos, comment edits, dep bumps with no source
change); empty `git status` (nothing to audit — tell the user).

## What this does NOT do
- It does NOT apply fixes. Every agent is read-only; the output is a gap list.
- It does NOT replace `/safe-edit`'s coder ↔ reviewer loop (that's for high-blast-radius
  changes: sim/netcode, the PAL boundary, embodiment, secrets/infra).

## Procedure

### 1. Sanity-check the diff exists
`git status`. If staged and unstaged are both empty, abort: nothing to audit.

### 2. Confirm it's not trivial
If trivial (typo, comment, single-line dep bump, generated-file regen only), abort with a
one-line "trivial — skipping `/check`".

### 3. Spawn agents in parallel
Decide by diff content:
- **Docs-only diff** (`docs/`, `*.md`, README, CLAUDE.md) — the engine doesn't exist yet,
  so this is the common case. Spawn only `design-doc-keeper`. (This is effectively
  `/check-docs`.)
- **Engine/infra diff** — send a single message with three Agent calls:
  - `code-reviewer` — "Review the working diff against the project's documented
    invariants (CLAUDE.md non-negotiables, decisions.md ADRs, the determinism checklist,
    the PAL boundary). Output the strict format from your spec."
  - `test-gap-checker` — "Audit the working diff for missing unit + determinism/checksum
    test surface. Output the format from your spec."
  - `design-doc-keeper` — "The diff may have invalidated a doc (architecture, platforms,
    decisions, README repo-map). Report which docs need updating; mechanical fixes only."

  Parallel because they're independent (all read `git diff` + files).

### 4. Aggregate
```
## /check report

**Change:** <one-sentence summary>

### Code review (`code-reviewer`)
Status: <CLEAN | NEEDS_CHANGES | n/a — docs-only>
<verbatim findings, or "no concrete findings">

### Test gaps (`test-gap-checker`)
<verbatim verdicts, or "test surface is consistent" / "n/a — no engine code">

### Doc gaps (`design-doc-keeper`)
<verbatim verdicts, or "doc set is clean">

### Recommendation
<one of: all clean — ready to commit · N test gap(s) — land now or follow-up ·
N code-review finding(s) — apply or push back · multiple gaps — list and let the user pick>
```

### 5. Hand off
Ask how to proceed. **Do not** apply fixes automatically. Do not commit.

## Tone
Don't narrate the fan-out. The user sees: a one-line "Running review + test-gap + doc
checks…", the aggregated report, and a short "Apply the test gaps? Land as-is? Follow-up
task?" at the end.
