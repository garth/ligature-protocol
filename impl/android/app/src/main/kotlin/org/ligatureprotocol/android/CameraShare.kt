package org.ligatureprotocol.android

import android.Manifest
import android.annotation.SuppressLint
import android.app.Activity
import android.app.Notification
import android.app.NotificationManager
import android.app.PendingIntent
import android.app.Service
import android.content.BroadcastReceiver
import android.content.Context
import android.content.Intent
import android.content.pm.PackageManager
import android.content.pm.ServiceInfo
import android.hardware.camera2.CameraCaptureSession
import android.hardware.camera2.CameraCharacteristics
import android.hardware.camera2.CameraDevice
import android.hardware.camera2.CameraManager
import android.hardware.camera2.CaptureRequest
import android.hardware.camera2.params.OutputConfiguration
import android.hardware.camera2.params.SessionConfiguration
import android.media.MediaCodec
import android.media.MediaCodecInfo
import android.media.MediaFormat
import android.os.Bundle
import android.os.IBinder
import android.util.Log
import android.util.Range
import android.view.Surface
import androidx.core.content.ContextCompat
import org.ligatureprotocol.core.CameraInfo
import java.util.concurrent.ExecutorService
import java.util.concurrent.Executors
import java.util.concurrent.atomic.AtomicBoolean

/**
 * The phone's cameras shared with another of the person's devices (milestone
 * 10): a viewer's request, or the user's own cast, starts
 * [CameraShareService], which puts a camera's pictures into an H.264 encoder
 * and the microphone's sound into an Opus encoder and hands both to the node.
 *
 * Android asks for the camera and the microphone once each, from the Camera
 * card, and until the camera is allowed the phone offers none. Android also
 * lets a camera start only from an app the user can see, so a request that
 * comes while the app is in the background opens a transparent activity
 * where the accessibility service allows that, and is otherwise a
 * notification to tap. While a camera runs Android shows its green
 * indicator, and the share's notification carries Stop sharing.
 *
 * A camera's pictures are the sensor's, which lies along the phone's long
 * side: held upright, the picture is on its side. The phone is turned to
 * landscape to be a webcam.
 */
object CameraShare {
    private const val TAG = "ligature"
    private const val REQUEST_NOTIFICATION = 7004
    private const val WAKE_START = "camera"

    /** How long a request stays answerable: the node gives up after 60 s. */
    private const val REQUEST_LIFETIME_MS = 55_000L

    /** The largest picture a camera is listed and captured at. */
    const val MAX_WIDTH = 1920
    const val MAX_HEIGHT = 1080

    /** What the viewer asked for, until the share starts. */
    data class Request(
        val viewer: String,
        val camera: String,
        val width: Int,
        val height: Int,
        val frameRate: Int,
        val audioChannels: Int,
        val cast: Boolean,
        val at: Long,
    )

    /** Set while the user's own cast waits for its camera, so the share says so. */
    @Volatile
    var casting: Boolean = false

    @Volatile
    var pending: Request? = null
        private set

    /** Whether the user let the app use the camera. */
    fun allowed(context: Context): Boolean =
        context.checkSelfPermission(Manifest.permission.CAMERA) == PackageManager.PERMISSION_GRANTED

    /** Whether a shared camera can carry the microphone. */
    fun canShareMicrophone(context: Context): Boolean =
        ScreenShare.hasOpusEncoder() && ScreenShare.soundAllowed(context)

    /**
     * The size a camera is listed with: the largest of [sizes] within
     * [MAX_WIDTH] by [MAX_HEIGHT], or the smallest when none fits.
     */
    fun bestSize(sizes: List<Pair<Int, Int>>): Pair<Int, Int>? {
        val real = sizes.filter { it.first > 0 && it.second > 0 }
        return real.filter { it.first <= MAX_WIDTH && it.second <= MAX_HEIGHT }
            .maxByOrNull { it.first.toLong() * it.second }
            ?: real.minByOrNull { it.first.toLong() * it.second }
    }

