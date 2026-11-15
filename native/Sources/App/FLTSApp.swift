import AppKit
import SwiftUI

@main
struct FLTSApp: App {
    @NSApplicationDelegateAdaptor(AppDelegate.self) private var delegate
    @Environment(\.openWindow) private var openWindow

    var body: some Scene {
        WindowGroup("FLTS", id: "main") {
            RootView()
                .environment(delegate.model)
                .frame(minWidth: 720, minHeight: 480)
        }
        .defaultSize(width: 1180, height: 800)
        .commands { AppCommands(model: delegate.model) }

        Window("Lyrics", id: "lyrics") {
            LyricsWindow()
                .environment(delegate.model)
                .frame(minWidth: 420, minHeight: 360)
        }
        .defaultSize(width: 520, height: 680)

        Settings {
            SettingsView()
                .environment(delegate.model)
        }
    }
}

@MainActor
final class AppDelegate: NSObject, NSApplicationDelegate {
    let model = AppModel()

    func applicationDidFinishLaunching(_ notification: Notification) {
        Appearance.apply()
        // Unit tests run inside the app; they must not open a real library.
        guard ProcessInfo.processInfo.environment["XCTestConfigurationFilePath"] == nil else { return }
        model.start()
    }

    func applicationDidBecomeActive(_ notification: Notification) {
        model.sync.wake()
        Task { await model.activity.reconcile() }
    }

    func applicationShouldTerminate(_ sender: NSApplication) -> NSApplication.TerminateReply {
        Task { @MainActor in
            await model.shutdown()
            sender.reply(toApplicationShouldTerminate: true)
        }
        return .terminateLater
    }

    func applicationShouldTerminateAfterLastWindowClosed(_ sender: NSApplication) -> Bool {
        true
    }
}

struct AppCommands: Commands {
    let model: AppModel
    @Environment(\.openWindow) private var openWindow
    @FocusedValue(\.readerControls) private var reader
    @FocusedValue(\.bookActions) private var book

    var body: some Commands {
        CommandGroup(replacing: .newItem) {
            Button("Import Book…") { model.showImport = true }
                .keyboardShortcut("o")
        }
        CommandGroup(after: .sidebar) {
            Button("Larger Text") { ReaderPreferences.shared.fontSize += 1 }
                .keyboardShortcut("+")
            Button("Smaller Text") { ReaderPreferences.shared.fontSize -= 1 }
                .keyboardShortcut("-")
            Button("Actual Size") { ReaderPreferences.shared.fontSize = ReaderPreferences.defaultFontSize }
                .keyboardShortcut("0")
            Divider()
        }
        CommandMenu("Book") {
            Button("Next Page") { reader?.next() }
                .keyboardShortcut(.rightArrow, modifiers: [.command])
                .disabled(reader == nil)
            Button("Previous Page") { reader?.previous() }
                .keyboardShortcut(.leftArrow, modifiers: [.command])
                .disabled(reader == nil)
            Divider()
            Button("Translate Chapter") { book?.translateChapter() }
                .keyboardShortcut("t", modifiers: [.command, .shift])
                .disabled(book == nil)
            Button("Show Chapters") { book?.toggleChapters() }
                .keyboardShortcut("c", modifiers: [.command, .control])
                .disabled(book == nil)
            Button("Show Word Details") { book?.toggleWordPanel() }
                .keyboardShortcut("w", modifiers: [.command, .control])
                .disabled(book == nil)
            Divider()
            Button("Library") { model.selectedBookId = nil }
                .keyboardShortcut("l", modifiers: [.command, .option])
                .disabled(book == nil)
        }
        CommandGroup(after: .windowList) {
            Button("Lyrics") { openWindow(id: "lyrics") }
                .keyboardShortcut("l", modifiers: [.command, .shift])
        }
        CommandGroup(after: .help) {
            Button("Reveal Library in Finder") { Platform.revealLibrary(model.config.libraryRoot) }
        }
    }
}

struct BookActions {
    let translateChapter: () -> Void
    let toggleWordPanel: () -> Void
    let toggleChapters: () -> Void
}

extension FocusedValues {
    @Entry var readerControls: ReaderControls?
    @Entry var bookActions: BookActions?
}

/// Reader preferences shared by the menu and the reader.
@MainActor
@Observable
final class ReaderPreferences {
    static let shared = ReaderPreferences()
    static let defaultFontSize: CGFloat = 18

    var fontSize: CGFloat {
        didSet {
            fontSize = min(max(fontSize, 12), 36)
            AppDefaults.store.set(Double(fontSize), forKey: "readerFontSize")
        }
    }

    private init() {
        let saved = AppDefaults.store.double(forKey: "readerFontSize")
        fontSize = saved > 0 ? CGFloat(saved) : Self.defaultFontSize
    }

    func typography(language: String? = nil) -> ReaderTypography {
        var t = ReaderTypography()
        t.fontSize = fontSize
        t.language = language
        return t
    }
}

/// Light, dark, or the system's choice, under Reading in Settings.
enum Appearance: String, CaseIterable, Identifiable {
    case system, light, dark
    var id: Self { self }

    static let key = "appearance"

    var title: String {
        switch self {
        case .system: "Match System"
        case .light: "Light"
        case .dark: "Dark"
        }
    }

    @MainActor
    static func apply() {
        let raw = AppDefaults.store.string(forKey: key) ?? Appearance.system.rawValue
        switch Appearance(rawValue: raw) ?? .system {
        case .system: NSApp.appearance = nil
        case .light: NSApp.appearance = NSAppearance(named: .aqua)
        case .dark: NSApp.appearance = NSAppearance(named: .darkAqua)
        }
    }
}
