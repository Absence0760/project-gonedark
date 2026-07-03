#!/usr/bin/env python3
"""Tests for viz-green-guard.py.

Runs the hook as a subprocess for the paths that exit before viz (non-commit and
non-render commits), and white-box tests the render-triggered paths by loading the
module and stubbing the two seams (`_staged_paths`, `_run_viz`) so no real GPU — or
even a real `cargo run` — is needed. Run: `python3 .claude/hooks/viz-green-guard.test.py`.

Mirrors git-scope-guard.test.py's style. Not wired into CI (viz is a LOCAL GPU tool);
this pins the guard's trigger + fail-open logic so an edit can't silently reopen the
hole that let RED render commits land on main.
"""

import importlib.util
import json
import subprocess
import sys
from pathlib import Path

HOOK = str(Path(__file__).with_name("viz-green-guard.py"))

failures = []


def _record(label, expected, got):
    ok = expected == got
    if not ok:
        failures.append((label, expected, got))
    print(f"  [{'ok' if ok else 'FAIL'}] expect {str(expected):5} got {str(got):5}  {label}")


# --- subprocess cases: only the paths that exit BEFORE running viz --------------------
# (a render-triggered commit would actually shell out to `cargo run`, so those are
#  white-boxed below with stubbed seams instead.)

def subprocess_decision(command):
    """Return 'deny' if the hook blocks, else 'allow' — from the emitted JSON."""
    out = subprocess.run(
        [sys.executable, HOOK],
        input=json.dumps({"tool_name": "Bash", "tool_input": {"command": command}}),
        capture_output=True, text=True,
    ).stdout.strip()
    if not out:
        return "allow"
    try:
        return json.loads(out)["hookSpecificOutput"]["permissionDecision"]
    except (ValueError, KeyError):
        return "allow"


print("subprocess (pre-viz exits):")
# A non-commit git command never triggers, even with the word in a message.
_record("git status", "allow", subprocess_decision("git status"))
_record("git log -5", "allow", subprocess_decision("git log --oneline -5"))
# A totally unrelated command.
_record("echo hi", "allow", subprocess_decision('echo "commit this"'))

# A non-Bash tool → allow (exit 0), regardless of payload contents.
_out = subprocess.run(
    [sys.executable, HOOK],
    input=json.dumps({"tool_name": "Edit", "tool_input": {"command": "git commit -m x"}}),
    capture_output=True, text=True,
).stdout.strip()
_record("non-Bash tool (Edit)", "allow", "deny" if _out else "allow")


# --- white-box: load the module and stub the two seams --------------------------------
spec = importlib.util.spec_from_file_location("vgg", HOOK)
vgg = importlib.util.module_from_spec(spec)
spec.loader.exec_module(vgg)

COMMIT = {"tool_name": "Bash",
          "tool_input": {"command": 'git commit -m "feat(render): x" -- render/src/hud.rs'}}


def _raise(*_a, **_k):
    raise AssertionError("viz must not run for this case")


VIZ_FAIL_OUTPUT = (
    "[command] top-down view should draw player + enemy units on the lit field\n"
    "  PASS  command_not_dark: dark fraction 0.100\n"
    "  FAIL  command_has_player_units: 3 player-blue px (>50)\n"
    "  FAIL  command_draws_kind_glyphs: 12 flat unit-kind-glyph px (>150)\n"
    "\nRESULT: 2 visual assertion(s) FAILED ✗"
)
VIZ_PASS_OUTPUT = "...\nRESULT: all visual assertions passed ✓"
VIZ_SKIP_OUTPUT = ("SKIP: no wgpu adapter available (headless/CI without a GPU) — "
                   "nothing rendered.")
VIZ_BUILD_ERROR = ("error[E0308]: mismatched types\n  --> render/src/hud.rs:42:9\n"
                   "error: could not compile `gonedark-render`")


def decide(payload, staged, viz):
    """Run vgg.run with stubbed seams; return the ('kind', message) decision."""
    vgg._staged_paths = lambda _repo, s=staged: list(s)
    vgg._run_viz = viz
    return vgg.run(payload)


print("\nwhite-box (stubbed seams):")

# 1. Commit staging NO render path → allow, and viz is NOT run.
d, _ = decide(COMMIT, ["core/src/sim.rs", "app/src/main.rs"], _raise)
_record("commit, no render path → allow (viz not run)", "allow", d)

# 2. Commit staging a render/ path, viz FAILS an assertion → deny (names the assertion).
d, msg = decide(COMMIT, ["render/src/hud.rs"], lambda _r: (1, VIZ_FAIL_OUTPUT))
_record("render path + viz FAIL → deny", "deny", d)
_record("  deny msg names failed assertion", True,
        "command_has_player_units" in (msg or ""))
_record("  deny msg points at pnpm desktop:viz", True, "pnpm desktop:viz" in (msg or ""))

# 2b. Same for a viz-runner/ path.
d, _ = decide(COMMIT, ["viz-runner/src/main.rs"], lambda _r: (1, VIZ_FAIL_OUTPUT))
_record("viz-runner path + viz FAIL → deny", "deny", d)

# 3. Render path, but NO GPU (SKIP) → allow + warn, does not block.
d, msg = decide(COMMIT, ["render/src/lib.rs"], lambda _r: (0, VIZ_SKIP_OUTPUT))
_record("render path + no GPU → warn", "warn", d)
_record("  warn mentions headless/GPU", True, "GPU" in (msg or ""))

# 4. Render path, viz PASSES → allow (no message).
d, _ = decide(COMMIT, ["render/src/lib.rs"], lambda _r: (0, VIZ_PASS_OUTPUT))
_record("render path + viz PASS → allow", "allow", d)

# 5. Render path, viz exits non-zero with NO assertion marker (build error) → OPEN/warn.
d, _ = decide(COMMIT, ["render/src/hud.rs"], lambda _r: (101, VIZ_BUILD_ERROR))
_record("render path + build error → warn (fail-open)", "warn", d)

# 6. Render path, viz could not launch / timed out (rc None) → OPEN/warn, never block.
d, _ = decide(COMMIT, ["render/src/hud.rs"], lambda _r: (None, "viz-runner timed out"))
_record("render path + timeout → warn (fail-open)", "warn", d)

# 7. Internal error (a seam raises unexpectedly) → fail OPEN (allow), never deny.
vgg._staged_paths = _raise  # raises AssertionError inside run()
vgg._run_viz = lambda _r: (1, VIZ_FAIL_OUTPUT)
d, _ = vgg.run(COMMIT), None
_record("internal error → fail OPEN (allow)", "allow", d[0])

# 8. Non-commit command → allow without touching any seam.
vgg._staged_paths = _raise
d, _ = vgg.run({"tool_name": "Bash", "tool_input": {"command": "git status"}})
_record("non-commit (white-box) → allow, no seam call", "allow", d)


if failures:
    print(f"\n{len(failures)} FAILED:")
    for label, expected, got in failures:
        print(f"  {label!r}: expected {expected}, got {got}")
    sys.exit(1)
print("\nAll viz-green-guard cases passed.")
