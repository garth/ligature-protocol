package org.ligatureprotocol.android

import android.media.AudioAttributes
import android.media.AudioFormat
import android.media.AudioTrack
import android.media.MediaCodec
import android.media.MediaFormat
import android.util.Log
import java.nio.ByteBuffer
import java.nio.ByteOrder

/**
 * Plays the sound of the share this phone watches (milestone 9): the node
 * hands each 20 ms Opus packet to [feed], `MediaCodec` decodes it and an
 * [AudioTrack] in its low-latency mode plays it as it comes, so the sound
 * stays close to the picture. Muted, packets are dropped before the decoder.
 *
 * The thread that plays is the one that lets the decoder and the track go,
 * once it has finished with them: `AudioTrack.write` reads the decoder's own
 * output buffer, and a decoder released under it from another thread leaves
 * the write copying from memory that is gone, which ends the app (LP-202).
 */
object WatchSound {
    private const val TAG = "ligature"
    const val SAMPLE_RATE = 48_000

    /** What an Opus decoder is told to decode and discard after a seek: 80 ms. */
    const val SEEK_PRE_ROLL_NS = 80_000_000L

    /** A share's sound being played: its decoder, its track, and whether it has been told to end. */
    private class Playing(val codec: MediaCodec, val track: AudioTrack) {
        @Volatile
        var ended = false
    }

    private var playing: Playing? = null
    private var startedAt = 0L

    @Volatile
    var muted = false

    /** The share has sound in [channels] channels: start the decoder and the track. */
    @Synchronized
    fun start(channels: Int) {
        releaseLocked()
        if (channels !in 1..2) return
        val format = MediaFormat.createAudioFormat(MediaFormat.MIMETYPE_AUDIO_OPUS, SAMPLE_RATE, channels).apply {
            setByteBuffer("csd-0", ByteBuffer.wrap(opusHead(channels)))
            setByteBuffer("csd-1", ByteBuffer.wrap(nanosLe(0)))
            setByteBuffer("csd-2", ByteBuffer.wrap(nanosLe(SEEK_PRE_ROLL_NS)))
        }
        val c = runCatching { MediaCodec.createDecoderByType(MediaFormat.MIMETYPE_AUDIO_OPUS) }.getOrElse {
            Log.w(TAG, "no Opus decoder; the share is silent here", it)
            return
        }
        val mask = if (channels == 1) AudioFormat.CHANNEL_OUT_MONO else AudioFormat.CHANNEL_OUT_STEREO
        val t = runCatching {
            c.configure(format, null, null, 0)
            c.start()
            val min = AudioTrack.getMinBufferSize(SAMPLE_RATE, mask, AudioFormat.ENCODING_PCM_16BIT)
            AudioTrack.Builder()
                .setAudioAttributes(
                    AudioAttributes.Builder()
                        .setUsage(AudioAttributes.USAGE_MEDIA)
                        .setContentType(AudioAttributes.CONTENT_TYPE_MOVIE)
                        .build(),
                )
                .setAudioFormat(
                    AudioFormat.Builder()
                        .setSampleRate(SAMPLE_RATE)
                        .setChannelMask(mask)
                        .setEncoding(AudioFormat.ENCODING_PCM_16BIT)
                        .build(),
                )
                .setBufferSizeInBytes(maxOf(min, bufferBytes(channels)))
                .setPerformanceMode(AudioTrack.PERFORMANCE_MODE_LOW_LATENCY)
                .setTransferMode(AudioTrack.MODE_STREAM)
                .build()
                .also { it.play() }
        }.getOrElse {
            Log.w(TAG, "the sound would not start; the share is silent here", it)
            runCatching { c.release() }
            return
        }
        val p = Playing(c, t)
        playing = p
        startedAt = System.nanoTime()
        Thread({ play(p) }, "ligature-watch-sound").start()
    }

    /** One Opus packet from the node. */
    @Synchronized
    fun feed(bytes: ByteArray) {
        val c = playing?.codec ?: return
        if (muted) return
        val index = runCatching { c.dequeueInputBuffer(5_000) }.getOrDefault(-1)
        // Behind: this packet is lost and the next one plays, which is
        // better for sound than playing late.
        if (index < 0) return
        val buffer = c.getInputBuffer(index) ?: return
        buffer.clear()
        if (bytes.size > buffer.capacity()) {
            c.queueInputBuffer(index, 0, 0, 0, 0)
            return
        }
        buffer.put(bytes)
        c.queueInputBuffer(index, 0, bytes.size, (System.nanoTime() - startedAt) / 1000, 0)
    }

    /** Plays what the decoder gives until told to end, then lets the decoder and the track go. */
    private fun play(p: Playing) {
        val (c, t) = p.codec to p.track
        val info = MediaCodec.BufferInfo()
        try {
            while (!p.ended) {
                val index = try {
                    c.dequeueOutputBuffer(info, 100_000)
                } catch (e: IllegalStateException) {
                    return
                }
                if (index < 0) continue
                val out = runCatching { c.getOutputBuffer(index) }.getOrNull()
                if (out != null && info.size > 0 && !p.ended) {
                    out.position(info.offset)
                    out.limit(info.offset + info.size)
                    runCatching { t.write(out, info.size, AudioTrack.WRITE_BLOCKING) }
                }
                runCatching { c.releaseOutputBuffer(index, false) }.onFailure { return }
            }
        } finally {
            // Nothing else uses either by now: [release] forgot them before
            // it told this thread to end.
            runCatching { c.stop() }
            runCatching { c.release() }
            runCatching { t.release() }
        }
    }

    /** The watch is over, or its sound is. */
    @Synchronized
    fun release() = releaseLocked()

    /**
     * Forgets the sound being played and tells its thread to end. Pausing
     * the track lets go of a write that waits on it; the thread releases the
     * decoder and the track itself.
     */
    private fun releaseLocked() {
        val p = playing
        playing = null
        muted = false
        if (p != null) {
            p.ended = true
            runCatching { p.track.pause() }
            runCatching { p.track.flush() }
        }
    }

    /**
     * The identification header an Opus decoder is configured with (RFC 7845
     * section 5.1): version 1, the channels, no pre-skip, 48 kHz, no gain,
     * the mono-or-stereo channel mapping.
     */
    fun opusHead(channels: Int): ByteArray = ByteBuffer.allocate(19).order(ByteOrder.LITTLE_ENDIAN).apply {
        put("OpusHead".toByteArray(Charsets.US_ASCII))
        put(1)
        put(channels.toByte())
        putShort(0)
        putInt(SAMPLE_RATE)
        putShort(0)
        put(0)
    }.array()

    /** A count of nanoseconds as `MediaCodec`'s Opus decoder reads it: 8 bytes, native order. */
    fun nanosLe(nanos: Long): ByteArray =
        ByteBuffer.allocate(8).order(ByteOrder.nativeOrder()).putLong(nanos).array()

    /** The track's buffer: 60 ms of 16-bit samples, the most the sound may lag. */
    fun bufferBytes(channels: Int): Int = SAMPLE_RATE * 60 / 1000 * channels * 2
}
