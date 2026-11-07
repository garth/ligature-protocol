package org.ligatureprotocol.android

import android.content.Context
import android.os.PowerManager

/**
 * Keeps the screen on while another device drives or watches this phone: a
 * dark screen gives no picture and takes no touch. Android's keyguard stays
 * whatever it was; whoever drives must get past it as anyone would.
 */
object ScreenWake {
    private val reasons = HashSet<String>()
    private var lock: PowerManager.WakeLock? = null

    /** Wakes the screen and keeps it on until [release] with the same reason. */
    @Suppress("DEPRECATION") // No other wake lock lights the screen from a service.
    @Synchronized
    fun hold(context: Context, reason: String) {
        reasons += reason
        if (lock != null) return
        lock = context.applicationContext.getSystemService(PowerManager::class.java)
            .newWakeLock(
                PowerManager.SCREEN_BRIGHT_WAKE_LOCK or PowerManager.ACQUIRE_CAUSES_WAKEUP,
                "ligature:remote",
            )
            .apply {
                setReferenceCounted(false)
                acquire()
            }
    }

    /** One reason for keeping the screen on is over. */
    @Synchronized
    fun release(reason: String) {
        reasons -= reason
        if (reasons.isNotEmpty()) return
        lock?.takeIf { it.isHeld }?.release()
        lock = null
    }
}
