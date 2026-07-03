#!/usr/bin/env python3
"""PreToolUse Bash guard that keeps the `viz-runner` visual baseline green.

CLAUDE.md says every visual workstream must land with `viz-runner` green — yet
recent render commits (D52 silhouettes, WS-D footprints, palette threading) shipped
while leaving the pixel-level visual assertions RED on `main`, because nothing
enforced it locally (viz is a GPU-gated LOCAL tool, deliberately *not* in the no-GPU
CI matrix). This hook closes that gap: when a commit touches render-affecting code it
runs the viz-runner and BLOCKS the commit only on a confirmed visual-assertion
failure.

Mirrors `git-scope-guard.py`'s I/O contract exactly:
  - reads the PreToolUse payload on stdin (JSON);
  - a non-`Bash` tool, or a command that isn't a `git commit`, → allow (exit 0, silent);
  - a block is a `permissionDecision: "deny"` JSON object on stdout + exit 0.

Trigger (scoped, so it does NOT fire on every engine/app commit):
  a `git commit` whose STAGED paths (`git diff --cached --name-only`) include something
  under `render/` or `viz-runner/`. No staged render path → allow, viz not run.

Decision, once triggered — run `cargo run -p gonedark-viz-runner` and classify:
  - exit 0                              → PASS  → allow.
  - `SKIP: no wgpu adapter` in output   → SKIP  → allow + one-line stderr warning
                                                   (headless host can't verify).
  - exit != 0 WITH a visual-assertion marker (`  FAIL  …` / `RESULT: … FAILED`)
                                        → FAIL  → DENY, naming the failed assertions
                                                   and pointing at `pnpm desktop:viz`.
  - exit != 0 with NO such marker (build error, etc.), or the run couldn't complete
    (timeout / cargo missing)           → OPEN  → allow + warning (unattributable —
                                                   never block on a non-viz failure).

**Fail OPEN on everything unexpected.** A bug in this guard must never wedge a commit;
only a confirmed viz-assertion failure blocks. Every path is wrapped so an exception,
a missing tool, or a timeout resolves to "allow".
"""

import json
import os
import shlex
import subprocess
import sys

# Staged paths under these prefixes make a commit "render-affecting" and arm the gate.
# Kept tight (just the two dirs that own the rendered output) so ordinary engine/app/
# core/docs commits never pay the viz cost.
RENDER_PREFIXES = ("render/", "viz-runner/")

# Give a cold `cargo run` room to recompile the render/engine crates before we render.
# Sits just under the hook's own timeout in settings.json (180s); a slower box hits the
# subprocess timeout first and we fail OPEN (a warning, not a block).
VIZ_TIMEOUT_SECS = 170


def _deny(reason):
    """Block the command — same JSON contract as git-scope-guard.py (deny + exit 0)."""
    print(json.dumps({
        "hookSpecificOutput": {
            "hookEventName": "PreToolUse",
            "permissionDecision": "deny",
            "permissionDecisionReason": reason,
        }
    }))
    sys.exit(0)


def _repo_root():
    """Repo root = three levels up from .claude/hooks/viz-green-guard.py."""
    return os.path.dirname(os.path.dirname(os.path.dirname(os.path.abspath(__file__))))


def _segments(command):
    """Split a shell command into segments on operators, respecting quotes.

    Verbatim in spirit from git-scope-guard.py so `A && git commit …` still sees the
    commit. Returns [] on an unparseable command (fail-open: we won't guess)."""
    normalised = command.replace("\n", " ; ")
    lex = shlex.shlex(normalised, posix=True, punctuation_chars=";&|<>()")
    lex.whitespace_split = True
    try:
        tokens = list(lex)
    except ValueError:
        return []
    segments, current = [], []
    for tok in tokens:
        if tok and all(c in ";&|<>()" for c in tok):
            if current:
                segments.append(current)
                current = []
        else:
            current.append(tok)
    if current:
        segments.append(current)
    return segments


def _git_subcommand(tokens):
    """Return (subcommand, args) for a git invocation in `tokens`, or None."""
    i = 0
    while i < len(tokens) and not (tokens[i] == "git" or tokens[i].endswith("/git")):
        i += 1
    if i >= len(tokens):
        return None
    i += 1
    takes_value = {"-C", "-c", "--git-dir", "--work-tree", "--namespace",
                   "--exec-path", "--super-prefix"}
    while i < len(tokens):
        t = tokens[i]
        if t in takes_value:
            i += 2
        elif t.startswith("-"):
            i += 1
        else:
            break
    if i >= len(tokens):
        return None
    return tokens[i], tokens[i + 1:]


def _is_git_commit(command):
    """True if any segment of `command` is a `git commit …` invocation."""
    if "commit" not in command:
        return False
    for tokens in _segments(command):
        parsed = _git_subcommand(tokens)
        if parsed and parsed[0] == "commit":
            return True
    return False


