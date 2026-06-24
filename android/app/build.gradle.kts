// App module — packages the Rust cdylib (libgonedark.so) into a NativeActivity APK/AAB.
//
// The Rust → .so build is done by `cargo-ndk` (see README.md) which writes per-ABI
// libraries into `src/main/jniLibs/<abi>/`. The `cargoNdkBuild` task below wires that
// step so a plain `./gradlew :app:assembleDebug` builds the native lib first. Phase 1
// targets arm64-v8a only (real device); add x86_64 for the emulator if needed
// (platforms.md / roadmap "Emulator caveat").

import org.gradle.api.tasks.Exec

plugins {
    id("com.android.application")
}

android {
    namespace = "com.jaredhoward.goingdark"
    // Pin compileSdk to a version your installed SDK provides.
    compileSdk = 35

    defaultConfig {
        applicationId = "com.jaredhoward.goingdark"
        // android-activity / NativeActivity floor; 24 is a safe lower bound, 26 if you
        // want AAudio low-latency guarantees without back-compat shims.
        minSdk = 24
        targetSdk = 35
        versionCode = 1
        versionName = "0.0.0"

        ndk {
            // Phase 1 ships arm64 only (proves real arm64 determinism — invariant #7).
            abiFilters += listOf("arm64-v8a")
        }
    }

    buildTypes {
        getByName("debug") {
            isDebuggable = true
        }
        getByName("release") {
            isMinifyEnabled = false
        }
    }

    // The cdylib is delivered as a prebuilt .so under src/main/jniLibs (written by
    // cargo-ndk), so no externalNativeBuild (CMake/ndk-build) block is needed.
    sourceSets {
        getByName("main") {
            jniLibs.srcDirs("src/main/jniLibs")
        }
    }
}

// --- cargo-ndk wiring ------------------------------------------------------------------
// Build the Rust cdylib for arm64-v8a straight into jniLibs before the native libs are
// merged. Requires `cargo-ndk` on PATH and `ANDROID_NDK_HOME` set (see README.md).
//
// Profile: `debug` build type -> cargo debug; otherwise `--release`. Kept simple here —
// one task for the debug path that the README's `assembleDebug` flow depends on.
val cargoNdkBuild by tasks.registering(Exec::class) {
    group = "rust"
    description = "Build the Rust cdylib (../../pal-android) for arm64-v8a into jniLibs"
    workingDir = rootProject.projectDir.parentFile // repo root (../ from android/)
    commandLine(
        "cargo", "ndk",
        "-t", "arm64-v8a",
        "-o", "android/app/src/main/jniLibs",
        "build", "-p", "gonedark-pal-android",
    )
}

// Make every native-lib merge depend on the cargo build. Covers debug + release variants.
tasks.matching { it.name.startsWith("merge") && it.name.contains("JniLibFolders") }
    .configureEach { dependsOn(cargoNdkBuild) }
// Belt-and-suspenders: also gate preBuild so a fresh checkout builds the lib first.
tasks.named("preBuild").configure { dependsOn(cargoNdkBuild) }
