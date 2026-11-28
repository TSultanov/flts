package com.TS.FLTS.ui

import android.content.ClipData
import android.content.ClipboardManager
import android.content.Context
import android.os.Build
import android.provider.Settings
import androidx.compose.foundation.Image
import androidx.compose.foundation.background
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.text.KeyboardActions
import androidx.compose.foundation.text.KeyboardOptions
import androidx.compose.foundation.text.selection.SelectionContainer
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.outlined.QrCodeScanner
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.Button
import androidx.compose.material3.ButtonDefaults
import androidx.compose.material3.Icon
import androidx.compose.material3.LinearProgressIndicator
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedButton
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.FilterQuality
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.platform.LocalFocusManager
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.text.input.ImeAction
import androidx.compose.ui.text.input.KeyboardCapitalization
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import com.TS.FLTS.core.any
import com.TS.FLTS.platform.PairingScanner
import com.TS.FLTS.platform.pairingScannerSupported
import com.TS.FLTS.platform.qrCode
import com.TS.FLTS.stores.AppModel
import com.TS.FLTS.stores.SyncStore
import kotlinx.coroutines.delay
import kotlinx.coroutines.launch
import uniffi.flts_ffi.DeviceEntry
import uniffi.flts_ffi.StoreIssues
import uniffi.flts_ffi.SyncState
import uniffi.flts_ffi.SyncStatus

/// The name other devices see when none was set yet.
private fun suggestedDeviceName(context: Context): String? {
    val named = runCatching { Settings.Global.getString(context.contentResolver, Settings.Global.DEVICE_NAME) }.getOrNull()
    return (named?.takeIf { it.isNotBlank() } ?: Build.MODEL)?.takeIf { it.isNotBlank() }
}

