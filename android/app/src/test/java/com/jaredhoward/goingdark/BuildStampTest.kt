package com.jaredhoward.goingdark

import org.junit.Assert.assertEquals
import org.junit.Test

/**
 * JVM unit tests for the title screen's pure build-stamp seam (BuildStamp.kt). The Compose UI that
 * consumes it is device-gated chrome (D32) and exempt, but the formatting logic is testable here
 * with no device — so it is tested (CLAUDE.md testing rule).
 */
class BuildStampTest {
    @Test
    fun debug_build_is_the_dev_channel() {
        assertEquals("dev", buildChannel(isDebug = true))
    }

    @Test
    fun release_build_is_the_release_channel() {
        assertEquals("release", buildChannel(isDebug = false))
    }

    @Test
    fun stamp_formats_channel_and_version() {
        assertEquals("build dev · v0.0.0", buildStamp("dev", "0.0.0"))
    }

    @Test
    fun stamp_normalises_case_and_trims_whitespace() {
        assertEquals("build release · v1.2.3", buildStamp("  RELEASE ", " 1.2.3 "))
    }

    @Test
    fun stamp_matches_the_path_mainactivity_uses() {
        // The exact composition MainActivity performs: channel from the debug flag, then format.
        assertEquals("build dev · v0.0.0", buildStamp(buildChannel(isDebug = true), "0.0.0"))
        assertEquals("build release · v0.0.0", buildStamp(buildChannel(isDebug = false), "0.0.0"))
    }
}
