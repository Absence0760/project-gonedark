#!/usr/bin/env bash
# Going Dark — Rust engine Android build/install/run loop (Phase 1 build-order step 6).
#
# Wraps cargo-ndk + Gradle + adb behind one entry point; the root package.json pnpm
# scripts (`android:*`) call into here.
#
#   Usage:  scripts/android.sh [command]
#     setup     install the host toolchain (cargo-ndk + the rustup android target)
#     build     cargo-ndk: build libgonedark_pal_android.so into android/app/.../jniLibs
#     apk       gradle :app:assembleDebug (its cargoNdkBuild task builds the .so first)
#     install   build the APK, then `adb install -r` to a connected device
#     run|dev   install + `am start` the NativeActivity + stream logcat  (the inner loop)
#     logcat    just tail the app's logs (tag `gonedark`)
#   (default command: run)
#
# Env knobs:  ANDROID_HOME / ANDROID_SDK_ROOT (SDK path), ANDROID_NDK_HOME (else newest
#   ndk/<ver> under the SDK is auto-selected), GONEDARK_ABI (default arm64-v8a),
#   GONEDARK_PROFILE (debug|release, default debug), ADB (adb path).
#
# Requires: cargo-ndk, an Android SDK + NDK, adb, and a USB-debugging device for
# install/run. Phase 1 ships arm64-v8a only (proves real-arm64 determinism, invariant #7).
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
SDK="${ANDROID_HOME:-${ANDROID_SDK_ROOT:-$HOME/Android/Sdk}}"
ADB="${ADB:-$SDK/platform-tools/adb}"
ABI="${GONEDARK_ABI:-arm64-v8a}"
PROFILE="${GONEDARK_PROFILE:-debug}"
PKG="com.jaredhoward.goingdark"
ACTIVITY="$PKG/android.app.NativeActivity"
APK="$ROOT/android/app/build/outputs/apk/debug/app-debug.apk"

# Resolve ANDROID_NDK_HOME (cargo-ndk and the Gradle cargoNdkBuild task both need it).
# Prefer an already-set value, then ANDROID_NDK_ROOT, then the newest ndk/<ver> in the SDK.
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

# Run a Gradle task from android/: prefer the committed wrapper, else a system/Studio gradle.
run_gradle() {
	if [[ -x "$ROOT/android/gradlew" ]]; then
		(cd "$ROOT/android" && ./gradlew "$@")
	elif command -v gradle >/dev/null 2>&1; then
		(cd "$ROOT/android" && gradle "$@")
	else
		echo "!! No Gradle wrapper (android/gradlew) and no 'gradle' on PATH." >&2
		echo "   Generate the wrapper once (needs Gradle — Android Studio bundles one):" >&2
		echo "     cd android && gradle wrapper --gradle-version 8.11" >&2
		echo "   or open android/ in Android Studio (/opt/android-studio) and build there." >&2
		exit 1
	fi
}

cmd_setup() {
	echo ">> installing cargo-ndk + the aarch64-linux-android rust target"
	command -v cargo-ndk >/dev/null 2>&1 || cargo install cargo-ndk
	rustup target add aarch64-linux-android
	echo ">> done. (NDK + SDK themselves come from Android Studio's SDK manager.)"
}

cmd_build() {
	need_cargo_ndk
	resolve_ndk
	local flags=()
	[[ "$PROFILE" == "release" ]] && flags+=(--release)
	echo ">> cargo ndk build ($ABI, $PROFILE) -> android/app/src/main/jniLibs"
	(cd "$ROOT" && cargo ndk -t "$ABI" -o android/app/src/main/jniLibs build \
		-p gonedark-pal-android "${flags[@]}")
}

cmd_apk() {
	# :app:assembleDebug triggers the cargoNdkBuild Gradle task, so the .so is built here.
	need_cargo_ndk
	resolve_ndk
	echo ">> gradle :app:assembleDebug"
	run_gradle :app:assembleDebug
}

cmd_install() {
	cmd_apk
	need_device
	echo ">> adb install -r $APK"
	"$ADB" install -r "$APK"
}

cmd_run() {
	cmd_install
	echo ">> am start $ACTIVITY"
	"$ADB" shell am start -n "$ACTIVITY"
	cmd_logcat
}

cmd_logcat() {
	need_device
	echo ">> adb logcat -s gonedark:V   (Ctrl-C to stop)"
	"$ADB" logcat -s gonedark:V
}

case "${1:-run}" in
	setup) cmd_setup ;;
	build) cmd_build ;;
	apk) cmd_apk ;;
	install) cmd_install ;;
	run | dev) cmd_run ;;
	logcat) cmd_logcat ;;
	-h | --help | help) sed -n '2,30p' "${BASH_SOURCE[0]}" ;;
	*)
		echo "usage: scripts/android.sh {setup|build|apk|install|run|logcat}" >&2
		exit 2
		;;
esac
