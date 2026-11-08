package org.ligatureprotocol.android

/**
 * How a watched desktop's picture is zoomed in its box (LP-173): scaled by
 * [scale] from its top left corner and moved by ([x], [y]) pixels, never
 * smaller than the box and never leaving a gap at an edge.
 */
data class Zoom(val scale: Float = 1f, val x: Float = 0f, val y: Float = 0f) {
    /**
     * This zoom after one step of a pinch: the scale times [factor] about the
     * fingers' centre ([cx], [cy]), and the picture moved by the centre's own
     * movement ([dx], [dy]), in a box of [width] by [height].
     */
    fun pinched(factor: Float, cx: Float, cy: Float, dx: Float, dy: Float, width: Float, height: Float): Zoom {
        val next = (scale * factor).coerceIn(1f, MAX_SCALE)
        val ratio = next / scale
        val nx = cx - (cx - x) * ratio + dx
        val ny = cy - (cy - y) * ratio + dy
        return Zoom(
            next,
            nx.coerceIn(width * (1 - next), 0f),
            ny.coerceIn(height * (1 - next), 0f),
        )
    }

    /** Where a touch at ([px], [py]) in the box lands on the desktop, as fractions. */
    fun fraction(px: Float, py: Float, width: Float, height: Float): Pair<Double, Double> = Pair(
        ((px - x) / scale / width).toDouble().coerceIn(0.0, 1.0),
        ((py - y) / scale / height).toDouble().coerceIn(0.0, 1.0),
    )

    companion object {
        /** The closest a picture is zoomed. */
        const val MAX_SCALE = 5f
    }
}
