package org.ligatureprotocol.android.ui

import org.junit.Assert.assertEquals
import org.junit.Test
import org.ligatureprotocol.core.TouchEvent

class RemoteKeyboardTest {
    private val backspace = listOf(TouchEvent.Key(KEY_BACKSPACE, true), TouchEvent.Key(KEY_BACKSPACE, false))

    @Test
    fun aKeyIsPressedThenReleased() {
        assertEquals(backspace, press(KEY_BACKSPACE))
    }

    @Test
    fun theFieldUnchangedSendsNothing() {
        assertEquals(emptyList<TouchEvent>(), remoteEdit(SENTINEL))
    }

    @Test
    fun theSentinelDeletedIsABackspace() {
        assertEquals(backspace, remoteEdit(""))
    }

    @Test
    fun whatFollowsTheSentinelIsText() {
        assertEquals(listOf(TouchEvent.Text("héllo €")), remoteEdit(SENTINEL + "héllo €"))
    }

    @Test
    fun theSentinelTypedOverIsABackspaceThenTheText() {
        assertEquals(backspace + TouchEvent.Text("ab"), remoteEdit("a" + SENTINEL + "b"))
        assertEquals(backspace + TouchEvent.Text("x"), remoteEdit("x"))
    }
}
