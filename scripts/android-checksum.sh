#!/usr/bin/env bash
# Going Dark — on-device arm64 determinism harness (Phase 1 exit criterion, invariant #7).
#
# The definitive proof that the deterministic fixed-point sim is bit-identical on the real
# arm64 phone: build the headless `gonedark-sim-runner` for aarch64-linux-android, run it
# ON the device, and diff its per-tick checksum stream against the SAME runner on the host.
# Identical streams -> on-device determinism holds. Any divergence is a desync — a real
# bug (invariant #7), printed and surfaced as a non-zero exit, NEVER silenced.
#
#   Usage:  scripts/android-checksum.sh [ticks]   (default 300)
#
# Env knobs:  ANDROID_HOME / ANDROID_SDK_ROOT (SDK path), ANDROID_NDK_HOME (else newest
#   ndk/<ver> under the SDK is auto-selected), ADB (adb path).
#
# Requires: cargo-ndk, an Android SDK + NDK, adb, and a USB-debugging arm64 device.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
SDK="${ANDROID_HOME:-${ANDROID_SDK_ROOT:-$HOME/Android/Sdk}}"
ADB="${ADB:-$SDK/platform-tools/adb}"
ABI="arm64-v8a"
TICKS="${1:-300}"
DEVICE_BIN="/data/local/tmp/gonedark-sim-runner"
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

need_device() {
	if ! "$ADB" get-state >/dev/null 2>&1; then
		echo "!! no device. Plug in the phone, enable USB debugging, accept the RSA prompt." >&2
		echo "   check with:  $ADB devices" >&2
		exit 1
	fi
}

need_cargo_ndk
resolve_ndk
need_device

# Scratch dir for the two checksum streams (kept out of the repo).
SCRATCH="${TMPDIR:-/tmp}"
WORK="$(mktemp -d "${SCRATCH%/}/gonedark-checksum.XXXXXX")"
HOST_STREAM="$WORK/host.txt"
DEVICE_STREAM="$WORK/device.txt"
cleanup() {
	rm -rf "$WORK"
	# Best-effort device cleanup (don't fail the harness if the device vanished).
	"$ADB" shell rm -f "$DEVICE_BIN" >/dev/null 2>&1 || true
}
trap cleanup EXIT

# 1. Build the headless sim-runner for arm64 (core-only; no GPU/platform deps).
echo ">> cargo ndk build sim-runner ($ABI, release)"
(cd "$ROOT" && cargo ndk -t "$ABI" build -p gonedark-sim-runner --release)
[[ -f "$HOST_BIN" ]] || {
	echo "!! expected binary not found: $HOST_BIN" >&2
	exit 1
}

# 2. Push it to the device and run it there for N ticks (capture STDOUT — the checksum
#    stream — only; the final summary goes to stderr).
echo ">> adb push -> $DEVICE_BIN"
"$ADB" push "$HOST_BIN" "$DEVICE_BIN" >/dev/null
"$ADB" shell chmod 755 "$DEVICE_BIN"
echo ">> running on-device for $TICKS ticks"
"$ADB" shell "$DEVICE_BIN" "$TICKS" >"$DEVICE_STREAM" 2>/dev/null

# 3. Run the SAME sim-runner on the host for the same N ticks.
echo ">> running on-host for $TICKS ticks"
(cd "$ROOT" && cargo run -q -p gonedark-sim-runner --release -- "$TICKS") >"$HOST_STREAM"

# 4. Diff the two checksum streams. Identical -> determinism holds. Divergence -> real bug.
echo ">> diffing on-device vs on-host checksum streams ($TICKS ticks)"
if diff -u "$HOST_STREAM" "$DEVICE_STREAM"; then
	echo "OK: on-device arm64 determinism holds — sim bit-identical to desktop over $TICKS ticks."
else
	echo "!! DESYNC: on-device arm64 sim diverged from desktop (see diff above)." >&2
	echo "   This is a real determinism bug (invariant #7) — never silence it." >&2
	exit 1
fi