    /** How far a captured size may pass the agreed one: an encoder's rounding to 16. */
    private const val SIZE_SLACK = 16

    /**
     * The size a share is captured at: the largest of [sizes] that fits
     * within [width] by [height], give or take an encoder's rounding, and
     * has that shape, so the picture is the agreed one or a smaller one of
     * the same shape and never a stretched one; the smallest of that shape
     * when none fits, and the listed size when the camera has none of that
     * shape.
     */
    fun fitSize(sizes: List<Pair<Int, Int>>, width: Int, height: Int): Pair<Int, Int>? {
        if (width <= 0 || height <= 0) return bestSize(sizes)
        val shaped = sizes.filter { (w, h) ->
            w > 0 && h > 0 &&
                Math.abs(w.toLong() * height - h.toLong() * width) <= maxOf(w, width).toLong() * SIZE_SLACK
        }
        return shaped.filter { it.first <= width + SIZE_SLACK && it.second <= height + SIZE_SLACK }
            .maxByOrNull { it.first.toLong() * it.second }
            ?: shaped.minByOrNull { it.first.toLong() * it.second }
            ?: bestSize(sizes)
    }

    /**
     * The frame rate range a share asks the camera for: one that holds
     * [rate] steadily where the camera has it, else the narrowest that
     * reaches it, else the fastest the camera has.
     */
    fun rateRange(ranges: List<Pair<Int, Int>>, rate: Int): Pair<Int, Int>? {
        val real = ranges.filter { it.first > 0 && it.second >= it.first }
        return real.filter { it.first <= rate && rate <= it.second }
            .minWithOrNull(compareBy<Pair<Int, Int>> { it.second - it.first }.thenBy { it.second })
            ?: real.maxByOrNull { it.second }
    }

    /** The frame rates a camera is listed with: the tops of its ranges, at most 60, the highest first. */
    fun listedRates(ranges: List<Pair<Int, Int>>): List<Int> =
        ranges.map { it.second.coerceAtMost(60) }.filter { it > 0 }.distinct().sortedDescending()

    /** Which way a camera looks, as the protocol says it. */
    fun facing(lens: Int?): String = when (lens) {
        CameraCharacteristics.LENS_FACING_FRONT -> "front"
        CameraCharacteristics.LENS_FACING_BACK -> "back"
        else -> "external"
    }

    /** What a camera is called on the other device. */
    fun name(facing: String, id: String): String = when (facing) {
        "front" -> "Front camera"
        "back" -> "Back camera"
        else -> "Camera $id"
    }

    private fun sizes(c: CameraCharacteristics): List<Pair<Int, Int>> =
        c.get(CameraCharacteristics.SCALER_STREAM_CONFIGURATION_MAP)
            ?.getOutputSizes(MediaCodec::class.java)
            ?.map { it.width to it.height }
            .orEmpty()

    private fun ranges(c: CameraCharacteristics): List<Pair<Int, Int>> =
        c.get(CameraCharacteristics.CONTROL_AE_AVAILABLE_TARGET_FPS_RANGES)
            ?.map { it.lower to it.upper }
            .orEmpty()

    /**
     * The phone's cameras, the back one first: reading what they are needs
     * no permission, and nothing here opens one. A phone often lists several
     * lenses on one side; each is a camera of its own.
     */
    fun cameras(context: Context): List<CameraInfo> = runCatching {
        val manager = context.getSystemService(CameraManager::class.java)
        manager.cameraIdList.mapNotNull { id ->
            val c = manager.getCameraCharacteristics(id)
            val (width, height) = bestSize(sizes(c)) ?: return@mapNotNull null
            val facing = facing(c.get(CameraCharacteristics.LENS_FACING))
            CameraInfo(
                id = id,
                name = name(facing, id),
                facing = facing,
                width = width.toUInt(),
                height = height.toUInt(),
                frameRates = listedRates(ranges(c)).map { it.toUInt() },
            )
        }.sortedBy { if (it.facing == "back") 0 else if (it.facing == "front") 1 else 2 }
    }.getOrElse {
        Log.w(TAG, "cannot list the cameras", it)
        emptyList()
    }

