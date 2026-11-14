import Foundation
import Observation

/// One open book: its chapters, summary status and saved reading position.
@MainActor
@Observable
final class BookStore {
    let bookId: UUID
    private(set) var chapters: [ChapterMeta] = []
    private(set) var summary: BookSummaryStatus?
    private(set) var readingState: BookReadingState?
    private(set) var loaded = false
    var currentChapter: Int?
    var error: String?

    private let core: any CoreService
    private let events: EventHub
    private var tokens: [EventHub.Token] = []
    private var chaptersReloadScheduled = false

    init(bookId: UUID, core: any CoreService, events: EventHub) {
        self.bookId = bookId
        self.core = core
        self.events = events
        tokens.append(
            events.on { [weak self] event in
                guard let self else { return }
                switch event {
                case .bookUpdated(let id) where id == bookId && !chaptersReloadScheduled:
                    chaptersReloadScheduled = true
                    Task {
                        try? await Task.sleep(for: .milliseconds(300))
                        self.chaptersReloadScheduled = false
                        await self.loadChapters()
                    }
                case .summaryGenerationProgress(let e) where e.bookId == bookId:
                    Task { await self.loadSummary() }
                default: break
                }
            })
    }

    func close() {
        events.cancel(tokens)
        tokens = []
    }

    func load() async {
        await loadChapters()
        readingState = (try? await core.getBookReadingState(bookId: bookId)) ?? nil
        if currentChapter == nil {
            currentChapter = readingState?.chapterId ?? chapters.first?.id
        }
        loaded = true
        await loadSummary()
    }

    func loadChapters() async {
        do {
            chapters = try await core.listBookChapters(bookId: bookId)
        } catch {
            self.error = error.localizedDescription
        }
    }

    func loadSummary() async {
        summary = try? await core.getBookSummaryStatus(bookId: bookId)
    }

    func save(chapterId: Int, paragraphId: Int, pageOffset: Int) {
        let state = BookReadingState(chapterId: chapterId, paragraphId: paragraphId, pageOffset: pageOffset)
        guard state != readingState else { return }
        readingState = state
        Task { [core, bookId] in
            try? await core.saveBookReadingState(bookId: bookId, state: state)
        }
    }

    func translateChapter(_ chapterId: Int, model: String) async {
        do {
            try await core.translateChapter(bookId: bookId, chapterId: chapterId, model: model, useCache: true)
        } catch {
            self.error = error.localizedDescription
        }
    }
}
