package org.ligatureprotocol.android.ui

import android.view.SurfaceHolder
import android.view.SurfaceView
import androidx.activity.compose.BackHandler
import androidx.compose.foundation.background
import androidx.compose.foundation.gestures.awaitEachGesture
import androidx.compose.foundation.gestures.awaitFirstDown
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.aspectRatio
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.safeDrawingPadding
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
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.input.pointer.PointerEventPass
import androidx.compose.ui.input.pointer.PointerInputScope
import androidx.compose.ui.input.pointer.pointerInput
import androidx.compose.ui.input.pointer.positionChange
import androidx.compose.ui.unit.dp
import androidx.compose.ui.viewinterop.AndroidView
import org.ligatureprotocol.android.WatchDecoder
import org.ligatureprotocol.core.TouchEvent
import kotlin.math.abs

private const val BTN_LEFT: UShort = 272u
private const val KEY_BACKSPACE: UShort = 14u
private const val KEY_ENTER: UShort = 28u
private const val TAP_SLOP_PX = 12f
private const val LONG_PRESS_MS = 400L
private const val SCROLL_STEP_PX = 40f

/**
 * A desktop's screen on the phone (milestone 8): the picture decoded into a
 * surface, and touch on it drives the desktop. A tap clicks where it lands, a
 * drag moves the pointer there, a touch held still then dragged drags with
 * the button down, two fingers scroll, and the field types. Leaving ends the
 * watch and the drive.
 */
@Composable
fun WatchScreen(
    deviceName: String,
    width: Int,
    height: Int,
    canDrive: Boolean,
    onTouch: (x: Double, y: Double, events: List<TouchEvent>) -> Unit,
    onType: (List<TouchEvent>) -> Unit,
    onStop: () -> Unit,
) {
    BackHandler(onBack = onStop)
    var typed by remember { mutableStateOf("") }
    Column(
        modifier = Modifier
            .fillMaxSize()
            .safeDrawingPadding()
            .padding(8.dp),
        verticalArrangement = Arrangement.spacedBy(8.dp),
    ) {
        Row(Modifier.fillMaxWidth(), horizontalArrangement = Arrangement.SpaceBetween, verticalAlignment = Alignment.CenterVertically) {
            Text("Watching $deviceName", style = MaterialTheme.typography.titleMedium)
            TextButton(onClick = onStop) { Text("Done") }
        }
        Box(
            Modifier
                .fillMaxWidth()
                .weight(1f)
                .background(Color.Black),
            contentAlignment = Alignment.Center,
        ) {
            Box(
                Modifier
                    .aspectRatio(width.toFloat() / height.toFloat())
                    .then(if (canDrive) Modifier.pointerInput(Unit) { watchGestures(onTouch) } else Modifier),
            ) {
                AndroidView(
                    factory = { context ->
                        SurfaceView(context).apply {
                            holder.addCallback(
                                object : SurfaceHolder.Callback {
                                    override fun surfaceCreated(holder: SurfaceHolder) =
                                        WatchDecoder.configure(holder.surface, width, height)

                                    override fun surfaceChanged(holder: SurfaceHolder, format: Int, w: Int, h: Int) {}

                                    override fun surfaceDestroyed(holder: SurfaceHolder) = WatchDecoder.release()
                                },
                            )
                        }
                    },
                    modifier = Modifier.fillMaxSize(),
                )
            }
        }
        if (canDrive) {
            OutlinedTextField(
                value = typed,
                onValueChange = { value ->
                    if (value.length > typed.length) {
                        onType(listOf(TouchEvent.Text(value.substring(typed.length))))
                    } else if (value.length < typed.length) {
                        onType(listOf(TouchEvent.Key(KEY_BACKSPACE, true), TouchEvent.Key(KEY_BACKSPACE, false)))
                    }
                    typed = ""
                },
                label = { Text("Type on $deviceName") },
                singleLine = true,
                modifier = Modifier.fillMaxWidth(),
            )
            Row(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                OutlinedButton(onClick = {
                    onType(listOf(TouchEvent.Key(KEY_BACKSPACE, true), TouchEvent.Key(KEY_BACKSPACE, false)))
                }) { Text("Backspace") }
                OutlinedButton(onClick = {
                    onType(listOf(TouchEvent.Key(KEY_ENTER, true), TouchEvent.Key(KEY_ENTER, false)))
                }) { Text("Enter") }
            }
        } else {
            Text("This device does not take input from the phone.", style = MaterialTheme.typography.bodySmall)
        }
    }
}

/**
 * Touch as absolute positions: every event carries where the finger is, as
 * fractions of the picture, so the desktop's pointer follows it.
 */
private suspend fun PointerInputScope.watchGestures(onTouch: (Double, Double, List<TouchEvent>) -> Unit) {
    fun frac(p: Offset): Pair<Double, Double> =
        Pair((p.x / size.width).toDouble().coerceIn(0.0, 1.0), (p.y / size.height).toDouble().coerceIn(0.0, 1.0))
    awaitEachGesture {
        val first = awaitFirstDown(requireUnconsumed = false, pass = PointerEventPass.Main)
        val start = first.position
        val startedAt = System.currentTimeMillis()
        var moved = false
        var dragging = false
        var twoFingers = false
        var scroll = Offset.Zero
        var last = start
        while (true) {
            val event = awaitPointerEvent(PointerEventPass.Main)
            val pressed = event.changes.filter { it.pressed }
            if (pressed.isEmpty()) {
                val (x, y) = frac(last)
                when {
                    dragging -> onTouch(x, y, listOf(TouchEvent.Button(BTN_LEFT, false)))
                    !moved && !twoFingers -> onTouch(
                        x,
                        y,
                        listOf(TouchEvent.Button(BTN_LEFT, true), TouchEvent.Button(BTN_LEFT, false)),
                    )
                }
                event.changes.forEach { it.consume() }
                break
            }
            if (pressed.size >= 2) {
                twoFingers = true
                val avg = pressed.fold(Offset.Zero) { acc, c -> acc + c.positionChange() } / pressed.size.toFloat()
                scroll += avg
                val sy = (scroll.y / SCROLL_STEP_PX).toInt()
                val sx = (scroll.x / SCROLL_STEP_PX).toInt()
                if (sx != 0 || sy != 0) {
                    scroll = Offset(scroll.x - sx * SCROLL_STEP_PX, scroll.y - sy * SCROLL_STEP_PX)
                    val (x, y) = frac(last)
                    onTouch(x, y, listOf(TouchEvent.Scroll(-sx.toDouble(), -sy.toDouble())))
                }
                pressed.forEach { it.consume() }
                continue
            }
            val change = pressed.first()
            last = change.position
            if (!moved && (abs(last.x - start.x) > TAP_SLOP_PX || abs(last.y - start.y) > TAP_SLOP_PX)) {
                moved = true
            }
            if (!moved && !dragging && System.currentTimeMillis() - startedAt >= LONG_PRESS_MS) {
                dragging = true
                val (x, y) = frac(last)
                onTouch(x, y, listOf(TouchEvent.Button(BTN_LEFT, true)))
            }
            if (moved && change.positionChange() != Offset.Zero) {
                val (x, y) = frac(last)
                onTouch(x, y, emptyList())
            }
            change.consume()
        }
    }
}
