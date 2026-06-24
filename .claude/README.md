# .claude/

Project-scoped sub-agents, slash commands, and hooks. Checked into git so every
contributor (and every future Claude session) gets the same review surface. Adapted from
the patterns in the sibling repos (`project-account-payables`, `project-running`) and
retargeted at *Going Dark*'s invariants. The repo is **pre-production, design-only** (the
Rust engine hasn't started — decisions.md D10), so the engine-code tooling below is
written to **bail cleanly until `crates/` exist** — it's in place now so it's ready the
moment code lands, and the determinism rules don't have to be reinvented under pressure.

## Sub-agents (`agents/`)

| Agent | What it does |
|---|---|
| [`design-doc-keeper`](agents/design-doc-keeper.md) | Keeps the `docs/` corpus internally consistent — decision-log format/numbering, open-questions sync, cross-reference/link integrity, README repo-map, no-contradiction-with-invariants. Mechanical fixes only. Invoked by `/check-docs` and `/check`. |
| [`determinism-auditor`](agents/determinism-auditor.md) | Read-only audit of engine/sim code for anything that breaks the deterministic fixed-point sim or cross-platform lockstep (invariants #1, #2, #4, #7). Backend for `/audit-determinism`. No-op while design-only. |
| [`code-reviewer`](agents/code-reviewer.md) | Reviews the working diff against the documented invariants (CLAUDE.md non-negotiables, decisions.md ADRs, the determinism checklist, the PAL boundary). Outputs `CLEAN`/`NEEDS_CHANGES` with concrete file:line findings. Read-only. Defers docs-only diffs to `design-doc-keeper`. Invoked by `/safe-edit` and `/check`. |
| [`test-gap-checker`](agents/test-gap-checker.md) | Cross-references modified sim/core source against the tests in the diff — unit tests plus the cross-platform per-tick checksum coverage any sim/netcode change must keep (invariant #7). Reports gaps; never writes tests. Invoked by `/check`. |

## Slash commands (`commands/`)

| Command | What it does |
|---|---|
| [`/check-docs`](commands/check-docs.md) | Consistency sweep over the design corpus via `design-doc-keeper`. The pre-commit gate during pre-production (docs are the product). |
| [`/decision`](commands/decision.md) | Append an ADR-style `Dn` to `docs/decisions.md` (append-only, **Why** mandatory) and sync any doc it touches; migrate a resolved `Qn` out of `open-questions.md`. |
| [`/check`](commands/check.md) | Pre-commit gate: `code-reviewer` + `test-gap-checker` + `design-doc-keeper` in parallel against the working diff (docs-only diffs run just the doc-keeper). Advisory; applies nothing. |
| [`/safe-edit`](commands/safe-edit.md) | Coder ↔ reviewer loop (max 2 cycles) for high-blast-radius changes — sim/netcode, the PAL boundary, embodiment, secrets/infra. ~2-3x a normal edit. |
| [`/audit-determinism`](commands/audit-determinism.md) | On-demand read-only determinism sweep via `determinism-auditor` + a CI-matrix check. Heavier than `/check`; hands fixes off to `/safe-edit` or `/bug-hunt`. |
| [`/bug-hunt`](commands/bug-hunt.md) | Go wide for real correctness bugs (determinism leaks first), reproduce each with a probe, fix at the root, lock with a regression test, sweep siblings. Multi-round; commits scoped; never pushes. |
| [`/fix-ci`](commands/fix-ci.md) | Fix a failing GitHub Actions job. Root-causes, reproduces with CI's command, fixes without band-aids, lands coverage. A cross-platform checksum mismatch is treated as a real desync — never narrowed away. |

## Hooks (`hooks/`)

| Hook | When it runs | What it does |
|---|---|---|
| [`git-scope-guard.py`](hooks/git-scope-guard.py) | PreToolUse on `Bash` | **Multi-session safety — the "don't commit another session's work" guard.** Concurrent Claude sessions (and `--worktree` runs sharing the primary checkout) can otherwise sweep up each other's in-flight edits. Denies the unscoped whole-tree ops (`git add -A`/`.`/`-u`, bare `git commit`, `git commit -a`/`--amend` with staged changes, `git stash` without `-- <path>`, `git reset --hard`, `git checkout`/`restore .`, `git rm .`, `git clean -f`) and points each denial at the path-scoped alternative. Path-scoped ops (`git add <path>`, `git commit -m "…" -- <path>`) pass through. Generic — verbatim from the siblings; covered by [`git-scope-guard.test.py`](hooks/git-scope-guard.test.py) (37 cases, green here). |
| [`unmerged-worktree-check.sh`](hooks/unmerged-worktree-check.sh) | SessionStart (`startup`/`resume`) | Lists every local branch holding commits not yet on `main` (a forgotten `--worktree` branch) so the work gets consolidated before it's lost. Silent when everything's on `main`; fail-open. |
| [`sim-determinism-guard.sh`](hooks/sim-determinism-guard.sh) | PostToolUse on `Edit`/`Write`/`MultiEdit` | **Project-specific.** Flags floats (`f32`/`f64`, FP literals), std/libm transcendentals, and process-randomised `HashMap`/`HashSet` iteration leaking into the fixed-point sim — the bug class that desyncs lockstep **silently** (invariants #1, #7). Scoped to `.rs` under the future sim/core crate paths, so **INERT until engine code exists**. Bypass a false positive with `// noqa: <rule>` + rationale. |

Wired in [`settings.json`](settings.json), which also carries `permissions`: an **allow**
list pre-approving safe high-frequency commands (read-only git/gh, `cargo`
build/check/test/clippy/fmt-check, `terraform` fmt-check/validate/plan, search), and a
**deny** list hard-blocking shared-checkout footguns (`git commit --no-verify`,
`push --force`/`-f`, `reset --hard`), state-mutating infra (`terraform apply`/`destroy`),
and `sudo` / `rm -rf /` / `curl | sh`.

## Parallel development with worktrees

Run independent tasks in isolated checkouts so two sessions never fight over one working
tree:

```
claude --worktree <name>          # new git worktree on its own branch, off HEAD
```

- **`settings.json` → `worktree.baseRef: "head"`** branches each worktree from the current
  `HEAD`, so a worktree starts from a clean, known base.
- **[`.worktreeinclude`](../.worktreeinclude)** (repo root) lists the *gitignored* personal
  files copied into each new worktree (committed defaults like `.env.development` already
  travel as tracked content). Build caches (`/target`, `infra/.terraform`) are
  deliberately not copied — rebuild with `cargo build` / `terraform init` inside the
  worktree.
- **`git-scope-guard.py`** is what makes this safe: even with several checkouts sharing
  state, no session can stage or commit anything but the explicit paths it names — so a
  worktree branch can't absorb another session's edits.
- **`unmerged-worktree-check.sh`** fires at session start and lists any worktree branch
  whose commits haven't reached `main`, so parallel work gets consolidated and never
  stranded. To land it: from the primary checkout, `git merge <branch>`.

## Adapting as the engine lands (roadmap Phase 0/1)
"Keep the framework, swap in the specifics."
- Tighten `sim-determinism-guard.sh`'s `SIM_PATH_RE` to the finalised crate layout once
  it's decided.
- Point `code-reviewer` / `test-gap-checker` at the real sim/core crate, the PAL boundary,
  and the CI workflow paths as they appear (they're written generically today).
- Stand up the cross-platform per-tick checksum CI matrix (invariant #7); `/fix-ci` and
  `/audit-determinism` already assume it exists.
- The siblings also carry domain-specific surfaces — persona bug-hunters, GDPR/app-store
  compliance auditors, DB-migration coordinators — that are **not** relevant to a game
  engine and were intentionally left out. Add new ones as ADRs introduce new invariants.

## What lives here vs. what doesn't
- **In `.claude/`**: agent/command definitions, hooks, project-scoped `settings.json`.
- **Not in `.claude/`**: `settings.local.json` (per-user, git-ignored), hook-test
  bytecode (`hooks/__pycache__/`, git-ignored), anything user-specific.
