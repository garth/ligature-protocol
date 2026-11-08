package org.ligatureprotocol.android

import org.ligatureprotocol.core.DriveAction
import org.ligatureprotocol.core.TouchEvent

/**
 * Touch events waiting to be sent, made into one batch: consecutive motion is
 * summed and consecutive positions keep only the newest, so the pointer goes
 * where the finger is now rather than replaying where it was. Everything
 * else (buttons, scroll, keys, text) keeps its place and order.
 */
internal fun coalesceTouch(events: List<TouchEvent>): List<TouchEvent> {
    val out = ArrayList<TouchEvent>(events.size)
    for (e in events) {
        val last = out.lastOrNull()
        when {
            e is TouchEvent.Position && last is TouchEvent.Position -> out[out.size - 1] = e
            e is TouchEvent.Motion && last is TouchEvent.Motion ->
                out[out.size - 1] = TouchEvent.Motion(last.dx + e.dx, last.dy + e.dy)
            else -> out.add(e)
        }
    }
    return out
}

/**
 * Adds [action] to the gestures waiting for the one under way: only the newest
 * move of a run is worth making, and swipes in a row add up, so a fast pointer
 * or wheel never leaves the finger behind.
 */
internal fun coalesceDrive(queue: MutableList<DriveAction>, action: DriveAction) {
    val last = queue.lastOrNull()
    when {
        action is DriveAction.TouchMove && last is DriveAction.TouchMove -> queue[queue.size - 1] = action
        action is DriveAction.Swipe && last is DriveAction.Swipe ->
            queue[queue.size - 1] = DriveAction.Swipe(last.x, last.y, last.dx + action.dx, last.dy + action.dy)
        else -> queue.add(action)
    }
}

/**
 * Whether a finger's move from one point to another goes nowhere, within half
 * a pixel. Such a move is not made: Android leaves a continued stroke of no
 * length unfinished, and the release after it then has nothing to continue,
 * so the finger stays down and a click becomes a long press (LP-167). A Mac
 * trackpad reports the same point again just before a release.
 */
internal fun goesNowhere(fromX: Float, fromY: Float, toX: Float, toY: Float): Boolean {
    val dx = toX - fromX
    val dy = toY - fromY
    return dx * dx + dy * dy < 0.25f
}

/**
 * Whether a stroke Android ended is started again: only for a finger the
 * remote button still holds down, and only when it moves, release included.
 * SystemUI cancels the stroke under way as the keyguard's bouncer appears in
 * the middle of a swipe; without the rest of the swipe and its release the
 * bouncer stays half open and the next tap closes it (LP-167). A release where
 * the finger already was is not made, since that would be a tap.
 */
internal fun restartsStroke(fingerDown: Boolean, moved: Boolean): Boolean = fingerDown && moved

