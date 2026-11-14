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
            events.on("book_updated", as: UUID.self) { [weak self] id in
                guard let self, id == bookId, !self.chaptersReloadScheduled else { return }
                self.chaptersReloadScheduled = true
                Task {
                    try? await Task.sleep(for: .milliseconds(300))
                    self.chaptersReloadScheduled = false
                    await self.loadChapters()
                }
            })
        tokens.append(
            events.on("summary_generation_progress", as: SummaryProgressEvent.self) { [weak self] e in
                guard let self, e.bookId == bookId else { return }
                Task { await self.loadSummary() }
            })
    }

    func close() {
        events.cancel(tokens)
        tokens = []
    }

    func load() async {
        await loadChapters()
        readingState = (try? await core.invoke(
            "get_book_reading_state", ["bookId": .uuid(bookId)], as: BookReadingState?.self)) ?? nil
        if currentChapter == nil {
            currentChapter = readingState?.chapterId ?? chapters.first?.id
        }
        loaded = true
        await loadSummary()
    }

    func loadChapters() async {
        do {
            chapters = try await core.invoke("list_book_chapters", ["bookId": .uuid(bookId)])
        } catch {
            self.error = error.localizedDescription
        }
    }

    func loadSummary() async {
        summary = try? await core.invoke("get_book_summary_status", ["bookId": .uuid(bookId)])
    }

    func save(chapterId: Int, paragraphId: Int, pageOffset: Int) {
        let state = BookReadingState(chapterId: chapterId, paragraphId: paragraphId, pageOffset: pageOffset)
        guard state != readingState else { return }
        readingState = state
        Task { [core, bookId] in
            try? await core.call(
                "save_book_reading_state",
                [
                    "bookId": .uuid(bookId), "chapterId": .int(chapterId), "paragraphId": .int(paragraphId),
                    "pageOffset": .int(pageOffset),
                ])
        }
    }

    func translateChapter(_ chapterId: Int, model: String) async {
        do {
            try await core.call(
                "translate_chapter",
                ["bookId": .uuid(bookId), "chapterId": .int(chapterId), "model": .string(model), "useCache": true])
        } catch {
            self.error = error.localizedDescription
        }
    }
}
