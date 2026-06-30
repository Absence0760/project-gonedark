#!/usr/bin/env bash
# Going Dark — non-interactive on-device FPS + thermal capture (Phase 3 on-device perf validation).
#
# Captures the live in-app heartbeat the running engine logs once per second
# (`heartbeat: <fps> fps | frame <n> | tick <t> | checksum <c>`) plus the startup thermal line
# (`thermal: initial state <S> | power <P>`), for a bounded window, and SUMMARISES sustained FPS
# (min/median/max), the tick-count progression, and the observed thermal state(s). It flags
# whether sustained FPS held at/near the 60 fps target, and any thermal escalation off Nominal —
# the datum that would reopen the D21 dual-rate question on mid-range silicon.
#
# Unlike `pnpm android:logcat` (which tails interactively until Ctrl-C), this is a one-shot
# capture+summary: it clears logcat, records for DURATION seconds, then prints the digest and exits.
#
#   Usage:  scripts/android-fps.sh [seconds] [device-serial]   (default 30 s)
#
# The app must already be RUNNING in the engine view (the heartbeat only fires while the engine
# loop presents frames). Start it first with `pnpm android:dev` and tap "Start" in the launcher.
# (`am start` only opens the launcher shell; the non-exported engine NativeActivity can't be
# started from adb — see scripts/android.sh / the D35 manifest split.) With zero heartbeats
# captured this prints a clear "app not running" error.
#
# Picking a device (you have several): set GONEDARK_DEVICE=<serial>, or pass the serial as the
# second arg. One device -> auto-selected; several & none chosen -> it stops and lists them.
#
# Env knobs:  ANDROID_HOME / ANDROID_SDK_ROOT (SDK path), GONEDARK_DEVICE (adb serial),
#   ADB (adb path), GONEDARK_FPS_TARGET (target fps, default 60).
#
# Requires: adb and a USB-debugging arm64 device with the app already running.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
SDK="${ANDROID_HOME:-${ANDROID_SDK_ROOT:-$HOME/Android/Sdk}}"
ADB="${ADB:-$SDK/platform-tools/adb}"
DURATION="${1:-30}"
TARGET_FPS="${GONEDARK_FPS_TARGET:-60}"
# Target device serial: GONEDARK_DEVICE, else the optional 2nd CLI arg, else auto/none.
DEVICE="${GONEDARK_DEVICE:-${2:-}}"

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

resolve_device

SCRATCH="${TMPDIR:-/tmp}"
WORK="$(mktemp -d "${SCRATCH%/}/gonedark-fps.XXXXXX")"
LOGFILE="$WORK/logcat.txt"
cleanup() { rm -rf "$WORK"; }
trap cleanup EXIT

# Clear the device log buffer so we only summarise THIS window, then capture the app's tag for
# DURATION seconds. `timeout` exits 124 when the window elapses (the normal path), so tolerate it;
# fall back to a backgrounded adb we kill if `timeout` is unavailable.
echo ">> clearing logcat + capturing gonedark logs for ${DURATION}s on $DEVICE"
adb_dev logcat -c
if command -v timeout >/dev/null 2>&1; then
	timeout "$DURATION" "$ADB" -s "$DEVICE" logcat -v time -s gonedark:V >"$LOGFILE" 2>/dev/null || true
else
	"$ADB" -s "$DEVICE" logcat -v time -s gonedark:V >"$LOGFILE" 2>/dev/null &
	LOGPID=$!
	sleep "$DURATION"
	kill "$LOGPID" >/dev/null 2>&1 || true
	wait "$LOGPID" 2>/dev/null || true
fi

# Heartbeat FPS samples (one per second the engine presented frames).
FPS_VALUES="$(grep -F 'heartbeat:' "$LOGFILE" | grep -oE '[0-9]+\.[0-9]+ fps' | awk '{print $1}')"
HEARTBEATS="$(printf '%s\n' "$FPS_VALUES" | grep -c . || true)"

if [[ "$HEARTBEATS" -eq 0 ]]; then
	echo "!! no heartbeats captured in ${DURATION}s — the app isn't presenting frames." >&2
	echo "   Start the engine first:  pnpm android:dev   (then tap \"Start\" in the launcher)," >&2
	echo "   leave it in the engine view, and re-run:  pnpm android:fps" >&2
	exit 1
fi

# FPS distribution: count min median max mean, computed over the sorted samples.
read -r FPS_N FPS_MIN FPS_MED FPS_MAX FPS_MEAN < <(
	printf '%s\n' "$FPS_VALUES" | sort -n | awk '
		{ a[NR] = $1; sum += $1 }
		END {
			n = NR
			med = (n % 2) ? a[(n + 1) / 2] : (a[n / 2] + a[n / 2 + 1]) / 2
			printf "%d %.1f %.1f %.1f %.1f", n, a[1], med, a[n], sum / n
		}'
)

# Tick progression (first/last) from the heartbeat lines — confirms the sim advanced steadily.
TICK_FIRST="$(grep -F 'heartbeat:' "$LOGFILE" | grep -oE 'tick [0-9]+' | head -1 | awk '{print $2}')"
TICK_LAST="$(grep -F 'heartbeat:' "$LOGFILE" | grep -oE 'tick [0-9]+' | tail -1 | awk '{print $2}')"

# Thermal: the states observed (logged at startup as `thermal: initial state <S> ...`). The
# heartbeat doesn't re-log thermal, so escalation detection is limited to the startup reading(s)
# present in this window — but ANY non-Nominal state is flagged.
THERMAL_STATES="$(grep -F 'thermal:' "$LOGFILE" | grep -oE 'state [A-Za-z]+' | awk '{print $2}' | sort -u | paste -sd, -)"

echo
echo "== FPS over ${DURATION}s ($FPS_N heartbeats, target ${TARGET_FPS} fps) =="
echo "   min ${FPS_MIN} | median ${FPS_MED} | max ${FPS_MAX} | mean ${FPS_MEAN} fps"
if [[ -n "$TICK_FIRST" && -n "$TICK_LAST" ]]; then
	echo "   sim tick advanced ${TICK_FIRST} -> ${TICK_LAST} (Δ $((TICK_LAST - TICK_FIRST)) ticks)"
fi

# Verdict: median vs target, then whether the worst sample also held (sustained).
awk -v med="$FPS_MED" -v mn="$FPS_MIN" -v t="$TARGET_FPS" 'BEGIN {
	if (med + 0 >= t + 0) {
		if (mn + 0 >= t + 0)
			printf "   VERDICT: sustained at/above %d fps (even the worst sample held).\n", t
		else
			printf "   VERDICT: median holds >= %d fps, but dipped to %.1f fps — investigate the low samples.\n", t, mn
	} else {
		printf "   VERDICT: median %.1f fps is BELOW the %d fps target — sustained FPS did not hold.\n", med, t
	}
}'

echo
echo "== thermal =="
if [[ -z "$THERMAL_STATES" ]]; then
	echo "   (no thermal line in this window — it logs at engine startup; restart the engine"
	echo "    inside the capture window to record it)"
elif [[ "$THERMAL_STATES" == "Nominal" ]]; then
	echo "   state(s): Nominal — no thermal escalation observed."
else
	echo "   state(s): $THERMAL_STATES" >&2
	echo "   !! thermal escalation off Nominal observed — the datum that reopens D21 (dual-rate)." >&2
fi
