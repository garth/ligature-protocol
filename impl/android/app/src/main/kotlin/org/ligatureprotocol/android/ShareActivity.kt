package org.ligatureprotocol.android

import android.app.Activity
import android.content.Intent
import android.os.Bundle
import android.widget.Toast

/** "Send to my devices" in the share sheet: the text comes in the intent. */
class ShareActivity : Activity() {
    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        if (!NodeHolder.isRunning) NodeService.start(this)
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
}
