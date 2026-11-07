package org.ligatureprotocol.android

import org.junit.Assert.assertEquals
import org.junit.Test
import org.ligatureprotocol.core.DriveAction
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

class CoalesceDriveTest {
    @Test
    fun onlyTheNewestMoveOfARunIsKept() {
        val queue = ArrayList<DriveAction>()
        coalesceDrive(queue, DriveAction.TouchDown(1.0, 1.0))
        coalesceDrive(queue, DriveAction.TouchMove(2.0, 2.0))
        coalesceDrive(queue, DriveAction.TouchMove(3.0, 3.0))
        coalesceDrive(queue, DriveAction.TouchUp(3.0, 3.0))
        assertEquals(
            listOf(DriveAction.TouchDown(1.0, 1.0), DriveAction.TouchMove(3.0, 3.0), DriveAction.TouchUp(3.0, 3.0)),
            queue,
        )
    }

    @Test
    fun swipesInARowAddUpFromTheFirstStart() {
        val queue = ArrayList<DriveAction>()
        coalesceDrive(queue, DriveAction.Swipe(5.0, 6.0, 0.0, -100.0))
        coalesceDrive(queue, DriveAction.Swipe(5.0, 6.0, 10.0, -50.0))
        assertEquals(listOf<DriveAction>(DriveAction.Swipe(5.0, 6.0, 10.0, -150.0)), queue)
    }

    @Test
    fun aFingerGoingDownIsNeverDropped() {
        val queue = ArrayList<DriveAction>()
        coalesceDrive(queue, DriveAction.TouchUp(1.0, 1.0))
        coalesceDrive(queue, DriveAction.TouchDown(1.0, 1.0))
        coalesceDrive(queue, DriveAction.TouchUp(1.0, 1.0))
        assertEquals(3, queue.size)
    }
}
