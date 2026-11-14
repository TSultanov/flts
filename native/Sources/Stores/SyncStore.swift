import Foundation
import Observation

@MainActor
@Observable
final class SyncStore: CoreBound {
    private(set) var status = SyncStatus.disabled
    private(set) var thisDevice: ThisDevice?
    private(set) var devices: [DeviceEntry] = []
    private(set) var pending: [PendingDevice] = []
    var error: String?

    private let events: EventHub
    private var core: (any CoreService)?

    init(events: EventHub) {
        self.events = events
    }

    func attach(_ core: any CoreService) {
        self.core = core
        events.on("sync_status_changed") { [weak self] _ in
            Task { await self?.refresh() }
        }
        Task { await refresh() }
    }

    func refresh() async {
        guard let core else { return }
        if let status: SyncStatus = try? await core.invoke("get_sync_status") {
            self.status = status
        }
    }

    /// Device lists only matter while the sync settings are on screen.
    func refreshDevices() async {
        guard let core else { return }
        thisDevice = (try? await core.invoke("sync_get_this_device", as: ThisDevice?.self)) ?? nil
        devices = (try? await core.invoke("sync_list_devices")) ?? []
        pending = (try? await core.invoke("sync_list_pending")) ?? []
    }

    func wake() {
        guard let core else { return }
        Task { try? await core.call("sync_wake") }
    }

    func setEnabled(_ enabled: Bool) async {
        await run("sync_set_enabled", ["enabled": .bool(enabled)])
    }

    func setDeviceName(_ name: String) async {
        await run("sync_set_device_name", ["name": .string(name)])
    }

    func add(deviceId: String, name: String) async {
        await run("sync_add_device", ["deviceId": .string(deviceId), "name": .string(name)])
    }

    func remove(deviceId: String) async {
        await run("sync_remove_device", ["deviceId": .string(deviceId)])
    }

    func resetIdentity() async {
        await run("sync_reset_identity")
    }

    func keepIdentity() async {
        await run("sync_keep_identity")
    }

    func webUIURL() async -> URL? {
        guard let core else { return nil }
        let raw = (try? await core.invoke("sync_web_ui_url", as: String?.self)) ?? nil
        return raw.flatMap(URL.init(string:))
    }

    private func run(_ cmd: String, _ args: [String: JSONValue] = [:]) async {
        guard let core else { return }
        do {
            try await core.call(cmd, args)
            error = nil
        } catch {
            self.error = error.localizedDescription
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

    init(events: EventHub) {
        self.events = events
    }

    func attach(_ core: any CoreService) {
        self.core = core
        events.on("anki_sync_status_changed") { [weak self] _ in
            Task { await self?.refresh() }
        }
        Task { await refresh() }
    }

    func refresh() async {
        guard let core else { return }
        if let status: AnkiSyncStatus = try? await core.invoke("get_anki_sync_status") {
            self.status = status
        }
    }

    func syncNow() async {
        guard let core else { return }
        do {
            try await core.call("sync_anki_now")
            error = nil
        } catch {
            self.error = error.localizedDescription
        }
        await refresh()
    }
}