/// The Sync page's sections, for the settings sheet's scrolling column.
@Composable
internal fun SyncSettings(model: AppModel) {
    val sync = model.sync
    val config = model.config.config
    val enabled = config?.syncEnabled == true
    // While an action runs nothing on the page takes input.
    val idle = !sync.busy
    val context = LocalContext.current
    val focus = LocalFocusManager.current
    var addId by remember { mutableStateOf("") }
    var addName by remember { mutableStateOf("") }
    var confirmReset by remember { mutableStateOf(false) }
    var resetting by remember { mutableStateOf(false) }
    var webUi by remember { mutableStateOf<String?>(null) }
    var removing by remember { mutableStateOf<DeviceEntry?>(null) }
    var scanning by remember { mutableStateOf(false) }

    LaunchedEffect(sync.status.state) { webUi = sync.webUiUrl() }
    LaunchedEffect(Unit) { sync.refresh() }
    LaunchedEffect(sync.status) { sync.refreshDevices() }
    // New pairing requests show up without waiting for a status change.
    LaunchedEffect(Unit) {
        while (true) {
            delay(4000)
            if (model.config.config?.syncEnabled == true) sync.refreshDevices()
        }
    }

    SettingsSection {
        if (!enabled) {
            Text(
                "FLTS syncs your library with your other devices through a built-in Syncthing engine. If you run Syncthing separately on this device, stop it before enabling sync.",
                style = MaterialTheme.typography.bodyMedium, color = MaterialTheme.colorScheme.onSurfaceVariant)
        }
        SettingsSwitchRow(
            "Sync this library with my other devices", enabled, Modifier.testTag("sync.enabled"), enabled = idle,
        ) { on ->
            model.scope.launch {
                if (on && model.config.config?.syncDeviceName == null) {
                    suggestedDeviceName(context)?.let { sync.setDeviceName(it) }
                }
                sync.setEnabled(on)
            }
        }
        if (enabled) {
            val currentName = config?.syncDeviceName ?: sync.thisDevice?.name ?: ""
            var deviceName by remember { mutableStateOf(currentName) }
            LaunchedEffect(currentName) { deviceName = currentName }
            val trimmed = deviceName.trim()
            val canRename = trimmed.isNotEmpty() && trimmed != currentName
            val rename = {
                if (canRename) {
                    focus.clearFocus()
                    model.scope.launch { sync.setDeviceName(trimmed) }
                }
            }
            Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                OutlinedTextField(
                    value = deviceName, onValueChange = { deviceName = it }, enabled = idle, singleLine = true,
                    label = { Text("This device's name") },
                    keyboardOptions = KeyboardOptions(
                        capitalization = KeyboardCapitalization.Words, imeAction = ImeAction.Done),
                    keyboardActions = KeyboardActions(onDone = { rename() }),
                    modifier = Modifier.weight(1f).testTag("sync.deviceName"),
                )
                TextButton(
                    onClick = { rename() }, enabled = idle && canRename, modifier = Modifier.testTag("sync.rename"),
                ) { Text("Rename") }
            }
            SyncStatusRow(sync.status)
        }
    }

    if (enabled) {
        sync.status.storeIssues?.takeIf { it.any }?.let { issues ->
            StoreIssuesSection(model, issues, resetting, idle) { confirmReset = true }
        }

        sync.thisDevice?.let { device ->
            SettingsSection("Pair a device") {
                Row(horizontalArrangement = Arrangement.spacedBy(16.dp)) {
                    val payload = SyncStore.pairingPayload(device.deviceId, device.name)
                    remember(payload) { qrCode(payload) }?.let { qr ->
                        Image(
                            bitmap = qr, contentDescription = "Pairing code", filterQuality = FilterQuality.None,
                            modifier = Modifier.size(132.dp).testTag("sync.qr"))
                    }
                    Column(Modifier.weight(1f), verticalArrangement = Arrangement.spacedBy(8.dp)) {
                        Text(
                            "Scan this code on your other device, or enter this device ID there:",
                            style = MaterialTheme.typography.bodyMedium,
                            color = MaterialTheme.colorScheme.onSurfaceVariant)
                        SelectionContainer {
                            Text(
                                device.deviceId, style = MaterialTheme.typography.bodySmall,
                                fontFamily = FontFamily.Monospace, modifier = Modifier.testTag("sync.deviceId"))
                        }
                        OutlinedButton(
                            onClick = {
                                context.getSystemService(ClipboardManager::class.java)
                                    ?.setPrimaryClip(ClipData.newPlainText("Device ID", device.deviceId))
                            },
                            enabled = idle, modifier = Modifier.testTag("sync.copyDeviceId"),
                        ) { Text("Copy Device ID") }
                    }
                }
                if (pairingScannerSupported(context)) {
                    OutlinedButton(
                        onClick = { scanning = true }, enabled = idle,
                        modifier = Modifier.fillMaxWidth().testTag("sync.scan"),
                    ) {
                        Icon(Icons.Outlined.QrCodeScanner, contentDescription = null, modifier = Modifier.size(18.dp))
                        Text("Scan Another Device's Code", Modifier.padding(start = 8.dp))
                    }
                }
                OutlinedTextField(
                    value = addId, onValueChange = { addId = it }, enabled = idle, singleLine = true,
                    label = { Text("Other device's ID or pairing code") },
                    keyboardOptions = settingsLiteralKeyboard(action = ImeAction.Next),
                    modifier = Modifier.fillMaxWidth().testTag("sync.addId"),
                )
                OutlinedTextField(
                    value = addName, onValueChange = { addName = it }, enabled = idle, singleLine = true,
                    label = { Text("Its name") },
                    keyboardOptions = KeyboardOptions(
                        capitalization = KeyboardCapitalization.Words, imeAction = ImeAction.Done),
                    keyboardActions = KeyboardActions(onDone = { focus.clearFocus() }),
                    modifier = Modifier.fillMaxWidth().testTag("sync.addName"),
                )
                Button(
                    onClick = {
                        val parsed = SyncStore.parsePairing(addId)
                        if (parsed == null) {
                            sync.error = "Enter a valid device ID or pairing code."
                        } else {
                            val (id, parsedName) = parsed
                            val name = addName.ifEmpty { parsedName ?: "" }
                            focus.clearFocus()
                            model.scope.launch {
                                sync.add(id, name)
                                if (sync.error == null) {
                                    addId = ""
                                    addName = ""
                                }
                            }
                        }
                    },
                    enabled = idle && addId.isNotBlank(),
                    modifier = Modifier.align(Alignment.End).testTag("sync.addDevice"),
                ) { Text("Add Device") }
            }
        }

        if (sync.pending.isNotEmpty()) {
            SettingsSection("Asking to pair") {
                for (p in sync.pending) {
                    Row(verticalAlignment = Alignment.CenterVertically) {
                        DeviceLabel(p.name, p.deviceId, Modifier.weight(1f))
                        TextButton(
                            onClick = { model.scope.launch { sync.add(p.deviceId, p.name) } }, enabled = idle,
                            modifier = Modifier.testTag("sync.accept.${p.deviceId}"),
                        ) { Text("Accept") }
                    }
                }
            }
        }

        SettingsSection("Devices") {
            if (sync.devices.isEmpty()) {
                Text("No other devices yet", color = MaterialTheme.colorScheme.onSurfaceVariant)
            }
            for (d in sync.devices) {
                Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(10.dp)) {
                    val dot = if (d.connected) SettingsGood else MaterialTheme.colorScheme.onSurfaceVariant.copy(alpha = 0.4f)
                    Box(Modifier.size(8.dp).background(dot, CircleShape))
                    DeviceLabel(d.name, d.deviceId, Modifier.weight(1f))
                    TextButton(
                        onClick = { removing = d }, enabled = idle,
                        colors = ButtonDefaults.textButtonColors(contentColor = MaterialTheme.colorScheme.error),
                        modifier = Modifier.testTag("sync.remove.${d.deviceId}"),
                    ) { Text("Remove") }
                }
            }
        }
    }

    webUi?.let { url ->
        SettingsSection(footer = { SettingsFooter("For diagnosing sync; FLTS manages these settings itself.") }) {
            // Shown, not linked: opening it would suspend this app, which serves the page.
            Text("Syncthing web UI")
            SelectionContainer {
                Text(
                    url, style = MaterialTheme.typography.bodySmall, fontFamily = FontFamily.Monospace,
                    color = MaterialTheme.colorScheme.onSurfaceVariant, modifier = Modifier.testTag("sync.webUi"))
            }
        }
    }

    sync.error?.let {
        Text(
            it, color = MaterialTheme.colorScheme.error, style = MaterialTheme.typography.bodyMedium,
            modifier = Modifier.testTag("sync.error"))
    }

    if (scanning) {
        PairingScanner(
            found = { payload ->
                val parsed = SyncStore.parsePairing(payload)
                addId = parsed?.first ?: payload
                parsed?.second?.takeIf { it.isNotEmpty() }?.let { addName = it }
            },
            onDismiss = { scanning = false },
        )
    }

    removing?.let { device ->
        AlertDialog(
            onDismissRequest = { removing = null },
            title = { Text("Stop syncing with “${device.name.ifEmpty { "this device" }}”?") },
            text = { Text("It has to be paired again to sync with this device.") },
            confirmButton = {
                TextButton(
                    onClick = {
                        removing = null
                        model.scope.launch { sync.remove(device.deviceId) }
                    },
                    colors = ButtonDefaults.textButtonColors(contentColor = MaterialTheme.colorScheme.error),
                    modifier = Modifier.testTag("sync.remove.confirm"),
                ) { Text("Remove Device") }
            },
            dismissButton = { TextButton(onClick = { removing = null }) { Text("Cancel") } },
        )
    }

    if (confirmReset) {
        AlertDialog(
            onDismissRequest = { confirmReset = false },
            title = { Text("New sync identity") },
            text = {
                Text(
                    "This device will get a new sync identity and sync will restart. It then has to be paired with your other devices again, where it shows up as a new device.")
            },
            confirmButton = {
                TextButton(
                    onClick = {
                        confirmReset = false
                        resetting = true
                        model.scope.launch {
                            sync.resetIdentity()
                            resetting = false
                        }
                    },
                    colors = ButtonDefaults.textButtonColors(contentColor = MaterialTheme.colorScheme.error),
                    modifier = Modifier.testTag("sync.reset.confirm"),
                ) { Text("Give This Device a New Identity") }
            },
            dismissButton = { TextButton(onClick = { confirmReset = false }) { Text("Cancel") } },
        )
    }
}

