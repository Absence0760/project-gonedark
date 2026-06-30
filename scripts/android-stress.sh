#!/usr/bin/env bash
# Going Dark — on-device arm64 sim-cost profiling harness (Phase 3 on-device perf validation).
#
# Runs the headless 200-unit `stress` scene ON the real arm64 phone with the runner's `--time`
# profiler, and reports per-tick sim cost (median ms) against the 16.6 ms 60 Hz budget — the
# datum that says whether mid-range silicon can carry the 200-unit power budget (the Phase 1
# caveat: validated on a flagship; mid-range frame-rate/thermal is Phase 3).
#
# It captures BOTH streams from the runner: stdout (the `<tick> <checksum>` stream) and stderr
# (the `timing` / `timing-json` lines). It then:
#   1. diffs the on-device checksum stream against the SAME runner on the host — so this ALSO
#      guards determinism at scale (invariant #7). A divergence is a real desync bug: printed and
#      surfaced as a NON-ZERO exit, never silenced.
#   2. prints the host AND device `timing` lines side by side, plus a budget verdict for each
#      (median ms/tick over/under the 16.6 ms 60 Hz tick budget).
#
#   Usage:  scripts/android-stress.sh [ticks] [device-serial]   (default 300 ticks)
#
# Picking a device (you have several): set GONEDARK_DEVICE=<serial>, or pass the serial as the
# second arg. One device -> auto-selected; several & none chosen -> it stops and lists them.
#
# Env knobs:  ANDROID_HOME / ANDROID_SDK_ROOT (SDK path), ANDROID_NDK_HOME (else newest
#   ndk/<ver> under the SDK is auto-selected), GONEDARK_DEVICE (adb serial), ADB (adb path),
#   GONEDARK_STRESS_UNITS (total units in the stress scene, default 200; min 2).
#
# Requires: cargo-ndk, an Android SDK + NDK, adb, and a USB-debugging arm64 device.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
SDK="${ANDROID_HOME:-${ANDROID_SDK_ROOT:-$HOME/Android/Sdk}}"
ADB="${ADB:-$SDK/platform-tools/adb}"
ABI="arm64-v8a"
TICKS="${1:-300}"
UNITS="${GONEDARK_STRESS_UNITS:-200}"
SCENE="stress:$UNITS"
# 60 Hz tick budget: 1000 ms / 60 = 16.666… ms per tick.
BUDGET_MS="16.6"
# Target device serial: GONEDARK_DEVICE, else the optional 2nd CLI arg, else auto/none.
DEVICE="${GONEDARK_DEVICE:-${2:-}}"
DEVICE_BIN="/data/local/tmp/gonedark-sim-runner"
DEVICE_ERR="/data/local/tmp/gonedark-sim-runner.stderr"
HOST_BIN="$ROOT/target/aarch64-linux-android/release/gonedark-sim-runner"

# Resolve ANDROID_NDK_HOME (cargo-ndk needs it). Prefer an already-set value, then
# ANDROID_NDK_ROOT, then the newest ndk/<ver> in the SDK. (Mirrors scripts/android.sh.)
resolve_ndk() {
	[[ -n "${ANDROID_NDK_HOME:-}" ]] && return
	if [[ -n "${ANDROID_NDK_ROOT:-}" ]]; then
		export ANDROID_NDK_HOME="$ANDROID_NDK_ROOT"
		return
	fi
	local newest
	newest="$(ls -d "$SDK"/ndk/* 2>/dev/null | sort -V | tail -1 || true)"
	[[ -n "$newest" ]] && export ANDROID_NDK_HOME="$newest"
	if [[ -z "${ANDROID_NDK_HOME:-}" ]]; then
		echo "!! No Android NDK found. Install one (sdkmanager 'ndk;<ver>') or export" >&2
		echo "   ANDROID_NDK_HOME. Looked under: $SDK/ndk/" >&2
		exit 1
	fi
	echo ">> ANDROID_NDK_HOME=$ANDROID_NDK_HOME"
}

need_cargo_ndk() {
	command -v cargo-ndk >/dev/null 2>&1 && return
	echo "!! cargo-ndk not installed. Run:  pnpm android:setup   (or: cargo install cargo-ndk)" >&2
	exit 1
}

# Resolve the target device into DEVICE, or fail with a list (same rule as scripts/android.sh):
# GONEDARK_DEVICE / 2nd arg wins; else the sole connected device; else stop and list them.
resolve_device() {
	local serials count
	serials="$("$ADB" devices | awk 'NR>1 && $2=="device" {print $1}')"
	count="$(printf '%s\n' "$serials" | grep -c . || true)"
	if [[ -n "$DEVICE" ]]; then
		if ! printf '%s\n' "$serials" | grep -qx "$DEVICE"; then
			echo "!! GONEDARK_DEVICE='$DEVICE' is not a connected device. Connected:" >&2
			"$ADB" devices -l >&2
			exit 1
		fi
		return
	fi
	if [[ "$count" -eq 0 ]]; then
		echo "!! no device. Plug in the phone, enable USB debugging, accept the RSA prompt." >&2
		echo "   check with:  $ADB devices" >&2
		exit 1
	elif [[ "$count" -gt 1 ]]; then
		echo "!! $count devices connected — pick one with GONEDARK_DEVICE=<serial>:" >&2
		"$ADB" devices -l >&2
		exit 1
	fi
	DEVICE="$(printf '%s\n' "$serials" | head -1)"
	echo ">> device: $DEVICE"
}

