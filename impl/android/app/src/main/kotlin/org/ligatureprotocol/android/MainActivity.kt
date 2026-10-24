package org.ligatureprotocol.android

import android.Manifest
import android.content.ClipData
import android.content.ClipboardManager
import android.content.Intent
import android.content.pm.PackageManager
import android.net.Uri
import android.os.Build
import android.os.Bundle
import android.os.PowerManager
import android.provider.Settings
import android.widget.Toast
import androidx.activity.ComponentActivity
import androidx.activity.compose.setContent
import androidx.activity.enableEdgeToEdge
import androidx.activity.result.contract.ActivityResultContracts
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Surface
import androidx.compose.material3.darkColorScheme
import androidx.compose.material3.lightColorScheme
import androidx.compose.foundation.isSystemInDarkTheme
import androidx.compose.runtime.getValue
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import org.ligatureprotocol.android.ui.MainScreen
import org.ligatureprotocol.android.ui.TouchpadScreen

/** The one screen: status, create or pair, devices, send the clipboard. */
class MainActivity : ComponentActivity() {
    private val askNotifications =
        registerForActivityResult(ActivityResultContracts.RequestPermission()) {}

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        // The system bars follow the theme, light or dark; the screen pads
        // itself with safeDrawingPadding.
        enableEdgeToEdge()
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.TIRAMISU &&
            checkSelfPermission(Manifest.permission.POST_NOTIFICATIONS) != PackageManager.PERMISSION_GRANTED
        ) {
            askNotifications.launch(Manifest.permission.POST_NOTIFICATIONS)
        }
        NodeService.start(this)
        setContent {
            val dark = isSystemInDarkTheme()
            MaterialTheme(colorScheme = if (dark) darkColorScheme() else lightColorScheme()) {
                Surface {
                    val state by NodeHolder.state.collectAsStateWithLifecycle()
                    val touchpad = state.touchpad
                    if (touchpad != null) {
                        TouchpadScreen(
                            deviceName = touchpad.name,
                            onEvents = NodeHolder::touch,
                            onStop = NodeHolder::stopTouchpad,
                        )
                        return@Surface
                    }
                    MainScreen(
                        state = state,
                        onCreatePerson = NodeHolder::createPerson,
                        onStartJoin = NodeHolder::startJoin,
                        onStartIntroduce = NodeHolder::startIntroduce,
                        onConfirmPairing = NodeHolder::confirmPairing,
                        onAbortPairing = NodeHolder::abortPairing,
                        onDismissPairing = NodeHolder::dismissPairing,
                        onRevoke = NodeHolder::revoke,
                        onSendClipboard = ::sendClipboard,
                        onReceiveClipboard = ::receiveClipboard,
                        onCopy = ::copy,
                        onShare = ::share,
                        onBatterySettings = ::batterySettings,
                        batteryUnrestricted = batteryUnrestricted(),
                        onNotificationSettings = ::notificationSettings,
                        onToggleApp = NodeHolder::setAppEnabled,
                        onTouchpad = NodeHolder::startTouchpad,
                    )
                }
            }
        }
    }

    override fun onResume() {
        super.onResume()
        // Back from Settings: notification access may have changed.
        if (NodeHolder.isRunning) NodeHolder.refreshNotificationAccess(this)
    }

    override fun onWindowFocusChanged(hasFocus: Boolean) {
        super.onWindowFocusChanged(hasFocus)
        // With focus the app may read the clipboard: send what is there, in
        // case the user copied something and then opened the app.
        if (hasFocus && NodeHolder.isRunning) {
            NodeHolder.readAndSend(this)
        }
    }

    private fun sendClipboard() {
        NodeHolder.readAndSend(this) { n ->
            val message = when (n) {
                null -> getString(R.string.clipboard_empty)
                0 -> getString(R.string.sent_to_none)
                else -> resources.getQuantityString(R.plurals.sent_to, n, n)
            }
            runOnUiThread { Toast.makeText(this, message, Toast.LENGTH_SHORT).show() }
        }
    }

    private fun receiveClipboard() {
        NodeHolder.requestClipboard { n ->
            val message = if (n == 0) getString(R.string.sent_to_none)
            else resources.getQuantityString(R.plurals.asked_n, n, n)
            runOnUiThread { Toast.makeText(this, message, Toast.LENGTH_SHORT).show() }
        }
    }

    private fun copy(text: String) {
        getSystemService(ClipboardManager::class.java)
            .setPrimaryClip(ClipData.newPlainText("Ligature offer", text))
    }

    private fun share(text: String) {
        val send = Intent(Intent.ACTION_SEND).apply {
            type = "text/plain"
            putExtra(Intent.EXTRA_TEXT, text)
        }
        startActivity(Intent.createChooser(send, null))
    }

    private fun batteryUnrestricted(): Boolean =
        getSystemService(PowerManager::class.java).isIgnoringBatteryOptimizations(packageName)

    private fun notificationSettings() {
        startActivity(Intent(Settings.ACTION_NOTIFICATION_LISTENER_SETTINGS))
    }

    private fun batterySettings() {
        startActivity(
            Intent(Settings.ACTION_REQUEST_IGNORE_BATTERY_OPTIMIZATIONS, Uri.parse("package:$packageName")),
        )
    }
}
