package org.ligatureprotocol.android.ui

import android.app.Activity
import android.content.pm.ActivityInfo
import android.graphics.SurfaceTexture
import android.view.Surface
import android.view.TextureView
import androidx.activity.compose.BackHandler
import androidx.compose.foundation.background
import androidx.compose.foundation.gestures.awaitEachGesture
import androidx.compose.foundation.gestures.awaitFirstDown
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.BoxWithConstraints
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.aspectRatio
import androidx.compose.foundation.layout.fillMaxHeight
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.safeDrawingPadding
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedButton
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.DisposableEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clipToBounds
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.TransformOrigin
import androidx.compose.ui.graphics.graphicsLayer
import androidx.compose.ui.input.pointer.PointerEventPass
import androidx.compose.ui.input.pointer.PointerInputScope
import androidx.compose.ui.input.pointer.pointerInput
import androidx.compose.ui.input.pointer.positionChange
import androidx.compose.ui.layout.onSizeChanged
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.unit.dp
import androidx.compose.ui.viewinterop.AndroidView
import kotlin.math.abs
import kotlin.math.hypot
import org.ligatureprotocol.android.WatchDecoder
import org.ligatureprotocol.android.Zoom
import org.ligatureprotocol.core.TouchEvent

private const val BTN_LEFT: UShort = 272u
private const val TAP_SLOP_PX = 12f
private const val LONG_PRESS_MS = 400L
private const val SCROLL_STEP_PX = 40f

