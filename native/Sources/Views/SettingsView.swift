import SwiftUI

enum SettingsTab: String {
    case translation, reading, sync, anki, spotify, storage
}

struct SettingsView: View {
    /// Shared with the toolbar's sync button, which opens the Sync tab.
    @AppStorage("settingsTab", store: AppDefaults.store) private var tab = SettingsTab.translation
    @Environment(AppModel.self) private var model

    var body: some View {
        TabView(selection: $tab) {
            Form { TranslationSettings() }
                .formStyle(.grouped)
                .tabItem { Label("Translation", systemImage: "character.bubble") }
                .tag(SettingsTab.translation)
            Form { ReadingSettings() }
                .formStyle(.grouped)
                .tabItem { Label("Reading", systemImage: "textformat.size") }
                .tag(SettingsTab.reading)
            SyncSettings()
                .tabItem { Label("Sync", systemImage: "arrow.triangle.2.circlepath") }
                .tag(SettingsTab.sync)
            Form { AnkiSettings() }
                .formStyle(.grouped)
                .tabItem { Label("Anki", systemImage: "rectangle.stack") }
                .tag(SettingsTab.anki)
            Form { SpotifySettings() }
                .formStyle(.grouped)
                .tabItem { Label("Spotify", systemImage: "music.note") }
                .tag(SettingsTab.spotify)
            Form { StorageSettings() }
                .formStyle(.grouped)
                .tabItem { Label("Storage", systemImage: "internaldrive") }
                .tag(SettingsTab.storage)
        }
        .frame(width: 580, height: 560)
        // Saves happen on every tab; a failure must show wherever it happened.
        .overlay(alignment: .bottom) {
            if let error = model.config.error {
                ErrorBanner(message: "Could not save: \(error)") { model.config.error = nil }
            }
        }
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
            if config.translationProvider == .google {
                Section {
                    PurgeCachesRow(hasKey: !(config.geminiApiKey ?? "").isEmpty)
                } footer: {
                    Text("Gemini keeps prompt caches on Google's servers for a while; this removes them now.")
                }
            }
        } else {
            ProgressView()
        }
    }
}

private struct PurgeCachesRow: View {
    let hasKey: Bool
    @Environment(AppModel.self) private var model
    @State private var busy = false
    @State private var result: String?

    var body: some View {
        HStack {
            Button(busy ? "Removing…" : "Remove Server Caches") {
                busy = true
                Task {
                    do {
                        let n = try await model.config.purgeGeminiCaches()
                        result = n == 1 ? "Removed 1 cached item." : "Removed \(n) cached items."
                    } catch {
                        result = error.userMessage
                    }
                    busy = false
                }
            }
            .disabled(busy || !hasKey)
            if let result { Text(result).foregroundStyle(.secondary) }
        }
    }
}

/// OpenRouter (and providers that ask for it) group models by family: the
/// id prefix before "/".
enum ModelFamilies {
    static func family(of id: String) -> String {
        let normalized = id.hasPrefix("~") ? String(id.dropFirst()) : id
        guard let slash = normalized.firstIndex(of: "/"), slash != normalized.startIndex else { return "other" }
        return String(normalized[..<slash])
    }

    static func label(_ family: String) -> String {
        family == "other"
            ? "Other" : family.split(separator: "-").map { $0.prefix(1).uppercased() + $0.dropFirst() }.joined(separator: " ")
    }

    static func families(_ models: [ModelInfo]) -> [String] {
        Set(models.map { family(of: $0.id) }).sorted { label($0).localizedCompare(label($1)) == .orderedAscending }
    }
}

private struct ModelPicker: View {
    let config: AppConfig
    @Environment(AppModel.self) private var model
    @State private var family = ""

