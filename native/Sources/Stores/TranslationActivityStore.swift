import Foundation
import Observation

/// A paragraph's translation in flight.
struct TranslationActivity: Equatable, Sendable {
    var requestId: Int
    var progressChars: Int
    var expectedChars: Int
}

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
        events.on { [weak self] event in
            guard let self else { return }
            switch event {
            case .translationStarted(let e): started(e)
            case .translationProgress(let e): progress(e)
            case .translationFinished(let e): finished(e)
            case .bookDeleted(let id):
                reconcileToken += 1
                entries = entries.filter { $0.key.bookId != id }
            default: break
            }
        }
        Task { await reconcile() }
    }

    func started(_ e: TranslationStarted) {
        reconcileToken += 1
        entries[Key(bookId: e.bookId, paragraphId: e.paragraphId)] = TranslationActivity(
            requestId: e.requestId, progressChars: 0, expectedChars: e.expectedChars)
    }

    func progress(_ e: TranslationProgress) {
        let key = Key(bookId: e.bookId, paragraphId: e.paragraphId)
        guard let existing = entries[key], existing.requestId == e.requestId else { return }
        entries[key] = TranslationActivity(
            requestId: e.requestId, progressChars: e.progressChars, expectedChars: e.expectedChars)
    }

    func finished(_ e: TranslationFinished) {
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
        guard let rows = try? await core.listTranslationActivity() else { return }
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