    /** A viewer asked, or the user cast: start now if the app is in front, else through an activity or a tap. */
    fun request(
        context: Context,
        viewer: String,
        camera: String,
        width: Int,
        height: Int,
        frameRate: Int,
        audioChannels: Int,
    ) {
        val request = Request(viewer, camera, width, height, frameRate, audioChannels, casting, System.currentTimeMillis())
        casting = false
        if (!allowed(context)) {
            // Only the user allows the camera, from the Camera card.
            log("a camera was asked for but is not allowed; refusing")
            NodeHolder.cameraShareAnswered(false)
            return
        }
        pending = request
        ScreenShare.createChannel(context)
        val open = Intent(context, CameraStartActivity::class.java).addFlags(Intent.FLAG_ACTIVITY_NEW_TASK)
        // Android starts a camera only for an app the user can see, and lets
        // an app open an activity from the background while the user has its
        // accessibility service on.
        if (ScreenShare.foreground || DriveService.isOn) {
            if (!ScreenShare.foreground) ScreenWake.hold(context, WAKE_START)
            runCatching { context.startActivity(open) }.onSuccess { return }
        }
        val tap = PendingIntent.getActivity(
            context,
            REQUEST_NOTIFICATION,
            open,
            PendingIntent.FLAG_IMMUTABLE or PendingIntent.FLAG_UPDATE_CURRENT,
        )
        val notification = Notification.Builder(context, ScreenShare.CHANNEL_ID)
            .setSmallIcon(R.drawable.ic_tile)
            .setContentTitle("$viewer wants to see this phone's camera")
            .setContentText("Tap to share it")
            .setContentIntent(tap)
            .setAutoCancel(true)
            .setTimeoutAfter(REQUEST_LIFETIME_MS)
            .build()
        context.getSystemService(NotificationManager::class.java).notify(REQUEST_NOTIFICATION, notification)
    }

    /** The request, if it is still answerable; it is answered only once. */
    fun take(): Request? {
        ScreenWake.release(WAKE_START)
        val r = pending ?: return null
        pending = null
        return r.takeIf { System.currentTimeMillis() - it.at < REQUEST_LIFETIME_MS }
    }

    fun clearRequest(context: Context) {
        ScreenWake.release(WAKE_START)
        pending = null
        context.getSystemService(NotificationManager::class.java).cancel(REQUEST_NOTIFICATION)
    }

    /** The node ends the share: the viewer stopped or went away. */
    fun stop(context: Context) {
        clearRequest(context)
        context.stopService(Intent(context, CameraShareService::class.java))
    }

    fun keyframe() {
        CameraShareService.current?.requestKeyframe()
    }

    /** The share notification's Stop action. */
    class StopReceiver : BroadcastReceiver() {
        override fun onReceive(context: Context, intent: Intent) {
            CameraShareService.current?.end("sharer")
        }
    }

    internal fun log(message: String) = Log.i(TAG, "camera share: $message")
}

/**
 * Transparent, and in front for an instant: that is when Android lets the
 * app start a camera. Starts the service for the pending request and goes.
 */
class CameraStartActivity : Activity() {
    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        val request = CameraShare.take()
        if (request != null) {
            runCatching { CameraShareService.start(this, request) }.onFailure {
                Log.w("ligature", "the camera service did not start", it)
                NodeHolder.cameraShareAnswered(false)
            }
        }
        finish()
    }
}

/**
 * The share itself: a foreground service of the camera type, and of the
 * microphone type when its sound goes too, a camera whose pictures go
 * straight into an H.264 encoder's input surface, and a thread handing each
 * access unit to the node with the parameter sets before every keyframe.
 */
