# Going Dark — Android app scaffold

Minimal Gradle project that packages the Rust `cdylib` (from `../pal-android`) into a
NativeActivity APK/AAB. This is **Phase 1 build-order step 6** scaffolding (see
`docs/phase-1-plan.md` §5, `docs/platforms.md` §8).

> **Status: builds an installable arm64 debug APK.** On the dev workstation (NDK 28,
> `cargo-ndk` 4.x, JDK 21, Gradle 8.11 via the committed wrapper) `pnpm android:apk`
> produces `app/build/outputs/apk/debug/app-debug.apk` (`com.jaredhoward.goingdark`,
> bundling `lib/arm64-v8a/libgonedark_pal_android.so`). **Not yet run on a device**, and
> `android_main` is the PAL backend + entry point only — the shared sim/render game loop
> is wired in Phase 2. Built against the pinned APIs (`android-activity` 0.6, `jni` 0.21,
> `ndk` 0.9, `wgpu` 29).

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

    pnpm android:setup     # one-time: cargo install cargo-ndk + rustup target add aarch64-linux-android
    pnpm android:build     # cargo-ndk: build libgonedark_pal_android.so into jniLibs (debug)
    pnpm android:apk       # gradle :app:assembleDebug -> app-debug.apk
    pnpm android:install   # build the APK + adb install -r to a connected device
    pnpm android:dev       # install + am start + stream logcat (the inner loop)
    pnpm android:logcat    # tail the app's logs (tag `gonedark`)

`GONEDARK_PROFILE=release pnpm android:build` builds a release `.so`. Desktop side:
`pnpm dev` runs the winit/wgpu app, `pnpm build` builds the workspace, `pnpm sim` runs the
headless determinism checksum stream. The exact underlying commands follow.

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
  traits backed by Android (touch input, wgpu/Vulkan surface, AAudio/AAssetManager
  stubs).
- The produced library is `libgonedark_pal_android.so`; `AndroidManifest.xml`'s
  `android.app.lib_name` is `gonedark_pal_android` (no `lib`/`.so`). If the integrator
  instead makes `app` the cdylib, rename both consistently.

## Not verified

- No for-target build has run here (no NDK/cargo-ndk). The AGP/Gradle/SDK/NDK version
  pins (`build.gradle.kts`, this README) are sensible defaults, not locked against a
  real build — adjust to the toolchain on the build machine.
- `android-activity` 0.6 / `jni` 0.21 / `wgpu` 29 API usage in `../pal-android/src/lib.rs`
  was written from the pinned-version docs; a couple of input/event accessor calls are
  flagged with inline `NOTE:` comments to re-check on the real toolchain.
