import Foundation
import Observation

/// One chapter's paragraphs as display text, kept current under translation
/// and sync. A fetch in flight when its paragraph changes is superseded by a
/// newer one (per-paragraph epochs), so a late reply never overwrites fresher text.
@MainActor
@Observable
final class ChapterStore {
    let bookId: UUID
    let chapterId: Int
    private(set) var paragraphIds: [Int] = []
    private(set) var paragraphs: [Int: ParagraphText] = [:]
    private(set) var loaded = false
    /// The book's language as a BCP 47 tag, for hyphenation.
    private(set) var language: String?
    var error: String?

    /// Bumped on every content change, with the ids that changed (nil = all).
    private(set) var revision = 0
    private(set) var changedIds: Set<Int>?

    private let core: any CoreService
    private let events: EventHub
    private var tokens: [EventHub.Token] = []
    private var epochs: [Int: Int] = [:]
    private var pending: Set<Int> = []
    private var flushScheduled = false

    static let batchSize = 64

    init(bookId: UUID, chapterId: Int, core: any CoreService, events: EventHub) {
        self.bookId = bookId
        self.chapterId = chapterId
        self.core = core
        self.events = events
        tokens.append(
            events.on("paragraph_updated", as: ParagraphUpdatedEvent.self) { [weak self] e in
                guard let self, e.bookId == bookId else { return }
                self.refetch([e.paragraphId])
            })
        tokens.append(
            events.on("cards_updated") { [weak self] _ in
                guard let self else { return }
                self.refetch(self.paragraphIds)
            })
        tokens.append(
            events.on("book_updated", as: UUID.self) { [weak self] id in
                guard let self, id == bookId else { return }
                Task { await self.reloadIds() }
            })
    }

    func close() {
        events.cancel(tokens)
        tokens = []
    }

    func load() async {
        do {
            paragraphIds = try await core.invoke(
                "get_book_chapter_paragraph_ids", ["bookId": .uuid(bookId), "chapterId": .int(chapterId)])
            let fetched = try await fetch(paragraphIds)
            paragraphs = Dictionary(uniqueKeysWithValues: fetched.map { ($0.id, $0) })
            loaded = true
            changedIds = nil
            revision += 1
        } catch {
            self.error = error.localizedDescription
            loaded = true
        }
    }

    private func reloadIds() async {
        guard let ids: [Int] = try? await core.invoke(
            "get_book_chapter_paragraph_ids", ["bookId": .uuid(bookId), "chapterId": .int(chapterId)])
        else { return }
        // Same ids: text changes arrive as `paragraph_updated`, and the queue
        // emits `book_updated` on every save, so refetching here would storm.
        if ids != paragraphIds {
            await load()
        }
    }

    private func fetch(_ ids: [Int]) async throws -> [ParagraphText] {
        var out: [ParagraphText] = []
        out.reserveCapacity(ids.count)
        var start = 0
        while start < ids.count {
            let slice = Array(ids[start..<min(start + Self.batchSize, ids.count)])
            let batch: ParagraphTexts = try await core.invoke(
                "get_paragraph_texts_batch", ["bookId": .uuid(bookId), "paragraphIds": .ints(slice)])
            if language == nil, !batch.language.isEmpty {
                language = Locale.Language(identifier: batch.language).minimalIdentifier
            }
            out.append(contentsOf: batch.paragraphs)
            start += Self.batchSize
        }
        return out
    }

    /// Coalesces bursts (a chapter translation emits one event per paragraph).
    func refetch(_ ids: [Int]) {
        let mine = Set(paragraphIds)
        let wanted = ids.filter(mine.contains)
        guard !wanted.isEmpty else { return }
        for id in wanted { epochs[id, default: 0] += 1 }
        pending.formUnion(wanted)
        guard !flushScheduled else { return }
        flushScheduled = true
        Task { @MainActor in
            await Task.yield()
            flushScheduled = false
            let ids = pending.sorted()
            pending = []
            let started = Dictionary(uniqueKeysWithValues: ids.map { ($0, epochs[$0, default: 0]) })
            guard let fetched = try? await fetch(ids) else { return }
            var changed = Set<Int>()
            for p in fetched where epochs[p.id, default: 0] == started[p.id] {
                if paragraphs[p.id] != p {
                    paragraphs[p.id] = p
                    changed.insert(p.id)
                }
            }
            guard !changed.isEmpty else { return }
            changedIds = changed
            revision += 1
        }
    }

    func translate(paragraphId: Int, model: String, useCache: Bool = true) async {
        do {
            try await core.call(
                "translate_paragraph",
                [
                    "bookId": .uuid(bookId), "paragraphId": .int(paragraphId), "model": .string(model),
                    "useCache": .bool(useCache),
                ])
        } catch {
            self.error = error.localizedDescription
        }
    }

    var untranslatedCount: Int {
        paragraphIds.reduce(0) { $0 + ((paragraphs[$1]?.translated ?? false) ? 0 : 1) }
    }
}