# adb, scoped to the resolved target device.
adb_dev() { "$ADB" -s "$DEVICE" "$@"; }

# Pull the median_ms out of a runner stderr file's `timing-json` line (no jq needed — the JSON is
# one stable line). Echoes the numeric median, or nothing if the line is absent.
median_ms() {
	grep -oE '"median_ms":[0-9]+\.[0-9]+' "$1" 2>/dev/null | head -1 | sed 's/.*://'
}

# Print a budget verdict for a median ms value against the 16.6 ms 60 Hz tick budget.
budget_verdict() {
	local label="$1" median="$2"
	if [[ -z "$median" ]]; then
		echo "   $label: (no timing-json line parsed — see the raw timing line above)"
		return
	fi
	awk -v m="$median" -v l="$label" -v b="$BUDGET_MS" 'BEGIN {
		if (m + 0 <= b + 0)
			printf "   %s: %.3f ms/tick median — UNDER the %.1f ms 60 Hz budget (%.3f ms headroom)\n", l, m, b, b - m
		else
			printf "   %s: %.3f ms/tick median — OVER the %.1f ms 60 Hz budget by %.3f ms\n", l, m, b, m - b
	}'
}

need_cargo_ndk
resolve_ndk
resolve_device

# Scratch dir for the two checksum streams + the two timing reports (kept out of the repo).
SCRATCH="${TMPDIR:-/tmp}"
WORK="$(mktemp -d "${SCRATCH%/}/gonedark-stress.XXXXXX")"
HOST_STREAM="$WORK/host.txt"
HOST_TIMING="$WORK/host.timing"
DEVICE_STREAM="$WORK/device.txt"
DEVICE_TIMING="$WORK/device.timing"
cleanup() {
	rm -rf "$WORK"
	# Best-effort device cleanup (don't fail the harness if the device vanished).
	adb_dev shell rm -f "$DEVICE_BIN" "$DEVICE_ERR" >/dev/null 2>&1 || true
}
trap cleanup EXIT

# 1. Build the headless sim-runner for arm64 (core-only; no GPU/platform deps).
echo ">> cargo ndk build sim-runner ($ABI, release)"
(cd "$ROOT" && cargo ndk -t "$ABI" build -p gonedark-sim-runner --release)
[[ -f "$HOST_BIN" ]] || {
	echo "!! expected binary not found: $HOST_BIN" >&2
	exit 1
}

# 2. Push it and run the timed stress scene on the device. The runner writes the checksum stream
#    to stdout and the `timing`/`timing-json` lines to stderr — keep them apart regardless of the
#    adb shell protocol by redirecting the runner's stderr to a file ON the device (then pull it),
#    so the captured stdout is a clean checksum stream for the diff.
echo ">> adb -s $DEVICE push -> $DEVICE_BIN"
adb_dev push "$HOST_BIN" "$DEVICE_BIN" >/dev/null
adb_dev shell chmod 755 "$DEVICE_BIN"
echo ">> running on-device ($DEVICE): $SCENE for $TICKS ticks (--time)"
adb_dev shell "$DEVICE_BIN '$TICKS' '$SCENE' --time 2>'$DEVICE_ERR'" >"$DEVICE_STREAM"
adb_dev pull "$DEVICE_ERR" "$DEVICE_TIMING" >/dev/null 2>&1 || true

# 3. Run the SAME sim-runner + scene on the host for the same N ticks.
echo ">> running on-host: $SCENE for $TICKS ticks (--time)"
(cd "$ROOT" && cargo run -q -p gonedark-sim-runner --release -- "$TICKS" "$SCENE" --time) \
	>"$HOST_STREAM" 2>"$HOST_TIMING"

# 4. Diff the two checksum streams. Identical -> determinism holds at scale. Divergence -> bug.
echo ">> diffing on-device vs on-host checksum streams ($SCENE, $TICKS ticks)"
if diff -u "$HOST_STREAM" "$DEVICE_STREAM"; then
	echo "OK: on-device arm64 sim bit-identical to desktop over $TICKS ticks ($SCENE)."
else
	echo "!! DESYNC: on-device arm64 sim diverged from desktop (see diff above)." >&2
	echo "   This is a real determinism bug (invariant #7) — never silence it." >&2
	exit 1
fi

# 5. Report per-tick sim cost: the raw timing lines + a budget verdict for each.
echo
echo "== per-tick sim cost ($SCENE, $TICKS ticks) =="
echo "-- host  --"
grep -E '^timing ' "$HOST_TIMING" || echo "   (no host timing line captured)"
echo "-- device --"
grep -E '^timing ' "$DEVICE_TIMING" || echo "   (no device timing line captured — see stderr below)"
echo
echo "== 60 Hz budget ($BUDGET_MS ms/tick) =="
budget_verdict "host  " "$(median_ms "$HOST_TIMING")"
budget_verdict "device" "$(median_ms "$DEVICE_TIMING")"
