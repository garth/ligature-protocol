package org.ligatureprotocol.android

import org.junit.Assert.assertEquals
import org.junit.Test

class ZoomTest {
    private val box = 1000f to 500f

    private fun assertAt(expected: Pair<Double, Double>, actual: Pair<Double, Double>) {
        assertEquals(expected.first, actual.first, 1e-5)
        assertEquals(expected.second, actual.second, 1e-5)
    }

    @Test
    fun unzoomedATouchIsItsFractionOfThePicture() {
        val z = Zoom()
        assertAt(0.25 to 0.5, z.fraction(250f, 250f, box.first, box.second))
    }

    @Test
    fun zoomingKeepsThePointUnderTheFingersStill() {
        // Doubling about (500, 250): what was under the fingers stays there.
        val z = Zoom().pinched(2f, 500f, 250f, 0f, 0f, box.first, box.second)
        assertEquals(2f, z.scale)
        assertAt(0.5 to 0.5, z.fraction(500f, 250f, box.first, box.second))
        // Halfway from the centre to the left edge of the box is a quarter
        // of the way into the zoomed picture's middle half.
        assertAt(0.375 to 0.5, z.fraction(250f, 250f, box.first, box.second))
    }

    @Test
    fun aPinchThatMovesPansTheZoomedPicture() {
        val z = Zoom().pinched(2f, 500f, 250f, 0f, 0f, box.first, box.second)
            .pinched(1f, 500f, 250f, 100f, 0f, box.first, box.second)
        // Moving the fingers right by 100 brings 50 picture pixels from the left.
        assertAt(0.45 to 0.5, z.fraction(500f, 250f, box.first, box.second))
    }

    @Test
    fun thePictureNeverLeavesTheBoxNorShrinksBelowFit() {
        val z = Zoom().pinched(0.5f, 500f, 250f, 0f, 0f, box.first, box.second)
        assertEquals(Zoom(), z)
        val panned = Zoom().pinched(2f, 0f, 0f, 5000f, 5000f, box.first, box.second)
        assertEquals(0f, panned.x)
        assertEquals(0f, panned.y)
        val far = Zoom().pinched(2f, 1000f, 500f, -5000f, -5000f, box.first, box.second)
        assertEquals(-1000f, far.x)
        assertEquals(-500f, far.y)
    }

    @Test
    fun zoomStopsAtFiveTimes() {
        val z = Zoom().pinched(100f, 500f, 250f, 0f, 0f, box.first, box.second)
        assertEquals(Zoom.MAX_SCALE, z.scale)
    }
}
