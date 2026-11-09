package org.ligatureprotocol.android

import android.app.Notification
import android.app.NotificationManager
import android.app.PendingIntent
import android.content.BroadcastReceiver
import android.content.Context
import android.content.Intent
import org.ligatureprotocol.core.ScreenOffer

/**
 * A cast another of the person's devices offers this phone (milestone 9).
 * With the app in front the watch screen opens at once; with the
 * accessibility service on, Android lets the app open itself from the
 * background, so the screen wakes and the cast shows; otherwise a
 * notification says who is casting, a tap opens it and a swipe declines. The
 * node gives an offer a minute.
 */
object CastOffers {
    private const val NOTIFICATION = 7003
    private const val WAKE = "cast"

    /** How long an offer stays answerable: the node gives up after 60 s. */
    const val LIFETIME_MS = 55_000L
    const val EXTRA_ACCEPT = "org.ligatureprotocol.android.ACCEPT_CAST"
    private const val EXTRA_PEER = "peer"

    /** What the app does with an offer. */
    enum class Show {
        /** Open the watch screen now. */
        OPEN,

        /** Tell the user and wait for a tap. */
        NOTIFY,
    }

    /** An offer and when it came. */
    data class Pending(val offer: ScreenOffer, val at: Long)

    @Volatile
    private var pending: Pending? = null

    /** Whether an offer opens at once: the app is in front, or may put itself there. */
    fun decide(foreground: Boolean, accessibilityOn: Boolean): Show =
        if (foreground || accessibilityOn) Show.OPEN else Show.NOTIFY

    /** The notification's title. */
    fun title(offer: ScreenOffer): String = "${offer.peerName} is casting to this phone"

    /** The notification's text: what is cast, and whether sound comes. */
    fun text(offer: ScreenOffer): String {
        val what = if (offer.kind == "window") "A window (${offer.name})" else "Its screen"
        val sound = if (offer.audio) ", with sound" else ""
        return "$what$sound. Tap to watch, swipe away to decline."
    }

    /** The node says [offer] waits. */
    fun offered(context: Context, offer: ScreenOffer) {
        pending = Pending(offer, System.currentTimeMillis())
        when (decide(ScreenShare.foreground, DriveService.isOn)) {
            Show.OPEN -> if (ScreenShare.foreground) {
                NodeHolder.acceptOffer(offer.peer)
            } else {
                ScreenWake.hold(context, WAKE)
                runCatching { context.startActivity(openIntent(context, offer.peer)) }
                    .onFailure { notify(context, offer) }
            }
            Show.NOTIFY -> notify(context, offer)
        }
    }

    /** The offer from [peer], if it is still answerable; it is answered once. */
    fun take(context: Context, peer: String): ScreenOffer? {
        ScreenWake.release(WAKE)
        context.getSystemService(NotificationManager::class.java).cancel(NOTIFICATION)
        val p = pending ?: return null
        if (p.offer.peer != peer) return null
        pending = null
        return p.offer.takeIf { System.currentTimeMillis() - p.at < LIFETIME_MS }
    }

    private fun openIntent(context: Context, peer: String): Intent =
        Intent(context, MainActivity::class.java)
            .addFlags(Intent.FLAG_ACTIVITY_NEW_TASK or Intent.FLAG_ACTIVITY_SINGLE_TOP)
            .putExtra(EXTRA_ACCEPT, peer)

    private fun notify(context: Context, offer: ScreenOffer) {
        ScreenShare.createChannel(context)
        val tap = PendingIntent.getActivity(
            context,
            NOTIFICATION,
            openIntent(context, offer.peer),
            PendingIntent.FLAG_IMMUTABLE or PendingIntent.FLAG_UPDATE_CURRENT,
        )
        val decline = PendingIntent.getBroadcast(
            context,
            NOTIFICATION,
            Intent(context, DeclineReceiver::class.java).putExtra(EXTRA_PEER, offer.peer),
            PendingIntent.FLAG_IMMUTABLE or PendingIntent.FLAG_UPDATE_CURRENT,
        )
        val notification = Notification.Builder(context, ScreenShare.CHANNEL_ID)
            .setSmallIcon(R.drawable.ic_tile)
            .setContentTitle(title(offer))
            .setContentText(text(offer))
            .setContentIntent(tap)
            .setDeleteIntent(decline)
            .setAutoCancel(true)
            .setTimeoutAfter(LIFETIME_MS)
            .build()
        context.getSystemService(NotificationManager::class.java).notify(NOTIFICATION, notification)
    }

    /** The notification was swiped away: the cast is declined. */
    class DeclineReceiver : BroadcastReceiver() {
        override fun onReceive(context: Context, intent: Intent) {
            val peer = intent.getStringExtra(EXTRA_PEER) ?: return
            if (take(context, peer) != null) NodeHolder.declineOffer(peer)
        }
    }
}
