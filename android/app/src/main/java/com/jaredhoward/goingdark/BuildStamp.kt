package com.jaredhoward.goingdark

/**
 * Pure formatting for the title screen's build-channel + version stamp — e.g. `build dev · v0.0.0`.
 *
 * Kept as free functions with **no Android types** so they are unit-testable on the plain JVM
 * (`src/test`, no device). This is the testable seam the surrounding Compose UI is exempt from:
 * the chrome is native, device-gated glue (D32), but any real logic still gets a test (CLAUDE.md
 * testing rule) — so the one bit of formatting logic lives here, away from the composables.
 */

/** Format the title-screen stamp from a build `channel` (e.g. "dev") and a `version` (e.g. "0.0.0"). */
fun buildStamp(channel: String, version: String): String =
    "build ${channel.trim().lowercase()} · v${version.trim()}"

/** The build channel from the debuggable flag: a debug build is the "dev" channel, else "release". */
fun buildChannel(isDebug: Boolean): String = if (isDebug) "dev" else "release"