/**
 * A desktop's screen on the phone (milestone 8): the picture decoded into a
 * texture, and touch on it drives the desktop. A tap clicks where it lands, a
 * drag moves the pointer there, a touch held still then dragged drags with
 * the button down, two fingers scroll, a pinch zooms the picture (LP-173),
 * and the field types. A desktop wider than tall turns the screen to
 * landscape while it is watched. Leaving ends the watch and the drive.
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
    val activity = LocalContext.current as? Activity
    DisposableEffect(width > height) {
        val before = activity?.requestedOrientation
        if (width > height) activity?.requestedOrientation = ActivityInfo.SCREEN_ORIENTATION_SENSOR_LANDSCAPE
        onDispose { if (before != null) activity.requestedOrientation = before }
    }
    BoxWithConstraints(
        Modifier
            .fillMaxSize()
            .safeDrawingPadding()
            .padding(8.dp),
    ) {
        if (maxWidth > maxHeight) {
            Row(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                Picture(width, height, canDrive, onTouch, Modifier.weight(1f).fillMaxHeight())
                Column(
                    Modifier.width(220.dp).fillMaxHeight().verticalScroll(rememberScrollState()),
                    verticalArrangement = Arrangement.spacedBy(8.dp),
                ) {
                    Text("Watching $deviceName", style = MaterialTheme.typography.titleMedium)
                    TextButton(onClick = onStop) { Text("Done") }
                    Controls(deviceName, canDrive, onType)
                }
            }
        } else {
            Column(verticalArrangement = Arrangement.spacedBy(8.dp)) {
                Row(
                    Modifier.fillMaxWidth(),
                    horizontalArrangement = Arrangement.SpaceBetween,
                    verticalAlignment = Alignment.CenterVertically,
                ) {
                    Text("Watching $deviceName", style = MaterialTheme.typography.titleMedium)
                    TextButton(onClick = onStop) { Text("Done") }
                }
                Picture(width, height, canDrive, onTouch, Modifier.fillMaxWidth().weight(1f))
                Controls(deviceName, canDrive, onType)
            }
        }
    }
}

/** The keyboard field and keys, or why there are none. */
@Composable
private fun Controls(deviceName: String, canDrive: Boolean, onType: (List<TouchEvent>) -> Unit) {
    if (canDrive) {
        RemoteKeyboardField(
            label = "Type on $deviceName",
            onEvents = onType,
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

/**
 * The picture at the desktop's shape, zoomed by a pinch: a texture view, which
 * unlike a surface view is scaled and clipped with the rest of the screen.
 */
@Composable
private fun Picture(
    width: Int,
    height: Int,
    canDrive: Boolean,
    onTouch: (Double, Double, List<TouchEvent>) -> Unit,
    modifier: Modifier,
) {
    var zoom by remember { mutableStateOf(Zoom()) }
    var boxWidth by remember { mutableStateOf(1f) }
    var boxHeight by remember { mutableStateOf(1f) }
    Box(modifier.background(Color.Black), contentAlignment = Alignment.Center) {
        Box(
            Modifier
                .aspectRatio((width.toFloat() / height.toFloat()).takeIf { it.isFinite() && it > 0f } ?: 1f)
                .clipToBounds()
                .onSizeChanged {
                    boxWidth = it.width.toFloat()
                    boxHeight = it.height.toFloat()
                }
                .pointerInput(canDrive) {
                    watchGestures(
                        canDrive = canDrive,
                        fraction = { p -> zoom.fraction(p.x, p.y, boxWidth, boxHeight) },
                        onPinch = { factor, centre, pan ->
                            zoom = zoom.pinched(factor, centre.x, centre.y, pan.x, pan.y, boxWidth, boxHeight)
                        },
                        onTouch = onTouch,
                    )
                },
        ) {
            AndroidView(
                factory = { context ->
                    TextureView(context).apply {
                        surfaceTextureListener = object : TextureView.SurfaceTextureListener {
                            override fun onSurfaceTextureAvailable(texture: SurfaceTexture, w: Int, h: Int) =
                                WatchDecoder.configure(Surface(texture), width, height)

                            override fun onSurfaceTextureSizeChanged(texture: SurfaceTexture, w: Int, h: Int) {}

                            override fun onSurfaceTextureDestroyed(texture: SurfaceTexture): Boolean {
                                WatchDecoder.release()
                                return true
                            }

                            override fun onSurfaceTextureUpdated(texture: SurfaceTexture) {}
                        }
                    }
                },
                modifier = Modifier
                    .fillMaxSize()
                    .graphicsLayer {
                        transformOrigin = TransformOrigin(0f, 0f)
                        scaleX = zoom.scale
                        scaleY = zoom.scale
                        translationX = zoom.x
                        translationY = zoom.y
                    },
            )
        }
    }
}

/** A two-finger gesture's spread must change this much before it is a pinch. */
private const val PINCH_SLOP_PX = 24f

/**
 * Touch as absolute positions: every event carries where the finger is, as
 * fractions of the desktop through the zoom, so the desktop's pointer follows
 * it. Two fingers whose spread changes zoom and pan the picture; two that
 * move together scroll the desktop.
 */
private suspend fun PointerInputScope.watchGestures(
    canDrive: Boolean,
    fraction: (Offset) -> Pair<Double, Double>,
    onPinch: (factor: Float, centre: Offset, pan: Offset) -> Unit,
    onTouch: (Double, Double, List<TouchEvent>) -> Unit,
) {
    awaitEachGesture {
        val first = awaitFirstDown(requireUnconsumed = false, pass = PointerEventPass.Main)
        val start = first.position
        val startedAt = System.currentTimeMillis()
        var moved = false
        var dragging = false
        var twoFingers = false
        var pinching = false
        var scrolling = false
        var startSpan = 0f
        var lastSpan = 0f
        var lastCentre = Offset.Zero
        var scroll = Offset.Zero
        var last = start
        while (true) {
            val event = awaitPointerEvent(PointerEventPass.Main)
            val pressed = event.changes.filter { it.pressed }
            if (pressed.isEmpty()) {
                val (x, y) = fraction(last)
                when {
                    !canDrive -> {}
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
                val centre = pressed.fold(Offset.Zero) { acc, c -> acc + c.position } / pressed.size.toFloat()
                val span = pressed.map { hypot(it.position.x - centre.x, it.position.y - centre.y) }.average().toFloat()
                if (!twoFingers) {
                    twoFingers = true
                    startSpan = span
                    lastSpan = span
                    lastCentre = centre
                }
                if (!pinching && !scrolling) {
                    if (abs(span - startSpan) > PINCH_SLOP_PX) {
                        pinching = true
                    } else if ((centre - lastCentre).getDistance() > TAP_SLOP_PX) {
                        scrolling = true
                    }
                }
                if (pinching) {
                    onPinch(if (lastSpan > 0f) span / lastSpan else 1f, centre, centre - lastCentre)
                    lastSpan = span
                    lastCentre = centre
                } else if (scrolling && canDrive) {
                    val avg = pressed.fold(Offset.Zero) { acc, c -> acc + c.positionChange() } / pressed.size.toFloat()
                    scroll += avg
                    val sy = (scroll.y / SCROLL_STEP_PX).toInt()
                    val sx = (scroll.x / SCROLL_STEP_PX).toInt()
                    if (sx != 0 || sy != 0) {
                        scroll = Offset(scroll.x - sx * SCROLL_STEP_PX, scroll.y - sy * SCROLL_STEP_PX)
                        val (x, y) = fraction(last)
                        onTouch(x, y, listOf(TouchEvent.Scroll(-sx.toDouble(), -sy.toDouble())))
                    }
                }
                pressed.forEach { it.consume() }
                continue
            }
            if (twoFingers || !canDrive) {
                // The rest of a two-finger gesture, or a picture that takes no input.
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
                val (x, y) = fraction(last)
                onTouch(x, y, listOf(TouchEvent.Button(BTN_LEFT, true)))
            }
            if (moved && change.positionChange() != Offset.Zero) {
                val (x, y) = fraction(last)
                onTouch(x, y, emptyList())
            }
            change.consume()
        }
    }
}
