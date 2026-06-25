# Going Dark — Android app scaffold

Minimal Gradle project that packages the Rust `cdylib` (from `../pal-android`) into a
NativeActivity APK/AAB. This is **Phase 1 build-order step 6** scaffolding (see
`docs/phase-1-plan.md` §5, `docs/platforms.md` §8).

> **Status: runs the slice on a real arm64 device.** On the dev workstation (NDK 28,
> `cargo-ndk` 4.x, JDK 21, Gradle 8.11 via the committed wrapper) `pnpm android:apk`
> produces `app/build/outputs/apk/debug/app-debug.apk` (`com.jaredhoward.goingdark`,
> bundling `lib/arm64-v8a/libgonedark_pal_android.so`), and `android_main` drives the
> **shared `engine::Game` loop** (D20) — on an Adreno 750 the unit moves via the flow
> field, tap-to-move works, and a provisional two-finger-tap embody toggle flips the world
> dark. Built against `android-activity` 0.6, `jni` 0.21, `ndk` 0.9, `wgpu` 29. **Two
> on-device sign-offs still gate Phase 1 DONE:** `pnpm android:checksum` (sim bit-identical
> vs desktop, below) and the per-second FPS heartbeat in `adb logcat`.

## Prerequisites (the build machine)

- **Android SDK** (platform `android-35`, build-tools) and **Android NDK**
  (`ANDROID_NDK_HOME` exported, or `ndk.dir` in `local.properties`). On this user's
  Fedora workstation, install via Android Studio's SDK manager (Android Studio lives at
  `/opt/android-studio`, NOT Flatpak — see `~/CLAUDE.md`).
- **cargo-ndk**: `cargo install cargo-ndk`
- **The android Rust target**: `rustup target add aarch64-linux-android`
  (add `x86_64-linux-android` too if you want the emulator — it runs x86_64).
- **Gradle** is **not** needed up front — the wrapper (`gradlew` + `gradle-wrapper.jar`,
  pinned to 8.11) is committed and downloads its own Gradle on first run.

## The Gradle wrapper is committed

`gradlew`, `gradlew.bat`, and `gradle/wrapper/gradle-wrapper.jar` are checked in (pinned to
Gradle 8.11, compatible with AGP 8.7.2) — the standard reproducible-build setup. You do not
need a system Gradle; `./gradlew` (or the `pnpm android:*` scripts) bootstraps it. To bump
the pinned version later: `cd android && ./gradlew wrapper --gradle-version <x>`.

## pnpm shortcuts (the easy path)

The root `package.json` wraps the commands below behind `scripts/android.sh`, which
auto-resolves `ANDROID_NDK_HOME` (newest `ndk/<ver>` under the SDK) so you don't have to
export it:

    pnpm android:devices   # list connected devices/emulators + their serials
    pnpm android:setup     # one-time: cargo install cargo-ndk + rustup target add aarch64-linux-android
    pnpm android:build     # cargo-ndk: build libgonedark_pal_android.so into jniLibs (debug)
    pnpm android:apk       # gradle :app:assembleDebug -> app-debug.apk
    pnpm android:install   # build the APK + adb install -r to the target device
    pnpm android:dev       # install + am start + stream logcat (the inner loop)
    pnpm android:logcat    # tail the app's logs (tag `gonedark`)

**Several devices connected?** `install`/`dev`/`logcat`/`checksum` need to know which one.
One device auto-selects; with several, pick it with `GONEDARK_DEVICE=<serial>` (from
`pnpm android:devices`) or append the serial after `--`:

    GONEDARK_DEVICE=ABC123 pnpm android:dev
    pnpm android:dev -- ABC123

`GONEDARK_PROFILE=release pnpm android:build` builds a release `.so`; `GONEDARK_ABI=<abi>`
overrides the target ABI (default `arm64-v8a`). **Desktop side** (this machine, not a phone):
`pnpm play` runs the game, `pnpm desktop:build` builds the workspace, `pnpm desktop:sim` runs
the headless determinism checksum stream. `pnpm help` lists every task by target. The exact
underlying commands follow.

## On-device determinism proof (`pnpm android:checksum`)

The Phase 1 exit gate / invariant #7: prove the deterministic fixed-point sim is
**bit-identical on the real arm64 phone** vs the desktop. `pnpm android:checksum`
(`scripts/android-checksum.sh`) builds the headless `gonedark-sim-runner` (core-only, no
GPU/PAL deps) for `aarch64-linux-android` with `cargo ndk`, `adb push`es it to
`/data/local/tmp`, runs it on-device, and diffs its per-tick `<tick> <checksum>` stream
against the **same** runner on the host:

    pnpm android:checksum         # 300 ticks (default)
    pnpm android:checksum 1000    # override the tick count