class CameraShareService : Service() {
    private var device: CameraDevice? = null
    private var session: CameraCaptureSession? = null
    private var codec: MediaCodec? = null
    private var surface: Surface? = null
    private var drain: Thread? = null
    private var sound: PlaybackSound? = null
    private var executor: ExecutorService? = null
    private val answered = AtomicBoolean(false)

    @Volatile
    private var ended = false

    override fun onBind(intent: Intent?): IBinder? = null

    override fun onStartCommand(intent: Intent?, flags: Int, startId: Int): Int {
        current?.takeIf { it !== this }?.end("sharer")
        current = this
        val viewer = intent?.getStringExtra(EXTRA_VIEWER) ?: "another device"
        val camera = intent?.getStringExtra(EXTRA_CAMERA).orEmpty()
        val width = intent?.getIntExtra(EXTRA_WIDTH, 0) ?: 0
        val height = intent?.getIntExtra(EXTRA_HEIGHT, 0) ?: 0
        val rate = intent?.getIntExtra(EXTRA_RATE, 30) ?: 30
        val audioChannels = intent?.getIntExtra(EXTRA_AUDIO, 0) ?: 0
        val cast = intent?.getBooleanExtra(EXTRA_CAST, false) ?: false
        // The microphone, when the share agreed to it and the user allowed
        // recording: a camera that cannot have it goes on silent.
        val microphone = audioChannels > 0 && CameraShare.canShareMicrophone(this)
        ScreenShare.createChannel(this)
        try {
            startForeground(
                NOTIFICATION,
                notification(viewer, cast, microphone),
                ServiceInfo.FOREGROUND_SERVICE_TYPE_CAMERA or
                    (if (microphone) ServiceInfo.FOREGROUND_SERVICE_TYPE_MICROPHONE else 0),
            )
        } catch (e: Exception) {
            // Android refuses a camera service started where the user cannot
            // see the app.
            Log.w("ligature", "Android did not let the camera service start", e)
            answer(false)
            stopSelf()
            return START_NOT_STICKY
        }
        if (camera.isEmpty() || width <= 0 || height <= 0 || !CameraShare.allowed(this)) {
            answer(false)
            stopSelf()
            return START_NOT_STICKY
        }
        try {
            begin(camera, width, height, rate)
            if (microphone) {
                sound = runCatching {
                    PlaybackSound(null, audioChannels, NodeHolder::cameraShareAudio).also { it.start() }
                }.onFailure { Log.w("ligature", "the microphone is not captured; sharing silent", it) }.getOrNull()
            } else if (audioChannels > 0) {
                CameraShare.log("the microphone was asked for but recording is not allowed; sharing silent")
            }
        } catch (e: Exception) {
            fail("the camera did not start", e)
        }
        return START_NOT_STICKY
    }

    /** Tells the node once whether the share started. */
    private fun answer(ok: Boolean) {
        if (answered.compareAndSet(false, true)) NodeHolder.cameraShareAnswered(ok)
    }

    /** Before the share runs, a failure refuses it; after, it ends it. */
    private fun fail(what: String, e: Throwable? = null) {
        Log.w("ligature", "camera share: $what", e)
        if (answered.compareAndSet(false, true)) {
            NodeHolder.cameraShareAnswered(false)
            ended = true
            stopSelf()
        } else {
            end("error")
        }
    }

