#!/usr/bin/env bash
# Going Dark — Phase 0 prototype: build APK, install to a connected phone, launch,
# and stream logs. THROWAWAY tooling for the control prototype (roadmap Phase 0).
#
#   Usage:  ./deploy.sh            # export + install + launch + logcat
#           ./deploy.sh build      # export the APK only
#           ./deploy.sh logcat     # just tail the app's logs
#
# Requires: godot (4.6.x) on PATH, adb on PATH, a phone in USB-debugging mode.
set -euo pipefail

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PKG="com.gonedark.phase0proto"
APK="$HERE/out/gonedark-phase0.apk"
ADB="${ADB:-$HOME/Android/Sdk/platform-tools/adb}"

build() {
	mkdir -p "$HERE/out"
	echo ">> exporting debug APK (arm64)…"
	godot --headless --path "$HERE" --export-debug "Android" "$APK"
}

need_device() {
	if ! "$ADB" get-state >/dev/null 2>&1; then
		echo "!! no device. Plug in the phone, enable USB debugging, accept the RSA prompt."
		echo "   check with:  $ADB devices"
		exit 1
	fi
}

install_run() {
	need_device
	echo ">> installing…"
	"$ADB" install -r "$APK"
	echo ">> launching…"
	"$ADB" shell monkey -p "$PKG" -c android.intent.category.LAUNCHER 1 >/dev/null
}

logcat() {
	need_device
	echo ">> logs (Ctrl-C to stop)…"
	"$ADB" logcat -c || true
	"$ADB" logcat godot:V GodotEngine:V "*:S"
}

case "${1:-all}" in
	build)  build ;;
	logcat) logcat ;;
	all)    build; install_run; logcat ;;
	*) echo "usage: $0 [build|logcat|all]"; exit 2 ;;
esac
