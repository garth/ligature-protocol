package org.ligatureprotocol.android.ui

import androidx.compose.foundation.text.KeyboardActions
import androidx.compose.foundation.text.KeyboardOptions
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Modifier
import androidx.compose.ui.text.TextRange
import androidx.compose.ui.text.input.ImeAction
import androidx.compose.ui.text.input.TextFieldValue
import org.ligatureprotocol.core.TouchEvent

/** Backspace and Enter, by evdev code. */
internal const val KEY_BACKSPACE: UShort = 14u
internal const val KEY_ENTER: UShort = 28u

/**
 * What the field always holds: a zero-width space with the cursor after it,
 * so the keyboard's delete has something to remove, which an empty field
 * would not, and no keyboard sends a delete it cannot see take effect.
 */
private const val SENTINEL = "​"

/** A key pressed and released. */
internal fun press(code: UShort): List<TouchEvent> =
    listOf(TouchEvent.Key(code, true), TouchEvent.Key(code, false))

/**
 * The events one change of the field stands for, given what the field held
 * before ([SENTINEL]) and `text` after: the sentinel deleted is a
 * backspace, anything after it is text, and the sentinel replaced (a
 * selection typed over) is a backspace and then the text.
 */
internal fun remoteEdit(text: String): List<TouchEvent> = when {
    text == SENTINEL -> emptyList()
    text.isEmpty() -> press(KEY_BACKSPACE)
    text.startsWith(SENTINEL) -> listOf(TouchEvent.Text(text.removePrefix(SENTINEL)))
    else -> press(KEY_BACKSPACE) + TouchEvent.Text(text.replace(SENTINEL, ""))
}

private fun fresh() = TextFieldValue(SENTINEL, TextRange(SENTINEL.length))

/**
 * The keyboard for a desktop the phone drives (milestones 7 and 8): every
 * character typed goes to the desktop as text, the keyboard's delete as
 * Backspace and its action key as Enter, and the field never keeps any of it.
 */
@Composable
fun RemoteKeyboardField(
    label: String,
    onEvents: (List<TouchEvent>) -> Unit,
    modifier: Modifier = Modifier,
) {
    var value by remember { mutableStateOf(fresh()) }
    OutlinedTextField(
        value = value,
        onValueChange = { next ->
            val events = remoteEdit(next.text)
            if (events.isNotEmpty()) onEvents(events)
            value = fresh()
        },
        label = { Text(label) },
        singleLine = true,
        keyboardOptions = KeyboardOptions(imeAction = ImeAction.Send),
        keyboardActions = KeyboardActions(onSend = { onEvents(press(KEY_ENTER)) }),
        modifier = modifier,
    )
}
