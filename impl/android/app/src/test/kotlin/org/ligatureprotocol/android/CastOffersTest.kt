package org.ligatureprotocol.android

import org.junit.Assert.assertEquals
import org.junit.Assert.assertTrue
import org.junit.Test
import org.ligatureprotocol.core.ScreenOffer

class CastOffersTest {
    private fun offer(kind: String = "display", audio: Boolean = true) = ScreenOffer(
        peer = "z6Mk",
        peerName = "deskbox",
        kind = kind,
        name = "Firefox",
        width = 1600u,
        height = 1000u,
        audio = audio,
    )

    /** In front, or allowed to come to the front, the cast opens; else the user is told. */
    @Test
    fun anOfferOpensAtOnceOnlyWhereAndroidLetsTheAppShowItself() {
        assertEquals(CastOffers.Show.OPEN, CastOffers.decide(foreground = true, accessibilityOn = false))
        assertEquals(CastOffers.Show.OPEN, CastOffers.decide(foreground = false, accessibilityOn = true))
        assertEquals(CastOffers.Show.NOTIFY, CastOffers.decide(foreground = false, accessibilityOn = false))
    }

    @Test
    fun theNotificationSaysWhoCastsWhatAndWhetherSoundComes() {
        assertEquals("deskbox is casting to this phone", CastOffers.title(offer()))
        assertEquals(
            "Its screen, with sound. Tap to watch, swipe away to decline.",
            CastOffers.text(offer()),
        )
        val window = CastOffers.text(offer(kind = "window", audio = false))
        assertTrue(window, window.startsWith("A window (Firefox). Tap"))
        // A camera's cast, with its microphone.
        val camera = CastOffers.text(offer(kind = "camera"))
        assertTrue(camera, camera.startsWith("Its camera (Firefox), with sound. Tap"))
    }
}