    // The caller checks CAMERA (CameraShare.allowed).
    @SuppressLint("MissingPermission")
    private fun begin(camera: String, width: Int, height: Int, rate: Int) {
        val manager = getSystemService(CameraManager::class.java)
        val characteristics = manager.getCameraCharacteristics(camera)
        val sizes = characteristics.get(CameraCharacteristics.SCALER_STREAM_CONFIGURATION_MAP)
            ?.getOutputSizes(MediaCodec::class.java)
            ?.map { it.width to it.height }
            .orEmpty()
        val (w, h) = CameraShare.fitSize(sizes, width, height)
            ?: throw IllegalStateException("the camera has no size an encoder takes")
        val ranges = characteristics.get(CameraCharacteristics.CONTROL_AE_AVAILABLE_TARGET_FPS_RANGES)
            ?.map { it.lower to it.upper }
            .orEmpty()
        val range = CameraShare.rateRange(ranges, rate)

        fun format(prependHeaders: Boolean) = MediaFormat.createVideoFormat(MediaFormat.MIMETYPE_VIDEO_AVC, w, h).apply {
            setInteger(MediaFormat.KEY_COLOR_FORMAT, MediaCodecInfo.CodecCapabilities.COLOR_FormatSurface)
            setInteger(MediaFormat.KEY_BIT_RATE, ScreenShareService.bitRate(w, h, rate))
            setInteger(MediaFormat.KEY_FRAME_RATE, rate)
            // Keyframes at the start and when asked for; this is the safety net.
            setInteger(MediaFormat.KEY_I_FRAME_INTERVAL, ScreenShareService.keyframeBackstopSeconds(rate))
            if (prependHeaders) setInteger(MediaFormat.KEY_PREPEND_HEADER_TO_SYNC_FRAMES, 1)
        }
        var c = MediaCodec.createEncoderByType(MediaFormat.MIMETYPE_VIDEO_AVC)
        try {
            c.configure(format(true), null, null, MediaCodec.CONFIGURE_FLAG_ENCODE)
        } catch (e: IllegalArgumentException) {
            CameraShare.log("the encoder will not repeat its headers; the app does")
            c.release()
            c = MediaCodec.createEncoderByType(MediaFormat.MIMETYPE_VIDEO_AVC)
            c.configure(format(false), null, null, MediaCodec.CONFIGURE_FLAG_ENCODE)
        }
        val input = c.createInputSurface()
        c.start()
        codec = c
        surface = input
        drain = Thread({ drain(c) }, "ligature-camera-encoder").apply { start() }

        val callbacks = Executors.newSingleThreadExecutor()
        executor = callbacks
        manager.openCamera(
            camera,
            callbacks,
            object : CameraDevice.StateCallback() {
                override fun onOpened(opened: CameraDevice) {
                    if (ended) {
                        opened.close()
                        return
                    }
                    device = opened
                    runCatching { configure(opened, input, range) }.onFailure { fail("the camera's session was not made", it) }
                }

                override fun onDisconnected(gone: CameraDevice) {
                    gone.close()
                    fail("the camera was taken away")
                }

                override fun onError(broken: CameraDevice, error: Int) {
                    broken.close()
                    fail("the camera failed ($error)")
                }
            },
        )
        CameraShare.log("opening camera $camera at ${w}x$h, $rate fps")
    }

