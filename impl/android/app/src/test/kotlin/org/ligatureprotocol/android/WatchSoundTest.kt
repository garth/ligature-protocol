package org.ligatureprotocol.android

import org.junit.Assert.assertArrayEquals
import org.junit.Assert.assertEquals
import org.junit.Test

class WatchSoundTest {
    /** RFC 7845 section 5.1, as the Opus decoder is configured with it. */
    @Test
    fun theOpusHeaderNamesTheChannelsAt48KilohertzWithNoPreSkipOrGain() {
        val head = WatchSound.opusHead(2)
        assertEquals(19, head.size)
        assertEquals("OpusHead", String(head, 0, 8, Charsets.US_ASCII))
        assertArrayEquals(
            byteArrayOf(1, 2, 0, 0, 0x80.toByte(), 0xBB.toByte(), 0, 0, 0, 0, 0),
            head.copyOfRange(8, 19),
        )
        assertEquals(1, WatchSound.opusHead(1)[9].toInt())
    }

    @Test
    fun aNanosecondCountIsEightBytes() {
        assertArrayEquals(ByteArray(8), WatchSound.nanosLe(0))
        val preRoll = WatchSound.nanosLe(WatchSound.SEEK_PRE_ROLL_NS)
        assertEquals(8, preRoll.size)
        val back = java.nio.ByteBuffer.wrap(preRoll).order(java.nio.ByteOrder.nativeOrder()).long
        assertEquals(80_000_000L, back)
    }

    /** Sixty milliseconds of 16-bit samples: the most the sound may lag. */
    @Test
    fun theTrackBuffersSixtyMilliseconds() {
        assertEquals(11_520, WatchSound.bufferBytes(2))
        assertEquals(5_760, WatchSound.bufferBytes(1))
    }
}
