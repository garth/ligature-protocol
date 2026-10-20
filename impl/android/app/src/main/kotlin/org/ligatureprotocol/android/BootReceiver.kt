package org.ligatureprotocol.android

import android.content.BroadcastReceiver
import android.content.Context
import android.content.Intent

/** Starts the node when the phone boots, so gossip and the clipboard work without opening the app. */
class BootReceiver : BroadcastReceiver() {
    override fun onReceive(context: Context, intent: Intent) {
        if (intent.action == Intent.ACTION_BOOT_COMPLETED) {
            NodeService.start(context)
        }
    }
}
