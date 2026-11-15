import SwiftUI

struct SyncSettings: View {
    @Environment(AppModel.self) private var model
    @State private var addId = ""
    @State private var addName = ""
    @State private var confirmReset = false
    @State private var webUI: URL?

    var body: some View {
        let sync = model.sync
        let config = model.config
        Form {
            Section {
                if config.config?.syncEnabled != true {
                    Text("FLTS syncs your library with your other devices through a built-in Syncthing engine. If you run Syncthing separately on this Mac, stop it before enabling sync.")
                        .font(.callout)
                        .foregroundStyle(.secondary)
                }
                Toggle(
                    "Sync this library with my other devices",
                    isOn: Binding(
                        get: { config.config?.syncEnabled ?? false },
                        set: { on in Task { await sync.setEnabled(on) } }))
                if config.config?.syncEnabled == true {
                    CommitField(title: "This device's name", value: config.config?.syncDeviceName ?? sync.thisDevice?.name ?? "")
                    { name in
                        Task { await sync.setDeviceName(name) }
                    }
                    SyncStatusRow(status: sync.status)
                }
            }

            if config.config?.syncEnabled == true {
                if let issues = sync.status.storeIssues, issues.any {
                    StoreIssuesSection(issues: issues, confirmReset: $confirmReset)
                }

                if let device = sync.thisDevice {
                    Section("Pair a device") {
                        HStack(alignment: .top, spacing: 16) {
                            if let qr = Platform.qrCode(SyncStore.pairingPayload(deviceId: device.deviceId, name: device.name)) {
                                Image(nsImage: qr)
                                    .interpolation(.none)
                                    .resizable()
                                    .frame(width: 132, height: 132)
                                    .accessibilityLabel("Pairing code")
                            }
                            VStack(alignment: .leading, spacing: 8) {
                                Text("Scan this code on your iPad or phone, or enter this device ID there:")
                                    .foregroundStyle(.secondary)
                                Text(device.deviceId)
                                    .font(.system(.caption, design: .monospaced))
                                    .textSelection(.enabled)
                                Button("Copy Device ID") { Platform.copy(device.deviceId) }
                                    .controlSize(.small)
                            }
                        }
                        TextField("Other device's ID or pairing code", text: $addId)
                        TextField("Its name", text: $addName)
                        HStack {
                            Spacer()
                            Button("Add Device") {
                                guard let parsed = SyncStore.parsePairing(addId) else { return }
                                let name = addName.isEmpty ? (parsed.name ?? "") : addName
                                Task {
                                    await sync.add(deviceId: parsed.deviceId, name: name)
                                    addId = ""
                                    addName = ""
                                }
                            }
                            .disabled(addId.trimmingCharacters(in: .whitespaces).isEmpty)
                        }
                    }
                }

                if !sync.pending.isEmpty {
                    Section("Asking to pair") {
                        ForEach(sync.pending) { p in
                            HStack {
                                VStack(alignment: .leading) {
                                    Text(p.name.isEmpty ? "Unnamed device" : p.name)
                                    Text(p.deviceId).font(.caption.monospaced()).foregroundStyle(.secondary)
                                        .lineLimit(1).truncationMode(.middle)
                                }
                                Spacer()
                                Button("Accept") { Task { await sync.add(deviceId: p.deviceId, name: p.name) } }
                            }
                        }
                    }
                }

                Section("Devices") {
                    if sync.devices.isEmpty {
                        Text("No other devices yet").foregroundStyle(.secondary)
                    }
                    ForEach(sync.devices) { d in
                        HStack {
                            Circle().fill(d.connected ? Color.green : Color.secondary.opacity(0.4))
                                .frame(width: 8, height: 8)
                            VStack(alignment: .leading) {
                                Text(d.name.isEmpty ? "Unnamed device" : d.name)
                                Text(d.deviceId).font(.caption.monospaced()).foregroundStyle(.secondary)
                                    .lineLimit(1).truncationMode(.middle)
                            }
                            Spacer()
                            Button("Remove", role: .destructive) { Task { await sync.remove(deviceId: d.deviceId) } }
                                .buttonStyle(.borderless)
                        }
                    }
                }
            }

            if let webUI {
                Section {
                    Link("Open the Syncthing web UI", destination: webUI)
                } footer: {
                    Text("For diagnosing sync; FLTS manages these settings itself.")
                }
            }

            if let error = sync.error {
                Text(error).foregroundStyle(.red).font(.callout)
            }
        }
        .formStyle(.grouped)
        .task(id: sync.status.state) { webUI = await sync.webUIURL() }
        .task {
            await sync.refresh()
            await sync.refreshDevices()
        }
        .task(id: sync.status) { await sync.refreshDevices() }
        // New pairing requests show up without waiting for a status change.
        .task {
            while !Task.isCancelled {
                try? await Task.sleep(for: .seconds(4))
                if config.config?.syncEnabled == true { await sync.refreshDevices() }
            }
        }
        .disabled(sync.busy)
        .confirmationDialog("Reset this device's sync identity?", isPresented: $confirmReset) {
            Button("Reset Identity", role: .destructive) { Task { await sync.resetIdentity() } }
            Button("This Is the Original", role: .none) { Task { await sync.keepIdentity() } }
        } message: {
            Text("Another device uses the same identity, usually because it was restored from a backup of this one. Reset the copy; every pairing on it has to be redone.")
        }
    }
}

struct SyncStatusRow: View {
    let status: SyncStatus

    var body: some View {
        HStack {
            switch status.state {
            case .disabled: Label("Off", systemImage: "pause.circle")
            case .starting: Label("Starting…", systemImage: "hourglass")
            case .online:
                Label(
                    status.deviceCount == 0
                        ? "Online — no devices paired yet" : "Online · \(status.connectedCount) of \(status.deviceCount) connected",
                    systemImage: "checkmark.circle"
                )
                .foregroundStyle(.green)
            case .syncing:
                if let c = status.completion {
                    ProgressView(value: c / 100) { Text("Syncing…") }
                } else {
                    Label("Syncing…", systemImage: "arrow.triangle.2.circlepath")
                }
            case .error:
                Label(status.lastError ?? "Sync failed", systemImage: "exclamationmark.triangle").foregroundStyle(.red)
            }
        }
    }
}

private struct StoreIssuesSection: View {
    let issues: StoreIssues
    @Binding var confirmReset: Bool

    var body: some View {
        Section("Needs attention") {
            if issues.newerVersion > 0 || issues.stalled {
                Label("Another device uses a newer version of the app. Update this device to see its latest changes.", systemImage: "arrow.up.circle")
            }
            if issues.unreadable > 0 {
                Label("Some synced data could not be read. Changes from another device may be missing.", systemImage: "exclamationmark.triangle")
            }
            if issues.strangers > 0 {
                Label("The sync folder contains conflict copies that are being ignored.", systemImage: "doc.on.doc")
            }
            if issues.sharedIdentity {
                HStack {
                    Label("Another device shares this device's identity.", systemImage: "person.2.slash")
                    Spacer()
                    Button("Resolve…") { confirmReset = true }
                }
            }
        }
    }
}
