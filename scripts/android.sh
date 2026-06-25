#!/usr/bin/env bash
# Going Dark — Rust engine Android build/install/run loop (Phase 1 build-order step 6).
#
# Wraps cargo-ndk + Gradle + adb behind one entry point; the root package.json pnpm
# scripts (`android:*`) call into here.
#
#   Usage:  scripts/android.sh [command] [device-serial]
#     setup     install the host toolchain (cargo-ndk + the rustup android target)
#     devices   list the connected devices/emulators (their serials)
#     build     cargo-ndk: build libgonedark_pal_android.so into android/app/.../jniLibs
#     apk       gradle :app:assembleDebug (its cargoNdkBuild task builds the .so first)
#     install   build the APK, then `adb install -r` to the target device
#     run|dev   install + `am start` the launcher (MainActivity) + stream logcat  (inner loop)
#     logcat    just tail the app's logs (tag `gonedark`)
#   (default command: run)
#
# Picking a device (you have several): set GONEDARK_DEVICE=<serial>, or pass the serial as
# the second arg (`scripts/android.sh run <serial>`, i.e. `pnpm android:run -- <serial>`).
# With exactly one device connected it is auto-selected; with several and none chosen the
# install/run/logcat commands stop and print the list. `scripts/android.sh devices` shows it.
#
# Env knobs:  ANDROID_HOME / ANDROID_SDK_ROOT (SDK path), ANDROID_NDK_HOME (else newest
#   ndk/<ver> under the SDK is auto-selected), GONEDARK_ABI (default arm64-v8a),
#   GONEDARK_PROFILE (debug|release, default debug), GONEDARK_DEVICE (adb serial),
#   ADB (adb path).
#
# Requires: cargo-ndk, an Android SDK + NDK, adb, and a USB-debugging device for
# install/run. Phase 1 ships arm64-v8a only (proves real-arm64 determinism, invariant #7).
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
SDK="${ANDROID_HOME:-${ANDROID_SDK_ROOT:-$HOME/Android/Sdk}}"
ADB="${ADB:-$SDK/platform-tools/adb}"
ABI="${GONEDARK_ABI:-arm64-v8a}"
PROFILE="${GONEDARK_PROFILE:-debug}"
# Target device serial: GONEDARK_DEVICE, else the optional 2nd CLI arg, else auto/none.
DEVICE="${GONEDARK_DEVICE:-${2:-}}"
# Launch the exported Compose app shell (the LAUNCHER); it starts the non-exported
# NativeActivity (the Rust engine) internally on "Start". adb (uid 2000) cannot
# `am start` the non-exported NativeActivity directly — that's a SecurityException
# ("not exported from uid"). See android/app/src/main/AndroidManifest.xml (D35 split).
PKG="com.jaredhoward.goingdark"
ACTIVITY="$PKG/.MainActivity"
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

# Serials of all devices in the `device` state (one per line).
online_serials() {
	"$ADB" devices | awk 'NR>1 && $2=="device" {print $1}'
}

# Resolve the target device into DEVICE, or fail with a helpful list. With several devices
# connected adb refuses to act without -s, so a build "for which device?" is answered HERE:
# GONEDARK_DEVICE / CLI arg wins; else the sole connected device; else stop and list them.
resolve_device() {
	local serials count
	serials="$(online_serials)"
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
		echo "!! no device/emulator. Plug in the phone (USB debugging, accept the RSA prompt)" >&2
		echo "   or start an emulator, then:  pnpm android:devices" >&2
		exit 1
	elif [[ "$count" -gt 1 ]]; then
		echo "!! $count devices connected — pick one (you have several):" >&2
		"$ADB" devices -l >&2
		echo "   GONEDARK_DEVICE=<serial> pnpm android:run    (or: pnpm android:run -- <serial>)" >&2
		echo "   e.g.  GONEDARK_DEVICE=$(printf '%s\n' "$serials" | head -1) pnpm android:run" >&2
		exit 1
	fi
	DEVICE="$(printf '%s\n' "$serials" | head -1)"
	echo ">> device: $DEVICE"
}

# adb, scoped to the resolved target device.
adb_dev() { "$ADB" -s "$DEVICE" "$@"; }

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

# Copy libc++_shared.so (the NDK C++ runtime our cdylib links against — oboe-sys compiles
# Oboe's C++, see pal-android/build.rs) into jniLibs so it ships in the APK. Without it the
# app dies in dlopen at launch. The Gradle apk path does this via the copyCxxShared task;
# this covers the direct `cargo ndk build` path. Maps the ABI to its toolchain triple and
# resolves the host prebuilt dir rather than hardcoding linux-x86_64.
copy_cxx_shared() {
	local triple
	case "$ABI" in
		arm64-v8a) triple=aarch64-linux-android ;;
		armeabi-v7a) triple=arm-linux-androideabi ;;
		x86_64) triple=x86_64-linux-android ;;
		x86) triple=i686-linux-android ;;
		*) echo "!! unknown ABI '$ABI' — can't locate libc++_shared.so" >&2; exit 1 ;;
	esac
	local prebuilt src dest
	prebuilt="$(ls -d "$ANDROID_NDK_HOME"/toolchains/llvm/prebuilt/*/ 2>/dev/null | head -1)"
	src="${prebuilt%/}/sysroot/usr/lib/$triple/libc++_shared.so"
	dest="$ROOT/android/app/src/main/jniLibs/$ABI"
	if [[ ! -f "$src" ]]; then
		echo "!! libc++_shared.so not found at $src" >&2
		exit 1
	fi
	mkdir -p "$dest"
	cp -f "$src" "$dest/"
	echo ">> bundled libc++_shared.so -> jniLibs/$ABI"
}

cmd_build() {
	need_cargo_ndk
	resolve_ndk
	local flags=()
	[[ "$PROFILE" == "release" ]] && flags+=(--release)
	echo ">> cargo ndk build ($ABI, $PROFILE) -> android/app/src/main/jniLibs"
	(cd "$ROOT" && cargo ndk -t "$ABI" -o android/app/src/main/jniLibs build \
		-p gonedark-pal-android "${flags[@]}")
	copy_cxx_shared
}

cmd_apk() {
	# :app:assembleDebug triggers the cargoNdkBuild Gradle task, so the .so is built here.
	need_cargo_ndk
	resolve_ndk
	echo ">> gradle :app:assembleDebug"
	run_gradle :app:assembleDebug
}

cmd_devices() {
	echo ">> connected devices/emulators (serials):"
	"$ADB" devices -l
}

cmd_install() {
	cmd_apk
	resolve_device
	echo ">> adb -s $DEVICE install -r $APK"
	adb_dev install -r "$APK"
}

cmd_run() {
	cmd_install
	echo ">> am start $ACTIVITY  (launcher; taps through to the engine on \"Start\") (on $DEVICE)"
	adb_dev shell am start -n "$ACTIVITY"
	cmd_logcat
}

cmd_logcat() {
	resolve_device
	echo ">> adb -s $DEVICE logcat -s gonedark:V   (Ctrl-C to stop)"
	adb_dev logcat -s gonedark:V
}

case "${1:-run}" in
	setup) cmd_setup ;;
	devices) cmd_devices ;;
	build) cmd_build ;;
	apk) cmd_apk ;;
	install) cmd_install ;;
	run | dev) cmd_run ;;
	logcat) cmd_logcat ;;
	-h | --help | help) sed -n '2,36p' "${BASH_SOURCE[0]}" ;;
	*)
		echo "usage: scripts/android.sh {setup|devices|build|apk|install|run|logcat} [serial]" >&2
		exit 2
		;;
esac
