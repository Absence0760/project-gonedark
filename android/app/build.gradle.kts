// App module — the native **app shell** (D32 "Boot & title" landing, Jetpack Compose) plus
// the Rust engine packaged as a NativeActivity.
//
// Two layers live here, kept strictly apart (D32):
//   * the Kotlin/Compose shell (MainActivity + the title screen) — out-of-match chrome, the
//     LAUNCHER the player lands on;
//   * the shared Rust engine cdylib (libgonedark_pal_android.so, built by `cargo-ndk` from
//     `../../pal-android`), loaded by `android.app.NativeActivity` when the shell starts a
//     match. The cargo-ndk wiring at the bottom is unchanged from the Phase-1 scaffold.
//
// The shell holds NO game/sim logic — it reaches the shared `core` only through the
// GPU-free, logic-free `core::shell` seam (D34) the same way the PAL does. Today "Start"
// just launches the engine activity (match-config handoff across the seam is deferred with
// match-setup, Q5). Phase 1 ships arm64-v8a only; the Compose launcher itself is pure JVM
// bytecode and renders on the x86_64 emulator too (only an embodied match needs the matching
// native ABI — see README "Emulator caveat").

import org.gradle.api.tasks.Exec

plugins {
    id("com.android.application")
    id("org.jetbrains.kotlin.android")
    id("org.jetbrains.kotlin.plugin.compose")
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
            // Phase 1 ships arm64 only (proves real arm64 determinism — invariant #7). The
            // Compose shell does not depend on this; only an embodied match does.
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

    buildFeatures {
        compose = true
        // BuildConfig.VERSION_NAME / DEBUG feed the title screen's build-channel + version
        // stamp (see BuildStamp.kt).
        buildConfig = true
    }

    compileOptions {
        sourceCompatibility = JavaVersion.VERSION_17
        targetCompatibility = JavaVersion.VERSION_17
    }
    kotlinOptions {
        jvmTarget = "17"
    }

    // The cdylib is delivered as a prebuilt .so under src/main/jniLibs (written by
    // cargo-ndk), so no externalNativeBuild (CMake/ndk-build) block is needed.
    sourceSets {
        getByName("main") {
            jniLibs.srcDirs("src/main/jniLibs")
        }
    }
}

dependencies {
    // Compose, pinned via the BOM so the artifacts stay mutually compatible (2024.10.01
    // pairs with Kotlin 2.0.21 / the 2.0.21 compose-compiler plugin).
    val composeBom = platform("androidx.compose:compose-bom:2024.10.01")
    implementation(composeBom)
    implementation("androidx.activity:activity-compose:1.9.3")
    implementation("androidx.compose.ui:ui")
    implementation("androidx.compose.ui:ui-tooling-preview")
    implementation("androidx.compose.material3:material3")
    debugImplementation("androidx.compose.ui:ui-tooling")

    // The build-stamp seam is a pure Kotlin fn (BuildStamp.kt); cover it with a plain JVM
    // unit test (no device needed). The Compose UI itself is device-gated glue (D32 chrome).
    testImplementation("junit:junit:4.13.2")
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

// Bundle the NDK C++ shared runtime alongside our cdylib. `oboe-sys` compiles Oboe's C++,
// so libgonedark_pal_android.so is linked against libc++_shared.so (see ../../pal-android/
// build.rs). That .so is NOT part of our crate's output and is not bundled by default, so the
// app would `dlopen`-fail at launch with "library libc++_shared.so not found". Copy it from
// the active NDK into the same jniLibs/<abi> dir cargo-ndk writes to. The host prebuilt dir
// (linux-x86_64 / darwin-* / windows-*) is resolved rather than hardcoded.
// Resolve the NDK the same way scripts/android.sh / cargo-ndk do — from ANDROID_NDK_HOME /
// ANDROID_NDK_ROOT, else the newest ndk/<ver> under the SDK. Deliberately NOT via
// `android.ndkDirectory`, which forces NDK auto-detection at configuration time and fails
// with "NDK is not installed" unless `ndkVersion` is pinned (the cargoNdkBuild task above
// avoids it for the same reason).
fun resolveNdkDir(): File {
    System.getenv("ANDROID_NDK_HOME")?.let { return File(it) }
    System.getenv("ANDROID_NDK_ROOT")?.let { return File(it) }
    val sdk = System.getenv("ANDROID_HOME") ?: System.getenv("ANDROID_SDK_ROOT")
    val ndkRoot = sdk?.let { File("$it/ndk") }
    val newest = ndkRoot?.listFiles()?.filter { it.isDirectory }?.maxByOrNull { it.name }
    return newest ?: throw GradleException(
        "Cannot locate the Android NDK. Set ANDROID_NDK_HOME, or install one via the SDK manager."
    )
}

val copyCxxShared by tasks.registering(Copy::class) {
    group = "rust"
    description = "Copy libc++_shared.so (the C++ runtime oboe needs) from the NDK into jniLibs"
    val abi = "arm64-v8a"
    val triple = "aarch64-linux-android"
    val prebuilt = File(resolveNdkDir(), "toolchains/llvm/prebuilt").listFiles()
        ?.firstOrNull { it.isDirectory }
        ?: throw GradleException("No NDK llvm prebuilt toolchain under the resolved NDK")
    from(File(prebuilt, "sysroot/usr/lib/$triple/libc++_shared.so"))
    into(layout.projectDirectory.dir("src/main/jniLibs/$abi"))
}

// Make every native-lib merge depend on the cargo build AND the C++ runtime copy. Covers
// debug + release variants.
tasks.matching { it.name.startsWith("merge") && it.name.contains("JniLibFolders") }
    .configureEach { dependsOn(cargoNdkBuild, copyCxxShared) }
// Belt-and-suspenders: also gate preBuild so a fresh checkout builds the lib first.
tasks.named("preBuild").configure { dependsOn(cargoNdkBuild, copyCxxShared) }
