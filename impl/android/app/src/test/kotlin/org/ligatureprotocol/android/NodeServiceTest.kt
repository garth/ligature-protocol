package org.ligatureprotocol.android

import org.junit.Assert.assertEquals
import org.junit.Test

class NodeServiceTest {
    @Test
    fun aFailedStartIsTriedAgainSoonThenLessOften() {
        assertEquals(5_000L, NodeService.retryDelayMs(0))
        assertEquals(10_000L, NodeService.retryDelayMs(1))
        assertEquals(20_000L, NodeService.retryDelayMs(2))
    }

    @Test
    fun theRetryNeverStopsAndNeverWaitsMoreThanFiveMinutes() {
        assertEquals(300_000L, NodeService.retryDelayMs(7))
        assertEquals(300_000L, NodeService.retryDelayMs(1_000))
    }
}
