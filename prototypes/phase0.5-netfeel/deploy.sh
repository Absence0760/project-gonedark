#!/usr/bin/env bash
# Going Dark — Phase 0.5 netfeel spike: build APK, install, launch, stream logs.
# THROWAWAY tooling. Two clients needed — see docs/phase-0.5-plan.md.
#
#   ./deploy.sh           # export + install + launch + logcat (phone = one client)
#   ./deploy.sh build     # export the APK only
#   ./deploy.sh host      # run a HOST instance on THIS desktop (the other client)
#   ./deploy.sh join IP   # run a CLIENT on this desktop, connecting to IP
#
# Typical bring-up: `./deploy.sh host` on the desktop, then the phone JOINs the
# desktop's LAN IP (shown on the host's lobby screen). Or two phones on one Wi-Fi.
set -euo pipefail

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PKG="com.gonedark.phase05netfeel"
APK="$HERE/out/gonedark-phase05-netfeel.apk"
ADB="${ADB:-$HOME/Android/Sdk/platform-tools/adb}"

build() {
	mkdir -p "$HERE/out"
	echo ">> exporting debug APK (arm64)…"
	godot --headless --path "$HERE" --export-debug "Android" "$APK"
}

need_device() {
	"$ADB" get-state >/dev/null 2>&1 || { echo "!! no device — plug in, enable USB debugging."; exit 1; }
}

case "${1:-all}" in
	build) build ;;
	host)  godot --path "$HERE" -- --host ;;
	join)  godot --path "$HERE" -- --join="${2:?usage: deploy.sh join <ip>}" ;;
	all)
		build; need_device
		"$ADB" install -r "$APK"
		"$ADB" shell monkey -p "$PKG" -c android.intent.category.LAUNCHER 1 >/dev/null
		"$ADB" logcat -c || true
		echo ">> logs (Ctrl-C to stop)…"
		"$ADB" logcat godot:V GodotEngine:V "*:S"
		;;
	*) echo "usage: $0 [build|host|join <ip>|all]"; exit 2 ;;
esac
