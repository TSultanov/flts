package com.TS.FLTS.ui

import androidx.compose.animation.core.RepeatMode
import androidx.compose.animation.core.animateFloat
import androidx.compose.animation.core.infiniteRepeatable
import androidx.compose.animation.core.rememberInfiniteTransition
import androidx.compose.animation.core.tween
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.width
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.outlined.CloudDone
import androidx.compose.material.icons.outlined.CloudOff
import androidx.compose.material.icons.outlined.CloudSync
import androidx.compose.material.icons.outlined.ErrorOutline
import androidx.compose.material.icons.outlined.Layers
import androidx.compose.material.icons.outlined.LibraryAddCheck
import androidx.compose.material.icons.outlined.Settings
import androidx.compose.material.icons.outlined.SyncProblem
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.alpha
import androidx.compose.ui.graphics.vector.ImageVector
import androidx.compose.ui.unit.dp
import com.TS.FLTS.core.AppPrefs
import com.TS.FLTS.core.any
import com.TS.FLTS.stores.AppModel
import kotlinx.coroutines.launch
import uniffi.flts_ffi.AnkiSyncState
import uniffi.flts_ffi.SyncState

/// The settings page a toolbar item can open Settings on.
const val SETTINGS_PAGE_SYNC = "sync"

/// A pulse while busy: rotating spun the whole cloud.
@Composable
fun pulseAlpha(active: Boolean): Float {
    if (!active || AppPrefs.animationsOff) return 1f
    val transition = rememberInfiniteTransition(label = "pulse")
    val alpha by transition.animateFloat(
        initialValue = 1f, targetValue = 0.35f,
        animationSpec = infiniteRepeatable(tween(700), RepeatMode.Reverse), label = "pulse",
    )
    return alpha
}

/// The library's status items: sync status, Anki sync, Settings. The reader's
/// toolbar is left to the book.
@Composable
fun AppToolbarActions(model: AppModel) {
    val config = model.config.config
    if (config?.syncEnabled == true) {
        val status = model.sync.status
        val busy = status.state == SyncState.STARTING || status.state == SyncState.SYNCING
        val label = when (status.state) {
            SyncState.DISABLED -> "Sync is off"
            SyncState.STARTING -> "Sync is starting"
            SyncState.ONLINE ->
                if (status.deviceCount == 0) "Online — no devices paired yet"
                else "Synced · ${status.connectedCount} of ${status.deviceCount} devices connected"
            SyncState.SYNCING -> status.completion?.let { "Syncing ${it.toInt()}%" } ?: "Syncing"
            SyncState.ERROR -> status.lastError ?: "Sync failed"
        }
        val icon: ImageVector = when (status.state) {
            SyncState.DISABLED -> Icons.Outlined.CloudOff
            SyncState.STARTING, SyncState.SYNCING -> Icons.Outlined.CloudSync
            SyncState.ONLINE -> if (status.storeIssues?.any == true) Icons.Outlined.ErrorOutline else Icons.Outlined.CloudDone
            SyncState.ERROR -> Icons.Outlined.ErrorOutline
        }
        IconButton(onClick = {
            model.settingsPage = SETTINGS_PAGE_SYNC
            model.showSettings = true
        }) {
            Row(verticalAlignment = Alignment.CenterVertically) {
                Icon(icon, contentDescription = label, modifier = Modifier.alpha(pulseAlpha(busy)))
                val completion = status.completion
                if (status.state == SyncState.SYNCING && completion != null) {
                    Spacer(Modifier.width(2.dp))
                    Text("${completion.toInt()}%", style = MaterialTheme.typography.labelSmall)
                }
            }
        }
    }
    // As in the web app: hidden while Anki isn't running.
    if (config?.ankiSyncEnabled == true && model.config.isConfigured && model.anki.reachable) {
        val state = model.anki.status.state
        val icon = when (state) {
            AnkiSyncState.ERR -> Icons.Outlined.SyncProblem
            AnkiSyncState.OK -> Icons.Outlined.LibraryAddCheck
            else -> Icons.Outlined.Layers
        }
        IconButton(onClick = { model.scope.launch { model.anki.syncNow() } }, enabled = state != AnkiSyncState.SYNCING) {
            Icon(icon, contentDescription = "Sync Anki", modifier = Modifier.alpha(pulseAlpha(state == AnkiSyncState.SYNCING)))
        }
    }
    IconButton(onClick = {
        model.settingsPage = null
        model.showSettings = true
    }) {
        Icon(Icons.Outlined.Settings, contentDescription = "Settings")
    }
}
