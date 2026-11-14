import SwiftUI

/// The web app's top bar on every page: sync status, Anki sync, Lyrics.
struct AppToolbarItems: ToolbarContent {
    @Environment(AppModel.self) private var model
    @Environment(\.openWindow) private var openWindow
    @Environment(\.openSettings) private var openSettings

    var body: some ToolbarContent {
        ToolbarItemGroup(placement: .automatic) {
            if model.config.config?.syncEnabled == true {
                Button {
                    openSettings()
                } label: {
                    Label(syncLabel, systemImage: syncSymbol)
                }
                .help(syncLabel)
            }
            if model.config.config?.ankiSyncEnabled == true {
                Button {
                    Task { await model.anki.syncNow() }
                } label: {
                    Label("Sync Anki", systemImage: ankiSymbol)
                }
                .disabled(model.anki.status.state == .syncing)
                .help(ankiHelp)
            }
            Button {
                openWindow(id: "lyrics")
            } label: {
                Label("Lyrics", systemImage: "music.note.list")
            }
            .help("Translated lyrics for what Spotify is playing (⇧⌘L)")
        }
    }

    private var syncLabel: String {
        let s = model.sync.status
        switch s.state {
        case .disabled: return "Sync is off"
        case .starting: return "Sync is starting"
        case .online: return "Synced · \(s.connectedCount) of \(s.deviceCount) devices connected"
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
        case .err, .unreachable: "exclamationmark.arrow.triangle.2.circlepath"
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
