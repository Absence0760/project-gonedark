// Root build script. Declares the Android Gradle Plugin + Kotlin/Compose plugin versions
// for subprojects. Pin AGP to a version compatible with the installed Android SDK/Gradle on
// the build machine; bump as needed.
//
// Kotlin 2.0.x ships the Compose compiler as a first-class Gradle plugin
// (`org.jetbrains.kotlin.plugin.compose`, versioned in lockstep with Kotlin) rather than the
// old `composeOptions.kotlinCompilerExtensionVersion`. 2.0.21 pairs with AGP 8.7.x + Gradle
// 8.11 (this scaffold's pins). The native-Compose app shell (D32 "Boot & title") lives in
// `:app`; the shared Rust engine is unchanged and still loaded by the NativeActivity.
plugins {
    id("com.android.application") version "8.7.2" apply false
    id("org.jetbrains.kotlin.android") version "2.0.21" apply false
    id("org.jetbrains.kotlin.plugin.compose") version "2.0.21" apply false
}
