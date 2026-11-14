import Foundation
import Observation

/// Active paragraph translations. Progress and finished events are unordered,
/// so only `started` (or a snapshot) creates an entry, and each event touches
/// only its own request.
@MainActor
@Observable
final class TranslationActivityStore: CoreBound {
    struct Key: Hashable {
        let bookId: UUID
        let paragraphId: Int
    }

    private(set) var entries: [Key: TranslationActivity] = [:]
    private(set) var lastFailure: String?
    private var reconcileToken = 0
    private let events: EventHub
    private var core: (any CoreService)?

    init(events: EventHub) {
        self.events = events
    }

    func attach(_ core: any CoreService) {
        self.core = core
        events.on("paragraph_translation_started", as: TranslationStartedEvent.self) { [weak self] e in
            self?.started(e)
        }
        events.on("paragraph_translation_progress", as: TranslationProgressEvent.self) { [weak self] e in
            self?.progress(e)
        }
        events.on("paragraph_translation_finished", as: TranslationFinishedEvent.self) { [weak self] e in
            self?.finished(e)
        }
        events.on("book_deleted", as: UUID.self) { [weak self] id in
            guard let self else { return }
            reconcileToken += 1
            entries = entries.filter { $0.key.bookId != id }
        }
        Task { await reconcile() }
    }

    func started(_ e: TranslationStartedEvent) {
        reconcileToken += 1
        entries[Key(bookId: e.bookId, paragraphId: e.paragraphId)] = TranslationActivity(
            requestId: e.requestId, progressChars: 0, expectedChars: e.expectedChars)
    }

    func progress(_ e: TranslationProgressEvent) {
        let key = Key(bookId: e.bookId, paragraphId: e.paragraphId)
        guard let existing = entries[key], existing.requestId == e.requestId else { return }
        entries[key] = TranslationActivity(
            requestId: e.requestId, progressChars: e.progressChars, expectedChars: e.expectedChars)
    }

    func finished(_ e: TranslationFinishedEvent) {
        if let error = e.error { lastFailure = error }
        reconcileToken += 1
        let key = Key(bookId: e.bookId, paragraphId: e.paragraphId)
        if entries[key]?.requestId == e.requestId {
            entries[key] = nil
        }
    }

    /// Adopts translations whose `started` this process never saw.
    func reconcile() async {
        guard let core else { return }
        reconcileToken += 1
        let token = reconcileToken
        guard let rows: [ActiveTranslationRow] = try? await core.invoke("list_paragraph_translation_activity")
        else { return }
        guard token == reconcileToken else { return }
        var fresh: [Key: TranslationActivity] = [:]
        for row in rows {
            fresh[Key(bookId: row.bookId, paragraphId: row.paragraphId)] = TranslationActivity(
                requestId: row.requestId, progressChars: row.progressChars, expectedChars: row.expectedChars)
        }
        entries = fresh
    }

    func activity(_ bookId: UUID, _ paragraphId: Int) -> TranslationActivity? {
        entries[Key(bookId: bookId, paragraphId: paragraphId)]
    }

    func clearFailure() {
        lastFailure = nil
    }
}
