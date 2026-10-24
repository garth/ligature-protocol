package org.ligatureprotocol.android

import android.app.Activity
import android.app.Notification
import android.app.NotificationChannel
import android.app.NotificationManager
import android.app.PendingIntent
import android.app.Service
import android.content.BroadcastReceiver
import android.content.Context
import android.content.Intent
import android.content.pm.ServiceInfo
import android.hardware.display.DisplayManager
import android.hardware.display.VirtualDisplay
import android.media.MediaCodec
import android.media.MediaCodecInfo
import android.media.MediaCodecList
import android.media.MediaFormat
import android.media.projection.MediaProjection
import android.media.projection.MediaProjectionManager
import android.os.Bundle
import android.os.Handler
import android.os.IBinder
import android.os.Looper
import android.util.Log
import androidx.activity.ComponentActivity
import androidx.activity.result.contract.ActivityResultContracts
import androidx.core.content.ContextCompat

/**
 * The phone's screen shared with another of the person's devices (milestone
 * 8): a viewer's request becomes a notification, or the dialog itself while
 * the app is in front; the user's consent in the media projection dialog
 * starts [ScreenShareService], which encodes the screen and hands each access
 * unit to the node. Android asks every time; the app never keeps the token.
 */
object ScreenShare {
    private const val TAG = "ligature"
    const val CHANNEL_ID = "screen"
    private const val REQUEST_NOTIFICATION = 7001
    /** How long a request stays answerable: the node gives up after 60 s. */
    private const val REQUEST_LIFETIME_MS = 55_000L

    /** What the viewer asked for, until the user answers. */
    data class Request(val viewer: String, val width: Int, val height: Int, val frameRate: Int, val at: Long)

    @Volatile
    var pending: Request? = null
        private set

    /** Whether a main activity is in front, so the dialog may open at once. */
    @Volatile
    var foreground: Boolean = false

    /** Whether this phone has an H.264 encoder, which sharing needs. */
    fun hasEncoder(): Boolean = MediaCodecList(MediaCodecList.REGULAR_CODECS).codecInfos.any { info ->
        info.isEncoder && info.supportedTypes.any { it.equals(MediaFormat.MIMETYPE_VIDEO_AVC, ignoreCase = true) }
    }

    fun createChannel(context: Context) {
        context.getSystemService(NotificationManager::class.java).createNotificationChannel(
            NotificationChannel(CHANNEL_ID, "Screen sharing", NotificationManager.IMPORTANCE_HIGH).apply {
                description = "Requests to watch this phone's screen, and the share while it runs"
            },
        )
    }

    /** A viewer asked: open the dialog if the app is in front, else notify. */
    fun request(context: Context, viewer: String, width: Int, height: Int, frameRate: Int) {
        pending = Request(viewer, width, height, frameRate, System.currentTimeMillis())
        createChannel(context)
        val open = Intent(context, ScreenConsentActivity::class.java)
            .addFlags(Intent.FLAG_ACTIVITY_NEW_TASK)
        if (foreground) {
            runCatching { context.startActivity(open) }.onSuccess { return }
        }
        val tap = PendingIntent.getActivity(
            context,
            REQUEST_NOTIFICATION,
            open,
            PendingIntent.FLAG_IMMUTABLE or PendingIntent.FLAG_UPDATE_CURRENT,
        )
        val notification = Notification.Builder(context, CHANNEL_ID)
            .setSmallIcon(R.drawable.ic_tile)
            .setContentTitle("$viewer wants to see your screen")
            .setContentText("Tap to choose whether to share it")
            .setContentIntent(tap)
            .setAutoCancel(true)
            .setTimeoutAfter(REQUEST_LIFETIME_MS)
            .build()
        context.getSystemService(NotificationManager::class.java).notify(REQUEST_NOTIFICATION, notification)
    }

    /** The request, if it is still answerable; it is answered only once. */
    fun take(): Request? {
        val r = pending ?: return null
        pending = null
        return r.takeIf { System.currentTimeMillis() - it.at < REQUEST_LIFETIME_MS }
    }

    fun clearRequest(context: Context) {
        pending = null
        context.getSystemService(NotificationManager::class.java).cancel(REQUEST_NOTIFICATION)
    }

    /** The node ends the share: the viewer stopped or went away. */
    fun stop(context: Context) {
        clearRequest(context)
        context.stopService(Intent(context, ScreenShareService::class.java))
    }

    fun keyframe() {
        ScreenShareService.current?.requestKeyframe()
    }

    /** The share notification's Stop action. */
    class StopReceiver : BroadcastReceiver() {
        override fun onReceive(context: Context, intent: Intent) {
            ScreenShareService.current?.end("sharer")
        }
    }

    internal fun log(message: String) = Log.i(TAG, "screen share: $message")
}

/**
 * Transparent: opens the media projection dialog for the pending request and
 * starts the service on consent, or tells the node the user said no.
 */
