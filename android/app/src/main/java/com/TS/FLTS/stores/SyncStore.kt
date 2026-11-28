package com.TS.FLTS.stores

import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.setValue
import com.TS.FLTS.core.CoreService
import com.TS.FLTS.core.EventHub
import com.TS.FLTS.core.ankiIdle
import com.TS.FLTS.core.runCatchingCancellable
import com.TS.FLTS.core.syncDisabled
import com.TS.FLTS.core.userMessage
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.launch
import org.json.JSONObject
import uniffi.flts_ffi.AnkiSyncState
import uniffi.flts_ffi.AnkiSyncStatus
import uniffi.flts_ffi.CoreEvent
import uniffi.flts_ffi.DeviceEntry
import uniffi.flts_ffi.PendingDevice
import uniffi.flts_ffi.SyncStatus
import uniffi.flts_ffi.ThisDevice

class SyncStore(private val events: EventHub, private val scope: CoroutineScope) : CoreBound {
    var status by mutableStateOf<SyncStatus>(syncDisabled)
        private set
    var thisDevice by mutableStateOf<ThisDevice?>(null)
        private set
    var devices by mutableStateOf<List<DeviceEntry>>(emptyList())
        private set
    var pending by mutableStateOf<List<PendingDevice>>(emptyList())
        private set

    /// True while an action runs; the settings disable their controls.
    var busy by mutableStateOf(false)
        private set
    var error by mutableStateOf<String?>(null)

    private var core: CoreService? = null

    override fun attach(core: CoreService) {
        this.core = core
        events.on { event ->
            if (event is CoreEvent.SyncStatusChanged) scope.launch { refresh() }
        }
        scope.launch { refresh() }
    }

    suspend fun refresh() {
        val core = core ?: return
        runCatchingCancellable { core.getSyncStatus() }.getOrNull()?.let { status = it }
    }

    /// Device lists only matter while the sync settings are on screen.
    suspend fun refreshDevices() {
        val core = core ?: return
        thisDevice = runCatchingCancellable { core.syncThisDevice() }.getOrNull()
        devices = runCatchingCancellable { core.syncListDevices() }.getOrDefault(emptyList())
        pending = runCatchingCancellable { core.syncListPending() }.getOrDefault(emptyList())
    }

    fun wake() {
        val core = core ?: return
        scope.launch { runCatchingCancellable { core.syncWake() } }
    }

    suspend fun setEnabled(enabled: Boolean) = run { it.syncSetEnabled(enabled) }

    suspend fun setDeviceName(name: String) = run { it.syncSetDeviceName(name) }

    suspend fun add(deviceId: String, name: String) = run { it.syncAddDevice(deviceId, name) }

    suspend fun remove(deviceId: String) = run { it.syncRemoveDevice(deviceId) }

    suspend fun resetIdentity() = run { it.syncResetIdentity() }

    suspend fun keepIdentity() = run { it.syncKeepIdentity() }

    suspend fun webUiUrl(): String? = core?.let { core -> runCatchingCancellable { core.syncWebUiUrl() }.getOrNull() }

    private suspend fun run(action: suspend (CoreService) -> Unit) {
        val core = core ?: return
        busy = true
        try {
            action(core)
            error = null
        } catch (e: CancellationException) {
            throw e
        } catch (e: Exception) {
            error = e.userMessage
        } finally {
            busy = false
        }
        refresh()
        refreshDevices()
    }

    companion object {
        /// Accepts the JSON pairing payload or a bare device id.
        fun parsePairing(content: String): Pair<String, String?>? {
            val trimmed = content.trim()
            if (trimmed.isEmpty()) return null
            val obj = runCatching { JSONObject(trimmed) }.getOrNull()
            val id = obj?.optString("deviceId").orEmpty()
            if (obj != null && id.isNotEmpty()) {
                return id to obj.optString("name").takeIf { it.isNotEmpty() }
            }
            return if (obj == null) trimmed to null else null
        }

        fun pairingPayload(deviceId: String, name: String?): String =
            JSONObject(sortedMapOf("deviceId" to deviceId, "name" to (name ?: ""))).toString()
    }
}

class AnkiStore(private val events: EventHub, private val scope: CoroutineScope) : CoreBound {
    var status by mutableStateOf<AnkiSyncStatus>(ankiIdle)
        private set
    var error by mutableStateOf<String?>(null)

    /// Anki has answered and is not known to be gone. A first attempt still
    /// under way does not count: the toolbar button would show and go again.
    var reachable by mutableStateOf(false)
        private set

    private var core: CoreService? = null

    override fun attach(core: CoreService) {
        this.core = core
        events.on { event ->
            if (event is CoreEvent.AnkiSyncStatusChanged) scope.launch { refresh() }
        }
        scope.launch { refresh() }
    }

    suspend fun refresh() {
        val core = core ?: return
        val status = runCatchingCancellable { core.getAnkiSyncStatus() }.getOrNull() ?: return
        this.status = status
        when (status.state) {
            AnkiSyncState.UNREACHABLE -> reachable = false
            AnkiSyncState.OK, AnkiSyncState.ERR -> reachable = true
            // A retry in progress says nothing yet; keep the last answer.
            AnkiSyncState.IDLE, AnkiSyncState.SYNCING -> {}
        }
    }

    suspend fun syncNow() {
        val core = core ?: return
        try {
            core.syncAnkiNow()
            error = null
        } catch (e: CancellationException) {
            throw e
        } catch (e: Exception) {
            error = e.userMessage
        }
        refresh()
    }
}
