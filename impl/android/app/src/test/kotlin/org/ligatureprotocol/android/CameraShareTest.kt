package org.ligatureprotocol.android

import android.hardware.camera2.CameraCharacteristics
import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Test

class CameraShareTest {
    private val sizes = listOf(4032 to 3024, 3840 to 2160, 1920 to 1080, 1280 to 720, 640 to 480, 640 to 360)

    /** A camera is listed at the largest size within 1080p, not at its sensor's. */
    @Test
    fun aCameraIsListedAtItsLargestSizeWithin1080p() {
        assertEquals(1920 to 1080, CameraShare.bestSize(sizes))
        // One that has nothing that small is listed at its smallest.
        assertEquals(2560 to 1440, CameraShare.bestSize(listOf(3840 to 2160, 2560 to 1440)))
        // Taller than 1080 rows does not fit, whatever its area.
        assertEquals(640 to 480, CameraShare.bestSize(listOf(1200 to 1600, 640 to 480)))
        assertNull(CameraShare.bestSize(emptyList()))
        assertNull(CameraShare.bestSize(listOf(0 to 0)))
    }

    /** A share is captured at the agreed size or a smaller one of its shape, never a stretched one. */
    @Test
    fun aShareIsCapturedAtASizeOfTheAgreedShape() {
        // The agreed size itself, where the camera has it.
        assertEquals(1280 to 720, CameraShare.fitSize(sizes, 1280, 720))
        assertEquals(1920 to 1080, CameraShare.fitSize(sizes, 1920, 1080))
        // A viewer that takes less gets the largest of that shape within it,
        // not the 4:3 size that happens to be nearer.
        assertEquals(640 to 360, CameraShare.fitSize(sizes, 960, 540))
        // Smaller than anything of that shape: the smallest of it.
        assertEquals(640 to 360, CameraShare.fitSize(sizes, 320, 180))
        // A shape the camera does not have: the size it is listed with.
        assertEquals(1920 to 1080, CameraShare.fitSize(sizes, 1000, 1000))
        // An encoder's rounding of a size to 16 is still that shape.
        assertEquals(1280 to 720, CameraShare.fitSize(sizes, 1280, 704))
        assertNull(CameraShare.fitSize(emptyList(), 1280, 720))
        assertEquals(1920 to 1080, CameraShare.fitSize(sizes, 0, 0))
    }

    /** The camera is asked to hold the share's rate, where it has a range that does. */
    @Test
    fun theCameraIsAskedForTheSharesRate() {
        val ranges = listOf(15 to 15, 15 to 30, 30 to 30, 15 to 60, 60 to 60)
        assertEquals(30 to 30, CameraShare.rateRange(ranges, 30))
        assertEquals(60 to 60, CameraShare.rateRange(ranges, 60))
        // No steady range at that rate: the narrowest that reaches it.
        assertEquals(15 to 30, CameraShare.rateRange(ranges, 24))
        // Faster than the camera goes: its fastest.
        assertEquals(15 to 30, CameraShare.rateRange(listOf(7 to 15, 15 to 30), 60))
        assertNull(CameraShare.rateRange(emptyList(), 30))
        assertNull(CameraShare.rateRange(listOf(0 to 0), 30))
    }

    @Test
    fun aCamerasRatesAreTheTopsOfItsRanges() {
        assertEquals(
            listOf(60, 30, 15),
            CameraShare.listedRates(listOf(15 to 15, 15 to 30, 30 to 30, 30 to 120, 60 to 60)),
        )
        assertEquals(emptyList<Int>(), CameraShare.listedRates(emptyList()))
    }

    @Test
    fun aCameraIsNamedForTheWayItLooks() {
        assertEquals("front", CameraShare.facing(CameraCharacteristics.LENS_FACING_FRONT))
        assertEquals("back", CameraShare.facing(CameraCharacteristics.LENS_FACING_BACK))
        assertEquals("external", CameraShare.facing(CameraCharacteristics.LENS_FACING_EXTERNAL))
        assertEquals("external", CameraShare.facing(null))
        assertEquals("Front camera", CameraShare.name("front", "1"))
        assertEquals("Back camera", CameraShare.name("back", "0"))
        assertEquals("Camera 4", CameraShare.name("external", "4"))
    }

    /** The share's notification says what is on, and whose doing it is. */
    @Test
    fun theNotificationSaysWhatIsShared() {
        assertEquals(
            "Sharing your camera and microphone with deskbox",
            CameraShareService.title("deskbox", cast = false, microphone = true),
        )
        assertEquals(
            "Casting your camera to deskbox",
            CameraShareService.title("deskbox", cast = true, microphone = false),
        )
    }
}
