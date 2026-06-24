// Going Dark — Android app scaffold (Phase 1 build-order step 6).
//
// Minimal Gradle project that packages the Rust `cdylib` (built by cargo-ndk from
// ../pal-android) into an APK/AAB and launches it via a NativeActivity. This is a
// SCAFFOLD: it has never been built here (no Android SDK/NDK on this workstation). See
// README.md for prerequisites and the exact build commands.

pluginManagement {
    repositories {
        google()
        mavenCentral()
        gradlePluginPortal()
    }
}

dependencyResolutionManagement {
    repositoriesMode.set(RepositoriesMode.FAIL_ON_PROJECT_REPOS)
    repositories {
        google()
        mavenCentral()
    }
}

rootProject.name = "GoingDark"
include(":app")
