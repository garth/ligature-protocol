package org.ligatureprotocol.android

import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test

class ScreenShareTest {
    @Test
    fun theBitRateIsATenthOfABitPerPixelPerFrameWithinBounds() {
        assertEquals(4_665_600, ScreenShareService.bitRate(1080, 1440, 30))
        assertEquals(1_000_000, ScreenShareService.bitRate(160, 160, 5))
        assertEquals(12_000_000, ScreenShareService.bitRate(3840, 2160, 60))
    }

    /** As the desktops' `settle_after`: a frame and a half, never under 50 ms. */
    @Test
    fun aPictureIsRepeatedAFrameAndAHalfAfterAChangeAndNeverSooner() {
        assertEquals(50_000L, ScreenShareService.settleAfterUs(30))
        assertEquals(50_000L, ScreenShareService.settleAfterUs(60))
        assertEquals(100_000L, ScreenShareService.settleAfterUs(15))
        assertEquals(1_500_000L, ScreenShareService.settleAfterUs(0))
    }

    /** As the desktops' `KEYFRAME_BACKSTOP`: 10,000 frames' time. */
    @Test
    fun theKeyframeIntervalIsOnlyASafetyNet() {
        assertEquals(333, ScreenShareService.keyframeBackstopSeconds(30))
        assertEquals(10_000, ScreenShareService.keyframeBackstopSeconds(0))
    }

    @Test
    fun aSequenceParameterSetIsFoundAtTheStart() {
        val sps = byteArrayOf(0, 0, 0, 1, 0x67, 0x42)
        val afterAud = byteArrayOf(0, 0, 0, 1, 0x09, 0xf0.toByte(), 0, 0, 1, 0x67, 0x42)
        val idr = byteArrayOf(0, 0, 0, 1, 0x65, 0x11)
        assertTrue(ScreenShareService.startsWithSps(sps))
        assertFalse("the first unit decides", ScreenShareService.startsWithSps(afterAud))
        assertFalse(ScreenShareService.startsWithSps(idr))
        assertFalse(ScreenShareService.startsWithSps(byteArrayOf(0, 0, 1)))
    }
}
