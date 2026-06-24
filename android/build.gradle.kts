// Root build script. Declares the Android Gradle Plugin version for subprojects.
// Pin AGP to a version compatible with the installed Android SDK/Gradle on the build
// machine; bump as needed (this scaffold has not been built/locked here).
plugins {
    id("com.android.application") version "8.7.2" apply false
}
