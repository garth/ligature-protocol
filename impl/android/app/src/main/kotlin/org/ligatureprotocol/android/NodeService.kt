package org.ligatureprotocol.android

import android.app.Notification
import android.app.PendingIntent
import android.app.Service
import android.content.ClipboardManager
import android.content.Context
import android.content.Intent
import android.content.pm.ServiceInfo
import android.net.ConnectivityManager
import android.net.Network
import android.net.wifi.WifiManager
import android.os.Handler
import android.os.IBinder
import android.os.Looper
import android.util.Log
import androidx.core.content.ContextCompat

/**
 * Keeps the node alive: a foreground service of type `connectedDevice` that
 * holds the Wi-Fi multicast lock (mDNS needs it) and starts the node from the
 * app's files directory. Receiving the clipboard needs nothing more: the
 * node sets the primary clip from here. Sending is automatic only when the
 * background-read permission is held; otherwise the notification's action,
 * the quick-settings tile and the share sheet send it.
 */
class NodeService : Service() {
    private var multicastLock: WifiManager.MulticastLock? = null
    private var clipboard: ClipboardManager? = null
    private var connectivity: ConnectivityManager? = null
    private val mainThread = Handler(Looper.getMainLooper())

    /**
     * The node starts on the first network and restarts discovery on every
     * later one: at boot the service runs before Wi-Fi has associated, and
     * an mDNS responder started then never announces or hears anything.
     */
    private val networkCallback = object : ConnectivityManager.NetworkCallback() {
        override fun onAvailable(network: Network) {
            mainThread.post { networkUp() }
        }
    }

    private fun networkUp() {
        if (NodeHolder.isRunning || starting) {
            NodeHolder.restartDiscovery()
            return
        }
        mainThread.removeCallbacks(retryStart)
        // Opening the store (with its verification pass) and binding the
        // sockets is disk and network work: off the main thread (LP-127).
        starting = true
        NodeHolder.async {
            val started = try {
                NodeHolder.start(this)
                true
            } catch (e: Exception) {
                Log.e("ligature", "node failed to start", e)
                false
            }
            mainThread.post {
                starting = false
                if (!started) {
                    // Nobody may open the app to start it again: keep trying,
                    // and a new network tries at once (networkUp).
                    mainThread.postDelayed(retryStart, retryDelayMs(failedStarts++))
                    return@post
                }
                failedStarts = 0
                clipboard = getSystemService(ClipboardManager::class.java).also {
                    it.addPrimaryClipChangedListener(clipListener)
                }
            }
        }
    }

    private var starting = false
    private var failedStarts = 0
    private val retryStart = Runnable { networkUp() }

    private val clipListener = ClipboardManager.OnPrimaryClipChangedListener {
        // Fires only when Android lets this app read: in the foreground, or
        // with READ_CLIPBOARD_IN_BACKGROUND on a platform-signed build.
        NodeHolder.readAndSend(this)
    }

    override fun onCreate() {
        super.onCreate()
        startForeground(
            NOTIFICATION_ID,
            notification(),
            ServiceInfo.FOREGROUND_SERVICE_TYPE_CONNECTED_DEVICE,
        )
        val wifi = applicationContext.getSystemService(WifiManager::class.java)
        multicastLock = wifi.createMulticastLock("ligature").apply {
            setReferenceCounted(false)
            acquire()
        }
        connectivity = getSystemService(ConnectivityManager::class.java).also {
            // Reports the current default network at once, if there is one,
            // and every change after.
            it.registerDefaultNetworkCallback(networkCallback)
        }
    }

    override fun onStartCommand(intent: Intent?, flags: Int, startId: Int): Int = START_STICKY

    override fun onDestroy() {
        mainThread.removeCallbacks(retryStart)
        connectivity?.unregisterNetworkCallback(networkCallback)
        clipboard?.removePrimaryClipChangedListener(clipListener)
        // The node's shutdown waits up to two seconds for its tasks: not on
        // the main thread.
        NodeHolder.async { NodeHolder.stop() }
        multicastLock?.release()
        super.onDestroy()
    }

    override fun onBind(intent: Intent?): IBinder? = null

    private fun notification(): Notification {
        val open = PendingIntent.getActivity(
            this,
            0,
            Intent(this, MainActivity::class.java),
            PendingIntent.FLAG_IMMUTABLE or PendingIntent.FLAG_UPDATE_CURRENT,
        )
        val send = PendingIntent.getActivity(
            this,
            1,
            Intent(this, SendClipboardActivity::class.java).addFlags(Intent.FLAG_ACTIVITY_NEW_TASK),
            PendingIntent.FLAG_IMMUTABLE or PendingIntent.FLAG_UPDATE_CURRENT,
        )
        return Notification.Builder(this, CHANNEL_ID)
            .setSmallIcon(R.drawable.ic_tile)
            .setContentTitle(getString(R.string.notification_title))
            .setContentIntent(open)
            .setCategory(Notification.CATEGORY_SERVICE)
            .setShowWhen(false)
            .setVisibility(Notification.VISIBILITY_SECRET)
            .addAction(
                Notification.Action.Builder(null, getString(R.string.notification_action_send), send).build(),
            )
            .build()
    }

    companion object {
        /** The service's channel, at the lowest importance Android allows it. */
        const val CHANNEL_ID = "node-quiet"

        /** The channel before LP-165: an app cannot lower a channel it made. */
        const val OLD_CHANNEL_ID = "node"
        private const val NOTIFICATION_ID = 1
        private const val FIRST_RETRY_MS = 5_000L
        private const val LONGEST_RETRY_MS = 300_000L

        /** How long to wait after [failures] failed starts in a row, for ever. */
        fun retryDelayMs(failures: Int): Long =
            if (failures >= 6) LONGEST_RETRY_MS else minOf(FIRST_RETRY_MS shl failures, LONGEST_RETRY_MS)

        fun start(context: Context) {
            ContextCompat.startForegroundService(context, Intent(context, NodeService::class.java))
        }

        fun stop(context: Context) {
            context.stopService(Intent(context, NodeService::class.java))
        }
    }
}