class ScreenConsentActivity : ComponentActivity() {
    private lateinit var request: ScreenShare.Request

    private val consent = registerForActivityResult(ActivityResultContracts.StartActivityForResult()) { result ->
        val data = result.data
        if (result.resultCode == Activity.RESULT_OK && data != null) {
            ScreenShareService.start(this, request, result.resultCode, data)
        } else {
            ScreenShare.log("the user did not consent")
            NodeHolder.screenShareAnswered(false)
        }
        finish()
    }

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        val r = ScreenShare.take()
        if (r == null) {
            finish()
            return
        }
        request = r
        val manager = getSystemService(MediaProjectionManager::class.java)
        consent.launch(manager.createScreenCaptureIntent())
    }
}

/**
 * The share itself: a foreground service of the media projection type (which
 * Android shows with its own cast indicator), a virtual display mirrored into
 * an H.264 encoder's input surface, and a thread handing each access unit to
 * the node with the parameter sets before every keyframe.
 */
class ScreenShareService : Service() {
    private var projection: MediaProjection? = null
    private var codec: MediaCodec? = null
    private var display: VirtualDisplay? = null
    private var drain: Thread? = null
    @Volatile private var ended = false

    override fun onBind(intent: Intent?): IBinder? = null

    override fun onStartCommand(intent: Intent?, flags: Int, startId: Int): Int {
        current?.takeIf { it !== this }?.end("sharer")
        current = this
        val data = intent?.getParcelableExtra(EXTRA_DATA, Intent::class.java)
        val resultCode = intent?.getIntExtra(EXTRA_RESULT_CODE, 0) ?: 0
        val viewer = intent?.getStringExtra(EXTRA_VIEWER) ?: "another device"
        val width = intent?.getIntExtra(EXTRA_WIDTH, 0) ?: 0
        val height = intent?.getIntExtra(EXTRA_HEIGHT, 0) ?: 0
        val rate = intent?.getIntExtra(EXTRA_RATE, 30) ?: 30
        ScreenShare.createChannel(this)
        startForeground(NOTIFICATION, notification(viewer), ServiceInfo.FOREGROUND_SERVICE_TYPE_MEDIA_PROJECTION)
        if (data == null || width <= 0 || height <= 0) {
            NodeHolder.screenShareAnswered(false)
            stopSelf()
            return START_NOT_STICKY
        }
        try {
            begin(resultCode, data, width, height, rate)
            NodeHolder.screenShareAnswered(true)
        } catch (e: Exception) {
            Log.w("ligature", "screen share did not start", e)
            NodeHolder.screenShareAnswered(false)
            release()
            stopSelf()
        }
        return START_NOT_STICKY
    }

    private fun begin(resultCode: Int, data: Intent, width: Int, height: Int, rate: Int) {
        // Encoders want dimensions in multiples of 16.
        val w = (width / 16 * 16).coerceAtLeast(16)
        val h = (height / 16 * 16).coerceAtLeast(16)
        val manager = getSystemService(MediaProjectionManager::class.java)
        val p = manager.getMediaProjection(resultCode, data)
            ?: throw IllegalStateException("Android gave no media projection")
        projection = p
        p.registerCallback(
            object : MediaProjection.Callback() {
                override fun onStop() = end("consent")
            },
            Handler(Looper.getMainLooper()),
        )
        fun format(prependHeaders: Boolean) = MediaFormat.createVideoFormat(MediaFormat.MIMETYPE_VIDEO_AVC, w, h).apply {
            setInteger(MediaFormat.KEY_COLOR_FORMAT, MediaCodecInfo.CodecCapabilities.COLOR_FormatSurface)
            setInteger(MediaFormat.KEY_BIT_RATE, bitRate(w, h, rate))
            setInteger(MediaFormat.KEY_FRAME_RATE, rate)
            setInteger(MediaFormat.KEY_I_FRAME_INTERVAL, 1)
            if (prependHeaders) setInteger(MediaFormat.KEY_PREPEND_HEADER_TO_SYNC_FRAMES, 1)
            // A still screen keeps producing frames, so a late viewer sees one.
            setLong(MediaFormat.KEY_REPEAT_PREVIOUS_FRAME_AFTER, 1_000_000L / rate)
        }
        // Not every encoder repeats the parameter sets itself; the drain loop
        // puts them before each keyframe where it did not.
        var c = MediaCodec.createEncoderByType(MediaFormat.MIMETYPE_VIDEO_AVC)
        try {
            c.configure(format(true), null, null, MediaCodec.CONFIGURE_FLAG_ENCODE)
        } catch (e: IllegalArgumentException) {
            ScreenShare.log("the encoder will not repeat its headers; the app does")
            c.release()
            c = MediaCodec.createEncoderByType(MediaFormat.MIMETYPE_VIDEO_AVC)
            c.configure(format(false), null, null, MediaCodec.CONFIGURE_FLAG_ENCODE)
        }
        val surface = c.createInputSurface()
        c.start()
        codec = c
        display = p.createVirtualDisplay(
            "ligature-share",
            w,
            h,
            resources.displayMetrics.densityDpi,
            DisplayManager.VIRTUAL_DISPLAY_FLAG_AUTO_MIRROR,
            surface,
            null,
            null,
        )
        drain = Thread({ drain(c) }, "ligature-screen-encoder").apply { start() }
        ScreenShare.log("sharing at ${w}x$h, $rate fps")
    }

