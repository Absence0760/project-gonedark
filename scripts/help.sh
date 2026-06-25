#!/usr/bin/env bash
# Going Dark — describe the pnpm task runner. package.json scripts can't carry per-script
# descriptions, so this prints them, grouped by the platform/device each one targets.
# Run with:  pnpm help
set -euo pipefail

cat <<'EOF'
Going Dark — pnpm tasks (grouped by what device/target they hit)

DESKTOP — your workstation (x86_64 host): the playable game + host build
  pnpm play                 Run the game on this machine (release build — smooth framerate)
  pnpm play:debug           Run the game on this machine (debug build — fast to compile)
  pnpm desktop:build        Compile the whole workspace FOR THIS MACHINE (host, debug)
  pnpm desktop:build:release Compile the whole workspace for this machine (host, release)
  pnpm desktop:sim          Headless determinism runner on this machine (per-tick checksums)
  pnpm desktop:sim:stress   200-unit scaling scene + per-tick timing stats (Phase 3 profiling)
  pnpm desktop:viz          Headless offscreen render smoke test (PNGs + pixel asserts; needs a GPU)
  pnpm desktop:server       Run the backend service on this machine (placeholder)

ANDROID — cross-compiled arm64, run on a phone/emulator over adb
  (Several devices? set GONEDARK_DEVICE=<serial> or append `-- <serial>`; one device auto-selects.)
  pnpm android:devices      List connected devices/emulators and their serials
  pnpm android:setup        One-time: install cargo-ndk + the aarch64-linux-android Rust target
  pnpm android:build        Cross-compile the native .so (arm64) into the APK's jniLibs
  pnpm android:apk          Build the installable arm64 debug APK (gradle :app:assembleDebug)
  pnpm android:install      Build the APK + install it to the target device
  pnpm android:dev          Install + launch + stream logcat on the target device (inner loop)
  pnpm android:logcat       Tail the app's on-device logs (tag `gonedark`)
  pnpm android:checksum     Prove on-device arm64 determinism vs the host (300 ticks default)

ASSETS — content tooling on this machine (needs Blender 5.x on PATH)
  pnpm assets:models        Generate greybox .glb + cooked .mesh models + manifest (D41/D44)

QUALITY — run on this machine
  pnpm test                 cargo test across the workspace
  pnpm lint                 cargo clippy (warnings = errors)
  pnpm fmt                  cargo fmt

SERVICES — local Docker backend for dev (NOT the game)
  pnpm services:up          Start the backend containers (docker compose up -d)
  pnpm services:down        Stop and remove them
  pnpm services:restart     Restart them
  pnpm services:status      Show container status
  pnpm services:logs        Follow container logs
  pnpm services:reset       Stop and wipe volumes (destructive)
  pnpm services:db          Start only Postgres
  pnpm services:redis       Start only Redis

Device/target env knobs: GONEDARK_DEVICE (adb serial), GONEDARK_ABI (default arm64-v8a),
GONEDARK_PROFILE (debug|release). See scripts/android.sh --help for the Android details.
EOF
