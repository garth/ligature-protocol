package org.ligatureprotocol.android

import android.app.Notification
import android.app.NotificationChannel
import android.app.NotificationManager
import android.content.ClipData
import android.content.ClipDescription
import android.content.ClipboardManager
import android.content.Context
import android.content.Intent
import android.app.PendingIntent
import android.content.BroadcastReceiver
import android.net.Uri
import android.graphics.BitmapFactory
import android.graphics.drawable.Icon
import android.os.Build
import android.os.PersistableBundle
import android.provider.Settings
import android.util.Log
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.update
import org.ligatureprotocol.core.ClipItem
import org.ligatureprotocol.core.ClipItemData
import org.ligatureprotocol.core.Config
import org.ligatureprotocol.core.HostClipValue
import org.ligatureprotocol.core.OutgoingFile
import org.ligatureprotocol.core.Device
import org.ligatureprotocol.core.DownloadTarget
import org.ligatureprotocol.core.Event
import org.ligatureprotocol.core.Host
import org.ligatureprotocol.core.InputTarget
import org.ligatureprotocol.core.ScreenTarget
import org.ligatureprotocol.core.ScreenWatch
import org.ligatureprotocol.core.TouchEvent
import org.ligatureprotocol.core.LocalNotification
import org.ligatureprotocol.core.LogLevel
import org.ligatureprotocol.core.LpException
import org.ligatureprotocol.core.Node
import org.ligatureprotocol.core.NotificationCopy
import org.ligatureprotocol.core.Pairing
import org.ligatureprotocol.core.PublishedDownload
import org.ligatureprotocol.core.Status
import java.io.File
import java.util.concurrent.Executors

/** A share this phone watches, and whether touch drives it yet. */
data class Watching(
    val target: ScreenTarget,
    val watch: ScreenWatch,
    val canDrive: Boolean,
    val driving: Boolean = false,
)

/** A ceremony as the screen shows it. */
data class PairingState(
    val offer: String? = null,
    val code: String? = null,
    val finished: Boolean = false,
    val error: String? = null,
)

/** An app the listener has seen post, and whether its notifications leave the phone. */
data class AppSwitch(val packageName: String, val label: String, val enabled: Boolean)

/** Everything the screen needs, refreshed on every event from the node. */
data class UiState(
    val running: Boolean = false,
    val status: Status? = null,
    val devices: List<Device> = emptyList(),
    val pairing: PairingState? = null,
    val lastError: String? = null,
    val backgroundRead: Boolean = false,
    /** The last text the node set here or sent from here. */
    val lastClipboard: String? = null,
    /** Whether the user granted notification access. */
    val notificationAccess: Boolean = false,
    /** The apps seen posting, with their switches. */
    val apps: List<AppSwitch> = emptyList(),
    /** The devices this phone can drive as a touchpad (milestone 7). */
    val inputTargets: List<InputTarget> = emptyList(),
    /** The device being driven, while the touchpad screen is open. */
    val touchpad: InputTarget? = null,
    /** The devices whose screen this phone can watch (milestone 8). */
    val screenTargets: List<ScreenTarget> = emptyList(),
    /** The share this phone watches, while the watch screen is open. */
    val watching: Watching? = null,
)

/**
 * The one node of this process and the host the node calls back into. The
 * service starts and stops it; activities read [state] and call the node.
 */
object NodeHolder : Host {
    private const val TAG = "ligature"

    private val _state = MutableStateFlow(UiState())
    val state: StateFlow<UiState> = _state.asStateFlow()

    @Volatile
    private var node: Node? = null

    @Volatile
    private var pairing: Pairing? = null

    private lateinit var appContext: Context

    @Volatile
    private var listenerConnected = false

    /**
     * One thread for every node call that waits on the network or the disk
     * (revoking, asking for clipboards, opening shared files, starting and
     * stopping), so none of them runs on the main thread or a binder thread
     * (LP-127). Results reach the caller through a callback.
     */
    private val worker = Executors.newSingleThreadExecutor { Thread(it, "ligature-host") }

    val isRunning: Boolean get() = node != null

    /** Runs [block] on the worker thread. */
    fun async(block: () -> Unit) {
        worker.execute(block)
    }

