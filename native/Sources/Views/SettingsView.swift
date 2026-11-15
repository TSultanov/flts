import SwiftUI

struct SettingsView: View {
    var body: some View {
        TabView {
            Form { TranslationSettings() }
                .formStyle(.grouped)
                .tabItem { Label("Translation", systemImage: "character.bubble") }
            Form { ReadingSettings() }
                .formStyle(.grouped)
                .tabItem { Label("Reading", systemImage: "textformat.size") }
            SyncSettings()
                .tabItem { Label("Sync", systemImage: "arrow.triangle.2.circlepath") }
            Form { AnkiSettings() }
                .formStyle(.grouped)
                .tabItem { Label("Anki", systemImage: "rectangle.stack") }
            Form { SpotifySettings() }
                .formStyle(.grouped)
                .tabItem { Label("Spotify", systemImage: "music.note") }
            Form { StorageSettings() }
                .formStyle(.grouped)
                .tabItem { Label("Storage", systemImage: "internaldrive") }
        }
        .frame(width: 560, height: 520)
    }
}

/// A text field that writes back on Return or when it loses focus, so the
/// backend is not reconfigured on every keystroke.
struct CommitField: View {
    let title: String
    let value: String
    var secure = false
    var prompt: String?
    let commit: (String) -> Void
    @State private var draft = ""
    @FocusState private var focused: Bool

    var body: some View {
        Group {
            if secure {
                SecureField(title, text: $draft, prompt: prompt.map(Text.init))
            } else {
                TextField(title, text: $draft, prompt: prompt.map(Text.init))
            }
        }
        .focused($focused)
        .onSubmit { save() }
        .onChange(of: focused) { _, isFocused in if !isFocused { save() } }
        .onAppear { draft = value }
        .onChange(of: value) { _, new in if !focused { draft = new } }
        .onDisappear { save() }
    }

    private func save() {
        if draft != value { commit(draft) }
    }
}

struct TranslationSettings: View {
    @Environment(AppModel.self) private var model

    var body: some View {
        let store = model.config
        if let config = store.config {
            Section {
                Picker(
                    "I read in",
                    selection: Binding(
                        get: { config.targetLanguageId },
                        set: { id in Task { await store.update { $0.targetLanguageId = id } } })
                ) {
                    if config.targetLanguageId.isEmpty { Text("Choose…").tag("") }
                    ForEach(store.languages) { Text($0.label).tag($0.id) }
                }
            }
            Section("Provider") {
                Picker(
                    "Provider",
                    selection: Binding(
                        get: { config.translationProvider },
                        set: { provider in
                            Task {
                                await store.update {
                                    $0.translationProvider = provider
                                    if let meta = store.provider(provider) { $0.model = meta.defaultModel }
                                }
                            }
                        })
                ) {
                    ForEach(store.providers) { Text($0.name).tag($0.id) }
                }
                CommitField(
                    title: "API key", value: config.apiKey(for: config.translationProvider) ?? "", secure: true,
                    prompt: "Paste your \(store.provider(config.translationProvider)?.name ?? "provider") API key"
                ) { key in
                    let provider = config.translationProvider
                    Task { await store.update { $0.setApiKey(key, for: provider) } }
                }
                ModelPicker(config: config)
            }
            Section("Performance") {
                Stepper(
                    "Parallel translations: \(config.translationConcurrency)",
                    value: Binding(
                        get: { config.translationConcurrency },
                        set: { n in Task { await store.update { $0.translationConcurrency = n } } }), in: 1...16)
            }
            if let error = store.error {
                Text(error).foregroundStyle(.red).font(.callout)
            }
        } else {
            ProgressView()
        }
    }
}

private struct ModelPicker: View {
    let config: AppConfig
    @Environment(AppModel.self) private var model

    var body: some View {
        let store = model.config
        let models = store.models(for: config.translationProvider)
        HStack {
            Picker(
                "Model",
                selection: Binding(
                    get: { config.model },
                    set: { id in Task { await store.update { $0.model = id } } })
            ) {
                if !models.contains(where: { $0.id == config.model }) {
                    Text(config.model).tag(config.model)
                }
                ForEach(models) { Text($0.name).tag($0.id) }
            }
            if store.modelsLoading {
                ProgressView().controlSize(.small)
            } else {
                Button {
                    Task { await store.loadModels() }
                } label: {
                    Image(systemName: "arrow.clockwise")
                }
                .buttonStyle(.borderless)
                .help("Refresh the model list")
            }
        }
    }
}

struct ReadingSettings: View {
    @Bindable private var prefs = ReaderPreferences.shared

    var body: some View {
        Section("Text") {
            Slider(value: $prefs.fontSize, in: 12...36, step: 1) {
                Text("Size")
            } minimumValueLabel: {
                Image(systemName: "textformat.size.smaller")
            } maximumValueLabel: {
                Image(systemName: "textformat.size.larger")
            }
            Text("The quick brown fox jumps over the lazy dog.")
                .font(.system(size: prefs.fontSize, design: .serif))
        }
    }
}

struct AnkiSettings: View {
    @Environment(AppModel.self) private var model

    var body: some View {
        let store = model.config
        if let config = store.config {
            Section {
                Toggle(
                    "Sync cards to Anki automatically",
                    isOn: Binding(
                        get: { config.ankiSyncEnabled },
                        set: { on in Task { await store.update { $0.ankiSyncEnabled = on } } }))
                CommitField(
                    title: "AnkiConnect address", value: config.ankiEndpoint ?? "",
                    prompt: "http://127.0.0.1:8765"
                ) { v in
                    Task { await store.update { $0.ankiEndpoint = v.isEmpty ? nil : v } }
                }
                CommitField(title: "AnkiConnect API key", value: config.ankiApiKey ?? "", secure: true) { v in
                    Task { await store.update { $0.ankiApiKey = v.isEmpty ? nil : v } }
                }
            } footer: {
                Text("Requires the AnkiConnect add-on in Anki desktop.")
            }
            Section("Status") {
                AnkiStatusRow()
            }
        }
    }
}

