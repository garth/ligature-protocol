package org.ligatureprotocol.android

import android.app.Notification
import android.content.Context
import android.graphics.Bitmap
import android.graphics.drawable.BitmapDrawable
import android.service.notification.NotificationListenerService
import android.service.notification.StatusBarNotification
import android.app.NotificationManager
import androidx.core.app.NotificationManagerCompat
import androidx.core.graphics.drawable.toBitmap
import org.ligatureprotocol.core.LocalNotification
import java.io.ByteArrayOutputStream

/**
 * Sees what the phone's apps post, once the user grants notification access,
 * and hands it to the node; sees the phone's own notifications go away and
 * the user dismiss the copies the node asked us to show. Ongoing, silent and
 * summary notifications, Ligature's own, and excluded apps never leave the
 * phone (milestone 3, LP-92).
 */
class NotificationListener : NotificationListenerService() {
    private val iconCache = HashMap<String, ByteArray?>()

    override fun onListenerConnected() {
        instance = this
        NodeHolder.setNotificationsEmitting(true)
    }

    override fun onListenerDisconnected() {
        instance = null
        NodeHolder.setNotificationsEmitting(false)
    }

    override fun onNotificationPosted(sbn: StatusBarNotification, rankingMap: RankingMap) {
        if (sbn.packageName == packageName) return
        val n = sbn.notification
        if (sbn.isOngoing ||
            n.flags and Notification.FLAG_GROUP_SUMMARY != 0 ||
            n.flags and Notification.FLAG_FOREGROUND_SERVICE != 0
        ) {
            return
        }
        val ranking = Ranking()
        if (rankingMap.getRanking(sbn.key, ranking) &&
            ranking.importance < NotificationManager.IMPORTANCE_DEFAULT
        ) {
            return
        }
        val extras = n.extras
        val title = extras.getCharSequence(Notification.EXTRA_TITLE)?.toString()?.takeIf { it.isNotBlank() }
        val body = (
            extras.getCharSequence(Notification.EXTRA_BIG_TEXT)
                ?: extras.getCharSequence(Notification.EXTRA_TEXT)
            )?.toString()?.takeIf { it.isNotBlank() }
        if (title == null && body == null) return
        NodeHolder.appSeen(this, sbn.packageName, appLabel(sbn.packageName))
        if (NodeHolder.isExcluded(this, sbn.packageName)) return
        NodeHolder.notificationPosted(
            LocalNotification(
                id = sbn.key,
                appId = sbn.packageName,
                appName = appLabel(sbn.packageName),
                title = title,
                body = body,
                postedAtUnix = sbn.postTime / 1000,
                iconPng = appIconPng(sbn.packageName),
            ),
        )
    }

    override fun onNotificationRemoved(sbn: StatusBarNotification, rankingMap: RankingMap, reason: Int) {
        if (sbn.packageName == packageName) {
            // One of our copies: tell the origin only if the user did it.
            val origin = sbn.notification.extras.getString(EXTRA_ORIGIN) ?: return
            val id = sbn.notification.extras.getString(EXTRA_ID) ?: return
            if (reason == REASON_CANCEL || reason == REASON_CANCEL_ALL || reason == REASON_CLICK) {
                NodeHolder.notificationCopyDismissed(origin, id)
            }
            return
        }
        // An app's own notification is gone by the user's or the app's hand;
        // a cancel we made on another device's behalf (REASON_LISTENER_CANCEL)
        // is not sent back, and a timeout clears nothing elsewhere.
        when (reason) {
            REASON_CLICK, REASON_CANCEL, REASON_CANCEL_ALL, REASON_APP_CANCEL, REASON_APP_CANCEL_ALL ->
                NodeHolder.notificationRemoved(sbn.key)
        }
    }

    private fun appLabel(packageName: String): String = runCatching {
        packageManager.getApplicationLabel(packageManager.getApplicationInfo(packageName, 0)).toString()
    }.getOrDefault(packageName)

    /** The app's launcher icon as a small PNG, cached per app; null if it cannot be rendered. */
    private fun appIconPng(packageName: String): ByteArray? = iconCache.getOrPut(packageName) {
        runCatching {
            val drawable = packageManager.getApplicationIcon(packageName)
            val bitmap = (drawable as? BitmapDrawable)?.bitmap ?: drawable.toBitmap(ICON_PX, ICON_PX)
            val scaled = if (bitmap.width > ICON_PX) Bitmap.createScaledBitmap(bitmap, ICON_PX, ICON_PX, true) else bitmap
            val out = ByteArrayOutputStream()
            scaled.compress(Bitmap.CompressFormat.PNG, 100, out)
            out.toByteArray().takeIf { it.size <= MAX_ICON_BYTES }
        }.getOrNull()
    }

    companion object {
        /** Extras on the copies we post, so a removal names the origin and id. */
        const val EXTRA_ORIGIN = "org.ligatureprotocol.origin"
        const val EXTRA_ID = "org.ligatureprotocol.id"
        private const val ICON_PX = 96
        private const val MAX_ICON_BYTES = 65536

        @Volatile
        private var instance: NotificationListener? = null

        /** Whether the user has granted this app notification access. */
        fun hasAccess(context: Context): Boolean =
            NotificationManagerCompat.getEnabledListenerPackages(context).contains(context.packageName)

        /** Cancels one of the phone's own notifications, for a dismissal from another device. */
        fun cancelOwn(key: String): Boolean {
            val listener = instance ?: return false
            return runCatching { listener.cancelNotification(key) }.isSuccess
        }
    }
}
