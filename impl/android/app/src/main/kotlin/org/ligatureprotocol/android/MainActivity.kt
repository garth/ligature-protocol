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
import androidx.activity.result.contract.ActivityResultContracts
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Surface
import androidx.compose.material3.darkColorScheme
import androidx.compose.material3.lightColorScheme
import androidx.compose.foundation.isSystemInDarkTheme
import androidx.compose.runtime.getValue
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import org.ligatureprotocol.android.ui.MainScreen

/** The one screen: status, create or pair, devices, send the clipboard. */
class MainActivity : ComponentActivity() {
    private val askNotifications =
        registerForActivityResult(ActivityResultContracts.RequestPermission()) {}

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
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
                        onCopy = ::copy,
                        onShare = ::share,
                        onBatterySettings = ::batterySettings,
                        batteryUnrestricted = batteryUnrestricted(),
                    )
                }
            }
        }
    }

    override fun onWindowFocusChanged(hasFocus: Boolean) {
        super.onWindowFocusChanged(hasFocus)
        // With focus the app may read the clipboard: send what is there, in
        // case the user copied something and then opened the app.
        if (hasFocus && NodeHolder.isRunning) {
            NodeHolder.readClipboard(this)?.let { NodeHolder.clipboardChanged(it) }
        }
    }

    private fun sendClipboard() {
        val text = NodeHolder.readClipboard(this)
        val message = when (text) {
            null -> getString(R.string.clipboard_empty)
            else -> {
                val n = NodeHolder.clipboardChanged(text)
                if (n == 0) getString(R.string.sent_to_none) else getString(R.string.sent_to, n)
            }
        }
        Toast.makeText(this, message, Toast.LENGTH_SHORT).show()
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

    private fun batterySettings() {
        startActivity(
            Intent(Settings.ACTION_REQUEST_IGNORE_BATTERY_OPTIMIZATIONS, Uri.parse("package:$packageName")),
        )
    }
}
