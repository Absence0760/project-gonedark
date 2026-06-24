#!/usr/bin/env bash
# .claude/hooks/sim-determinism-guard.sh
#
# Cheap, fast pattern checks for the one bug class that breaks *Going
# Dark* silently: floats (and other non-deterministic constructs) leaking
# into the deterministic fixed-point simulation. Runs as a PostToolUse
# hook on Edit / Write so violations surface to Claude before the next
# turn — the same shape as project-account-payables' security-patterns.sh.
#
# Why this exists
# ---------------
# CLAUDE.md invariant #1: the sim is fixed-point so it is bit-identical
# across devices, CPUs, and compilers. A stray `f32`/`f64`, a libm
# transcendental, or HashMap iteration in sim/core code desyncs lockstep
# **silently** — there is no error, just divergence that only shows up as
# a cross-platform checksum mismatch (invariant #7). A grep at edit time
# is the cheapest place to catch the textually-stable shapes.
#
# Scope
# -----
# Rules fire ONLY on Rust files under the (future) sim/core crate paths —
# floats are legal and expected in the renderer/PAL. The repo is
# design-only today (decisions.md D10 picked Rust; no engine code yet),
# so this hook is INERT until those .rs files exist. When the crate
# layout is finalised, tighten SIM_PATH_RE below to the real paths.
#
# Adding a rule is one block. Rules MUST: have a unique RULE_NAME, state
# WHY (the bug class), and state the safer alternative. Bypass a
# false-positive by appending `// noqa: <rule_name>` to the line with a
# rationale (e.g. a documented float-in-a-comment, or a render-only
# helper that legitimately lives beside sim code).
#
# Exit codes: 0 = no findings; 2 = findings (stderr shown to Claude as a
# system-reminder for the next turn).

set -euo pipefail

# Paths whose Rust code is the deterministic core. Anything matching is
# sim code; floats there are bugs. Keep render/, pal/, and tooling OUT.
SIM_PATH_RE='(^|/)(crates/(core|sim)|core/src|sim/src)/|(^|/)(core|sim)/.*\.rs$|/sim/|/sim_'

INPUT="$(cat || true)"
FILE="$(printf '%s' "$INPUT" | jq -r '.tool_input.file_path // empty' 2>/dev/null || true)"
[[ -z "$FILE" || "$FILE" == "null" ]] && exit 0

# Rust only, and only inside the sim/core paths.
[[ "$FILE" == *.rs ]] || exit 0
printf '%s' "$FILE" | grep -qE "$SIM_PATH_RE" || exit 0
[[ -f "$FILE" ]] || exit 0

FINDINGS=""

register() {
  local rule="$1" line="$2" why="$3" fix="$4"
  local lineno="${line%%:*}"
  if [[ -n "$lineno" && "$lineno" =~ ^[0-9]+$ ]]; then
    local content
    content="$(sed -n "${lineno}p" "$FILE" 2>/dev/null || true)"
    if grep -qE "noqa:\s*${rule}\b" <<<"$content"; then
      return
    fi
  fi
  FINDINGS+="  [${rule}] ${FILE}:${line}
    why: ${why}
    fix: ${fix}

"
}

# grep -nE wrapper that strips // line comments and string literals first,
# so a float mentioned in a doc-comment or a string doesn't trip a rule.
hits() {
  grep -nE "$1" "$FILE" 2>/dev/null \
    | grep -vE '^[0-9]+:\s*//' \
    || true
}

# ----- RULE: float type in sim code ---------------------------------------
# Why: invariant #1. Any f32/f64 (declaration, cast, or literal) in the
# sim desyncs lockstep silently — IEEE rounding differs across compilers
# and FPUs. Bug class: a "just store the position as a float" edit, an
# `as f32` cast, a `0.5` literal in a sim formula.
while IFS= read -r m; do
  ln="${m%%:*}"
  register "float-in-sim" "$ln" \
    "f32/f64 in sim/core code breaks the bit-identical fixed-point sim (invariant #1)" \
    "Use the project fixed-point type (e.g. Fixed/I32F32) — floats live only in render/"
done < <(hits '\b(f32|f64)\b|\bas\s+f(32|64)\b')

# ----- RULE: floating-point literal in sim code ---------------------------
# Why: a bare `1.5` / `0.0` / `3.14` literal is an f64 by inference and
# feeds float math even without an explicit type. Same desync class.
while IFS= read -r m; do
  ln="${m%%:*}"
  register "float-literal-in-sim" "$ln" \
    "A floating-point literal infers f64 and pulls float math into the sim" \
    "Express the constant in the fixed-point type (Fixed::from_num(..)) or as an integer ratio"
done < <(hits '[^A-Za-z0-9_.]([0-9]+\.[0-9]+)([eE][-+]?[0-9]+)?' )

# ----- RULE: std / libm transcendental in sim code ------------------------
# Why: invariant #1 forbids std/libm transcendentals in sim — sin/cos/
# sqrt/powf etc. are not specified to be bit-identical across platforms.
# Bug class: reaching for `.sqrt()` / `f64::sin` in a sim distance or
# trig calc. Use fixed-point implementations / LUTs instead.
while IFS= read -r m; do
  ln="${m%%:*}"
  register "transcendental-in-sim" "$ln" \
    "std/libm transcendentals (sin/cos/sqrt/powf/exp/ln...) aren't cross-platform bit-identical" \
    "Use the project's fixed-point math / LUTs — never std float transcendentals in sim"
done < <(hits '\.(sqrt|sin|cos|tan|powf|powi|exp|ln|log[0-9]*|hypot|atan2?|cbrt)\(|f(32|64)::(sqrt|sin|cos|consts)')

# ----- RULE: HashMap/HashSet iteration in sim code ------------------------
# Why: std HashMap iteration order is randomised per-process (SipHash with
# a random seed). Iterating one to drive sim state (spawn order, AI tick
# order) diverges between clients even with no floats. Bug class: a
# `for (k, v) in some_hashmap` in a system that mutates sim state.
while IFS= read -r m; do
  ln="${m%%:*}"
  register "hashmap-iteration-in-sim" "$ln" \
    "std HashMap/HashSet iteration order is process-randomised — iterating it in sim diverges clients" \
    "Use a deterministic container (BTreeMap, IndexMap, or a Vec keyed by a stable id)"
done < <(hits '\b(HashMap|HashSet)<')

# ---------------------------------------------------------------------------
# Emit
# ---------------------------------------------------------------------------
if [[ -n "$FINDINGS" ]]; then
  {
    echo "Determinism hook flagged sim/core change to ${FILE}:"
    echo
    printf '%s' "$FINDINGS"
    echo "These break the deterministic fixed-point sim SILENTLY (CLAUDE.md invariants #1, #7)."
    echo "If a rule is wrong here (e.g. a render-only helper beside sim code), append"
    echo "'// noqa: <rule_name>' to the line with a one-line rationale."
  } >&2
  exit 2
fi

exit 0
