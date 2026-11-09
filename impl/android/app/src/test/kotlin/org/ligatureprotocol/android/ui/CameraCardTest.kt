package org.ligatureprotocol.android.ui

import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test
import org.ligatureprotocol.core.CameraTarget

class CameraCardTest {
    private fun target(shows: Boolean, presents: Boolean) =
        CameraTarget(key = "z6Mk", name = "deskbox", shows = shows, presents = presents)

    /** A device is offered what it says it does with a camera, the webcam first. */
    @Test
    fun aDeviceIsOfferedWhatItDoesWithACamera() {
        assertEquals(
            listOf("Use as deskbox's webcam" to true, "Show on deskbox" to false),
            cameraCastActions(target(shows = true, presents = true)),
        )
        // A Mac or a phone shows a camera and presents none.
        assertEquals(
            listOf("Show on deskbox" to false),
            cameraCastActions(target(shows = true, presents = false)),
        )
        assertEquals(
            listOf("Use as deskbox's webcam" to true),
            cameraCastActions(target(shows = false, presents = true)),
        )
        assertTrue(cameraCastActions(target(shows = false, presents = false)).isEmpty())
    }

    @Test
    fun theCardShowsOnlyWhenThereIsSomethingToDo() {
        assertFalse(cameraCardShown(watchTargets = 0, castTargets = 0, casting = false))
        assertTrue(cameraCardShown(watchTargets = 1, castTargets = 0, casting = false))
        assertTrue(cameraCardShown(watchTargets = 0, castTargets = 1, casting = false))
        assertTrue(cameraCardShown(watchTargets = 0, castTargets = 0, casting = true))
    }

    @Test
    fun theWatchScreenSaysWhoseDoingItIsAndWhetherItIsACamera() {
        assertEquals("Watching deskbox", watchHeading("deskbox", cast = false, camera = false))
        assertEquals("deskbox is casting", watchHeading("deskbox", cast = true, camera = false))
        assertEquals("Watching deskbox's camera", watchHeading("deskbox", cast = false, camera = true))
        assertEquals("deskbox is casting its camera", watchHeading("deskbox", cast = true, camera = true))
    }
}
