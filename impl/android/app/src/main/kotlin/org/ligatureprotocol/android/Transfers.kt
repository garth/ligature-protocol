package org.ligatureprotocol.android

import android.app.Notification
import android.app.NotificationChannel
import android.app.NotificationManager
import android.app.PendingIntent
import android.content.BroadcastReceiver
import android.content.ContentResolver
import android.content.ContentValues
import android.content.Context
import android.content.Intent
import android.net.Uri
import android.os.Environment
import android.os.ParcelFileDescriptor
import android.provider.DocumentsContract
import android.provider.MediaStore
import android.provider.OpenableColumns
import android.util.Log
import org.ligatureprotocol.core.DownloadTarget
import org.ligatureprotocol.core.OutgoingFile
import org.ligatureprotocol.core.PublishedDownload
import org.ligatureprotocol.core.TransferInfo
import java.io.File

/**
 * What Android keeps from the library for file transfers (milestone 4,
 * LP-103): received files are rows in MediaStore's Downloads, pending while
 * the node writes through a descriptor opened here and published when the
 * node has verified them; shared content is opened here and read by the node
 * as `/proc/self/fd/N`; and every transfer is a notification with a progress
 * bar and a cancel action, replaced by one that opens the file when it lands.
 */
object Transfers {
    private const val TAG = "ligature"
    const val CHANNEL_ID = "transfers"
    private const val ACTION_CANCEL = "org.ligatureprotocol.android.CANCEL_TRANSFER"
    private const val EXTRA_TRANSFER = "transfer"

    private class Pending(val uri: Uri, val descriptor: ParcelFileDescriptor)

    private val pending = HashMap<ULong, Pending>()
    private var nextId = 1uL

    /** What was opened for each outgoing transfer, closed when it ends. */
    private val outgoing = HashMap<String, List<AutoCloseable>>()

    fun createChannel(context: Context) {
        context.getSystemService(NotificationManager::class.java).createNotificationChannel(
            NotificationChannel(
                CHANNEL_ID,
                context.getString(R.string.transfers_channel),
                NotificationManager.IMPORTANCE_LOW,
            ),
        )
    }

    // Receiving: Host.createDownload, finishDownload, abortDownload.

    @Synchronized
    fun createDownload(context: Context, name: String, mime: String, size: ULong): DownloadTarget? = runCatching {
        val resolver = context.contentResolver
        val values = ContentValues().apply {
            put(MediaStore.Downloads.DISPLAY_NAME, freeName(resolver, name))
            put(MediaStore.Downloads.MIME_TYPE, mime.ifBlank { "application/octet-stream" })
            put(MediaStore.Downloads.RELATIVE_PATH, Environment.DIRECTORY_DOWNLOADS)
            put(MediaStore.Downloads.IS_PENDING, 1)
            if (size > 0uL) put(MediaStore.Downloads.SIZE, size.toLong())
        }
        val uri = resolver.insert(MediaStore.Downloads.EXTERNAL_CONTENT_URI, values) ?: return null
        val descriptor = resolver.openFileDescriptor(uri, "w") ?: run {
            resolver.delete(uri, null, null)
            return null
        }
        val id = nextId++
        pending[id] = Pending(uri, descriptor)
        DownloadTarget(id, "/proc/self/fd/${descriptor.fd}")
    }.onFailure { Log.w(TAG, "cannot create a download", it) }.getOrNull()

    /**
     * The name rule of the plan (`04-files.md`, decision 9): `name`, or
     * `stem (2).ext`, `stem (3).ext`… past the names already in Downloads.
     * The query sees the rows this app may see, which under scoped storage
     * are its own; a clash with another app's file is still MediaStore's to
     * resolve, with its own numbering.
     */
    private fun freeName(resolver: ContentResolver, name: String): String {
        val stem = splitName(name).first
        val taken = HashSet<String>()
        runCatching {
            resolver.query(
                MediaStore.Downloads.EXTERNAL_CONTENT_URI,
                arrayOf(MediaStore.Downloads.DISPLAY_NAME),
                "${MediaStore.Downloads.RELATIVE_PATH} = ? AND ${MediaStore.Downloads.DISPLAY_NAME} LIKE ?",
                arrayOf("${Environment.DIRECTORY_DOWNLOADS}/", "$stem%"),
                null,
            )?.use { while (it.moveToNext()) it.getString(0)?.let(taken::add) }
        }.onFailure { Log.w(TAG, "cannot list Downloads for the name rule", it) }
        return freeName(name, taken)
    }

    /** A name's stem and its extension with the dot; a leading dot is no extension. */
    internal fun splitName(name: String): Pair<String, String> {
        val dot = name.lastIndexOf('.')
        return if (dot > 0) name.substring(0, dot) to name.substring(dot) else name to ""
    }

