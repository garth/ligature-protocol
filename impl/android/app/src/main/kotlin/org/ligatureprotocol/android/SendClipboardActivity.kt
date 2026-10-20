package org.ligatureprotocol.android

import android.app.Activity
import android.os.Bundle
import android.widget.Toast

/**
 * Reads the clipboard the moment this transparent window has focus, which is
 * the one state in which Android 10 and later let an app that is not the
 * keyboard read it, hands the text to the node and goes away.
 */
class SendClipboardActivity : Activity() {
    private var done = false

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        if (!NodeHolder.isRunning) NodeService.start(this)
    }

    override fun onWindowFocusChanged(hasFocus: Boolean) {
        super.onWindowFocusChanged(hasFocus)
        if (!hasFocus || done) return
        done = true
        val text = NodeHolder.readClipboard(this)
        val message = when {
            text == null -> getString(R.string.clipboard_empty)
            else -> {
                val n = NodeHolder.clipboardChanged(text)
                if (n == 0) getString(R.string.sent_to_none) else getString(R.string.sent_to, n)
            }
        }
        Toast.makeText(this, message, Toast.LENGTH_SHORT).show()
        finish()
    }
}
