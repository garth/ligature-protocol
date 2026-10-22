package org.ligatureprotocol.android

import android.app.Notification
import android.app.NotificationChannel
import android.app.NotificationManager
import android.content.ClipData
import android.content.ClipDescription
import android.content.ClipboardManager
import android.content.Context
import android.graphics.BitmapFactory
import android.graphics.drawable.Icon
import android.os.Build
import android.os.PersistableBundle
import android.provider.Settings
import android.util.Log
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.update
import org.ligatureprotocol.core.Config
import org.ligatureprotocol.core.Device
import org.ligatureprotocol.core.DownloadTarget
import org.ligatureprotocol.core.Event
import org.ligatureprotocol.core.Host
import org.ligatureprotocol.core.LocalNotification
import org.ligatureprotocol.core.LogLevel
import org.ligatureprotocol.core.LpException
import org.ligatureprotocol.core.Node
import org.ligatureprotocol.core.NotificationCopy
import org.ligatureprotocol.core.Pairing
import org.ligatureprotocol.core.PublishedDownload
import org.ligatureprotocol.core.Status
import java.io.File

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

    val isRunning: Boolean get() = node != null

    @Synchronized
    fun start(context: Context) {
        if (node != null) return
        appContext = context.applicationContext
        val dir = File(appContext.filesDir, "ligature").apply { mkdirs() }
        val config = Config(
            dataDir = dir.absolutePath,
            deviceName = deviceName(appContext),
            debugLogging = BuildConfig.DEBUG,
        )
        node = Node.start(config, this)
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
        _state.update { UiState() }
    }

    /** Re-reads status and devices from the node. */
    fun refresh() {
        val n = node ?: return
        val status = runCatching { n.status() }.getOrNull()
        val devices = runCatching { n.devices() }.getOrDefault(emptyList())
        val last = runCatching { n.clipboardText() }.getOrNull()
        _state.update { it.copy(status = status, devices = devices, lastClipboard = last) }
    }

    fun createPerson(name: String) = call { it.init(name) }

    /** The network came up or changed: start discovery again on it. */
    fun restartDiscovery() = call { it.restartDiscovery() }

    fun revoke(key: String) = call { it.revoke(key) }

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
    fun clipboardChanged(text: String): Int {
        val n = node ?: return 0
        n.clipboardChanged(text)
        refresh()
        return runCatching { n.status().clipboardPeers.size }.getOrDefault(0)
    }

    /** Text handed to the app (the share sheet): on the clipboard here and everywhere. */
    fun sendText(text: String): Int {
        setClipboard(text)
        return clipboardChanged(text)
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
     * Sends the content behind [uris] to [peer]. Opens each one and keeps it
     * open until the transfer's last event. Returns a message for the user.
     */
    fun sendFiles(context: Context, peer: String, uris: List<android.net.Uri>): String {
        val n = node ?: return context.getString(R.string.sent_to_none)
        val (files, handles) = Transfers.open(context, uris)
            ?: return context.getString(R.string.no_file_opened)
        return try {
            val transfer = n.sendFiles(peer, files)
            Transfers.keepOpen(transfer, handles)
            context.resources.getQuantityString(R.plurals.sending_files, files.size, files.size)
        } catch (e: LpException) {
            handles.forEach { runCatching { it.close() } }
            Log.w(TAG, "send failed", e)
            e.message ?: e.toString()
        }
    }

    fun cancelTransfer(transfer: String) {
        node?.cancelTransfer(transfer)
    }

    // Notifications (milestone 3).

    /** The listener connected or disconnected: the capability's `emits` follows. */
    fun setNotificationsEmitting(emitting: Boolean) {
        listenerConnected = emitting
        node?.setNotificationsEmitting(emitting)
        if (::appContext.isInitialized) refreshNotificationAccess(appContext)
    }

    fun notificationPosted(notification: LocalNotification) {
        node?.notificationPosted(notification)
    }

    fun notificationRemoved(id: String) {
        node?.notificationRemoved(id)
    }

    fun notificationCopyDismissed(origin: String, id: String) = call {
        it.notificationCopyDismissed(origin, id)
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

    /** Reads the primary clip if Android lets us (focused, or the background permission). */
    fun readClipboard(context: Context): String? {
        val manager = context.getSystemService(ClipboardManager::class.java)
        val clip = runCatching { manager.primaryClip }.getOrNull() ?: return null
        if (clip.itemCount == 0) return null
        return clip.getItemAt(0).coerceToText(context)?.toString()?.takeIf { it.isNotEmpty() }
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
