package org.ligatureprotocol.android

import android.app.Application
import android.app.NotificationChannel
import android.app.NotificationManager

/** Creates the notification channel the foreground service posts on. */
class LigatureApp : Application() {
    override fun onCreate() {
        super.onCreate()
        val manager = getSystemService(NotificationManager::class.java)
        manager.createNotificationChannel(
            NotificationChannel(
                NodeService.CHANNEL_ID,
                getString(R.string.notification_channel),
                NotificationManager.IMPORTANCE_LOW,
            ).apply {
                setShowBadge(false)
            },
        )
    }
}
