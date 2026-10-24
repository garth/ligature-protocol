package org.ligatureprotocol.android.ui

import androidx.activity.compose.BackHandler
import androidx.compose.foundation.background
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.safeDrawingPadding
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedButton
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.input.pointer.pointerInput
import androidx.compose.ui.input.pointer.positionChange
import androidx.compose.ui.unit.dp
import androidx.compose.ui.input.pointer.PointerEventPass
import androidx.compose.foundation.gestures.awaitEachGesture
import androidx.compose.foundation.gestures.awaitFirstDown
import org.ligatureprotocol.core.TouchEvent
import kotlin.math.abs

/** The left mouse button's evdev code. */
private const val BTN_LEFT: UShort = 272u

/** Backspace and Enter, by evdev code. */
private const val KEY_BACKSPACE: UShort = 14u
private const val KEY_ENTER: UShort = 28u

/** A touch that moves less than this is a tap. */
private const val TAP_SLOP_PX = 12f

/** A press held this long without moving starts a drag. */
private const val LONG_PRESS_MS = 400L

/** Finger pixels per wheel step when two fingers scroll. */
private const val SCROLL_STEP_PX = 40f

/**
 * The phone as a touchpad and keyboard for one desktop (milestone 7): one
 * finger moves, a tap clicks, a long press drags, two fingers scroll, the
 * field types. Leaving the screen ends the drive.
 */
@Composable
fun TouchpadScreen(
    deviceName: String,
    onEvents: (List<TouchEvent>) -> Unit,
    onStop: () -> Unit,
) {
    BackHandler(onBack = onStop)
    var typed by remember { mutableStateOf("") }
    Column(
        modifier = Modifier
            .fillMaxSize()
            .safeDrawingPadding()
            .padding(16.dp),
        verticalArrangement = Arrangement.spacedBy(12.dp),
    ) {
        Row(Modifier.fillMaxWidth(), horizontalArrangement = Arrangement.SpaceBetween, verticalAlignment = Alignment.CenterVertically) {
            Text("Driving $deviceName", style = MaterialTheme.typography.titleMedium)
            TextButton(onClick = onStop) { Text("Done") }
        }
        Box(
            modifier = Modifier
                .fillMaxWidth()
                .weight(1f)
                .background(MaterialTheme.colorScheme.surfaceVariant, RoundedCornerShape(16.dp))
                .pointerInput(Unit) { touchpad(onEvents) },
            contentAlignment = Alignment.Center,
        ) {
            Text(
                "Move with one finger, tap to click, hold to drag,\ntwo fingers to scroll.",
                style = MaterialTheme.typography.bodyMedium,
                color = MaterialTheme.colorScheme.onSurfaceVariant,
            )
        }
        OutlinedTextField(
            value = typed,
            onValueChange = { value ->
                // What the keyboard produced goes straight to the desktop;
                // the field never keeps it, so each change is new text.
                if (value.length > typed.length) {
                    onEvents(listOf(TouchEvent.Text(value.substring(typed.length))))
                } else if (value.length < typed.length) {
                    onEvents(press(KEY_BACKSPACE))
                }
                typed = ""
            },
            label = { Text("Type on $deviceName") },
            singleLine = true,
            modifier = Modifier.fillMaxWidth(),
        )
        Row(Modifier.fillMaxWidth(), horizontalArrangement = Arrangement.spacedBy(8.dp)) {
            OutlinedButton(onClick = { onEvents(press(KEY_BACKSPACE)) }) { Text("Backspace") }
            OutlinedButton(onClick = { onEvents(press(KEY_ENTER)) }) { Text("Enter") }
            OutlinedButton(onClick = { onEvents(click(273u)) }) { Text("Right click") }
        }
    }
}

private fun press(code: UShort): List<TouchEvent> =
    listOf(TouchEvent.Key(code, true), TouchEvent.Key(code, false))

private fun click(code: UShort): List<TouchEvent> =
    listOf(TouchEvent.Button(code, true), TouchEvent.Button(code, false))

/**
 * The gesture loop: one finger reports relative motion, a short still touch
 * is a click, a long still touch presses the button until the finger lifts
 * (a drag), and two fingers scroll by their average vertical and horizontal
 * movement in wheel steps.
 */
private suspend fun androidx.compose.ui.input.pointer.PointerInputScope.touchpad(
    onEvents: (List<TouchEvent>) -> Unit,
) {
    awaitEachGesture {
        val first = awaitFirstDown(requireUnconsumed = false, pass = PointerEventPass.Main)
        val start = first.position
        val startedAt = System.currentTimeMillis()
        var moved = false
        var dragging = false
        var twoFingers = false
        var scrollRemainder = Offset.Zero
        var lastCount = 1
        while (true) {
            val event = awaitPointerEvent(PointerEventPass.Main)
            val pressed = event.changes.filter { it.pressed }
            if (pressed.isEmpty()) {
                // Everything lifted.
                val change = event.changes.firstOrNull()
                val held = System.currentTimeMillis() - startedAt
                when {
                    dragging -> onEvents(listOf(TouchEvent.Button(BTN_LEFT, false)))
                    !moved && !twoFingers && held < LONG_PRESS_MS -> onEvents(click(BTN_LEFT))
                }
                change?.consume()
                break
            }
            if (pressed.size >= 2) {
                twoFingers = true
                // Two fingers: scroll by the average movement.
                val avg = pressed.fold(Offset.Zero) { acc, c -> acc + c.positionChange() } / pressed.size.toFloat()
                scrollRemainder += avg
                val stepsY = (scrollRemainder.y / SCROLL_STEP_PX).toInt()
                val stepsX = (scrollRemainder.x / SCROLL_STEP_PX).toInt()
                if (stepsY != 0 || stepsX != 0) {
                    scrollRemainder = Offset(
                        scrollRemainder.x - stepsX * SCROLL_STEP_PX,
                        scrollRemainder.y - stepsY * SCROLL_STEP_PX,
                    )
                    // Fingers moving down scroll the page up, as a touchpad does.
                    onEvents(listOf(TouchEvent.Scroll(-stepsX.toDouble(), -stepsY.toDouble())))
                }
                pressed.forEach { it.consume() }
                lastCount = pressed.size
                continue
            }
            if (lastCount >= 2) {
                // Back to one finger after a scroll: no click on lift.
                moved = true
                lastCount = 1
                continue
            }
            val change = pressed.first()
            val delta = change.positionChange()
            if (!moved && (abs(change.position.x - start.x) > TAP_SLOP_PX || abs(change.position.y - start.y) > TAP_SLOP_PX)) {
                moved = true
            }
            if (!moved && !dragging && System.currentTimeMillis() - startedAt >= LONG_PRESS_MS) {
                dragging = true
                onEvents(listOf(TouchEvent.Button(BTN_LEFT, true)))
            }
            if (delta != Offset.Zero && (moved || dragging)) {
                onEvents(listOf(TouchEvent.Motion(delta.x.toDouble(), delta.y.toDouble())))
            }
            change.consume()
        }
    }
}
