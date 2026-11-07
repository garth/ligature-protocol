package org.ligatureprotocol.android

import android.content.BroadcastReceiver
import android.content.Context
import android.content.Intent

/**
 * Starts the node when the phone boots and when the app is updated, which
 * kills the service, so nothing ever needs the app opened to bring it back.
 */
class BootReceiver : BroadcastReceiver() {
    override fun onReceive(context: Context, intent: Intent) {
        if (intent.action == Intent.ACTION_BOOT_COMPLETED ||
            intent.action == Intent.ACTION_MY_PACKAGE_REPLACED
        ) {
            NodeService.start(context)
        }
    }
}