@Composable
private fun DeviceLabel(name: String, deviceId: String, modifier: Modifier = Modifier) {
    Column(modifier) {
        Text(name.ifEmpty { "Unnamed device" })
        Text(
            deviceId, style = MaterialTheme.typography.bodySmall, fontFamily = FontFamily.Monospace,
            color = MaterialTheme.colorScheme.onSurfaceVariant, maxLines = 1, overflow = TextOverflow.MiddleEllipsis)
    }
}

@Composable
private fun SyncStatusRow(status: SyncStatus) {
    val completion = status.completion
    if (status.state == SyncState.SYNCING && completion != null) {
        Column(Modifier.testTag("sync.status"), verticalArrangement = Arrangement.spacedBy(6.dp)) {
            Text("Syncing…")
            LinearProgressIndicator(progress = { (completion / 100).toFloat().coerceIn(0f, 1f) }, Modifier.fillMaxWidth())
        }
        return
    }
    val (text, color) = when (status.state) {
        SyncState.DISABLED -> "Off" to Color.Unspecified
        SyncState.STARTING -> "Starting…" to Color.Unspecified
        SyncState.ONLINE -> {
            val text =
                if (status.deviceCount == 0) "Online — no devices paired yet"
                else "Online · ${status.connectedCount} of ${status.deviceCount} connected"
            text to SettingsGood
        }
        SyncState.SYNCING -> "Syncing…" to Color.Unspecified
        SyncState.ERROR -> (status.lastError ?: "Sync failed") to MaterialTheme.colorScheme.error
    }
    Text(text, color = color, modifier = Modifier.testTag("sync.status"))
}