    @Synchronized
    fun start(context: Context) {
        if (node != null) return
        appContext = context.applicationContext
        val dir = File(appContext.filesDir, "ligature").apply { mkdirs() }
        val config = Config(
            dataDir = dir.absolutePath,
            deviceName = deviceName(appContext),
            debugLogging = BuildConfig.DEBUG,
            cacheDir = appContext.cacheDir.absolutePath,
        )
        node = Node.start(config, this)
        val metrics = appContext.resources.displayMetrics
        node?.setScreenShare(
            ScreenShare.hasEncoder(),
            minOf(metrics.widthPixels, metrics.heightPixels).toUInt(),
            maxOf(metrics.widthPixels, metrics.heightPixels).toUInt(),
        )
        if (listenerConnected) node?.setNotificationsEmitting(true)
        _state.update {
            it.copy(
                running = true,
                backgroundRead = hasBackgroundRead(appContext),
                lastError = null,
                notificationAccess = NotificationListener.hasAccess(appContext),
                apps = apps(appContext),
            )
        }
        refresh()
    }

    @Synchronized
    fun stop() {
        pairing?.abort()
        pairing = null
        node?.stop()
        node = null
        synchronized(this) {
            clipHandles.forEach { runCatching { it.close() } }
            clipHandles = emptyList()
            previousClipHandles.forEach { runCatching { it.close() } }
            previousClipHandles = emptyList()
        }
        _state.update { UiState() }
    }

    /** Re-reads status and devices from the node. */
    fun refresh() {
        val n = node ?: return
        val status = runCatching { n.status() }.getOrNull()
        val devices = runCatching { n.devices() }.getOrDefault(emptyList())
        val last = runCatching { n.clipboardText() }.getOrNull()
        val targets = runCatching { n.inputTargets() }.getOrDefault(emptyList())
        val screens = runCatching { n.screenTargets() }.getOrDefault(emptyList())
        val driving = runCatching { n.inputDriving() }.getOrNull()
        _state.update {
            it.copy(
                status = status,
                devices = devices,
                lastClipboard = last,
                inputTargets = targets,
                screenTargets = screens,
                touchpad = if (driving == null) null else it.touchpad ?: targets.firstOrNull { t -> t.key == driving },
            )
        }
    }

    /** Opens the touchpad: starts driving [target] with the pointer at the middle of its screen. */
    fun startTouchpad(target: InputTarget) = async {
        call { n ->
            n.inputStart(target.key)
            _state.update { it.copy(touchpad = target) }
        }
    }

    /** The user left the touchpad screen: the drive ends and everything held is lifted. */
    fun stopTouchpad() = async {
        node?.inputStop()
        _state.update { it.copy(touchpad = null) }
        refresh()
    }

    /** Opens the watch screen: the device's primary display, at most 1920 by 1080. */
    fun startWatch(target: ScreenTarget) = async {
        call { n ->
            val watch = n.screenWatch(target.key, 1920u, 1080u, 30u)
            val canDrive = runCatching { n.inputTargets() }.getOrDefault(emptyList()).any { it.key == target.key }
            _state.update { it.copy(watching = Watching(target, watch, canDrive)) }
        }
    }

    /** The user left the watch screen: the watch and any drive end. */
    fun stopWatch() = async {
        val n = node
        if (_state.value.watching?.driving == true) n?.inputStop()
        n?.screenWatchStop()
        WatchDecoder.release()
        _state.update { it.copy(watching = null) }
        refresh()
    }

    /**
     * A touch on the watched picture at [x], [y]: the first one starts the
     * drive there, every one moves the pointer there, then [events] follow.
     */
    fun watchTouch(x: Double, y: Double, events: List<TouchEvent>) {
        if (_state.value.watching?.canDrive != true) return
        enqueue(listOf(TouchEvent.Position(x, y)) + events)
    }

    /** Typing on the watched desktop, once a touch has started the drive. */
    fun watchType(events: List<TouchEvent>) {
        if (_state.value.watching?.driving == true) enqueue(events)
    }

    /** The decoder lost its place: ask the sharer for a keyframe. */
    fun screenWatchKeyframe() {
        node?.screenWatchKeyframe()
    }

    /** The user answered the media projection dialog, or the service could not start. */
    fun screenShareAnswered(ok: Boolean) {
        node?.screenShareStarted(ok)
    }

    /** One encoded access unit of this phone's screen. */
    fun screenShareFrame(key: Boolean, bytes: ByteArray) {
        node?.screenShareFrame(key, bytes)
    }

