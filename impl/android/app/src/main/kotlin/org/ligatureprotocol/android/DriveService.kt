package org.ligatureprotocol.android

import android.accessibilityservice.AccessibilityService
import android.accessibilityservice.GestureDescription
import android.content.Intent
import android.graphics.Color
import android.graphics.Path
import android.graphics.PixelFormat
import android.graphics.drawable.GradientDrawable
import android.os.Build
import android.os.Handler
import android.os.Looper
import android.os.SystemClock
import android.util.Log
import android.view.Gravity
import android.view.KeyEvent
import android.view.View
import android.view.WindowManager
import android.view.accessibility.AccessibilityEvent
import android.view.accessibility.AccessibilityManager
import org.ligatureprotocol.core.DriveAction
import org.ligatureprotocol.core.DriveKey

/**
 * What lets another device drive this phone, once the user turns it on under
 * Accessibility: Android lets an app inject input in no other way. The node
 * translates what a controller sends into [DriveAction]s and this carries
 * them out, one gesture at a time, since a new gesture cancels the last.
 */
class DriveService : AccessibilityService() {
    private val main = Handler(Looper.getMainLooper())
    private val queue = ArrayList<DriveAction>()
    private var busy = false
    private var stroke: GestureDescription.StrokeDescription? = null
    private var lastX = 0f
    private var lastY = 0f
    private var pointer: View? = null
    private var lastCopyRead = 0L

    override fun onServiceConnected() {
        current = this
        NodeHolder.setDrivable(true, Build.VERSION.SDK_INT >= Build.VERSION_CODES.TIRAMISU)
    }

    override fun onUnbind(intent: Intent?): Boolean {
        current = null
        NodeHolder.setDrivable(false, false)
        main.post {
            hidePointer()
            ScreenWake.release(WAKE_DRIVE)
        }
        return super.onUnbind(intent)
    }

    override fun onAccessibilityEvent(event: AccessibilityEvent?) {
        if (event == null ||
            !isCopyIndicator(
                event.eventType,
                event.packageName,
                event.className,
                event.text.size,
                event.contentChangeTypes,
            )
        ) {
            return
        }
        val now = SystemClock.uptimeMillis()
        if (now - lastCopyRead < COPY_DEBOUNCE_MS) return
        if (NodeHolder.isOwnClipChange()) {
            Log.d("ligature", "the copy indicator, for a clip the node set")
            return
        }
        Log.d("ligature", "the copy indicator: reading the clipboard")
        readForCopy()
    }

    /**
     * Reads the clipboard for a copy just seen, then again once the indicator
     * would have gone: a copy while it is up raises no event, so a read that
     * finds something new is followed by another, until one finds nothing.
     */
    private fun readForCopy(followUp: Int = 0) {
        main.removeCallbacks(followUpRead)
        followUps = followUp
        lastCopyRead = SystemClock.uptimeMillis()
        ClipboardFocus.read(this) {
            val changed = NodeHolder.readAndSend(this, automatic = true)
            if (followsUp(changed, followUps)) main.postDelayed(followUpRead, indicatorLifetimeMs())
        }
    }

    private val followUpRead = Runnable {
        Log.d("ligature", "reading the clipboard again, for a copy the indicator hid")
        readForCopy(followUps + 1)
    }

    private var followUps = 0

    /** How long SystemUI keeps its indicator up after the last copy, and a margin. */
    private fun indicatorLifetimeMs(): Long =
        getSystemService(AccessibilityManager::class.java).getRecommendedTimeoutMillis(
            INDICATOR_MS,
            AccessibilityManager.FLAG_CONTENT_TEXT or AccessibilityManager.FLAG_CONTENT_CONTROLS,
        ).toLong() + INDICATOR_MARGIN_MS

    override fun onInterrupt() = Unit

    private fun perform(action: DriveAction) {
        when (action) {
            is DriveAction.Begin -> {
                ScreenWake.hold(this, WAKE_DRIVE)
                movePointer(action.x, action.y)
            }
            is DriveAction.Pointer -> movePointer(action.x, action.y)
            is DriveAction.End -> {
                hidePointer()
                ScreenWake.release(WAKE_DRIVE)
            }
            is DriveAction.Back -> performGlobalAction(GLOBAL_ACTION_BACK)
            is DriveAction.Home -> performGlobalAction(GLOBAL_ACTION_HOME)
            is DriveAction.Text -> commit(action.text)
            is DriveAction.Key -> {
                press(action)
                // The app copies when it handles the key: read once it has,
                // whatever the indicator does.
                if (copies(action)) {
                    lastCopyRead = SystemClock.uptimeMillis() + SHORTCUT_SETTLE_MS
                    main.postDelayed({ readForCopy() }, SHORTCUT_SETTLE_MS)
                }
            }
            is DriveAction.TouchDown -> gesture(action, action.x, action.y)
            is DriveAction.TouchMove -> gesture(action, action.x, action.y)
            is DriveAction.TouchUp -> gesture(action, action.x, action.y)
            is DriveAction.Swipe -> gesture(action, action.x, action.y)
        }
    }

