package org.ligatureprotocol.android

import org.junit.Assert.assertEquals
import org.junit.Test
import org.ligatureprotocol.core.TouchEvent

class TouchTest {
    @Test
    fun consecutiveMotionIsSummedAndOnlyTheNewestPositionKept() {
        val events = listOf(
            TouchEvent.Motion(1.0, 2.0),
            TouchEvent.Motion(3.0, -1.0),
            TouchEvent.Position(0.1, 0.1),
            TouchEvent.Position(0.2, 0.3),
            TouchEvent.Motion(5.0, 5.0),
        )
        assertEquals(
            listOf(TouchEvent.Motion(4.0, 1.0), TouchEvent.Position(0.2, 0.3), TouchEvent.Motion(5.0, 5.0)),
            coalesceTouch(events),
        )
    }

    @Test
    fun anythingElseKeepsItsPlaceAndBreaksARun() {
        val key = TouchEvent.Key(30u, true)
        val events = listOf(TouchEvent.Motion(1.0, 0.0), key, TouchEvent.Motion(2.0, 0.0), TouchEvent.Text("é"))
        assertEquals(events, coalesceTouch(events))
    }

    @Test
    fun nothingComesOfNothing() {
        assertEquals(emptyList<TouchEvent>(), coalesceTouch(emptyList()))
    }
}
