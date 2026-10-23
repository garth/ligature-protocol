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
        NodeHolder.readAndSend(this) { n ->
            val message = when (n) {
                null -> getString(R.string.clipboard_empty)
                0 -> getString(R.string.sent_to_none)
                else -> resources.getQuantityString(R.plurals.sent_to, n, n)
            }
            runOnUiThread {
                Toast.makeText(applicationContext, message, Toast.LENGTH_SHORT).show()
                finish()
            }
        }
    }
}