    /** `name`, or `stem (2).ext`, `stem (3).ext`… the first not in `taken`. */
    internal fun freeName(name: String, taken: Set<String>): String {
        if (name !in taken) return name
        val (stem, ext) = splitName(name)
        var n = 2
        while ("$stem ($n)$ext" in taken) n++
        return "$stem ($n)$ext"
    }

    @Synchronized
    fun finishDownload(context: Context, id: ULong): PublishedDownload? {
        val entry = pending.remove(id) ?: return null
        return runCatching {
            entry.descriptor.close()
            val resolver = context.contentResolver
            resolver.update(
                entry.uri,
                ContentValues().apply { put(MediaStore.Downloads.IS_PENDING, 0) },
                null,
                null,
            )
            var name = entry.uri.lastPathSegment ?: "file"
            resolver.query(entry.uri, arrayOf(OpenableColumns.DISPLAY_NAME), null, null, null)?.use {
                if (it.moveToFirst()) name = it.getString(0) ?: name
            }
            PublishedDownload(name, entry.uri.toString())
        }.onFailure { Log.w(TAG, "cannot publish a download", it) }.getOrNull()
    }

    @Synchronized
    fun abortDownload(context: Context, id: ULong) {
        val entry = pending.remove(id) ?: return
        runCatching { entry.descriptor.close() }
        runCatching { context.contentResolver.delete(entry.uri, null, null) }
            .onFailure { Log.w(TAG, "cannot remove an aborted download", it) }
    }

    // Sending: content the share sheet handed us.

    /**
     * Opens each URI for the node to read. Only a `content:` URI of another
     * app's provider is opened: a share intent or a clip can carry any URI,
     * and `file:` paths or this app's own provider would read with this
     * app's rights, its key file included (LP-127). A regular file is read
     * through its descriptor; content of unknown size is copied to the cache
     * first, since the descriptor says the exact size. Returns the files and
     * what to close when the transfer ends, or null if nothing could be
     * opened.
     */
    fun open(context: Context, uris: List<Uri>): Pair<List<OutgoingFile>, List<AutoCloseable>>? {
        val resolver = context.contentResolver
        val files = ArrayList<OutgoingFile>()
        val handles = ArrayList<AutoCloseable>()
        for (uri in uris) {
            if (!isShareable(uri)) {
                Log.w(TAG, "refusing to open $uri: not another app's content")
                continue
            }
            val name = displayName(context, uri)
            val mime = resolver.getType(uri) ?: ""
            val modified = modifiedAt(context, uri)
            val descriptor = runCatching { resolver.openFileDescriptor(uri, "r") }.getOrNull() ?: continue
            val size = descriptor.statSize
            if (size >= 0) {
                handles += descriptor
                files += OutgoingFile("/proc/self/fd/${descriptor.fd}", name, size.toULong(), mime, modified)
            } else {
                // A pipe or a provider that cannot say: copy once to learn the size.
                val copy = File(context.cacheDir, "share").apply { mkdirs() }
                    .resolve("${System.nanoTime()}-$name")
                val copied = runCatching {
                    ParcelFileDescriptor.AutoCloseInputStream(descriptor).use { input ->
                        copy.outputStream().use { input.copyTo(it) }
                    }
                }.isSuccess
                if (!copied) {
                    copy.delete()
                    continue
                }
                handles += AutoCloseable { copy.delete() }
                files += OutgoingFile(copy.absolutePath, name, copy.length().toULong(), mime, modified)
            }
        }
        if (files.isEmpty()) {
            handles.forEach { runCatching { it.close() } }
            return null
        }
        return files to handles
    }

    /** Whether [uri] is content another app serves, as opposed to a path or our own provider. */
    fun isShareable(uri: Uri): Boolean =
        uri.scheme == "content" &&
            !uri.authority.isNullOrEmpty() &&
            uri.authority != ClipboardProvider.AUTHORITY

    /**
     * The provider's modification time in Unix seconds: MediaStore's
     * `date_modified` (seconds) or a documents provider's `last_modified`
     * (milliseconds), whichever the provider has.
     */
    private fun modifiedAt(context: Context, uri: Uri): Long? {
        fun column(name: String, divisor: Long): Long? = runCatching {
            context.contentResolver.query(uri, arrayOf(name), null, null, null)?.use {
                if (it.moveToFirst() && !it.isNull(0)) (it.getLong(0) / divisor).takeIf { t -> t > 0 } else null
            }
        }.getOrNull()
        return column(MediaStore.MediaColumns.DATE_MODIFIED, 1)
            ?: column(DocumentsContract.Document.COLUMN_LAST_MODIFIED, 1000)
    }

