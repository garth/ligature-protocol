package org.ligatureprotocol.android

import android.app.Activity
import android.app.AlertDialog
import android.content.Intent
import android.net.Uri
import android.os.Build
import android.os.Bundle
import android.widget.Toast

/**
 * "Send to my devices" in the share sheet. Text goes to the clipboard of
 * every device, as before; files go to one device the user picks among those
 * that receive files (milestone 4).
 */
class ShareActivity : Activity() {
    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        if (!NodeHolder.isRunning) NodeService.start(this)
        val uris = streams()
        if (uris.isNotEmpty()) {
            pickDeviceAndSend(uris)
            return
        }
        val text = if (intent?.action == Intent.ACTION_SEND) {
            intent.getStringExtra(Intent.EXTRA_TEXT)?.takeIf { it.isNotEmpty() }
        } else {
            null
        }
        val message = when (text) {
            null -> getString(R.string.clipboard_empty)
            else -> {
                val n = NodeHolder.sendText(text)
                if (n == 0) getString(R.string.sent_to_none) else getString(R.string.sent_to, n)
            }
        }
        Toast.makeText(this, message, Toast.LENGTH_SHORT).show()
        finish()
    }

    @Suppress("DEPRECATION")
    private fun streams(): List<Uri> {
        val intent = intent ?: return emptyList()
        return when (intent.action) {
            Intent.ACTION_SEND -> listOfNotNull(
                if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.TIRAMISU) {
                    intent.getParcelableExtra(Intent.EXTRA_STREAM, Uri::class.java)
                } else {
                    intent.getParcelableExtra(Intent.EXTRA_STREAM)
                },
            )
            Intent.ACTION_SEND_MULTIPLE -> (
                if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.TIRAMISU) {
                    intent.getParcelableArrayListExtra(Intent.EXTRA_STREAM, Uri::class.java)
                } else {
                    intent.getParcelableArrayListExtra(Intent.EXTRA_STREAM)
                }
                ).orEmpty()
            else -> emptyList()
        }
    }

    private fun pickDeviceAndSend(uris: List<Uri>) {
        val targets = NodeHolder.fileTargets()
        if (targets.isEmpty()) {
            Toast.makeText(this, getString(R.string.no_file_target), Toast.LENGTH_LONG).show()
            finish()
            return
        }
        AlertDialog.Builder(this)
            .setTitle(resources.getQuantityString(R.plurals.send_files_title, uris.size, uris.size))
            .setItems(targets.map { it.second }.toTypedArray()) { _, which ->
                val result = NodeHolder.sendFiles(this, targets[which].first, uris)
                Toast.makeText(this, result, Toast.LENGTH_SHORT).show()
                finish()
            }
            .setOnCancelListener { finish() }
            .show()
    }
}
