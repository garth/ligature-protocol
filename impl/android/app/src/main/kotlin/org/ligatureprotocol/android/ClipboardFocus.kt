package org.ligatureprotocol.android

import android.content.Context
import android.graphics.PixelFormat
import android.os.Handler
import android.os.Looper
import android.util.Log
import android.view.Gravity
import android.view.View
import android.view.WindowManager

/**
 * Tells the clipboard changes the node made from the user's copies. The
 * listener fires for both and, in the background, cannot read which it was.
 */
class ClipEchoGuard {
    private val expected = ArrayDeque<Long>()

    /** The node is about to set the clipboard. */
    @Synchronized
    fun expect(nowMs: Long) {
        expected.addLast(nowMs)
    }

    /** Whether the change just seen is one the node made. */
    @Synchronized
    fun isEcho(nowMs: Long): Boolean {
        while (expected.isNotEmpty() && nowMs - expected.first() > LIFETIME_MS) expected.removeFirst()
        return expected.removeFirstOrNull() != null
    }

    companion object {
        /** A set whose change has not arrived by then never will. */
        const val LIFETIME_MS = 2_000L
    }
}

/**
 * Lets [DriveService] read the clipboard: Android allows it only to the app
 * with the focused window, so the service adds an invisible window of one
 * pixel, reads when it gains focus, and takes it away. The app in front loses
 * the focus for that moment.
 */
object ClipboardFocus {
    private const val TIMEOUT_MS = 1_000L
    private val mainThread = Handler(Looper.getMainLooper())
    private var window: View? = null

    /**
     * Runs [read] with the focus held; on the main thread, from the service's
     * own [context], which is what may add this kind of window. A change while
     * the window waits for focus is covered by the read it is waiting for.
     */
    fun read(context: Context, read: () -> Unit) {
        if (window != null) return
        val manager = context.getSystemService(WindowManager::class.java)
        val params = WindowManager.LayoutParams(
            1,
            1,
            WindowManager.LayoutParams.TYPE_ACCESSIBILITY_OVERLAY,
            WindowManager.LayoutParams.FLAG_NOT_TOUCHABLE or WindowManager.LayoutParams.FLAG_NOT_TOUCH_MODAL,
            PixelFormat.TRANSPARENT,
        ).apply { gravity = Gravity.TOP or Gravity.START }
        val view = object : View(context) {
            override fun onWindowFocusChanged(hasWindowFocus: Boolean) {
                super.onWindowFocusChanged(hasWindowFocus)
                if (!hasWindowFocus || window !== this) return
                read()
                // Not from inside this callback: Android goes on using the
                // view when it returns.
                mainThread.post { remove(manager) }
            }
        }
        window = view
        runCatching { manager.addView(view, params) }
            .onFailure {
                Log.w("ligature", "could not take focus to read the clipboard", it)
                window = null
                return
            }
        mainThread.postDelayed({ if (window === view) remove(manager) }, TIMEOUT_MS)
    }

    private fun remove(manager: WindowManager) {
        val view = window ?: return
        window = null
        runCatching { manager.removeViewImmediate(view) }
    }
}