    private fun drain(c: MediaCodec) {
        val info = MediaCodec.BufferInfo()
        var config: ByteArray? = null
        while (!ended) {
            val index = try {
                c.dequeueOutputBuffer(info, 100_000)
            } catch (e: IllegalStateException) {
                break
            }
            if (index < 0) continue
            val buffer = c.getOutputBuffer(index)
            if (buffer != null && info.size > 0) {
                buffer.position(info.offset)
                buffer.limit(info.offset + info.size)
                val bytes = ByteArray(info.size).also { buffer.get(it) }
                if (info.flags and MediaCodec.BUFFER_FLAG_CODEC_CONFIG != 0) {
                    config = bytes
                } else {
                    val key = info.flags and MediaCodec.BUFFER_FLAG_KEY_FRAME != 0
                    // The parameter sets before every keyframe, where the
                    // encoder did not put them there itself.
                    val out = if (key && config != null && !startsWithSps(bytes)) config!! + bytes else bytes
                    NodeHolder.screenShareFrame(key, out)
                }
            }
            runCatching { c.releaseOutputBuffer(index, false) }
            if (info.flags and MediaCodec.BUFFER_FLAG_END_OF_STREAM != 0) break
        }
    }

    fun requestKeyframe() {
        codec?.setParameters(Bundle().apply { putInt(MediaCodec.PARAMETER_KEY_REQUEST_SYNC_FRAME, 0) })
    }

    /** The share ends here: `consent` when Android stopped the projection. */
    fun end(reason: String) {
        if (ended) return
        ScreenShare.log("ended ($reason)")
        NodeHolder.screenShareEnded(reason)
        stopSelf()
    }

    override fun onDestroy() {
        if (!ended) NodeHolder.screenShareEnded("sharer")
        release()
        if (current === this) current = null
        super.onDestroy()
    }

    private fun release() {
        ended = true
        runCatching { display?.release() }
        runCatching { codec?.stop() }
        runCatching { codec?.release() }
        runCatching { projection?.stop() }
        runCatching { drain?.join(500) }
        display = null
        codec = null
        projection = null
    }

    private fun notification(viewer: String): Notification {
        val stop = PendingIntent.getBroadcast(
            this,
            0,
            Intent(this, ScreenShare.StopReceiver::class.java),
            PendingIntent.FLAG_IMMUTABLE,
        )
        return Notification.Builder(this, ScreenShare.CHANNEL_ID)
            .setSmallIcon(R.drawable.ic_tile)
            .setContentTitle("Sharing your screen with $viewer")
            .setOngoing(true)
            .addAction(Notification.Action.Builder(null, "Stop sharing", stop).build())
            .build()
    }

    companion object {
        private const val NOTIFICATION = 7002
        private const val EXTRA_RESULT_CODE = "resultCode"
        private const val EXTRA_DATA = "data"
        private const val EXTRA_VIEWER = "viewer"
        private const val EXTRA_WIDTH = "width"
        private const val EXTRA_HEIGHT = "height"
        private const val EXTRA_RATE = "rate"

        @Volatile
        var current: ScreenShareService? = null
            private set

        fun start(context: Context, request: ScreenShare.Request, resultCode: Int, data: Intent) {
            val intent = Intent(context, ScreenShareService::class.java)
                .putExtra(EXTRA_RESULT_CODE, resultCode)
                .putExtra(EXTRA_DATA, data)
                .putExtra(EXTRA_VIEWER, request.viewer)
                .putExtra(EXTRA_WIDTH, request.width)
                .putExtra(EXTRA_HEIGHT, request.height)
                .putExtra(EXTRA_RATE, request.frameRate)
            ContextCompat.startForegroundService(context, intent)
        }

        /** About 0.1 bit per pixel per frame, between 1 and 12 Mbit/s. */
        fun bitRate(width: Int, height: Int, rate: Int): Int =
            (width.toLong() * height * rate / 10).coerceIn(1_000_000, 12_000_000).toInt()

        /** Whether an access unit starts with a sequence parameter set. */
        fun startsWithSps(bytes: ByteArray): Boolean {
            var i = 0
            while (i + 3 < bytes.size && i < 64) {
                if (bytes[i] == 0.toByte() && bytes[i + 1] == 0.toByte() && bytes[i + 2] == 1.toByte()) {
                    return (bytes[i + 3].toInt() and 0x1f) == 7
                }
                i++
            }
            return false
        }
    }
}
