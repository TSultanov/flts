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
    var lastError: String?

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
            phase = .failed(error.localizedDescription)
        }
    }

    private func attach(_ core: any CoreService) {
        self.core = core
        for store in [config, library, activity, sync, anki, lyrics] as [any CoreBound] {
            store.attach(core)
        }
        events.on { [weak self] event in
            if case .bookDeleted(let id) = event, self?.selectedBookId == id { self?.selectedBookId = nil }
        }
        phase = .running
    }

    /// Bounded so a stuck backend cannot hold the app open.
    func shutdown() async {
        guard let core else { return }
        await withTaskGroup(of: Void.self) { group in
            group.addTask { await core.shutdown() }
            group.addTask { try? await Task.sleep(for: .seconds(12)) }
            await group.next()
            group.cancelAll()
        }
    }

    func report(_ error: Error) {
        lastError = error.localizedDescription
        log.error("\(error.localizedDescription, privacy: .public)")
    }
}

/// A store that starts loading once the core exists.
@MainActor
protocol CoreBound: AnyObject {
    func attach(_ core: any CoreService)
}
