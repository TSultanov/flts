import SwiftUI

/// The web app's top bar on every page: sync status, Anki sync, Lyrics.
struct AppToolbarItems: ToolbarContent {
    @Environment(AppModel.self) private var model
    @Environment(\.openWindow) private var openWindow
    @Environment(\.openSettings) private var openSettings
    @AppStorage("settingsTab", store: AppDefaults.store) private var settingsTab = SettingsTab.translation

    var body: some ToolbarContent {
        ToolbarItemGroup(placement: .automatic) {
            if model.config.config?.syncEnabled == true {
                Button {
                    settingsTab = .sync
                    openSettings()
                } label: {
                    HStack(spacing: 4) {
                        Image(systemName: syncSymbol)
                            .symbolEffect(.rotate, isActive: syncBusy)
                        if model.sync.status.state == .syncing, let c = model.sync.status.completion {
                            Text("\(Int(c))%").font(.caption.monospacedDigit())
                        }
                    }
                }
                .help(syncLabel)
                .accessibilityLabel(syncLabel)
            }
            // As in the web app: hidden while Anki isn't running.
            if model.config.config?.ankiSyncEnabled == true, model.config.isConfigured,
                model.anki.status.state != .unreachable
            {
                Button {
                    Task { await model.anki.syncNow() }
                } label: {
                    Image(systemName: ankiSymbol)
                        .symbolEffect(.rotate, isActive: model.anki.status.state == .syncing)
                }
                .disabled(model.anki.status.state == .syncing)
                .help(ankiHelp)
                .accessibilityLabel("Sync Anki")
            }
            Button {
                openWindow(id: "lyrics")
            } label: {
                Label("Lyrics", systemImage: "music.note.list")
            }
            .help("Translated lyrics for what Spotify is playing (⇧⌘L)")
        }
    }

    private var syncBusy: Bool {
        model.sync.status.state == .starting || model.sync.status.state == .syncing
    }

    private var syncLabel: String {
        let s = model.sync.status
        switch s.state {
        case .disabled: return "Sync is off"
        case .starting: return "Sync is starting"
        case .online:
            return s.deviceCount == 0
                ? "Online — no devices paired yet" : "Synced · \(s.connectedCount) of \(s.deviceCount) devices connected"
        case .syncing: return s.completion.map { "Syncing \(Int($0))%" } ?? "Syncing"
        case .error: return s.lastError ?? "Sync failed"
        }
    }

    private var syncSymbol: String {
        switch model.sync.status.state {
        case .disabled: "icloud.slash"
        case .starting, .syncing: "arrow.triangle.2.circlepath.icloud"
        case .online: (model.sync.status.storeIssues?.any ?? false) ? "exclamationmark.icloud" : "checkmark.icloud"
        case .error: "exclamationmark.icloud"
        }
    }

    private var ankiSymbol: String {
        switch model.anki.status.state {
        case .err: "exclamationmark.arrow.triangle.2.circlepath"
        case .ok: "checkmark.rectangle.stack"
        default: "rectangle.stack"
        }
    }

    private var ankiHelp: String {
        switch model.anki.status.state {
        case .unreachable: "Anki is not running"
        case .err: model.anki.status.lastError ?? "Anki sync failed"
        case .syncing: "Syncing cards to Anki…"
        default: "Sync cards to Anki now"
        }
    }
}
