import Foundation
import Observation

/// Owns the core and the app-wide stores.
@MainActor
@Observable
final class AppModel {
    enum Phase {
        case starting
        case running
        case failed(String)
    }

    private(set) var phase: Phase = .starting
    let events = EventHub()
    private(set) var core: (any CoreService)?

    let config: ConfigStore
    let library: LibraryStore
    let activity: TranslationActivityStore
    let sync: SyncStore
    let anki: AnkiStore
    let lyrics: LyricsStore

    /// The book open in the main window. Launch opens the library, as the web app does.
    var selectedBookId: UUID?
    var showImport = false
    /// iOS presents Settings as a sheet (macOS has its Settings window).
    var showSettings = false
    /// The open reader, for app-level keyboard commands on iOS: focus-based
    /// lookup finds nothing there until something has taken focus.
    var activeReader: ReaderControls?
    var activeBook: BookActions?
    var lastError: String?
    /// Set when the open book was deleted (often on another device).
    var deletedBookNotice: String?

    init() {
        config = ConfigStore(events: events)
        library = LibraryStore(events: events)
        activity = TranslationActivityStore(events: events)
        sync = SyncStore(events: events)
        anki = AnkiStore(events: events)
        lyrics = LyricsStore(events: events)
    }

    /// For tests: stores over a fake core.
    convenience init(core: any CoreService) {
        self.init()
        attach(core)
    }

    func start() {
        guard case .starting = phase, core == nil else { return }
        do {
            attach(try CoreClient.start(events: events))
        } catch {
            phase = .failed(error.userMessage)
        }
    }

    private func attach(_ core: any CoreService) {
        self.core = core
        for store in [config, library, activity, sync, anki, lyrics] as [any CoreBound] {
            store.attach(core)
        }
        events.on { [weak self] event in
            guard let self, case .bookDeleted(let id) = event, selectedBookId == id else { return }
            let title = library.book(id)?.title
            deletedBookNotice = title.map { "“\($0)” was deleted, possibly on another device." }
                ?? "The book you were reading was deleted, possibly on another device."
            selectedBookId = nil
        }
        phase = .running
    }

    /// Bounded so a stuck backend cannot hold the app open.
    /// Stops the core, but never keeps the app from quitting for long: a sync
    /// engine that is slow to stop is left behind after the limit.
    func shutdown(limit: Duration = .seconds(8)) async {
        guard let core else { return }
        // Not a task group: that would still wait for the core at its end.
        let (finished, signal) = AsyncStream<Void>.makeStream()
        Task {
            await core.shutdown()
            signal.yield()
        }
        Task {
            try? await Task.sleep(for: limit)
            signal.yield()
        }
        for await _ in finished { break }
    }

    func report(_ error: Error) {
        lastError = error.userMessage
        log.error("\(error.userMessage, privacy: .public)")
    }
}

/// A store that starts loading once the core exists.
@MainActor
protocol CoreBound: AnyObject {
    func attach(_ core: any CoreService)
}