    var body: some View {
        let store = model.config
        let models = store.models(for: config.translationProvider)
        let byFamily = config.translationProvider == .openrouter
            || store.provider(config.translationProvider)?.modelSelection == "family"
        let orphan = !models.isEmpty && !models.contains { $0.id == config.model }
        if byFamily && !models.isEmpty {
            let families = ModelFamilies.families(models)
            Picker(
                "Family",
                selection: Binding(
                    get: { families.contains(family) ? family : ModelFamilies.family(of: config.model) },
                    set: { picked in
                        family = picked
                        if let first = models.first(where: { ModelFamilies.family(of: $0.id) == picked }) {
                            Task { await store.update { $0.model = first.id } }
                        }
                    })
            ) {
                ForEach(families, id: \.self) { Text(ModelFamilies.label($0)).tag($0) }
            }
        }
        HStack {
            Picker(
                "Model",
                selection: Binding(
                    get: { config.model },
                    set: { id in Task { await store.update { $0.model = id } } })
            ) {
                if orphan {
                    Text(config.model).tag(config.model)
                }
                let shown = byFamily
                    ? models.filter { ModelFamilies.family(of: $0.id) == ModelFamilies.family(of: config.model) }
                    : models
                ForEach(shown) { Text($0.name).tag($0.id) }
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
        if orphan {
            Text("Not in the current catalog")
                .font(.caption)
                .foregroundStyle(.orange)
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
                Text("Cards sync to Anki desktop through the [AnkiConnect](https://ankiweb.net/shared/info/2055492159) add-on, every 5 minutes while FLTS runs.")
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
    @State private var busy: String?
    @State private var copied = false

    private static let dashboard = URL(string: "https://developer.spotify.com/dashboard")!
    private static let redirectURI = "http://127.0.0.1:53682/callback"

    var body: some View {
        let store = model.config
        let lyrics = model.lyrics
        if let config = store.config {
            Section {
                VStack(alignment: .leading, spacing: 6) {
                    Text("Enables “Up next” and silent preload of lyrics and translation for upcoming tracks while a playlist or album plays.")
                    Text("Create a Spotify Developer app at [developer.spotify.com/dashboard](\(Self.dashboard.absoluteString)), add this redirect URI, and paste the app's client ID below. Premium is required for queue access.")
                    HStack {
                        Text(Self.redirectURI).font(.system(.callout, design: .monospaced)).textSelection(.enabled)
                        Button {
                            Platform.copy(Self.redirectURI)
                            copied = true
                        } label: {
                            Image(systemName: copied ? "checkmark" : "doc.on.doc")
                        }
                        .buttonStyle(.borderless)
                        .help("Copy the redirect URI")
                    }
                }
                .font(.callout)
                .foregroundStyle(.secondary)
                CommitField(title: "Client ID", value: config.spotifyClientId ?? "") { v in
                    Task { await store.update { $0.spotifyClientId = v.isEmpty ? nil : v } }
                }
                HStack {
                    if lyrics.webStatus?.connected == true {
                        Label("Connected", systemImage: "checkmark.circle").foregroundStyle(.green)
                        Spacer()
                        Button(busy == "web" ? "…" : "Disconnect") { run("web") { await lyrics.disconnectWeb() } }
                    } else {
                        Text((config.spotifyClientId ?? "").isEmpty ? "Set your Spotify client ID first" : "Not connected")
                            .foregroundStyle(.secondary)
                        Spacer()
                        Button(busy == "web" ? "Connecting…" : "Connect…") {
                            run("web") { await lyrics.connectWeb(clientId: config.spotifyClientId ?? "") }
                        }
                        .disabled((config.spotifyClientId ?? "").isEmpty)
                    }
                }
                .disabled(busy != nil)
                if lyrics.webStatus?.premiumRequired == true {
                    Label("Spotify Premium is required for the queue.", systemImage: "exclamationmark.triangle")
                        .foregroundStyle(.orange)
                }
            } header: {
                Text("Spotify Web API")
            }
            Section("Lyrics") {
                Stepper(
                    "Preload \(config.spotifyPreloadCount) upcoming tracks",
                    value: Binding(
                        get: { config.spotifyPreloadCount },
                        set: { n in Task { await store.update { $0.spotifyPreloadCount = n } } }), in: 0...3)
                Toggle(
                    "Show the next track",
                    isOn: Binding(
                        get: { config.spotifyShowNextTrack },
                        set: { on in Task { await store.update { $0.spotifyShowNextTrack = on } } }))
            }
            Section {
                HStack {
                    if let cdp = lyrics.cdpStatus, cdp.available {
                        Label("Spotify lyrics bridge is active (port \(cdp.port))", systemImage: "checkmark.circle")
                            .foregroundStyle(.green)
                    } else {
                        Text(lyrics.cdpStatus?.hint ?? "Spotify lyrics bridge is off").foregroundStyle(.secondary)
                        Spacer()
                        Button(busy == "cdp" ? "Restarting Spotify…" : "Enable First-Party Lyrics") {
                            run("cdp") { await lyrics.restartSpotifyWithDevTools() }
                        }
                        .disabled(busy != nil)
                    }
                }
                HStack {
                    Text(lyrics.loginAgent?.installed == true ? "Spotify starts with the bridge at login" : "Start Spotify with the bridge at login")
                    Spacer()
                    Button(busy == "agent" ? "…" : (lyrics.loginAgent?.installed == true ? "Disable" : "Enable")) {
                        let installed = lyrics.loginAgent?.installed == true
                        run("agent") { installed ? await lyrics.removeLoginAgent() : await lyrics.installLoginAgent() }
                    }
                    .disabled(busy != nil)
                }
                Text(lyrics.loginAgent?.installed == true
                    ? "Turn off Spotify's own “Open automatically after you log in” setting so the two don't race."
                    : "Installs a login item that starts Spotify with the bridge every session, so you don't have to restart it.")
                    .font(.caption)
                    .foregroundStyle(.secondary)
            } header: {
                Text("Spotify Lyrics Fallback")
            } footer: {
                Text("When LRCLIB has no lyrics, FLTS can read Spotify's own through the desktop app.")
            }
            if let error = lyrics.error {
                Text("Last error: \(error)").foregroundStyle(.red).font(.callout)
            }
        }
        Color.clear.frame(height: 0)
            .task { await lyrics.refreshSpotifyStatus() }
    }

    private func run(_ key: String, _ action: @escaping () async -> Void) {
        busy = key
        Task {
            await action()
            busy = nil
        }
    }
}

struct StorageSettings: View {
    @Environment(AppModel.self) private var model

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
    }
}
