package org.ligatureprotocol.android.ui

import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.safeDrawingPadding
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.text.selection.SelectionContainer
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.Button
import androidx.compose.material3.Card
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedButton
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.setValue
import androidx.compose.ui.Modifier
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.dp
import org.ligatureprotocol.android.PairingState
import org.ligatureprotocol.android.UiState
import org.ligatureprotocol.core.Device
import org.ligatureprotocol.core.Status

@Composable
fun MainScreen(
    state: UiState,
    onCreatePerson: (String) -> Unit,
    onStartJoin: () -> Unit,
    onStartIntroduce: (String) -> Unit,
    onConfirmPairing: () -> Unit,
    onAbortPairing: () -> Unit,
    onDismissPairing: () -> Unit,
    onRevoke: (String) -> Unit,
    onSendClipboard: () -> Unit,
    onCopy: (String) -> Unit,
    onShare: (String) -> Unit,
    onBatterySettings: () -> Unit,
    batteryUnrestricted: Boolean,
) {
    Column(
        modifier = Modifier
            .fillMaxSize()
            .safeDrawingPadding()
            .verticalScroll(rememberScrollState())
            .padding(16.dp),
        verticalArrangement = Arrangement.spacedBy(16.dp),
    ) {
        Text("Ligature", style = MaterialTheme.typography.headlineMedium)
        state.lastError?.let {
            Text(it, color = MaterialTheme.colorScheme.error)
        }
        val status = state.status
        if (!state.running || status == null) {
            Text("Starting the node…")
            return@Column
        }
        StatusCard(status, state.backgroundRead)
        state.pairing?.let { pairing ->
            PairingCard(pairing, onConfirmPairing, onAbortPairing, onDismissPairing, onCopy, onShare)
        }
        if (!status.initialised) {
            if (state.pairing == null) SetupCard(onCreatePerson, onStartJoin)
        } else {
            ClipboardCard(status, onSendClipboard, state.backgroundRead, state.lastClipboard)
            if (state.pairing == null) IntroduceCard(onStartIntroduce)
            DevicesCard(state.devices, onRevoke)
        }
        if (!batteryUnrestricted) {
            Card {
                Column(Modifier.padding(16.dp), verticalArrangement = Arrangement.spacedBy(8.dp)) {
                    Text("Battery", style = MaterialTheme.typography.titleMedium)
                    Text(
                        "Android may stop the node in the background. Allow unrestricted " +
                            "battery use so your other devices can always reach this phone.",
                    )
                    OutlinedButton(onClick = onBatterySettings) { Text("Battery settings") }
                }
            }
        }
    }
}

@Composable
private fun StatusCard(status: Status, backgroundRead: Boolean) {
    Card {
        Column(Modifier.padding(16.dp), verticalArrangement = Arrangement.spacedBy(4.dp)) {
            if (status.initialised) {
                Text(status.name ?: "?", style = MaterialTheme.typography.titleMedium)
                Mono(status.person ?: "")
            } else {
                Text("Not set up yet", style = MaterialTheme.typography.titleMedium)
            }
            Spacer(Modifier.height(4.dp))
            Text("${status.deviceName}  ·  ${status.fingerprint}…  ·  port ${status.port}")
            Text(
                "${status.peers.size} connected, ${status.clipboardPeers.size} with a clipboard",
                style = MaterialTheme.typography.bodySmall,
            )
            Text(
                if (backgroundRead) "Clipboard: automatic (background read allowed)"
                else "Clipboard: receives automatically, sends on request",
                style = MaterialTheme.typography.bodySmall,
            )
        }
    }
}

@Composable
private fun SetupCard(onCreatePerson: (String) -> Unit, onStartJoin: () -> Unit) {
    var name by rememberSaveable { mutableStateOf("") }
    Card {
        Column(Modifier.padding(16.dp), verticalArrangement = Arrangement.spacedBy(12.dp)) {
            Text("Add this phone to my devices", style = MaterialTheme.typography.titleMedium)
            Text(
                "On a device that already has your keys, run `lpctl pair` with the offer " +
                    "this phone will show; then compare the six-digit codes.",
            )
            Button(onClick = onStartJoin, modifier = Modifier.fillMaxWidth()) { Text("Show pairing offer") }
            Spacer(Modifier.height(8.dp))
            Text("Or start here", style = MaterialTheme.typography.titleMedium)
            Text("This phone becomes your first device and holds your master key.")
            OutlinedTextField(
                value = name,
                onValueChange = { name = it },
                label = { Text("Your name") },
                singleLine = true,
                modifier = Modifier.fillMaxWidth(),
            )
            OutlinedButton(
                onClick = { onCreatePerson(name) },
                enabled = name.isNotBlank(),
                modifier = Modifier.fillMaxWidth(),
            ) { Text("Create my keys on this phone") }
        }
    }
}