    private fun gesture(action: DriveAction, x: Double, y: Double) {
        movePointer(x, y)
        coalesceDrive(queue, action)
        pump()
    }

    private fun pump() {
        if (busy) return
        val action = queue.removeFirstOrNull() ?: return
        val next = when (action) {
            is DriveAction.TouchDown -> {
                lastX = action.x.toFloat()
                lastY = action.y.toFloat()
                GestureDescription.StrokeDescription(line(lastX, lastY, lastX, lastY), 0, 1, true)
            }
            is DriveAction.TouchMove -> continued(action.x, action.y, MOVE_MS, true)
            is DriveAction.TouchUp -> continued(action.x, action.y, 1, false)
            is DriveAction.Swipe -> {
                val (w, h) = NodeHolder.screenPixels(this)
                val endX = (action.x + action.dx).coerceIn(0.0, w - 1.0).toFloat()
                val endY = (action.y + action.dy).coerceIn(0.0, h - 1.0).toFloat()
                GestureDescription.StrokeDescription(
                    line(action.x.toFloat(), action.y.toFloat(), endX, endY),
                    0,
                    SWIPE_MS,
                )
            }
            else -> null
        }
        if (next == null) {
            pump()
            return
        }
        stroke = next.takeIf { it.willContinue() }
        busy = true
        val sent = runCatching {
            dispatchGesture(
                GestureDescription.Builder().addStroke(next).build(),
                object : GestureResultCallback() {
                    override fun onCompleted(gestureDescription: GestureDescription?) = finished()

                    override fun onCancelled(gestureDescription: GestureDescription?) {
                        // The user touched the screen, or the system ended it:
                        // the finger is gone, so what continued it has no use.
                        stroke = null
                        finished()
                    }
                },
                main,
            )
        }.getOrDefault(false)
        if (!sent) {
            stroke = null
            finished()
        }
    }

    private fun finished() {
        busy = false
        pump()
    }

    private fun continued(x: Double, y: Double, ms: Long, more: Boolean): GestureDescription.StrokeDescription? {
        val from = stroke ?: return null
        val toX = x.toFloat()
        val toY = y.toFloat()
        val path = line(lastX, lastY, toX, toY)
        lastX = toX
        lastY = toY
        return from.continueStroke(path, 0, ms, more)
    }

    private fun line(fromX: Float, fromY: Float, toX: Float, toY: Float) = Path().apply {
        moveTo(fromX, fromY)
        lineTo(toX, toY)
    }

    private fun commit(text: String) {
        val connection = connection() ?: return
        connection.commitText(text, 1, null)
    }

    private fun press(action: DriveAction.Key) {
        val code = keyCode(action.key) ?: return
        val connection = connection() ?: return
        var meta = 0
        if (action.shift) meta = meta or KeyEvent.META_SHIFT_ON or KeyEvent.META_SHIFT_LEFT_ON
        if (action.control) meta = meta or KeyEvent.META_CTRL_ON or KeyEvent.META_CTRL_LEFT_ON
        if (action.alt) meta = meta or KeyEvent.META_ALT_ON or KeyEvent.META_ALT_LEFT_ON
        val now = SystemClock.uptimeMillis()
        connection.sendKeyEvent(KeyEvent(now, now, KeyEvent.ACTION_DOWN, code, 0, meta))
        connection.sendKeyEvent(KeyEvent(now, now, KeyEvent.ACTION_UP, code, 0, meta))
    }

