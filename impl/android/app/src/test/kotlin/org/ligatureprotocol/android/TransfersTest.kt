package org.ligatureprotocol.android

import org.junit.Assert.assertEquals
import org.junit.Test

class TransfersTest {
    @Test
    fun aNameNotTakenIsKept() {
        assertEquals("photo.jpg", Transfers.freeName("photo.jpg", setOf("other.jpg")))
    }

    @Test
    fun aClashGetsTheFirstFreeNumberBeforeTheExtension() {
        val taken = setOf("photo.jpg", "photo (2).jpg", "photo (4).jpg")
        assertEquals("photo (3).jpg", Transfers.freeName("photo.jpg", taken))
    }

    @Test
    fun theLastDotStartsTheExtensionAndALeadingDotDoesNot() {
        assertEquals("archive.tar" to ".gz", Transfers.splitName("archive.tar.gz"))
        assertEquals(".profile" to "", Transfers.splitName(".profile"))
        assertEquals("README" to "", Transfers.splitName("README"))
        assertEquals(".profile (2)", Transfers.freeName(".profile", setOf(".profile")))
        assertEquals("README (2)", Transfers.freeName("README", setOf("README")))
    }
}