@Composable
private fun PairingCard(
    pairing: PairingState,
    onConfirm: () -> Unit,
    onAbort: () -> Unit,
    onDismiss: () -> Unit,
    onCopy: (String) -> Unit,
    onShare: (String) -> Unit,
) {
    Card {
        Column(Modifier.padding(16.dp), verticalArrangement = Arrangement.spacedBy(12.dp)) {
            Text("Pairing", style = MaterialTheme.typography.titleMedium)
            when {
                pairing.finished && pairing.error != null -> {
                    Text("Failed: ${pairing.error}", color = MaterialTheme.colorScheme.error)
                    Button(onClick = onDismiss) { Text("Close") }
                }
                pairing.finished -> {
                    Text("Paired.")
                    Button(onClick = onDismiss) { Text("Close") }
                }
                pairing.code != null -> {
                    Text("Does the other device show this code?")
                    Text(
                        pairing.code.chunked(3).joinToString("  "),
                        style = MaterialTheme.typography.displayMedium,
                        fontFamily = FontFamily.Monospace,
                    )
                    Row(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                        Button(onClick = onConfirm) { Text("Yes, same code") }
                        OutlinedButton(onClick = onAbort) { Text("No") }
                    }
                }
                pairing.offer != null -> {
                    QrCode(pairing.offer)
                    Text("Scan this, or send the offer text to a device of yours and run:")
                    Mono("lpctl pair '<offer>'")
                    SelectionContainer { Mono(pairing.offer) }
                    Row(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                        OutlinedButton(onClick = { onCopy(pairing.offer) }) { Text("Copy") }
                        OutlinedButton(onClick = { onShare(pairing.offer) }) { Text("Share") }
                        TextButton(onClick = onAbort) { Text("Cancel") }
                    }
                    Text("Waiting for an introducer…", style = MaterialTheme.typography.bodySmall)
                }
                else -> {
                    Text("Connecting…")
                    TextButton(onClick = onAbort) { Text("Cancel") }
                }
            }
        }
    }
}

@Composable
private fun ClipboardCard(
    status: Status,
    onSend: () -> Unit,
    backgroundRead: Boolean,
    lastClipboard: String?,
) {
    Card {
        Column(Modifier.padding(16.dp), verticalArrangement = Arrangement.spacedBy(8.dp)) {
            Text("Clipboard", style = MaterialTheme.typography.titleMedium)
            Text(
                if (backgroundRead) {
                    "Copies on this phone go to your other devices as they happen, and theirs arrive here."
                } else {
                    "What you copy on your other devices arrives here by itself. To send this " +
                        "phone's clipboard, tap the button here, the Send clipboard tile in quick " +
                        "settings, the action on the notification, or share text to Ligature."
                },
            )
            Button(onClick = onSend, enabled = status.clipboardPeers.isNotEmpty()) {
                Text("Send clipboard now")
            }
            lastClipboard?.let {
                Text(
                    "Last synced: " + it.take(80).replace('\n', ' '),
                    style = MaterialTheme.typography.bodySmall,
                )
            }
        }
    }
}

@Composable
private fun IntroduceCard(onStartIntroduce: (String) -> Unit) {
    var offer by rememberSaveable { mutableStateOf("") }
    Card {
        Column(Modifier.padding(16.dp), verticalArrangement = Arrangement.spacedBy(8.dp)) {
            Text("Add another device", style = MaterialTheme.typography.titleMedium)
            Text("Paste the offer the new device shows (`ligature:pair/1?…`).")
            OutlinedTextField(
                value = offer,
                onValueChange = { offer = it },
                label = { Text("Offer") },
                modifier = Modifier.fillMaxWidth(),
            )
            OutlinedButton(
                onClick = { onStartIntroduce(offer.trim()) },
                enabled = offer.trim().startsWith("ligature:pair/"),
            ) { Text("Introduce") }
        }
    }
}

@Composable
private fun DevicesCard(devices: List<Device>, onRevoke: (String) -> Unit) {
    var toRevoke by remember { mutableStateOf<Device?>(null) }
    Card {
        Column(Modifier.padding(16.dp), verticalArrangement = Arrangement.spacedBy(8.dp)) {
            Text("Devices", style = MaterialTheme.typography.titleMedium)
            if (devices.isEmpty()) Text("None known.")
            devices.forEach { device ->
                Row(
                    Modifier.fillMaxWidth(),
                    horizontalArrangement = Arrangement.SpaceBetween,
                ) {
                    Column(Modifier.weight(1f)) {
                        Text(
                            buildString {
                                append(device.displayName ?: "?")
                                device.platform?.let { append("  ·  $it") }
                            },
                            fontWeight = if (device.thisDevice) FontWeight.Bold else FontWeight.Normal,
                        )
                        Text(
                            buildString {
                                append(device.role)
                                append("  ·  ")
                                append(device.fingerprint)
                                append('…')
                                if (device.introducer) append("  ·  introducer")
                                if (device.connected) append("  ·  connected")
                                if (device.revoked) append("  ·  REVOKED")
                                if (device.thisDevice) append("  ·  this phone")
                            },
                            style = MaterialTheme.typography.bodySmall,
                        )
                    }
                    if (!device.thisDevice && !device.revoked) {
                        TextButton(onClick = { toRevoke = device }) { Text("Revoke") }
                    }
                }
            }
        }
    }
    toRevoke?.let { device ->
        AlertDialog(
            onDismissRequest = { toRevoke = null },
            title = { Text("Revoke ${device.displayName ?: device.fingerprint}?") },
            text = { Text("It will no longer be able to connect to any of your devices. This cannot be undone.") },
            confirmButton = {
                TextButton(onClick = {
                    onRevoke(device.key)
                    toRevoke = null
                }) { Text("Revoke") }
            },
            dismissButton = { TextButton(onClick = { toRevoke = null }) { Text("Cancel") } },
        )
    }
}

@Composable
private fun Mono(text: String) {
    Text(text, fontFamily = FontFamily.Monospace, style = MaterialTheme.typography.bodySmall)
}
