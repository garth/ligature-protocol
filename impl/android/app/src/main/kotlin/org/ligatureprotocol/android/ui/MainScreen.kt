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
import androidx.compose.material3.Switch
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
import org.ligatureprotocol.android.AppSwitch
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
    onReceiveClipboard: () -> Unit,
    onCopy: (String) -> Unit,
    onShare: (String) -> Unit,
    onBatterySettings: () -> Unit,
    batteryUnrestricted: Boolean,
    onNotificationSettings: () -> Unit,
    onToggleApp: (String, Boolean) -> Unit,
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
        StatusCard(status, state.backgroundRead, state.notificationAccess)
        state.pairing?.let { pairing ->
            PairingCard(pairing, onConfirmPairing, onAbortPairing, onDismissPairing, onCopy, onShare)
        }
        if (!status.initialised) {
            if (state.pairing == null) SetupCard(onCreatePerson, onStartJoin)
        } else {
            ClipboardCard(status, onSendClipboard, onReceiveClipboard, state.backgroundRead, state.lastClipboard)
            NotificationsCard(state.notificationAccess, state.apps, onNotificationSettings, onToggleApp)
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
private fun StatusCard(status: Status, backgroundRead: Boolean, notificationAccess: Boolean) {
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
                "${status.peers.size} connected, ${status.clipboardPeers.size} with a clipboard, " +
                    "${status.filePeers.size} receiving files",
                style = MaterialTheme.typography.bodySmall,
            )
            Text(
                if (backgroundRead) "Clipboard: automatic (background read allowed)"
                else "Clipboard: receives automatically, sends on request",
                style = MaterialTheme.typography.bodySmall,
            )
            Text(
                if (notificationAccess) "Notifications: forwarding and displaying"
                else "Notifications: displaying only (no notification access)",
                style = MaterialTheme.typography.bodySmall,
            )
            Text(
                "Files: share to Ligature to send; received files land in Downloads",
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
    onReceive: () -> Unit,
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
            Row(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                Button(onClick = onSend, enabled = status.clipboardPeers.isNotEmpty()) {
                    Text("Send clipboard now")
                }
                OutlinedButton(onClick = onReceive, enabled = status.clipboardPeers.isNotEmpty()) {
                    Text("Receive now")
                }
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
private fun NotificationsCard(
    access: Boolean,
    apps: List<AppSwitch>,
    onSettings: () -> Unit,
    onToggle: (String, Boolean) -> Unit,
) {
    Card {
        Column(Modifier.padding(16.dp), verticalArrangement = Arrangement.spacedBy(8.dp)) {
            Text("Notifications", style = MaterialTheme.typography.titleMedium)
            if (!access) {
                Text(
                    "Your other devices' notifications appear here. To send this phone's to them, " +
                        "allow Ligature notification access in Settings.",
                )
                OutlinedButton(onClick = onSettings) { Text("Notification access") }
            } else {
                Text("What your apps post goes to your other devices, and theirs arrive here.")
                if (apps.isEmpty()) {
                    Text("Apps appear below as they post.", style = MaterialTheme.typography.bodySmall)
                }
                apps.forEach { app ->
                    Row(
                        Modifier.fillMaxWidth(),
                        horizontalArrangement = Arrangement.SpaceBetween,
                    ) {
                        Column(Modifier.weight(1f)) {
                            Text(app.label)
                            Text(app.packageName, style = MaterialTheme.typography.bodySmall)
                        }
                        Switch(checked = app.enabled, onCheckedChange = { onToggle(app.packageName, it) })
                    }
                }
                TextButton(onClick = onSettings) { Text("Notification access settings") }
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

/** One device as the person sees it: its binding and principal together. */
private class DeviceRow(val name: String?, val platform: String?, keys: List<Device>) {
    /** Binding first, as the desktops print them. */
    val keys = keys.sortedBy { if (it.role == "binding") 0 else 1 }
    val binding get() = keys.firstOrNull { it.role == "binding" }
    val connected get() = binding?.connected == true
    val introducer get() = binding?.introducer == true
    val thisDevice get() = keys.any { it.thisDevice }
    val revoked get() = keys.any { it.revoked }
}

/** Groups one entry per key into one row per device, in the order given. */
private fun groupDevices(devices: List<Device>): List<DeviceRow> {
    val rows = mutableListOf<DeviceRow>()
    for (d in devices) {
        val partner = if (d.role == "binding") {
            rows.indexOfFirst { r -> r.binding == null && r.keys.any { it.key == d.principal } }
        } else {
            rows.indexOfFirst { r -> r.keys.none { it.role == "principal" } && r.binding?.principal == d.key }
        }
        if (partner >= 0) {
            val r = rows[partner]
            rows[partner] = DeviceRow(r.name ?: d.displayName, r.platform ?: d.platform, r.keys + d)
        } else {
            rows += DeviceRow(d.displayName, d.platform, listOf(d))
        }
    }
    return rows
}

@Composable
private fun DevicesCard(devices: List<Device>, onRevoke: (String) -> Unit) {
    var toRevoke by remember { mutableStateOf<Device?>(null) }
    Card {
        Column(Modifier.padding(16.dp), verticalArrangement = Arrangement.spacedBy(8.dp)) {
            Text("Devices", style = MaterialTheme.typography.titleMedium)
            if (devices.isEmpty()) Text("None known.")
            groupDevices(devices).forEach { row ->
                Row(
                    Modifier.fillMaxWidth(),
                    horizontalArrangement = Arrangement.SpaceBetween,
                ) {
                    Column(Modifier.weight(1f)) {
                        Text(
                            buildString {
                                append(row.name ?: "?")
                                row.platform?.let { append("  ·  $it") }
                                if (row.thisDevice) append("  ·  this phone")
                                if (row.introducer) append("  ·  introducer")
                                if (row.connected) append("  ·  connected")
                                if (row.revoked) append("  ·  REVOKED")
                            },
                            fontWeight = if (row.thisDevice) FontWeight.Bold else FontWeight.Normal,
                        )
                        row.keys.forEach { key ->
                            Text(
                                buildString {
                                    append(key.role)
                                    append("  ·  ")
                                    append(key.fingerprint)
                                    append('…')
                                    if (key.revoked) append("  ·  revoked")
                                },
                                style = MaterialTheme.typography.bodySmall,
                            )
                        }
                    }
                    val target = row.binding ?: row.keys.first()
                    if (!row.thisDevice && !target.revoked) {
                        TextButton(onClick = { toRevoke = target }) { Text("Revoke") }
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