    private fun displayName(context: Context, uri: Uri): String {
        runCatching {
            context.contentResolver.query(uri, arrayOf(OpenableColumns.DISPLAY_NAME), null, null, null)?.use {
                if (it.moveToFirst()) return it.getString(0)?.takeIf { n -> n.isNotBlank() } ?: "file"
            }
        }
        return uri.lastPathSegment?.substringAfterLast('/')?.takeIf { it.isNotBlank() } ?: "file"
    }

    @Synchronized
    fun keepOpen(transfer: String, handles: List<AutoCloseable>) {
        outgoing[transfer] = handles
    }

    @Synchronized
    private fun release(transfer: String) {
        outgoing.remove(transfer)?.forEach { runCatching { it.close() } }
    }

    // Progress: Host.onEvent(TransferChanged).

    fun changed(context: Context, info: TransferInfo) {
        val manager = context.getSystemService(NotificationManager::class.java)
        val notificationId = info.transfer.hashCode()
        val direction = if (info.outgoing) context.getString(R.string.transfer_to, info.peerName)
        else context.getString(R.string.transfer_from, info.peerName)
        val what = if (info.filesTotal == 1u) info.firstName
        else context.resources.getQuantityString(R.plurals.transfer_files, info.filesTotal.toInt(), info.filesTotal.toInt())
        val outcome = info.outcome
        if (outcome == null) {
            val cancel = PendingIntent.getBroadcast(
                context,
                notificationId,
                Intent(context, TransferReceiver::class.java)
                    .setAction(ACTION_CANCEL)
                    .putExtra(EXTRA_TRANSFER, info.transfer),
                PendingIntent.FLAG_IMMUTABLE or PendingIntent.FLAG_UPDATE_CURRENT,
            )
            val total = info.totalBytes.toLong()
            val done = info.doneBytes.toLong()
            val builder = Notification.Builder(context, CHANNEL_ID)
                .setSmallIcon(R.drawable.ic_tile)
                .setContentTitle("$what · $direction")
                .setContentText(context.getString(R.string.transfer_progress, human(done), human(total)))
                .setOngoing(true)
                .setOnlyAlertOnce(true)
                .addAction(Notification.Action.Builder(null, context.getString(R.string.transfer_cancel), cancel).build())
            if (total > 0) {
                builder.setProgress(1000, (done * 1000 / total).toInt().coerceIn(0, 1000), false)
            } else {
                builder.setProgress(0, 0, true)
            }
            manager.notify(notificationId, builder.build())
            return
        }
        release(info.transfer)
        val builder = Notification.Builder(context, CHANNEL_ID)
            .setSmallIcon(R.drawable.ic_tile)
            .setAutoCancel(true)
        when {
            info.allReceived && !info.outgoing -> {
                builder.setContentTitle(context.getString(R.string.transfer_received, what))
                    .setContentText(direction)
                info.locations.firstOrNull()?.let { location ->
                    val view = Intent(Intent.ACTION_VIEW)
                        .setData(Uri.parse(location))
                        .addFlags(Intent.FLAG_GRANT_READ_URI_PERMISSION or Intent.FLAG_ACTIVITY_NEW_TASK)
                    builder.setContentIntent(
                        PendingIntent.getActivity(
                            context,
                            notificationId,
                            view,
                            PendingIntent.FLAG_IMMUTABLE or PendingIntent.FLAG_UPDATE_CURRENT,
                        ),
                    )
                }
            }
            info.allReceived -> builder.setContentTitle(context.getString(R.string.transfer_sent, what))
                .setContentText(direction)
            outcome == "cancelled" -> builder.setContentTitle(context.getString(R.string.transfer_cancelled, what))
                .setContentText(direction)
            else -> builder.setContentTitle(context.getString(R.string.transfer_failed, what))
                .setContentText("$direction · $outcome")
        }
        manager.notify(notificationId, builder.build())
    }

    private fun human(bytes: Long): String {
        if (bytes < 1024) return "$bytes B"
        val units = arrayOf("KiB", "MiB", "GiB", "TiB")
        var value = bytes.toDouble()
        var unit = -1
        while (value >= 1024.0 && unit < units.size - 1) {
            value /= 1024.0
            unit++
        }
        return String.format(java.util.Locale.ROOT, "%.1f %s", value, units[unit])
    }

    /** The notification's cancel action. */
    class TransferReceiver : BroadcastReceiver() {
        override fun onReceive(context: Context, intent: Intent) {
            if (intent.action != ACTION_CANCEL) return
            val transfer = intent.getStringExtra(EXTRA_TRANSFER) ?: return
            NodeHolder.cancelTransfer(transfer)
        }
    }
}
