# Going Dark — Android app scaffold

Minimal Gradle project that packages the Rust `cdylib` (from `../pal-android`) into a
NativeActivity APK/AAB. This is **Phase 1 build-order step 6** scaffolding (see
`docs/phase-1-plan.md` §5, `docs/platforms.md` §8).

> **Status: scaffold, not built here.** This workstation has **no Android SDK/NDK and no
> `cargo-ndk`**, so neither the Rust `aarch64-linux-android` target nor this Gradle app
> has been compiled. The files are structurally complete against the pinned APIs
> (`android-activity` 0.6, `jni` 0.21, `ndk` 0.9, `wgpu` 29) and must be built on a
> machine with the Android toolchain. The wrapper JAR is intentionally absent — generate
> it (below) rather than committing a binary.

## Prerequisites (the build machine)

- **Android SDK** (platform `android-35`, build-tools) and **Android NDK**
  (`ANDROID_NDK_HOME` exported, or `ndk.dir` in `local.properties`). On this user's
  Fedora workstation, install via Android Studio's SDK manager (Android Studio lives at
  `/opt/android-studio`, NOT Flatpak — see `~/CLAUDE.md`).
- **cargo-ndk**: `cargo install cargo-ndk`
- **The android Rust target**: `rustup target add aarch64-linux-android`
  (add `x86_64-linux-android` too if you want the emulator — it runs x86_64).
- **Gradle** (to generate the wrapper once): system Gradle or Android Studio's bundled one.

## One-time wrapper generation

The wrapper JAR + `gradlew` scripts are not committed. Generate them once:

    cd android && gradle wrapper --gradle-version 8.11

After this you can use `./gradlew` as shown below.

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
