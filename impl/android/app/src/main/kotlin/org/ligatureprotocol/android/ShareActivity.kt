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
 * every device once the user has seen it and confirmed, since any app can
 * send a share intent; files go to one device the user picks among those
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
        if (text == null) {
            Toast.makeText(this, getString(R.string.clipboard_empty), Toast.LENGTH_SHORT).show()
            finish()
            return
        }
        confirmAndSendText(text)
    }

    private fun confirmAndSendText(text: String) {
        val preview = if (text.length > PREVIEW_CHARS) text.take(PREVIEW_CHARS) + "…" else text
        AlertDialog.Builder(this)
            .setTitle(R.string.share_text_title)
            .setMessage(preview)
            .setPositiveButton(R.string.share_text_send) { _, _ ->
                NodeHolder.sendText(text) { n ->
                    val message = if (n == 0) getString(R.string.sent_to_none)
                    else resources.getQuantityString(R.plurals.sent_to, n, n)
                    runOnUiThread {
                        Toast.makeText(applicationContext, message, Toast.LENGTH_SHORT).show()
                        finish()
                    }
                }
            }
            .setNegativeButton(android.R.string.cancel) { _, _ -> finish() }
            .setOnCancelListener { finish() }
            .show()
    }

    private companion object {
        const val PREVIEW_CHARS = 400
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
                NodeHolder.sendFiles(this, targets[which].first, uris) { result ->
                    runOnUiThread {
                        Toast.makeText(applicationContext, result, Toast.LENGTH_SHORT).show()
                        finish()
                    }
                }
            }
            .setOnCancelListener { finish() }
            .show()
    }
}