    private fun connection() = if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.TIRAMISU) {
        inputMethod?.currentInputConnection.also {
            if (it == null) Log.d("ligature", "a key for the phone, with no text field focused")
        }
    } else {
        null
    }

    private fun movePointer(x: Double, y: Double) {
        val manager = getSystemService(WindowManager::class.java)
        val size = (POINTER_DP * resources.displayMetrics.density).toInt()
        val params = WindowManager.LayoutParams(
            size,
            size,
            WindowManager.LayoutParams.TYPE_ACCESSIBILITY_OVERLAY,
            WindowManager.LayoutParams.FLAG_NOT_FOCUSABLE or
                WindowManager.LayoutParams.FLAG_NOT_TOUCHABLE or
                WindowManager.LayoutParams.FLAG_LAYOUT_NO_LIMITS or
                WindowManager.LayoutParams.FLAG_LAYOUT_IN_SCREEN,
            PixelFormat.TRANSLUCENT,
        ).apply {
            gravity = Gravity.TOP or Gravity.START
            layoutInDisplayCutoutMode = WindowManager.LayoutParams.LAYOUT_IN_DISPLAY_CUTOUT_MODE_ALWAYS
            this.x = x.toInt() - size / 2
            this.y = y.toInt() - size / 2
        }
        runCatching {
            val shown = pointer
            if (shown == null) {
                val view = View(this).apply {
                    background = GradientDrawable().apply {
                        shape = GradientDrawable.OVAL
                        setColor(Color.argb(160, 255, 255, 255))
                        setStroke((2 * resources.displayMetrics.density).toInt(), Color.argb(200, 0, 0, 0))
                    }
                }
                manager.addView(view, params)
                pointer = view
            } else {
                manager.updateViewLayout(shown, params)
            }
        }.onFailure { Log.d("ligature", "the drive's pointer", it) }
    }

    private fun hidePointer() {
        val shown = pointer ?: return
        pointer = null
        runCatching { getSystemService(WindowManager::class.java).removeViewImmediate(shown) }
    }

    companion object {
        private const val WAKE_DRIVE = "drive"
        private const val MOVE_MS = 8L
        private const val SWIPE_MS = 200L
        private const val POINTER_DP = 20
        private const val COPY_DEBOUNCE_MS = 500L

        /** SystemUI's clipboard indicator stays up this long after a copy. */
        private const val INDICATOR_MS = 6_000
        private const val INDICATOR_MARGIN_MS = 1_000L

        /** Time for an app to copy after a driven Control+C. */
        private const val SHORTCUT_SETTLE_MS = 300L

        /** Follow-up reads in one run of copies, at most. */
        const val MAX_FOLLOW_UPS = 20
        private const val SYSTEM_UI = "com.android.systemui"

        /**
         * Whether this event is SystemUI showing its clipboard indicator. Android
         * tells an app out of focus nothing else of a copy: its listener does not
         * fire. The indicator carries no text or one item; the notification shade
         * and quick settings are the same kind of window, opening with many and
         * closing with one as a pane that disappears. Only the count is looked at.
         */
        internal fun isCopyIndicator(
            type: Int,
            packageName: CharSequence?,
            className: CharSequence?,
            texts: Int,
            contentChangeTypes: Int,
        ): Boolean =
            type == AccessibilityEvent.TYPE_WINDOW_STATE_CHANGED &&
                packageName?.toString() == SYSTEM_UI &&
                className?.toString() == "android.widget.FrameLayout" &&
                texts <= 1 &&
                contentChangeTypes == 0

        /**
         * Whether a read for a copy is followed by another: the first always, in
         * case more was copied while the indicator was up, and after that only
         * while each finds something new.
         */
        internal fun followsUp(changed: Boolean, done: Int): Boolean =
            done == 0 || (changed && done < MAX_FOLLOW_UPS)

        /** Whether a driven key copies: Control+C or Control+X. */
        internal fun copies(action: DriveAction.Key): Boolean {
            val key = action.key as? DriveKey.Char ?: return false
            return action.control && !action.alt && key.text.lowercase() in setOf("c", "x")
        }

        @Volatile
        private var current: DriveService? = null

        /** Whether the user has this service on. */
        val isOn: Boolean get() = current != null

        /** Carries out one thing the node asks; from any thread. */
        fun perform(action: DriveAction) {
            val service = current ?: return
            service.main.post { service.perform(action) }
        }

        /** Android's key code for a key the node names. */
        internal fun keyCode(key: DriveKey): Int? = when (key) {
            is DriveKey.Backspace -> KeyEvent.KEYCODE_DEL
            is DriveKey.Delete -> KeyEvent.KEYCODE_FORWARD_DEL
            is DriveKey.Enter -> KeyEvent.KEYCODE_ENTER
            is DriveKey.Tab -> KeyEvent.KEYCODE_TAB
            is DriveKey.Left -> KeyEvent.KEYCODE_DPAD_LEFT
            is DriveKey.Up -> KeyEvent.KEYCODE_DPAD_UP
            is DriveKey.Right -> KeyEvent.KEYCODE_DPAD_RIGHT
            is DriveKey.Down -> KeyEvent.KEYCODE_DPAD_DOWN
            is DriveKey.Home -> KeyEvent.KEYCODE_MOVE_HOME
            is DriveKey.End -> KeyEvent.KEYCODE_MOVE_END
            is DriveKey.PageUp -> KeyEvent.KEYCODE_PAGE_UP
            is DriveKey.PageDown -> KeyEvent.KEYCODE_PAGE_DOWN
            is DriveKey.Char -> charKeyCode(key.text)
        }

        private fun charKeyCode(text: String): Int? {
            val c = text.singleOrNull()?.lowercaseChar() ?: return null
            return when (c) {
                in 'a'..'z' -> KeyEvent.KEYCODE_A + (c - 'a')
                in '0'..'9' -> KeyEvent.KEYCODE_0 + (c - '0')
                else -> null
            }
        }
    }
}
