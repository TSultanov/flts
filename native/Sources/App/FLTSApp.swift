#if os(macOS)
import AppKit
import SwiftUI

@main
struct FLTSApp: App {
    @NSApplicationDelegateAdaptor(AppDelegate.self) private var delegate

    var body: some Scene {
        // One window, so it is listed in the Window menu and can be reopened
        // after closing it while Lyrics stays open.
        Window("FLTS", id: "main") {
            RootView()
                .environment(delegate.model)
                .frame(minWidth: 720, minHeight: 480)
                .transaction { if AppDefaults.animationsOff { $0.disablesAnimations = true } }
        }
        .defaultSize(width: 1180, height: 800)
        .commands { AppCommands(model: delegate.model) }

        Window("Lyrics", id: "lyrics") {
            LyricsWindow()
                .environment(delegate.model)
                .frame(minWidth: 420, minHeight: 360)
        }
        .defaultSize(width: 520, height: 680)
        .keyboardShortcut("l", modifiers: [.command, .shift])

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
        pinWindowSizeForTests()
    }

    /// UI tests set `FLTS_WINDOW_SIZE=WxH` (with a test config dir) for a known
    /// window size. A frame passed as a launch argument cannot be used: the
    /// saved frame then never changes, and AppKit dies re-laying out the toolbar.
    private func pinWindowSizeForTests() {
        let env = ProcessInfo.processInfo.environment
        let parts = (env["FLTS_WINDOW_SIZE"] ?? "").split(separator: "x").compactMap { Double($0) }
        guard parts.count == 2, !(env["FLTS_CONFIG_DIR"] ?? "").isEmpty else { return }
        testWindowSize = CGSize(width: parts[0], height: parts[1])
        NotificationCenter.default.addObserver(
            self, selector: #selector(pinWindowSize(_:)), name: NSWindow.didBecomeMainNotification, object: nil)
    }

    private var testWindowSize: CGSize?

    @objc private func pinWindowSize(_ note: Notification) {
        guard let window = note.object as? NSWindow, let wanted = testWindowSize else { return }
        NotificationCenter.default.removeObserver(self, name: NSWindow.didBecomeMainNotification, object: nil)
        let size = CGSize(
            width: max(wanted.width, window.minSize.width), height: max(wanted.height, window.minSize.height))
        let top = window.screen?.visibleFrame.maxY ?? window.frame.maxY
        window.setFrame(CGRect(x: 100, y: top - 60 - size.height, width: size.width, height: size.height), display: true)
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
    @FocusedValue(\.readerControls) private var reader
    @FocusedValue(\.bookActions) private var book

    var body: some Commands {
        CommandGroup(replacing: .newItem) {
            Button("Import Book…") { model.showImport = true }
                .keyboardShortcut("o")
                // Not during onboarding, where nothing can present the sheet.
                .disabled(!model.config.isConfigured)
            Divider()
            Button("Reveal Library in Finder") { Platform.revealLibrary(model.config.libraryRoot) }
        }
        CommandGroup(after: .sidebar) {
            Button("Larger Text") { ReaderPreferences.shared.fontSize += 1 }
                .keyboardShortcut("+")
            Button("Smaller Text") { ReaderPreferences.shared.fontSize -= 1 }
                .keyboardShortcut("-")
            Button("Actual Size") { ReaderPreferences.shared.fontSize = ReaderPreferences.defaultFontSize }
                .keyboardShortcut("0")
            Divider()
            Button("Wider Margins") { ReaderPreferences.shared.marginScale += ReaderPreferences.marginStep }
            Button("Narrower Margins") { ReaderPreferences.shared.marginScale -= ReaderPreferences.marginStep }
            Button("Default Margins") { ReaderPreferences.shared.marginScale = 1 }
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
            Button(book?.chaptersShown == true ? "Hide Chapters" : "Show Chapters") { book?.toggleChapters() }
                .keyboardShortcut("c", modifiers: [.command, .control])
                .disabled(book?.hasChapters != true)
            Button(book?.wordDetailsShown == true ? "Hide Word Details" : "Show Word Details") { book?.toggleWordPanel() }
                .keyboardShortcut("w", modifiers: [.command, .control])
                .disabled(book == nil)
            Divider()
            Button("Library") { model.selectedBookId = nil }
                .keyboardShortcut("l", modifiers: [.command, .option])
                .disabled(book == nil)
        }
    }
}
#endif
