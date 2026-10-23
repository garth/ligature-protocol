package org.ligatureprotocol.android

import android.content.ContentProvider
import android.content.ContentResolver
import android.content.ContentValues
import android.content.res.AssetFileDescriptor
import android.graphics.Bitmap
import android.graphics.Color
import android.os.Bundle
import android.os.CancellationSignal
import android.util.Log
import java.io.ByteArrayOutputStream
import android.database.Cursor
import android.database.MatrixCursor
import android.net.Uri
import android.os.ParcelFileDescriptor
import android.provider.OpenableColumns
import java.io.File
import java.io.FileOutputStream

/**
 * Serves the representations and files of a clipboard value a peer sent
 * (milestone 5, LP-109): each is a content URI under this provider whose last
 * segment is the node's key. Opening one asks the node for its bytes, which
 * fetches a promised part or file from the device that announced it on first
 * open, so pasting is what moves the bytes.
 */
class ClipboardProvider : ContentProvider() {
    override fun onCreate(): Boolean = true

    override fun getType(uri: Uri): String? = NodeHolder.clipItem(uri.lastPathSegment)?.mime

    override fun query(
        uri: Uri,
        projection: Array<String>?,
        selection: String?,
        selectionArgs: Array<String>?,
        sortOrder: String?,
    ): Cursor? {
        val item = NodeHolder.clipItem(uri.lastPathSegment) ?: return null
        val columns = projection ?: arrayOf(OpenableColumns.DISPLAY_NAME, OpenableColumns.SIZE)
        val cursor = MatrixCursor(columns)
        cursor.addRow(
            columns.map {
                when (it) {
                    OpenableColumns.DISPLAY_NAME -> item.name
                    OpenableColumns.SIZE -> item.size.toLong()
                    else -> null
                }
            },
        )
        return cursor
    }

    /**
     * A thumbnail request (the system's copy preview, a keyboard's clipboard
     * suggestions) carries [ContentResolver.EXTRA_SIZE]; it gets a placeholder
     * image rather than a fetch, so only a real read moves the bytes.
     */
    override fun openTypedAssetFile(
        uri: Uri,
        mimeTypeFilter: String,
        opts: Bundle?,
        signal: CancellationSignal?,
    ): AssetFileDescriptor? {
        if (opts?.containsKey(ContentResolver.EXTRA_SIZE) == true) {
            Log.i(TAG, "clipboard thumbnail for ${uri.lastPathSegment} asked by $callingPackage: placeholder")
            val png = placeholder()
            val pipe = ParcelFileDescriptor.createPipe()
            Thread {
                runCatching { FileOutputStream(pipe[1].fileDescriptor).use { it.write(png) } }
                runCatching { pipe[1].close() }
            }.start()
            return AssetFileDescriptor(pipe[0], 0, png.size.toLong())
        }
        return super.openTypedAssetFile(uri, mimeTypeFilter, opts, signal)
    }

    private fun placeholder(): ByteArray {
        val bitmap = Bitmap.createBitmap(96, 96, Bitmap.Config.ARGB_8888)
        bitmap.eraseColor(Color.rgb(0x3b, 0x3f, 0x58))
        val out = ByteArrayOutputStream()
        bitmap.compress(Bitmap.CompressFormat.PNG, 100, out)
        return out.toByteArray()
    }

    override fun openFile(uri: Uri, mode: String): ParcelFileDescriptor? = openFile(uri, mode, null)

    /**
     * A real read: the item's bytes, fetched from the announcing device if
     * they are promised. The fetch runs on its own thread so that a caller's
     * [CancellationSignal] ends the wait (the fetch itself finishes or times
     * out in the node); other node calls are not held up meanwhile.
     */
    override fun openFile(uri: Uri, mode: String, signal: CancellationSignal?): ParcelFileDescriptor? {
        if (mode != "r") return null
        val key = uri.lastPathSegment ?: return null
        Log.i(TAG, "clipboard item $key opened by $callingPackage")
        val fetch = java.util.concurrent.FutureTask { NodeHolder.clipboardItem(key) }
        Thread(fetch, "ligature-fetch").start()
        signal?.setOnCancelListener { fetch.cancel(true) }
        val data = try {
            fetch.get()
        } catch (_: java.util.concurrent.CancellationException) {
            Log.i(TAG, "clipboard item $key: read cancelled")
            throw android.os.OperationCanceledException()
        } catch (_: java.util.concurrent.ExecutionException) {
            null
        } ?: return null
        data.path?.let { return ParcelFileDescriptor.open(File(it), ParcelFileDescriptor.MODE_READ_ONLY) }
        val bytes = data.bytes ?: return null
        val pipe = ParcelFileDescriptor.createPipe()
        Thread {
            runCatching { FileOutputStream(pipe[1].fileDescriptor).use { it.write(bytes) } }
            runCatching { pipe[1].close() }
        }.start()
        return pipe[0]
    }

    override fun insert(uri: Uri, values: ContentValues?): Uri? = null
    override fun delete(uri: Uri, selection: String?, selectionArgs: Array<String>?): Int = 0
    override fun update(uri: Uri, values: ContentValues?, selection: String?, selectionArgs: Array<String>?): Int = 0

    companion object {
        private const val TAG = "ligature"
        const val AUTHORITY = "org.ligatureprotocol.android.clipboard"

        /** The content URI of the item under [key]. */
        fun uriFor(key: String): Uri = Uri.parse("content://$AUTHORITY/$key")
    }
}
