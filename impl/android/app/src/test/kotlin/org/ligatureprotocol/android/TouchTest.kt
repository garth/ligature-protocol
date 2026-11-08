package org.ligatureprotocol.android

import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
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
    fun aMoveThatGoesNowhereIsNotMade() {
        // A Mac trackpad reports the same point again just before the release;
        // a continued stroke of no length leaves the finger down (LP-167).
        assertTrue(goesNowhere(829.64f, 1760.27f, 829.64f, 1760.27f))
        assertTrue(goesNowhere(10f, 10f, 10.3f, 10.2f))
        assertFalse(goesNowhere(10f, 10f, 11f, 10f))
        assertFalse(goesNowhere(10f, 10f, 10f, 1.0e3f))
    }

    @Test
    fun aHeldFingerWhoseStrokeAndroidEndedStartsAgainWhenItMoves() {
        // The keyguard cancels the stroke as its bouncer appears mid-swipe; the
        // swipe must go on and its release land, or the bouncer stays half
        // open (LP-167).
        assertTrue(restartsStroke(fingerDown = true, moved = true))
    }

    @Test
    fun aStrokeIsNotStartedAgainForAFingerThatIsUpOrStill() {
        assertFalse(restartsStroke(fingerDown = false, moved = true))
        // A release where the finger was is no swipe: making it would be a tap.
        assertFalse(restartsStroke(fingerDown = true, moved = false))
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
