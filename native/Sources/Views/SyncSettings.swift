import SwiftUI

struct SyncSettings: View {
    @Environment(AppModel.self) private var model
    @State private var addId = ""
    @State private var addName = ""
    @State private var confirmReset = false
    @State private var resetting = false
    @State private var deviceName = ""
    @State private var webUI: URL?
    @State private var removing: DeviceEntry?
    #if os(iOS)
    @State private var scanning = false
    #endif

    var body: some View {
        let sync = model.sync
        let config = model.config
        Form {
            Section {
                if config.config?.syncEnabled != true {
                    Text("FLTS syncs your library with your other devices through a built-in Syncthing engine. If you run Syncthing separately on this device, stop it before enabling sync.")
                        .font(.callout)
                        .foregroundStyle(.secondary)
                }
                Toggle(
                    "Sync this library with my other devices",
                    isOn: Binding(
                        get: { config.config?.syncEnabled ?? false },
                        set: { on in
                            Task {
                                if on, config.config?.syncDeviceName == nil, let name = SyncStore.suggestedDeviceName {
                                    await sync.setDeviceName(name)
                                }
                                await sync.setEnabled(on)
                            }
                        }))
                if config.config?.syncEnabled == true {
                    HStack {
                        TextField("This device's name", text: $deviceName)
                            .onSubmit { rename() }
                        Button("Rename") { rename() }
                            .disabled(trimmedName.isEmpty || trimmedName == currentName)
                    }
                    .onAppear { deviceName = currentName }
                    .onChange(of: currentName) { _, name in deviceName = name }
                    SyncStatusRow(status: sync.status)
                }
            }

            if config.config?.syncEnabled == true {
                if let issues = sync.status.storeIssues, issues.any {
                    StoreIssuesSection(issues: issues, confirmReset: $confirmReset, resetting: resetting)
                }

                if let device = sync.thisDevice {
                    Section("Pair a device") {
                        HStack(alignment: .top, spacing: 16) {
                            if let qr = Platform.qrCode(SyncStore.pairingPayload(deviceId: device.deviceId, name: device.name)) {
                                qr
                                    .interpolation(.none)
                                    .resizable()
                                    .frame(width: 132, height: 132)
                                    .accessibilityLabel("Pairing code")
                            }
                            VStack(alignment: .leading, spacing: 8) {
                                Text("Scan this code on your other device, or enter this device ID there:")
                                    .foregroundStyle(.secondary)
                                Text(device.deviceId)
                                    .font(.system(.caption, design: .monospaced))
                                    .textSelection(.enabled)
                                Button("Copy Device ID") { Platform.copy(device.deviceId) }
                                    .controlSize(.small)
                            }
                        }
                        #if os(iOS)
                        if PairingScanner.isSupported {
                            Button {
                                scanning = true
                            } label: {
                                Label("Scan Another Device's Code", systemImage: "qrcode.viewfinder")
                            }
                            .sheet(isPresented: $scanning) {
                                PairingScanner { payload in
                                    if let parsed = SyncStore.parsePairing(payload) {
                                        addId = parsed.deviceId
                                        if let name = parsed.name, !name.isEmpty { addName = name }
                                    } else {
                                        addId = payload
                                    }
                                }
                            }
                        }
                        #endif
                        TextField("Other device's ID or pairing code", text: $addId)
                            .literalEntry()
                        TextField("Its name", text: $addName)
                        HStack {
                            Spacer()
                            Button("Add Device") {
                                guard let parsed = SyncStore.parsePairing(addId) else {
                                    sync.error = "Enter a valid device ID or pairing code."
                                    return
                                }
                                let trimmedName = addName.trimmingCharacters(in: .whitespacesAndNewlines)
                                let name = trimmedName.isEmpty ? (parsed.name ?? "") : trimmedName
                                Task {
                                    await sync.add(deviceId: parsed.deviceId, name: name)
                                    if sync.error == nil {
                                        addId = ""
                                        addName = ""
                                    }
                                }
                            }
                            .disabled(addId.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty)
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
                            Button("Remove", role: .destructive) { removing = d }
                                .buttonStyle(.borderless)
                        }
                    }
                }
            }

            if let webUI {
                Section {
                    #if os(macOS)
                    Link("Open the Syncthing web UI", destination: webUI)
                    #else
                    // Opening it would suspend this app, which serves the page.
                    LabeledContent("Syncthing web UI") {
                        Text(webUI.absoluteString).font(.caption.monospaced()).textSelection(.enabled)
                    }
                    #endif
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
        .confirmationDialog(
            "Stop syncing with “\(removing.map { $0.name.isEmpty ? "this device" : $0.name } ?? "")”?",
            isPresented: Binding(get: { removing != nil }, set: { if !$0 { removing = nil } }), titleVisibility: .visible
        ) {
            Button("Remove Device", role: .destructive) {
                if let id = removing?.deviceId { Task { await sync.remove(deviceId: id) } }
                removing = nil
            }
            Button("Cancel", role: .cancel) { removing = nil }
        } message: {
            Text("It has to be paired again to sync with this device.")
        }
        .confirmationDialog("New sync identity", isPresented: $confirmReset) {
            Button("Give This Device a New Identity", role: .destructive) {
                resetting = true
                Task {
                    await sync.resetIdentity()
                    resetting = false
                }
            }
            Button("Cancel", role: .cancel) {}
        } message: {
            Text("This device will get a new sync identity and sync will restart. It then has to be paired with your other devices again, where it shows up as a new device.")
        }
    }
}

extension SyncSettings {
    fileprivate var currentName: String {
        model.config.config?.syncDeviceName ?? model.sync.thisDevice?.name ?? ""
    }

    fileprivate var trimmedName: String {
        deviceName.trimmingCharacters(in: .whitespaces)
    }

    fileprivate func rename() {
        let name = trimmedName
        guard !name.isEmpty, name != currentName else { return }
        Task { await model.sync.setDeviceName(name) }
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
    let resetting: Bool
    /// Hidden once a choice was made here, until the issue clears and recurs.
    @State private var identityDismissed = false
    @Environment(AppModel.self) private var model
    private var sync: SyncStore { model.sync }

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
            if issues.sharedIdentity && !identityDismissed {
                VStack(alignment: .leading, spacing: 8) {
                    Label(
                        "This device shares its sync identity with another copy of the app, as happens after restoring a backup. One of the two needs a new identity and will then have to be paired again.",
                        systemImage: "person.2.slash"
                    )
                    .foregroundStyle(.red)
                    HStack {
                        Button(resetting ? "Giving this device a new identity…" : "Give this device a new identity") {
                            confirmReset = true
                        }
                        Button("Keep this device's identity") {
                            Task {
                                await sync.keepIdentity()
                                if sync.error == nil { identityDismissed = true }
                            }
                        }
                    }
                }
            }
        }
    }
}
