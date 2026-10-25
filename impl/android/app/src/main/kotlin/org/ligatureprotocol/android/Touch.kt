package org.ligatureprotocol.android

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
