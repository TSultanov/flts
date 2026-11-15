import Foundation
import Observation

@MainActor
@Observable
final class SyncStore: CoreBound {
    private(set) var status = SyncStatus.disabled
    private(set) var thisDevice: ThisDevice?
    private(set) var devices: [DeviceEntry] = []
    private(set) var pending: [PendingDevice] = []
    /// True while an action runs; the settings disable their controls.
    private(set) var busy = false
    var error: String?

    private let events: EventHub
    private var core: (any CoreService)?

    init(events: EventHub) {
        self.events = events
    }

    func attach(_ core: any CoreService) {
        self.core = core
        events.on { [weak self] event in
            if case .syncStatusChanged = event { Task { await self?.refresh() } }
        }
        Task { await refresh() }
    }

    func refresh() async {
        guard let core else { return }
        if let status = try? await core.getSyncStatus() {
            self.status = status
        }
    }

    /// Device lists only matter while the sync settings are on screen.
    func refreshDevices() async {
        guard let core else { return }
        thisDevice = (try? await core.syncThisDevice()) ?? nil
        devices = (try? await core.syncListDevices()) ?? []
        pending = (try? await core.syncListPending()) ?? []
    }

    func wake() {
        guard let core else { return }
        Task { try? await core.syncWake() }
    }

    func setEnabled(_ enabled: Bool) async {
        await run { try await $0.syncSetEnabled(enabled: enabled) }
    }

    func setDeviceName(_ name: String) async {
        await run { try await $0.syncSetDeviceName(name: name) }
    }

    func add(deviceId: String, name: String) async {
        await run { try await $0.syncAddDevice(deviceId: deviceId, name: name) }
    }

    func remove(deviceId: String) async {
        await run { try await $0.syncRemoveDevice(deviceId: deviceId) }
    }

    func resetIdentity() async {
        await run { try await $0.syncResetIdentity() }
    }

    func keepIdentity() async {
        await run { try await $0.syncKeepIdentity() }
    }

    func webUIURL() async -> URL? {
        guard let core else { return nil }
        let raw = (try? await core.syncWebUiUrl()) ?? nil
        return raw.flatMap(URL.init(string:))
    }

    private func run(_ action: (any CoreService) async throws -> Void) async {
        guard let core else { return }
        busy = true
        defer { busy = false }
        do {
            try await action(core)
            error = nil
        } catch {
            self.error = error.userMessage
        }
        await refresh()
        await refreshDevices()
    }

    /// Accepts the JSON pairing payload or a bare device id.
    nonisolated static func parsePairing(_ content: String) -> (deviceId: String, name: String?)? {
        let trimmed = content.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !trimmed.isEmpty else { return nil }
        if let data = trimmed.data(using: .utf8),
            let obj = try? JSONSerialization.jsonObject(with: data) as? [String: Any],
            let id = obj["deviceId"] as? String
        {
            return (id, obj["name"] as? String)
        }
        return (trimmed, nil)
    }

    nonisolated static func pairingPayload(deviceId: String, name: String?) -> String {
        let obj = ["deviceId": deviceId, "name": name ?? ""]
        let data = (try? JSONSerialization.data(withJSONObject: obj, options: [.sortedKeys])) ?? Data()
        return String(decoding: data, as: UTF8.self)
    }
}

@MainActor
@Observable
final class AnkiStore: CoreBound {
    private(set) var status = AnkiSyncStatus.idle
    var error: String?
    private let events: EventHub
    private var core: (any CoreService)?

    /// Anki has answered at least once and is not known to be gone. Before the
    /// first sync attempt the state is idle too, and must not count: the
    /// toolbar button would show and go again a second after launch.
    var reachable: Bool {
        switch status.state {
        case .unreachable: false
        case .idle: status.lastFinishedAtMs != nil
        case .syncing, .ok, .err: true
        }
    }

    init(events: EventHub) {
        self.events = events
    }

    func attach(_ core: any CoreService) {
        self.core = core
        events.on { [weak self] event in
            if case .ankiSyncStatusChanged = event { Task { await self?.refresh() } }
        }
        Task { await refresh() }
    }

    func refresh() async {
        guard let core else { return }
        if let status = try? await core.getAnkiSyncStatus() {
            self.status = status
        }
    }

    func syncNow() async {
        guard let core else { return }
        do {
            try await core.syncAnkiNow()
            error = nil
        } catch {
            self.error = error.userMessage
        }
        await refresh()
    }
}
