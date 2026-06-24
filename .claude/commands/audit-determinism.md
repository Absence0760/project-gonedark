---
description: On-demand determinism sweep — runs the determinism-auditor across the sim/core, then the PAL boundary, and reports every lockstep hazard by severity. Read-only; lands no fixes (use /bug-hunt or /safe-edit for that). Heavier than /check. Requires engine code.
argument-hint: "[optional focus — e.g. 'pathfinding', 'the lockstep step', 'the PAL boundary', or a path; omit for a full sweep]"
allowed-tools: Task, Bash, Read, Grep, Glob
---

Audit the engine for anything that can break the deterministic fixed-point simulation or
cross-platform lockstep — the bug class that desyncs **silently** (CLAUDE.md invariants
#1, #2, #4, #7). Read-only: this surfaces hazards; it does not fix them.

`$ARGUMENTS` is an optional focus area. If empty, sweep the whole sim/core.

**Precondition:** needs engine code. The repo is pre-production/design-only (decisions.md
D10) — if there are no Rust sources yet, say so and stop. Nothing to audit. (The
`sim-determinism-guard.sh` PostToolUse hook already guards new edits in the meantime.)

## What this is
The fix-and-land counterparts are `/bug-hunt` (wide, multi-round) and `/safe-edit`
(per-change). This command is the focused **read-only** sweep — heavier than `/check`,
lighter than a full bug hunt. Use it: before merging a netcode/sim branch; when
investigating a reported desync; as a periodic safety pass on the deterministic core.

## Procedure

### 1. Locate the deterministic core
Find the sim/core crate, the fixed-tick loop, the fixed-point types, the lockstep step,
and the checksum. Confine the audit to it plus the PAL boundary. **Renderer and PAL
internals are float-legal and out of the determinism contract** — don't flag floats there
(do flag the PAL *leaking into* core).

### 2. Spawn the determinism-auditor
Invoke the `determinism-auditor` agent with the focus from `$ARGUMENTS` (or "full sim/core
sweep" if empty):
> "Audit <focus> for determinism hazards per your spec. Lead with the highest-severity
> items; cite file:line for every finding. Read-only."

The agent walks its severity ladder: floats in sim → non-deterministic transcendentals →
unstable `HashMap`/`HashSet` iteration → unseeded/divergent RNG → address-dependent state
→ width-dependent integers in checksummed state → platform leakage into core →
wall-clock/frame-time in sim → missing cross-platform checksum CI.

### 3. Corroborate the textual shapes
While the agent runs, you may grep the obvious patterns yourself to cross-check
(`\bf(32|64)\b`, `HashMap<`, `thread_rng`, `Instant::now`, `as usize` in serialized
state) and confirm the CI matrix in `.github/workflows/` still spans
`{x86_64-pc-windows-msvc, x86_64-unknown-linux-gnu, aarch64-linux-android,
aarch64-apple-ios}` (invariant #7). Don't double-report — fold your corroboration into the
agent's findings.

### 4. Report
```
## /audit-determinism — <focus or "full sim/core">

**Verdict:** <CLEAN | N hazard(s)>

**Findings (severity-ordered):**
- [Critical|High|Medium] file:line — <what> — why it desyncs — the fix
- …

**CI matrix:** <all four targets run per-tick checksum diffing | gaps named>

**Recommended next step:** <"/safe-edit <the fix>" | "/bug-hunt <scope>" | "clean — no action">
```

End with the single highest-priority action. Do not apply fixes — hand off to `/safe-edit`
or `/bug-hunt`.