    /** The share of this phone's screen ended here. */
    fun screenShareEnded(reason: String) {
        node?.screenShareEnded(reason)
    }

    /** What the touchpad screen did; queued and sent in coalesced batches. */
    fun touch(events: List<TouchEvent>) = enqueue(events)

    /**
     * Touch arrives faster than a batch can be sent, so events wait in a queue
     * that one worker task drains: consecutive motion is summed and
     * consecutive positions keep only the newest, so the pointer goes where
     * the finger is now rather than replaying where it was.
     */
    private val touchQueue = ArrayList<TouchEvent>()
    private var touchFlushing = false

    private fun enqueue(events: List<TouchEvent>) {
        synchronized(touchQueue) {
            touchQueue.addAll(events)
            if (touchFlushing) return
            touchFlushing = true
        }
        async { flushTouch() }
    }

    private fun flushTouch() {
        while (true) {
            val batch = synchronized(touchQueue) {
                if (touchQueue.isEmpty()) {
                    touchFlushing = false
                    return
                }
                val drained = coalesceTouch(touchQueue)
                touchQueue.clear()
                drained
            }
            val n = node ?: continue
            val w = _state.value.watching
            if (w != null && w.canDrive && !w.driving) {
                // The first touch on a watched picture starts the drive there.
                val at = batch.firstNotNullOfOrNull { it as? TouchEvent.Position }
                if (at != null) {
                    val started = runCatching { n.inputStartAt(w.target.key, at.x, at.y) }
                    if (started.isFailure) {
                        Log.d(TAG, "cannot drive ${w.target.name}: ${started.exceptionOrNull()?.message}")
                        continue
                    }
                    _state.update { s -> s.copy(watching = s.watching?.copy(driving = true)) }
                }
            }
            runCatching { n.inputSend(batch) }.onFailure { Log.d(TAG, "touch events dropped: ${it.message}") }
        }
    }

    fun createPerson(name: String) = async { call { it.init(name) } }

    /** The network came up or changed: start discovery again on it. */
    fun restartDiscovery() = async { call { it.restartDiscovery() } }

    /** Revokes a device: the node pushes the revocation to every peer first. */
    fun revoke(key: String) = async { call { it.revoke(key) } }

    /** Starts the ceremony with this phone as the newcomer. */
    fun startJoin() = call {
        pairing?.abort()
        _state.update { s -> s.copy(pairing = PairingState()) }
        pairing = it.startJoin()
    }

    /** Starts the ceremony with this phone introducing the device that showed [offer]. */
    fun startIntroduce(offer: String) = call {
        pairing?.abort()
        _state.update { s -> s.copy(pairing = PairingState()) }
        pairing = it.startIntroduce(offer, true)
    }

    fun confirmPairing() {
        pairing?.confirm()
    }

    fun abortPairing() {
        pairing?.abort()
        pairing = null
        _state.update { it.copy(pairing = null) }
    }

    fun dismissPairing() {
        if (_state.value.pairing?.finished == true) {
            pairing = null
            _state.update { it.copy(pairing = null) }
        }
    }

    /**
     * The app could read the clipboard and found [text] on it: the node sends
     * it to the person's other devices. Returns how many of them offer the
     * clipboard right now.
     */
    private fun clipboardChanged(text: String): Int {
        val n = node ?: return 0
        n.clipboardChanged(text)
        refresh()
        return runCatching { n.status().clipboardPeers.size }.getOrDefault(0)
    }

    /**
     * Reads the clipboard, if Android lets us, with every representation the
     * clip carries (milestone 5): the text, the HTML, and the content URIs
     * opened for the node. The read happens on the calling thread, since it
     * is the caller's focus that permits it; the opens and the send happen on
     * the worker, and [onDone] is then called there with how many devices
     * offer the clipboard, or null when the clipboard holds nothing readable.
     */
    fun readAndSend(context: Context, onDone: (Int?) -> Unit = {}) {
        val n = node ?: return onDone(0)
        val clip = readClip(context) ?: return onDone(null)
        async {
            val read = openClip(context, clip)
            if (read == null) {
                onDone(null)
                return@async
            }
            synchronized(this) {
                // The node may still be sending the previous clip's files
                // from their descriptors: keep one generation back open.
                previousClipHandles.forEach { runCatching { it.close() } }
                previousClipHandles = clipHandles
                clipHandles = read.handles
            }
            n.clipboardChangedValue(read.text, read.html, read.contents)
            refresh()
            onDone(runCatching { n.status().clipboardPeers.size }.getOrDefault(0))
        }
    }