@Composable
private fun StoreIssuesSection(
    model: AppModel, issues: StoreIssues, resetting: Boolean, idle: Boolean, askReset: () -> Unit,
) {
    val sync = model.sync
    // Hidden once a choice was made here, until the issue clears and recurs.
    var identityDismissed by remember { mutableStateOf(false) }
    SettingsSection("Needs attention") {
        if (issues.newerVersion > 0 || issues.stalled) {
            Text("Another device uses a newer version of the app. Update this device to see its latest changes.")
        }
        if (issues.unreadable > 0) {
            Text("Some synced data could not be read. Changes from another device may be missing.")
        }
        if (issues.strangers > 0) {
            Text("The sync folder contains conflict copies that are being ignored.")
        }
        if (issues.sharedIdentity && !identityDismissed) {
            Text(
                "This device shares its sync identity with another copy of the app, as happens after restoring a backup. One of the two needs a new identity and will then have to be paired again.",
                color = MaterialTheme.colorScheme.error)
            OutlinedButton(
                onClick = askReset, enabled = idle, modifier = Modifier.fillMaxWidth().testTag("sync.resetIdentity"),
            ) { Text(if (resetting) "Giving this device a new identity…" else "Give this device a new identity") }
            OutlinedButton(
                onClick = {
                    model.scope.launch {
                        sync.keepIdentity()
                        if (sync.error == null) identityDismissed = true
                    }
                },
                enabled = idle, modifier = Modifier.fillMaxWidth().testTag("sync.keepIdentity"),
            ) { Text("Keep this device's identity") }
        }
    }
}