struct AnkiStatusRow: View {
    @Environment(AppModel.self) private var model

    var body: some View {
        let anki = model.anki
        HStack {
            switch anki.status.state {
            case .idle: Label("Not synced yet", systemImage: "circle.dashed")
            case .syncing: Label("Syncing…", systemImage: "arrow.triangle.2.circlepath")
            case .ok: Label(summary, systemImage: "checkmark.circle").foregroundStyle(.green)
            case .err: Label(anki.status.lastError ?? "Failed", systemImage: "exclamationmark.triangle").foregroundStyle(.red)
            case .unreachable: Label("Anki is not running", systemImage: "bolt.horizontal.circle").foregroundStyle(.orange)
            }
            Spacer()
            Button("Sync Now") { Task { await anki.syncNow() } }
                .disabled(anki.status.state == .syncing)
        }
        if let error = anki.error {
            Text(error).foregroundStyle(.red).font(.caption)
        }
    }

    private var summary: String {
        guard let r = model.anki.status.lastReport else { return "Synced" }
        return r.failed > 0 ? "\(r.succeeded) synced, \(r.failed) failed" : "\(r.totalCards) cards synced"
    }
}

struct SpotifySettings: View {
    @Environment(AppModel.self) private var model

    var body: some View {
        let store = model.config
        let lyrics = model.lyrics
        if let config = store.config {
            Section {
                CommitField(title: "Client ID", value: config.spotifyClientId ?? "") { v in
                    Task { await store.update { $0.spotifyClientId = v.isEmpty ? nil : v } }
                }
                HStack {
                    if lyrics.webStatus?.connected == true {
                        Label("Connected", systemImage: "checkmark.circle").foregroundStyle(.green)
                        Spacer()
                        Button("Disconnect") { Task { await lyrics.disconnectWeb() } }
                    } else {
                        Text(lyrics.webStatus?.lastError ?? "Not connected").foregroundStyle(.secondary)
                        Spacer()
                        Button("Connect…") {
                            Task { await lyrics.connectWeb(clientId: config.spotifyClientId ?? "") }
                        }
                        .disabled((config.spotifyClientId ?? "").isEmpty)
                    }
                }
                if lyrics.webStatus?.premiumRequired == true {
                    Text("The queue needs Spotify Premium.").font(.caption).foregroundStyle(.secondary)
                }
            } header: {
                Text("Spotify Web API")
            } footer: {
                Text("Use the client ID of your own Spotify developer app; it lets FLTS preload upcoming tracks.")
            }
            Section("Lyrics") {
                Stepper(
                    "Preload \(config.spotifyPreloadCount) upcoming tracks",
                    value: Binding(
                        get: { config.spotifyPreloadCount },
                        set: { n in Task { await store.update { $0.spotifyPreloadCount = n } } }), in: 0...10)
                Toggle(
                    "Show the next track",
                    isOn: Binding(
                        get: { config.spotifyShowNextTrack },
                        set: { on in Task { await store.update { $0.spotifyShowNextTrack = on } } }))
            }
            Section {
                HStack {
                    if lyrics.cdpStatus?.available == true {
                        Label("Spotify lyrics bridge is on", systemImage: "checkmark.circle").foregroundStyle(.green)
                    } else {
                        Text(lyrics.cdpStatus?.hint ?? "Spotify lyrics bridge is off").foregroundStyle(.secondary)
                    }
                    Spacer()
                    Button("Restart Spotify with Bridge") { Task { await lyrics.restartSpotifyWithDevTools() } }
                        .disabled(lyrics.cdpStatus?.available == true)
                }
                Toggle(
                    "Start Spotify with the bridge at login",
                    isOn: Binding(
                        get: { lyrics.loginAgent?.installed ?? false },
                        set: { on in
                            Task { on ? await lyrics.installLoginAgent() : await lyrics.removeLoginAgent() }
                        }))
            } header: {
                Text("Spotify Lyrics Fallback")
            } footer: {
                Text("When LRCLIB has no lyrics, FLTS can read Spotify's own through the desktop app.")
            }
            if let error = lyrics.error {
                Text(error).foregroundStyle(.red).font(.callout)
            }
        }
        Color.clear.frame(height: 0)
            .task { await lyrics.refreshSpotifyStatus() }
    }
}

struct StorageSettings: View {
    @Environment(AppModel.self) private var model
    @State private var purged: String?

    var body: some View {
        Section("Library") {
            LabeledContent("Location") {
                Text(model.config.libraryRoot ?? "—")
                    .textSelection(.enabled)
                    .lineLimit(2)
                    .truncationMode(.middle)
            }
            Button("Show in Finder") { Platform.revealLibrary(model.config.libraryRoot) }
        }
        Section {
            HStack {
                Button("Purge Gemini Caches") {
                    Task {
                        do {
                            let n = try await model.config.purgeGeminiCaches()
                            purged = "Removed \(n) cached prompts."
                        } catch {
                            purged = error.localizedDescription
                        }
                    }
                }
                if let purged { Text(purged).foregroundStyle(.secondary) }
            }
        } footer: {
            Text("Gemini keeps prompt caches on Google's servers for a while; this removes them now.")
        }
    }
}
