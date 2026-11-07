package org.ligatureprotocol.android

import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test

class ClipEchoGuardTest {
    @Test
    fun aChangeWeMadeIsAnEchoOnce() {
        val guard = ClipEchoGuard()
        guard.expect(1_000)
        assertTrue(guard.isEcho(1_010))
        assertFalse(guard.isEcho(1_020))
    }

    @Test
    fun aChangeNobodyExpectedIsTheUsers() {
        assertFalse(ClipEchoGuard().isEcho(1_000))
    }

    @Test
    fun twoSetsInARowAreTwoEchoes() {
        val guard = ClipEchoGuard()
        guard.expect(1_000)
        guard.expect(1_001)
        assertTrue(guard.isEcho(1_010))
        assertTrue(guard.isEcho(1_011))
        assertFalse(guard.isEcho(1_012))
    }

    @Test
    fun aSetWhoseChangeNeverCameDoesNotSwallowALaterCopy() {
        val guard = ClipEchoGuard()
        guard.expect(1_000)
        assertFalse(guard.isEcho(1_000 + ClipEchoGuard.LIFETIME_MS + 1))
    }
}

class CopyIndicatorTest {
    private val stateChanged = 32
    private val frame = "android.widget.FrameLayout"
    private val systemUi = "com.android.systemui"

    @Test
    fun systemUisEmptyFrameIsTheIndicator() {
        assertTrue(DriveService.isCopyIndicator(stateChanged, systemUi, frame, 0))
    }

    @Test
    fun theShadeIsTheSameWindowWithText() {
        assertFalse(DriveService.isCopyIndicator(stateChanged, systemUi, frame, 7))
    }

    @Test
    fun nothingElseIs() {
        assertFalse(DriveService.isCopyIndicator(stateChanged, "com.example", frame, 0))
        assertFalse(DriveService.isCopyIndicator(stateChanged, systemUi, "android.app.Dialog", 0))
        assertFalse(DriveService.isCopyIndicator(2048, systemUi, frame, 0))
        assertFalse(DriveService.isCopyIndicator(stateChanged, null, null, 0))
    }
}
