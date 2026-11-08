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
    private val paneDisappeared = 32

    @Test
    fun systemUisEmptyFrameIsTheIndicator() {
        assertTrue(DriveService.isCopyIndicator(stateChanged, systemUi, frame, 0, 0))
    }

    @Test
    fun theIndicatorMayCarryOneText() {
        // Seen on Android 15: the same copy indicator with one text item.
        assertTrue(DriveService.isCopyIndicator(stateChanged, systemUi, frame, 1, 0))
    }

    @Test
    fun theShadeIsTheSameWindowWithText() {
        assertFalse(DriveService.isCopyIndicator(stateChanged, systemUi, frame, 7, 0))
    }

    @Test
    fun theShadeClosingIsAPaneDisappearing() {
        assertFalse(DriveService.isCopyIndicator(stateChanged, systemUi, frame, 1, paneDisappeared))
    }

    @Test
    fun nothingElseIs() {
        assertFalse(DriveService.isCopyIndicator(stateChanged, "com.example", frame, 0, 0))
        assertFalse(DriveService.isCopyIndicator(stateChanged, systemUi, "android.app.Dialog", 0, 0))
        assertFalse(DriveService.isCopyIndicator(2048, systemUi, frame, 0, 0))
        assertFalse(DriveService.isCopyIndicator(stateChanged, null, null, 0, 0))
    }
}

class CopyFollowUpTest {
    @Test
    fun theIndicatorsReadIsAlwaysFollowedUp() {
        // A copy while the indicator is up raises no event: read again later.
        assertTrue(DriveService.followsUp(changed = false, done = 0))
        assertTrue(DriveService.followsUp(changed = true, done = 0))
    }

    @Test
    fun aFollowUpThatFoundSomethingNewIsFollowedUpAgain() {
        assertTrue(DriveService.followsUp(changed = true, done = 1))
        assertFalse(DriveService.followsUp(changed = false, done = 1))
    }

    @Test
    fun theChainEnds() {
        assertFalse(DriveService.followsUp(changed = true, done = DriveService.MAX_FOLLOW_UPS))
    }
}

class CopyShortcutTest {
    private fun key(c: String, control: Boolean = true, alt: Boolean = false) =
        org.ligatureprotocol.core.DriveAction.Key(
            org.ligatureprotocol.core.DriveKey.Char(c),
            shift = false,
            control = control,
            alt = alt,
        )

    @Test
    fun controlCAndXCopy() {
        assertTrue(DriveService.copies(key("c")))
        assertTrue(DriveService.copies(key("X")))
    }

    @Test
    fun otherKeysDoNot() {
        assertFalse(DriveService.copies(key("v")))
        assertFalse(DriveService.copies(key("c", control = false)))
        assertFalse(DriveService.copies(key("c", alt = true)))
    }
}