    /** Asks every connected device for its clipboard (**Receive now**); [onDone] gets how many. */
    fun requestClipboard(onDone: (Int) -> Unit) = async {
        onDone(runCatching { node?.requestClipboard()?.toInt() }.getOrNull() ?: 0)
    }

    /** Fetches the files the clipboard promises into Downloads (**Save**). */
    fun saveClipboardFiles() = async { runCatching { node?.saveClipboardFiles() } }

    /**
     * The provider asks for an item's bytes or file: a promised one is
     * fetched now, on the binder thread that asked, bounded by the node's
     * timeout. Null once the node has stopped.
     */
    fun clipboardItem(key: String): ClipItemData? = runCatching { node?.clipboardItem(key) }.getOrNull()

    /** What the provider knows about a key: type, name and size. */
    @Synchronized
    fun clipItem(key: String?): ClipItem? = key?.let { clipItems[it] }

    private var clipItems: Map<String, ClipItem> = emptyMap()
    private var clipHandles: List<AutoCloseable> = emptyList()
    private var previousClipHandles: List<AutoCloseable> = emptyList()

    /**
     * Text handed to the app (the share sheet), after the user confirmed it:
     * on the clipboard here and everywhere. [onDone] gets the device count.
     */
    fun sendText(text: String, onDone: (Int) -> Unit) = async {
        setClipboard(text)
        onDone(clipboardChanged(text))
    }

    // Files (milestone 4).

    /** The connected devices that receive files: key and display name. */
    fun fileTargets(): List<Pair<String, String>> {
        val n = node ?: return emptyList()
        val status = runCatching { n.status() }.getOrNull() ?: return emptyList()
        val devices = runCatching { n.devices() }.getOrDefault(emptyList())
        return status.filePeers.map { key ->
            key to (devices.firstOrNull { it.key == key }?.displayName ?: key.take(12) + "…")
        }
    }

    /**
     * Sends the content behind [uris] to [peer]: opens each one on the worker
     * (a provider without a size is copied first) and keeps it open until the
     * transfer's last event. [onDone] gets a message for the user.
     */
    fun sendFiles(context: Context, peer: String, uris: List<android.net.Uri>, onDone: (String) -> Unit) = async {
        val n = node
        if (n == null) {
            onDone(context.getString(R.string.sent_to_none))
            return@async
        }
        val opened = Transfers.open(context, uris)
        if (opened == null) {
            onDone(context.getString(R.string.no_file_opened))
            return@async
        }
        val (files, handles) = opened
        val message = try {
            val transfer = n.sendFiles(peer, files)
            Transfers.keepOpen(transfer, handles)
            context.resources.getQuantityString(R.plurals.sending_files, files.size, files.size)
        } catch (e: LpException) {
            handles.forEach { runCatching { it.close() } }
            Log.w(TAG, "send failed", e)
            e.message ?: e.toString()
        }
        onDone(message)
    }

    fun cancelTransfer(transfer: String) {
        node?.cancelTransfer(transfer)
    }

    // Notifications (milestone 3).

    /** The listener connected or disconnected: the capability's `emits` follows. */
    fun setNotificationsEmitting(emitting: Boolean) {
        listenerConnected = emitting
        async {
            runCatching { node?.setNotificationsEmitting(emitting) }
            if (::appContext.isInitialized) refreshNotificationAccess(appContext)
        }
    }

    fun notificationPosted(notification: LocalNotification) {
        node?.notificationPosted(notification)
    }

    fun notificationRemoved(id: String) {
        node?.notificationRemoved(id)
    }

    fun notificationCopyDismissed(origin: String, id: String) = async {
        call { it.notificationCopyDismissed(origin, id) }
    }

    /** Re-reads whether access is granted and the app list, for the screen. */
    fun refreshNotificationAccess(context: Context) {
        _state.update {
            it.copy(notificationAccess = NotificationListener.hasAccess(context), apps = apps(context))
        }
    }

