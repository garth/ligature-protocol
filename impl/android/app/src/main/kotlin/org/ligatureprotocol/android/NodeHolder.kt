package org.ligatureprotocol.android

import android.content.ClipData
import android.content.ClipDescription
import android.content.ClipboardManager
import android.content.Context
import android.os.Build
import android.os.PersistableBundle
import android.provider.Settings
import android.util.Log
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.update
import org.ligatureprotocol.core.Config
import org.ligatureprotocol.core.Device
import org.ligatureprotocol.core.Event
import org.ligatureprotocol.core.Host
import org.ligatureprotocol.core.LogLevel
import org.ligatureprotocol.core.LpException
import org.ligatureprotocol.core.Node
import org.ligatureprotocol.core.Pairing
import org.ligatureprotocol.core.Status
import java.io.File

/** A ceremony as the screen shows it. */
data class PairingState(
    val offer: String? = null,
    val code: String? = null,
    val finished: Boolean = false,
    val error: String? = null,
)

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
        _state.update {
            it.copy(
                running = true,
                backgroundRead = hasBackgroundRead(appContext),
                lastError = null,
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
