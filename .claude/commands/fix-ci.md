---
description: Fix a failing CI job from a GitHub Actions run. Root-causes the failure, fixes it without coding around it (no retry/timeout/skip band-aids), reproduces locally with the same command CI used, and lands coverage so it can't return silently. For a lockstep checksum-matrix failure, a desync is a real bug — never narrow the matrix to make it pass.
argument-hint: "<GitHub Actions run URL or run ID> [optional: which job]"
---

Fix the failing CI run `$ARGUMENTS`. Find the real cause, fix it at the root, add
coverage, stop before pushing.

## The two hard rules (override convenience)

1. **Do not code around the issue.** A red test catching a real defect is doing its job —
   fix the defect, not the test. Forbidden unless you can *prove* it's pure infra noise
   *and* name the structural reason: bumping a timeout/retry/`sleep`; adding
   `#[ignore]` / `--skip` / `continue-on-error` / `fail-fast: false`; loosening an
   assertion or widening a tolerance; deleting the case; re-running until green. A test
   guarding a project invariant — **per-tick checksum equality across platforms
   (invariant #7)**, no-floats-in-sim (#1), the PAL boundary (#2), deterministic
   iteration/RNG — is *especially* doing its job. **A cross-platform checksum mismatch is
   a desync, i.e. a real determinism bug. Never make it pass by dropping a platform from
   the matrix or relaxing the comparison.**
2. **Add coverage where the gap let it through.** Leave behind something that fails loudly
   next time — a pinning `#[test]`, a golden-tick/checksum case, an explicit assertion —
   in the **same commit** as the fix.

## Procedure

### 1. Pull the failure apart
- `gh run view <id>` — which job(s) failed and at which step. The CI matrix runs across
  `{x86_64-pc-windows-msvc, x86_64-unknown-linux-gnu, aarch64-linux-android,
  aarch64-apple-ios}` (platforms.md §7) — note *which platform(s)* diverged: a failure
  on one target but not others is the classic determinism leak.
- `gh run view <id> --log-failed` (or `--job=<job-id>`). Grep to the **first** real error
  — usually a `cargo clippy`/`cargo fmt --check` diff, a `cargo test` assertion, or a
  checksum-mismatch panic — not the final `exit 1`. `gh` rate-limits; anchor greps.
- Quote the failing job + step + error line back to the user so you're both anchored.

### 2. Classify it honestly
- **Genuine defect** — app/test/sim is wrong. Fix it; pin it.
- **Determinism leak** — passes on some targets, fails on others (or differs run-to-run).
  Root cause is almost always a float in sim, a non-deterministic transcendental, unstable
  `HashMap` iteration, unseeded RNG, or a width-dependent integer in checksummed state.
  Run the `determinism-auditor` agent on the touched sim/core paths. The fix is to make
  the computation bit-identical — never to silence the comparison.
- **Test bug** — asserts the wrong thing or races in its own setup. Fix it correctly.
- **Infra flake** — toolchain/cache/runner issue. Fix is to make the step deterministic,
  not retry it. "Passed on re-run" narrows toward flake but does not license a band-aid.

### 3. Read surrounding context before changing it
CI YAML and code carry comments documenting prior incidents. Read them; make them obsolete
by removing the failure mode and update the comment to match.

### 4. Reproduce locally, then verify the fix locally
Reproduce with the **same command CI used**, against CI's pinned toolchain.
- Lint/format: `cargo fmt -- --check` and `cargo clippy --all-targets -- -D warnings`.
- Tests: `cargo test` (or the scoped `cargo test -p <crate> <name>`).
- Determinism: run the checksum/golden-tick test; where you can cross-compile or target a
  second arch locally, do — reproducing the *cross-target* divergence is the whole point.
Confirm the failure reproduces **before** the fix and is gone **after**. Capture the
evidence (exit codes, checksums, counts) — report it, don't claim it.

### 5. Apply the fix at the lowest sensible layer
If the same broken shape appears in a sibling crate/system, fix all of them — spin up an
`Explore` agent to find every site first. Keep the blast radius proportional; prefer the
surgical, behaviour-stable change over a broad toolchain bump.

### 6. Sweep docs
If you changed a CI job's steps, a command, an env var, or a port, update its doc
(`docs/platforms.md`, `docs/infrastructure.md`, `docs/roadmap.md`) in the same turn.

### 7. Commit, don't push — then a review pass
- One coherent piece → one **path-scoped** commit, fix + coverage + doc together:
  `git commit -m "…" -- <paths>` (bare/whole-tree commits and `git add -A/.` are blocked
  by `git-scope-guard.py` — shared checkout).
- No AI/co-author trailer.
- Validate cheaply before committing (YAML: `python3 -c "import yaml;
  yaml.safe_load(open('<wf>'))"`; the relevant `cargo` check for code).
- **Never `git push`** — the operator publishes.
- For a determinism/netcode fix, run `code-reviewer` on the diff before handing back.

## Output
End with: failing job + platform(s) + step + root cause (1–2 sentences); the fix and *why
it's not a band-aid*; the coverage added; the local verification evidence (exact command +
result); residual risk (e.g. "correct only because CI pins Rust X.Y"). Keep it tight.