    /** The camera's pictures into the encoder, at the share's rate where the camera holds one. */
    private fun configure(opened: CameraDevice, input: Surface, range: Pair<Int, Int>?) {
        val callbacks = executor ?: return
        val request = opened.createCaptureRequest(CameraDevice.TEMPLATE_RECORD).apply {
            addTarget(input)
            if (range != null) set(CaptureRequest.CONTROL_AE_TARGET_FPS_RANGE, Range(range.first, range.second))
        }.build()
        opened.createCaptureSession(
            SessionConfiguration(
                SessionConfiguration.SESSION_REGULAR,
                listOf(OutputConfiguration(input)),
                callbacks,
                object : CameraCaptureSession.StateCallback() {
                    override fun onConfigured(configured: CameraCaptureSession) {
                        if (ended) return
                        session = configured
                        runCatching { configured.setRepeatingRequest(request, null, null) }
                            .onSuccess {
                                ScreenWake.hold(this@CameraShareService, WAKE_SHARE)
                                answer(true)
                                CameraShare.log("sharing")
                            }
                            .onFailure { fail("the camera did not start its pictures", it) }
                    }

                    override fun onConfigureFailed(failed: CameraCaptureSession) {
                        fail("the camera refused the encoder's surface")
                    }
                },
            ),
        )
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
                    val out = if (key && config != null && !ScreenShareService.startsWithSps(bytes)) config + bytes else bytes
                    NodeHolder.cameraShareFrame(key, out)
                }
            }
            runCatching { c.releaseOutputBuffer(index, false) }
            if (info.flags and MediaCodec.BUFFER_FLAG_END_OF_STREAM != 0) break
        }
    }

    fun requestKeyframe() {
        runCatching {
            codec?.setParameters(Bundle().apply { putInt(MediaCodec.PARAMETER_KEY_REQUEST_SYNC_FRAME, 0) })
        }
    }

    /** The share ends here: `sharer` from Stop sharing, `error` when the camera went. */
    fun end(reason: String) {
        if (ended) return
        ended = true
        CameraShare.log("ended ($reason)")
        if (answered.compareAndSet(false, true)) {
            NodeHolder.cameraShareAnswered(false)
        } else {
            NodeHolder.cameraShareEnded(reason)
        }
        stopSelf()
    }

    override fun onDestroy() {
        ScreenWake.release(WAKE_SHARE)
        if (!ended) {
            ended = true
            if (answered.compareAndSet(false, true)) {
                NodeHolder.cameraShareAnswered(false)
            } else {
                NodeHolder.cameraShareEnded("sharer")
            }
        }
        release()
        if (current === this) current = null
        super.onDestroy()
    }

    private fun release() {
        ended = true
        runCatching { sound?.stop() }
        sound = null
        runCatching { session?.close() }
        runCatching { device?.close() }
        runCatching { codec?.stop() }
        runCatching { codec?.release() }
        runCatching { surface?.release() }
        runCatching { drain?.join(500) }
        runCatching { executor?.shutdown() }
        session = null
        device = null
        codec = null
        surface = null
        executor = null
    }

    private fun notification(viewer: String, cast: Boolean, microphone: Boolean): Notification {
        val stop = PendingIntent.getBroadcast(
            this,
            0,
            Intent(this, CameraShare.StopReceiver::class.java),
            PendingIntent.FLAG_IMMUTABLE,
        )
        return Notification.Builder(this, ScreenShare.CHANNEL_ID)
            .setSmallIcon(R.drawable.ic_tile)
            .setContentTitle(title(viewer, cast, microphone))
            .setOngoing(true)
            .addAction(Notification.Action.Builder(null, "Stop sharing", stop).build())
            .build()
    }

    companion object {
        private const val WAKE_SHARE = "camera-share"
        private const val NOTIFICATION = 7005
        private const val EXTRA_VIEWER = "viewer"
        private const val EXTRA_CAMERA = "camera"
        private const val EXTRA_WIDTH = "width"
        private const val EXTRA_HEIGHT = "height"
        private const val EXTRA_RATE = "rate"
        private const val EXTRA_AUDIO = "audioChannels"
        private const val EXTRA_CAST = "cast"

        @Volatile
        var current: CameraShareService? = null
            private set

        /** What the share's notification says. */
        fun title(viewer: String, cast: Boolean, microphone: Boolean): String {
            val what = if (microphone) "your camera and microphone" else "your camera"
            return if (cast) "Casting $what to $viewer" else "Sharing $what with $viewer"
        }

        fun start(context: Context, request: CameraShare.Request) {
            val intent = Intent(context, CameraShareService::class.java)
                .putExtra(EXTRA_VIEWER, request.viewer)
                .putExtra(EXTRA_CAMERA, request.camera)
                .putExtra(EXTRA_WIDTH, request.width)
                .putExtra(EXTRA_HEIGHT, request.height)
                .putExtra(EXTRA_RATE, request.frameRate)
                .putExtra(EXTRA_AUDIO, request.audioChannels)
                .putExtra(EXTRA_CAST, request.cast)
            ContextCompat.startForegroundService(context, intent)
        }
    }
}
