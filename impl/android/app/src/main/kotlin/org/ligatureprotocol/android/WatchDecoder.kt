package org.ligatureprotocol.android

import android.media.MediaCodec
import android.media.MediaFormat
import android.util.Log
import android.view.Surface

/**
 * Decodes the share this phone watches into a surface (milestone 8): the
 * node hands each whole access unit to [feed], the decoder renders to the
 * watch screen's surface. Until the decoder has a keyframe, deltas are
 * dropped and a keyframe is asked for once.
 */
object WatchDecoder {
    private const val TAG = "ligature"
    private var codec: MediaCodec? = null
    private var drain: Thread? = null
    @Volatile private var haveKey = false
    @Volatile private var asked = false
    private var startedAt = 0L

    /** The watch screen's surface exists: decode into it at the share's size. */
    @Synchronized
    fun configure(surface: Surface, width: Int, height: Int) {
        releaseLocked()
        val format = MediaFormat.createVideoFormat(MediaFormat.MIMETYPE_VIDEO_AVC, width, height).apply {
            setInteger(MediaFormat.KEY_LOW_LATENCY, 1)
        }
        val c = runCatching { MediaCodec.createDecoderByType(MediaFormat.MIMETYPE_VIDEO_AVC) }.getOrElse {
            Log.w(TAG, "no H.264 decoder", it)
            return
        }
        runCatching {
            c.configure(format, surface, null, 0)
            c.start()
        }.onFailure {
            Log.w(TAG, "the decoder would not start", it)
            c.release()
            return
        }
        codec = c
        haveKey = false
        asked = false
        startedAt = System.nanoTime()
        drain = Thread({ drain(c) }, "ligature-watch-decoder").apply { start() }
        NodeHolder.screenWatchKeyframe()
    }

    /** One whole access unit from the node. */
    @Synchronized
    fun feed(key: Boolean, bytes: ByteArray) {
        val c = codec ?: return
        if (!haveKey && !key) {
            if (!asked) {
                asked = true
                NodeHolder.screenWatchKeyframe()
            }
            return
        }
        val index = runCatching { c.dequeueInputBuffer(10_000) }.getOrDefault(-1)
        if (index < 0) {
            // The decoder is behind: what follows depends on this frame, so
            // wait for the next keyframe.
            haveKey = false
            asked = false
            return
        }
        val buffer = c.getInputBuffer(index) ?: return
        buffer.clear()
        if (bytes.size > buffer.capacity()) {
            c.queueInputBuffer(index, 0, 0, 0, 0)
            haveKey = false
            return
        }
        buffer.put(bytes)
        val pts = (System.nanoTime() - startedAt) / 1000
        c.queueInputBuffer(index, 0, bytes.size, pts, if (key) MediaCodec.BUFFER_FLAG_KEY_FRAME else 0)
        if (key) haveKey = true
    }

    private fun drain(c: MediaCodec) {
        val info = MediaCodec.BufferInfo()
        while (true) {
            val index = try {
                c.dequeueOutputBuffer(info, 100_000)
            } catch (e: IllegalStateException) {
                return
            }
            if (index >= 0) {
                runCatching { c.releaseOutputBuffer(index, true) }.onFailure { return }
            }
            if (codec !== c) return
        }
    }

    /** The surface or the watch is gone. */
    @Synchronized
    fun release() = releaseLocked()

    private fun releaseLocked() {
        val c = codec ?: return
        codec = null
        runCatching { c.stop() }
        runCatching { c.release() }
        runCatching { drain?.join(500) }
        drain = null
    }
}