    /** The listener saw [packageName] post: remember it for the app list. */
    fun appSeen(context: Context, packageName: String, label: String) {
        val prefs = context.getSharedPreferences(PREFS, Context.MODE_PRIVATE)
        val seen = prefs.getStringSet(KEY_SEEN, emptySet()).orEmpty()
        val entry = "$packageName\u0000$label"
        if (seen.none { it.startsWith("$packageName\u0000") }) {
            prefs.edit().putStringSet(KEY_SEEN, seen + entry).apply()
            refreshNotificationAccess(context)
        }
    }

    fun isExcluded(context: Context, packageName: String): Boolean =
        context.getSharedPreferences(PREFS, Context.MODE_PRIVATE)
            .getStringSet(KEY_EXCLUDED, emptySet()).orEmpty().contains(packageName)

    /** Turns an app's notifications on or off for the other devices. */
    fun setAppEnabled(packageName: String, enabled: Boolean) {
        val prefs = appContext.getSharedPreferences(PREFS, Context.MODE_PRIVATE)
        val excluded = prefs.getStringSet(KEY_EXCLUDED, emptySet()).orEmpty()
        prefs.edit()
            .putStringSet(KEY_EXCLUDED, if (enabled) excluded - packageName else excluded + packageName)
            .apply()
        refreshNotificationAccess(appContext)
    }

    private fun apps(context: Context): List<AppSwitch> {
        val prefs = context.getSharedPreferences(PREFS, Context.MODE_PRIVATE)
        val excluded = prefs.getStringSet(KEY_EXCLUDED, emptySet()).orEmpty()
        return prefs.getStringSet(KEY_SEEN, emptySet()).orEmpty()
            .map { entry ->
                val parts = entry.split('\u0000', limit = 2)
                val pkg = parts[0]
                AppSwitch(pkg, parts.getOrElse(1) { pkg }, pkg !in excluded)
            }
            .sortedBy { it.label.lowercase() }
    }

    private const val PREFS = "ligature"
    private const val KEY_SEEN = "notifications.seen"
    private const val KEY_EXCLUDED = "notifications.excluded"

    /** A clip as read: its text, HTML and the content opened for the node. */
    class ClipRead(
        val text: String?,
        val html: String?,
        val contents: List<OutgoingFile>,
        val handles: List<AutoCloseable>,
    )

    /**
     * Reads the primary clip, if Android lets us, as it is: the text, the
     * HTML and the content URIs it carries. A clip of this app's own
     * provider is what a peer sent and is not read back. Nothing is opened
     * here; [openClip] does that, off the caller's thread.
     */
    private fun readClip(context: Context): Triple<String?, String?, List<Uri>>? {
        val manager = context.getSystemService(ClipboardManager::class.java)
        val clip = runCatching { manager.primaryClip }.getOrNull() ?: return null
        if (clip.itemCount == 0) return null
        var text: String? = null
        var html: String? = null
        val uris = ArrayList<Uri>()
        for (i in 0 until clip.itemCount) {
            val item = clip.getItemAt(i)
            if (text == null) text = item.text?.toString()?.takeIf { it.isNotEmpty() }
            if (html == null) html = item.htmlText?.takeIf { it.isNotEmpty() }
            item.uri?.let { uri ->
                if (uri.authority == ClipboardProvider.AUTHORITY) return null
                uris += uri
            }
        }
        if (text == null && uris.isEmpty()) {
            text = clip.getItemAt(0).coerceToText(context)?.toString()?.takeIf { it.isNotEmpty() }
        }
        if (text == null && html == null && uris.isEmpty()) return null
        return Triple(text, html, uris)
    }

    /** Opens the content URIs of a clip for the node to read. */
    private fun openClip(context: Context, clip: Triple<String?, String?, List<Uri>>): ClipRead? {
        val (text, html, uris) = clip
        val (contents, handles) = Transfers.open(context, uris) ?: (emptyList<OutgoingFile>() to emptyList())
        if (text == null && html == null && contents.isEmpty()) return null
        return ClipRead(text, html, contents, handles)
    }

    fun hasBackgroundRead(context: Context): Boolean =
        context.checkSelfPermission("android.permission.READ_CLIPBOARD_IN_BACKGROUND") ==
            android.content.pm.PackageManager.PERMISSION_GRANTED

    // Host: called from the node's threads.

