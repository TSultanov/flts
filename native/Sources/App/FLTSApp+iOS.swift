#if os(iOS)
import SwiftUI
import UIKit

@main
struct FLTSApp: App {
    @State private var model = AppModel()
    @Environment(\.scenePhase) private var phase
    @AppStorage(Appearance.key, store: AppDefaults.store) private var appearance = Appearance.system

    var body: some Scene {
        WindowGroup {
            // Titles and toolbars need a navigation container on iOS.
            NavigationStack {
                RootView()
            }
            .environment(model)
            .preferredColorScheme(appearance.colorScheme)
            .transaction { if AppDefaults.animationsOff { $0.disablesAnimations = true } }
            .task {
                // Unit tests run inside the app; they must not open a real library.
                guard ProcessInfo.processInfo.environment["XCTestConfigurationFilePath"] == nil else { return }
                await model.startAsync()
            }
            .onChange(of: phase) { _, phase in
                if phase == .background {
                    model.activeReader?.flushSave()
                }
                guard phase == .active else { return }
                // Back from the background: sync catches up, stale progress is dropped.
                model.sync.wake()
                Task { await model.activity.reconcile() }
            }
        }
        .commands { ReaderCommands(model: model) }
    }
}

/// The ⌘ shortcuts of a hardware keyboard (and the iPad's menu bar).
private struct ReaderCommands: Commands {
    let model: AppModel
    private var reader: ReaderControls? { model.activeReader }
    private var book: BookActions? { model.activeBook }
    private let prefs = ReaderPreferences.shared

    var body: some Commands {
        CommandMenu("Book") {
            Button("Next Page") { reader?.next() }
                .keyboardShortcut(.rightArrow, modifiers: [.command])
                .disabled(reader == nil)
            Button("Previous Page") { reader?.previous() }
                .keyboardShortcut(.leftArrow, modifiers: [.command])
                .disabled(reader == nil)
            Divider()
            Group {
                Button("Translate Chapter") { book?.translateChapter() }.keyboardShortcut("t", modifiers: [.command, .shift])
                Button(book?.wordDetailsShown == true ? "Hide Word Details" : "Show Word Details") { book?.toggleWordPanel() }
                    .keyboardShortcut("w", modifiers: [.command, .control])
                Button(book?.chaptersShown == true ? "Hide Chapters" : "Show Chapters") { book?.toggleChapters() }
                    .keyboardShortcut("c", modifiers: [.command, .control])
                    .disabled(book?.hasChapters != true)
                Button("Library") { model.selectedBookId = nil }.keyboardShortcut("l", modifiers: [.command, .option])
            }
            .disabled(book == nil)
            Divider()
            Button("Larger Text") { prefs.fontSize += 1 }.keyboardShortcut("+")
            Button("Smaller Text") { prefs.fontSize -= 1 }.keyboardShortcut("-")
            Button("Default Text Size") { prefs.fontSize = ReaderPreferences.defaultFontSize }.keyboardShortcut("0")
            Button("Wider Margins") { prefs.marginScale += ReaderPreferences.marginStep }
            Button("Narrower Margins") { prefs.marginScale -= ReaderPreferences.marginStep }
            Button("Default Margins") { prefs.marginScale = 1 }
            Divider()
            Button("Import Book…") { model.showImport = true }
                .keyboardShortcut("o")
                .disabled(!model.config.isConfigured || model.selectedBookId != nil)
            Button("Settings…") { model.showSettings = true }
                .keyboardShortcut(",")
                .disabled(!model.config.isConfigured || model.selectedBookId != nil)
        }
    }
}
#endif