Identical streams print `on-device arm64 determinism holds`; **any divergence prints the
diff and exits non-zero** — an on-device desync is a real determinism bug (invariant #7),
never silenced. Needs a connected USB-debugging arm64 device (same prerequisites as the
other `android:*` scripts). The device binary is cleaned up afterward.

## Build commands (exact)

Build the Rust cdylib for arm64 into `jniLibs`, then assemble the APK:

    cargo ndk -t arm64-v8a -o android/app/src/main/jniLibs build --release
    cd android && ./gradlew :app:assembleDebug

(The `:app:assembleDebug` task also wires `cargo ndk ... build` via the `cargoNdkBuild`
Gradle task in `app/build.gradle.kts`, so the explicit `cargo ndk` line is belt-and-
suspenders — run it standalone when iterating on Rust only.)

Run the two commands above from the **repo root** (the `cargo ndk` `-o` path is relative
to the repo root; `gradlew` lives in `android/`).

For a release/store build (AAB for Google Play, platforms.md §8):

    cargo ndk -t arm64-v8a -o android/app/src/main/jniLibs build --release
    cd android && ./gradlew :app:bundleRelease

## Dev inner loop (roadmap.md "Automated edit→build→deploy→test loop")

Plug a device (or start an emulator), then:

    cargo ndk -t arm64-v8a -o android/app/src/main/jniLibs build && \
      cd android && ./gradlew :app:assembleDebug && \
      adb install -r app/build/outputs/apk/debug/app-debug.apk && \
      adb shell am start -n com.jaredhoward.goingdark/android.app.NativeActivity && \
      adb logcat -s gonedark:V

`adb logcat -s gonedark:V` filters to our log tag (set in `android_main` via
`android_logger` with `.with_tag("gonedark")`), so crashes/lifecycle are readable. A
coding agent can script this whole cycle and self-diagnose from logcat.

> **Emulator caveat (roadmap.md):** the Android Emulator is x86_64 — build that ABI in
> debug (`cargo ndk -t x86_64 ...` and add `x86_64` to `abiFilters`) for the emulator.
> Real arm64 hardware is what proves the Phase 1 determinism exit criterion.

## What this maps to in the Rust workspace

- `../pal-android` is the `cdylib` crate. It exports `android_main` (via
  `android-activity`'s `native-activity` feature) and implements the `gonedark_pal`
  traits backed by Android: touch input, wgpu/Vulkan surface, a real low-latency
  **AAudio** sink (via `oboe`, D29 — mixes the same positioned cues as desktop through the
  shared `gonedark_pal::mix` math; audible output still owed an on-device listen), and an
  AAssetManager/storage stub.
- The produced library is `libgonedark_pal_android.so`; `AndroidManifest.xml`'s
  `android.app.lib_name` is `gonedark_pal_android` (no `lib`/`.so`). If the integrator
  instead makes `app` the cdylib, rename both consistently.

## Status / remaining

- The for-target build, APK assembly, and on-device run are all **done** (NDK 28 +
  cargo-ndk 4.x; Adreno 750). `android-activity` 0.6 / `jni` 0.21 / `wgpu` 29 usage in
  `../pal-android/src/lib.rs` is exercised on real hardware — an early run surfaced + fixed
  a real arm64 surface-config crash (the `downlevel_defaults` 2048 texture cap vs a 2340-wide
  screen).
- **Remaining to declare Phase 1 done:** run `pnpm android:checksum` against a device (sim
  bit-identical vs desktop) and read the FPS heartbeat in `adb logcat` (target frame rate).
- **Provisional, not shipped:** the two-finger-tap embody toggle is a dev binding; the real
  mobile control scheme (on-screen sticks / gyro) is a Phase 2 design call. The AGP/Gradle/
  SDK/NDK version pins are this workstation's — adjust on another build machine.
- **Audio (D29):** the `oboe`/AAudio sink is implemented and the crate builds for
  `aarch64-linux-android` with the NDK, but **audible output is owed an on-device listen** —
  run `pnpm android:dev`, listen for panned/muffled cues while embodied, and confirm logcat
  does NOT show `[audio] disabled (silent)` (which would mean the stream failed to open and
  the sink degraded to a silent no-op, per invariant #8).