    override fun setClipboard(text: String): Boolean {
        val manager = appContext.getSystemService(ClipboardManager::class.java)
        val clip = ClipData.newPlainText("Ligature", text)
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.TIRAMISU) {
            clip.description.extras = PersistableBundle().apply {
                putBoolean(ClipDescription.EXTRA_IS_SENSITIVE, false)
            }
        }
        return runCatching { manager.setPrimaryClip(clip) }.isSuccess
    }

    override fun setClipboardValue(value: HostClipValue): Boolean {
        synchronized(this) { clipItems = value.items.associateBy { it.key } }
        val manager = appContext.getSystemService(ClipboardManager::class.java)
        val mimes = ArrayList<String>()
        if (value.text != null) mimes += ClipDescription.MIMETYPE_TEXT_PLAIN
        if (value.html != null) mimes += ClipDescription.MIMETYPE_TEXT_HTML
        value.items.forEach { if (it.mime !in mimes) mimes += it.mime }
        if (mimes.isEmpty()) return false
        val uris = value.items.map { ClipboardProvider.uriFor(it.key) }
        val first = ClipData.Item(value.text ?: "", value.html, null, uris.firstOrNull())
        val clip = ClipData("Ligature", mimes.toTypedArray(), first)
        uris.drop(1).forEach { clip.addItem(ClipData.Item(it)) }
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.TIRAMISU) {
            // A remote-device clip: the system's copy overlay then shows no
            // preview, which would open the image URI and fetch it at once.
            clip.description.extras = PersistableBundle().apply {
                putBoolean(ClipDescription.EXTRA_IS_SENSITIVE, false)
                putBoolean(ClipDescription.EXTRA_IS_REMOTE_DEVICE, true)
            }
        }
        val ok = runCatching { manager.setPrimaryClip(clip) }.isSuccess
        val files = value.items.filter { it.file }
        if (ok && files.isNotEmpty()) {
            offerSave(files)
        } else {
            // The clipboard moved on: a Save for the old list would find it gone.
            appContext.getSystemService(NotificationManager::class.java).cancel(SAVE_CLIP_NOTIFICATION)
        }
        return ok
    }

    /** A file list arrived on the clipboard: offer to save it into Downloads. */
    private fun offerSave(files: List<ClipItem>) {
        val manager = appContext.getSystemService(NotificationManager::class.java)
        val save = PendingIntent.getBroadcast(
            appContext,
            0,
            Intent(appContext, SaveClipboardReceiver::class.java).setAction(ACTION_SAVE_CLIP),
            PendingIntent.FLAG_IMMUTABLE or PendingIntent.FLAG_UPDATE_CURRENT,
        )
        val title = if (files.size == 1) appContext.getString(R.string.clip_file_title, files[0].name)
        else appContext.resources.getQuantityString(R.plurals.clip_files_title, files.size, files.size)
        val notification = Notification.Builder(appContext, Transfers.CHANNEL_ID)
            .setSmallIcon(R.drawable.ic_tile)
            .setContentTitle(title)
            .setContentText(appContext.getString(R.string.clip_files_text))
            .setAutoCancel(true)
            .addAction(Notification.Action.Builder(null, appContext.getString(R.string.clip_save), save).build())
            .build()
        manager.notify(SAVE_CLIP_NOTIFICATION, notification)
    }

    const val ACTION_SAVE_CLIP = "org.ligatureprotocol.android.SAVE_CLIPBOARD_FILES"
    private const val SAVE_CLIP_NOTIFICATION = 7

    /** The Save action on the clipboard files notification. */
    class SaveClipboardReceiver : BroadcastReceiver() {
        override fun onReceive(context: Context, intent: Intent) {
            if (intent.action != ACTION_SAVE_CLIP) return
            context.getSystemService(NotificationManager::class.java).cancel(SAVE_CLIP_NOTIFICATION)
            saveClipboardFiles()
        }
    }

    override fun showNotification(copy: NotificationCopy): Boolean = runCatching {
        val manager = appContext.getSystemService(NotificationManager::class.java)
        val channelId = "origin:" + copy.origin
        manager.createNotificationChannel(
            NotificationChannel(
                channelId,
                appContext.getString(R.string.notifications_channel_prefix) + " " + copy.originName,
                NotificationManager.IMPORTANCE_DEFAULT,
            ),
        )
        val builder = Notification.Builder(appContext, channelId)
            .setSmallIcon(R.drawable.ic_ligature)
            .setContentTitle(copy.title ?: copy.appName)
            .setContentText(copy.body ?: "")
            .setSubText(copy.appName + " · " + copy.originName)
            .setWhen(copy.postedAtUnix * 1000)
            .setShowWhen(true)
            .setAutoCancel(true)
            .setGroup(copy.origin)
        copy.body?.let { builder.setStyle(Notification.BigTextStyle().bigText(it)) }
        copy.iconPng?.let { png ->
            BitmapFactory.decodeByteArray(png, 0, png.size)?.let { builder.setLargeIcon(Icon.createWithBitmap(it)) }
        }
        builder.extras.putString(NotificationListener.EXTRA_ORIGIN, copy.origin)
        builder.extras.putString(NotificationListener.EXTRA_ID, copy.id)
        manager.notify(copy.origin, copy.id.hashCode(), builder.build())
    }.isSuccess

    override fun dismissNotificationCopy(origin: String, id: String): Boolean = runCatching {
        appContext.getSystemService(NotificationManager::class.java).cancel(origin, id.hashCode())
    }.isSuccess

    override fun dismissOwnNotification(id: String): Boolean = NotificationListener.cancelOwn(id)

    override fun createDownload(name: String, mime: String, size: ULong): DownloadTarget? =
        Transfers.createDownload(appContext, name, mime, size)

    override fun finishDownload(id: ULong): PublishedDownload? = Transfers.finishDownload(appContext, id)

    override fun abortDownload(id: ULong) = Transfers.abortDownload(appContext, id)

    override fun screenFrame(peer: String, key: Boolean, bytes: ByteArray) = WatchDecoder.feed(key, bytes)

    override fun screenShareRequested(viewer: String, width: UInt, height: UInt, frameRate: UInt) =
        ScreenShare.request(appContext, viewer, width.toInt(), height.toInt(), frameRate.toInt())

    override fun screenShareStop() = ScreenShare.stop(appContext)

    override fun screenShareKeyframe() = ScreenShare.keyframe()

    override fun onEvent(event: Event) {
        when (event) {
            is Event.StatusChanged -> refresh()
            is Event.PairingOffer -> _state.update {
                it.copy(pairing = (it.pairing ?: PairingState()).copy(offer = event.offer))
            }
            is Event.PairingCode -> _state.update {
                it.copy(pairing = (it.pairing ?: PairingState()).copy(code = event.code))
            }
            is Event.PairingFinished -> {
                _state.update {
                    it.copy(
                        pairing = (it.pairing ?: PairingState()).copy(
                            finished = true,
                            error = event.error,
                        ),
                    )
                }
                refresh()
            }
            is Event.ClipboardReceived -> refresh()
            is Event.TransferChanged -> Transfers.changed(appContext, event.info)
            is Event.InputEnded -> {
                _state.update { s -> s.copy(touchpad = null, watching = s.watching?.copy(driving = false)) }
                refresh()
            }
            is Event.ScreenEnded -> {
                WatchDecoder.release()
                _state.update { it.copy(watching = null) }
                refresh()
            }
        }
    }

    override fun log(level: LogLevel, target: String, message: String) {
        val priority = when (level) {
            LogLevel.ERROR -> Log.ERROR
            LogLevel.WARN -> Log.WARN
            LogLevel.INFO -> Log.INFO
            LogLevel.DEBUG -> Log.DEBUG
            LogLevel.TRACE -> Log.VERBOSE
        }
        Log.println(priority, TAG, "$target: $message")
    }

    private fun call(block: (Node) -> Unit) {
        val n = node ?: return
        try {
            block(n)
            _state.update { it.copy(lastError = null) }
        } catch (e: LpException) {
            Log.w(TAG, "call failed", e)
            _state.update { it.copy(lastError = e.message ?: e.toString()) }
        } catch (e: RuntimeException) {
            // A call that landed between stop() and node = null, or a
            // binding error: shown, never fatal.
            Log.w(TAG, "call failed", e)
            _state.update { it.copy(lastError = e.message ?: e.toString()) }
        }
        refresh()
    }

    private fun deviceName(context: Context): String {
        val fromSettings = runCatching {
            Settings.Global.getString(context.contentResolver, Settings.Global.DEVICE_NAME)
        }.getOrNull()
        return fromSettings?.takeIf { it.isNotBlank() } ?: Build.MODEL
    }
}

private fun <T> MutableStateFlow<T>.asStateFlow(): StateFlow<T> = this