def _staged_paths(repo):
    """The repo-relative paths in the staged index (`git diff --cached --name-only`)."""
    result = subprocess.run(
        ["git", "-C", repo, "diff", "--cached", "--name-only"],
        capture_output=True, text=True, timeout=10,
    )
    return [line.strip() for line in result.stdout.splitlines() if line.strip()]


def _render_paths(paths):
    """Subset of `paths` that live under a render-affecting prefix."""
    return [p for p in paths if any(p.startswith(pre) for pre in RENDER_PREFIXES)]


def _run_viz(repo):
    """Run the viz-runner. Return (returncode, combined_output).

    A returncode of None means the run could not be completed (timeout / cargo not
    found / launch error) — the caller treats that as fail-OPEN, never a block."""
    try:
        proc = subprocess.run(
            ["cargo", "run", "-p", "gonedark-viz-runner"],
            cwd=repo, capture_output=True, text=True, timeout=VIZ_TIMEOUT_SECS,
        )
        return proc.returncode, (proc.stdout or "") + (proc.stderr or "")
    except subprocess.TimeoutExpired:
        return None, "viz-runner timed out"
    except Exception as exc:  # cargo missing, OSError, etc.
        return None, "viz-runner could not be launched: %s" % exc


def _failed_assertions(output):
    """Names + details of failed assertions, parsed from `  FAIL  <name>: <detail>`."""
    fails = []
    for line in output.splitlines():
        stripped = line.strip()
        if stripped.startswith("FAIL"):
            body = stripped[len("FAIL"):].strip()
            name, _, detail = body.partition(":")
            fails.append((name.strip(), detail.strip()))
    return fails


def _classify_viz(returncode, output):
    """Map a viz run to ('pass'|'skip'|'fail'|'open', payload).

    'fail' payload is the list of (name, detail) failed assertions; other payloads are
    a short human string. Only 'fail' blocks — everything else allows."""
    if "SKIP: no wgpu adapter" in output:
        return ("skip", "no wgpu adapter (headless)")
    if returncode == 0:
        return ("pass", "all visual assertions passed")
    if returncode is None:
        return ("open", output.strip() or "viz-runner did not complete")
    # Non-zero exit. Only treat it as a real visual FAILURE if the run got far enough to
    # emit an assertion marker; otherwise it's a compile/link error we can't attribute
    # to viz → fail OPEN.
    fails = _failed_assertions(output)
    if fails or ("RESULT:" in output and "FAILED" in output):
        return ("fail", fails)
    return ("open", "viz-runner exited %s with no visual-assertion marker "
                    "(likely a build error)" % returncode)


def _deny_reason(fails):
    """Build the block message: which assertions failed + how to reproduce."""
    lines = [
        "viz-runner VISUAL ASSERTIONS FAILED — this commit touches render/ or "
        "viz-runner/ but leaves the visual baseline RED.",
    ]
    if fails:
        lines.append("Failed assertion(s):")
        for name, detail in fails:
            lines.append("  - %s: %s" % (name, detail) if detail else "  - %s" % name)
    lines.append(
        "CLAUDE.md requires each visual workstream to land with viz green. Fix the "
        "render regression (or the assertion, if the baseline legitimately moved), "
        "then re-commit. Reproduce: `pnpm desktop:viz`."
    )
    return "\n".join(lines)


def run(payload):
    """Decide on a payload. Returns ('allow'|'deny'|'warn', message_or_None).

    Never raises — any unexpected error resolves to ('allow', None) (fail OPEN)."""
    try:
        if payload.get("tool_name") != "Bash":
            return ("allow", None)
        command = (payload.get("tool_input") or {}).get("command") or ""
        if not _is_git_commit(command):
            return ("allow", None)

        repo = _repo_root()
        render = _render_paths(_staged_paths(repo))
        if not render:
            # Commit doesn't touch rendering — most commits. Don't run viz.
            return ("allow", None)

        returncode, output = _run_viz(repo)
        kind, payload_detail = _classify_viz(returncode, output)
        if kind == "fail":
            return ("deny", _deny_reason(payload_detail))
        if kind == "skip":
            return ("warn",
                    "viz-green-guard: no GPU adapter on this host — viz-runner could "
                    "not verify the render baseline (headless). Commit allowed; run "
                    "`pnpm desktop:viz` on a GPU host to confirm.")
        if kind == "open":
            return ("warn",
                    "viz-green-guard: viz-runner did not complete cleanly (%s) — no "
                    "visual failure could be attributed, so the commit is allowed. Run "
                    "`pnpm desktop:viz` to check the render baseline." % payload_detail)
        return ("allow", None)  # pass
    except Exception:
        # A bug in the guard must never block a commit.
        return ("allow", None)


def main():
    try:
        payload = json.load(sys.stdin)
    except (ValueError, json.JSONDecodeError):
        sys.exit(0)
    try:
        decision, message = run(payload)
    except Exception:
        sys.exit(0)  # belt-and-braces fail-open
    if decision == "deny":
        _deny(message)  # prints JSON, exits 0
    if decision == "warn" and message:
        print(message, file=sys.stderr)
    sys.exit(0)


if __name__ == "__main__":
    main()
