import AppKit
import SwiftUI

/// Commands the toolbar and menus send to the reader canvas.
@MainActor
@Observable
final class ReaderControls {
    fileprivate weak var canvas: ReaderCanvas?
    var page = 0
    var pageCount = 0
    /// Single-key toggles as in the web reader (W, C), set by the book page.
    var toggleWordPanel: (() -> Void)?
    var toggleChapters: (() -> Void)?
    /// True if the word's details were open.
    var collapseWordPanel: (() -> Bool)?
    /// Idempotent, so two paths closing the list cannot reopen it.
    var hideChapters: (() -> Void)?
    var chaptersShown = false

    func next() { canvas?.nextSpread() }
    func previous() { canvas?.previousSpread() }
    func first() { canvas?.go(toPage: 0) }
    func focus() { canvas?.window?.makeFirstResponder(canvas) }
}

struct ReaderView: NSViewRepresentable {
    let book: BookStore
    let chapter: ChapterStore
    let word: WordStore
    let activity: TranslationActivityStore
    let controls: ReaderControls
    let typography: ReaderTypography
    let model: String
    /// Read by the caller's body so SwiftUI re-runs `updateNSView` on changes.
    let revision: Int
    let activityEntries: [TranslationActivityStore.Key: TranslationActivity]

    func makeCoordinator() -> Coordinator { Coordinator() }

    func makeNSView(context: Context) -> ReaderCanvas {
        let canvas = ReaderCanvas(frame: .zero)
        canvas.delegate = context.coordinator
        controls.canvas = canvas
        return canvas
    }

    func updateNSView(_ canvas: ReaderCanvas, context: Context) {
        let c = context.coordinator
        c.parent = self
        controls.canvas = canvas
        canvas.setTypography(typography)

        if c.chapterKey != ChapterKey(book: book.bookId, chapter: chapter.chapterId) {
            guard chapter.loaded else { return }
            c.chapterKey = ChapterKey(book: book.bookId, chapter: chapter.chapterId)
            c.revision = chapter.revision
            var anchor: ReadingAnchor?
            let saved = book.readingState.flatMap { $0.chapterId == chapter.chapterId ? $0 : nil }
            if let state = saved ?? book.positions[chapter.chapterId] {
                anchor = ReadingAnchor(paragraphId: state.paragraphId, pageOffset: state.pageOffset)
                // The exact line, if this device saved this very position.
                if let local = LocalPosition.load(book.bookId), local.state == state {
                    anchor?.line = local.line
                }
            }
            canvas.revealed = []
            canvas.selected = nil
            canvas.setContent(
                order: chapter.paragraphIds, paragraphs: chapter.paragraphs, typography: typography, anchor: anchor)
            DispatchQueue.main.async { canvas.window?.makeFirstResponder(canvas) }
        } else if c.revision != chapter.revision {
            c.revision = chapter.revision
            if canvas.contentOrder == chapter.paragraphIds {
                // Diff rather than trust `changedIds`: several store revisions
                // can land between two updates of this view.
                let changed = chapter.paragraphIds.compactMap { id -> ParagraphText? in
                    guard let p = chapter.paragraphs[id], p != canvas.contentParagraph(id) else { return nil }
                    return p
                }
                if !changed.isEmpty { canvas.replace(changed) }
            } else {
                let anchor = canvas.currentAnchor()
                canvas.setContent(
                    order: chapter.paragraphIds, paragraphs: chapter.paragraphs, typography: typography,
                    anchor: anchor)
            }
        }
        // Progress rings follow activity.
        canvas.needsDisplay = true
    }

    struct ChapterKey: Equatable {
        let book: UUID
        let chapter: Int
    }

    @MainActor
    final class Coordinator: ReaderCanvasDelegate {
        var parent: ReaderView?
        var chapterKey: ChapterKey?
        var revision = -1
        private var saveTask: Task<Void, Never>?

        func reader(_ canvas: ReaderCanvas, selected paragraphId: Int, wordIndex: Int) {
            guard let parent, let p = canvas.text.paragraphs[paragraphId], wordIndex < p.words.count else { return }
            let w = p.words[wordIndex]
            let surface = (p.text as NSString).substring(with: NSRange(location: w.start, length: w.len))
            parent.word.select(
                WordRef(bookId: parent.book.bookId, paragraphId: paragraphId, sentence: w.sentence, word: w.word, flatIndex: w.flatIndex),
                surface: surface)
        }

        func reader(_ canvas: ReaderCanvas, translate paragraphId: Int, useCache: Bool) {
            // Every way to translate waits for the chapter's summaries; a
            // retranslation (useCache false on a translated paragraph) does not.
            guard let parent else { return }
            let translated = canvas.contentParagraph(paragraphId)?.translated ?? false
            guard translated || parent.book.canTranslate(parent.chapter.chapterId) else { return }
            Task { await parent.chapter.translate(paragraphId: paragraphId, model: parent.model, useCache: useCache) }
        }

        func reader(_ canvas: ReaderCanvas, movedTo page: Int, of count: Int) {
            guard let parent else { return }
            parent.controls.page = page
            parent.controls.pageCount = count
            // A provisional layout knows only its own pages; wait for the full one.
            guard !canvas.isProvisional, let anchor = canvas.currentAnchor() else { return }
            let book = parent.book
            let chapterId = parent.chapter.chapterId
            saveTask?.cancel()
            saveTask = Task { @MainActor in
                try? await Task.sleep(for: .milliseconds(400))
                guard !Task.isCancelled else { return }
                let state = BookReadingState(
                    chapterId: chapterId, paragraphId: anchor.paragraphId, pageOffset: anchor.pageOffset)
                if let line = anchor.line { LocalPosition(state: state, line: line).save(book.bookId) }
                book.save(chapterId: chapterId, paragraphId: anchor.paragraphId, pageOffset: anchor.pageOffset)
            }
        }

        func readerCanTranslate(_ canvas: ReaderCanvas) -> Bool {
            guard let parent else { return true }
            return parent.book.canTranslate(parent.chapter.chapterId)
        }

        func readerDeselected(_ canvas: ReaderCanvas) {
            parent?.word.select(nil, surface: nil)
        }

        func readerToggle(_ canvas: ReaderCanvas, key: String) -> Bool {
            switch key {
            case "w": parent?.controls.toggleWordPanel?()
            case "c": parent?.controls.toggleChapters?()
            case "\u{1b}":
                // Escape backs out one step: the chapter list, the word's details, the word.
                guard let parent else { return false }
                if parent.controls.chaptersShown {
                    parent.controls.hideChapters?()
                } else if parent.controls.collapseWordPanel?() == true {
                } else if parent.word.selection != nil {
                    canvas.selected = nil
                    parent.word.select(nil, surface: nil)
                } else {
                    return false
                }
            default: return false
            }
            return true
        }

        func readerTranslationProgress(_ canvas: ReaderCanvas, paragraphId: Int) -> Double? {
            guard let parent, let a = parent.activity.activity(parent.book.bookId, paragraphId) else { return nil }
            return a.expectedChars > 0 ? Double(a.progressChars) / Double(a.expectedChars) : 0
        }
    }
}

/// The exact top line of the last position this device saved, next to the
/// synced (paragraph, pageOffset), which cannot express a line.
struct LocalPosition: Codable {
    let chapterId: Int
    let anchorParagraph: Int
    let pageOffset: Int
    let paragraphId: Int
    let start: Int

    init(state: BookReadingState, line: LineAnchor) {
        chapterId = state.chapterId
        anchorParagraph = state.paragraphId
        pageOffset = state.pageOffset
        paragraphId = line.paragraphId
        start = line.start
    }

    var state: BookReadingState {
        BookReadingState(chapterId: chapterId, paragraphId: anchorParagraph, pageOffset: pageOffset)
    }

    var line: LineAnchor { LineAnchor(paragraphId: paragraphId, start: start) }

    static func load(_ book: UUID) -> LocalPosition? {
        AppDefaults.store.data(forKey: "position." + book.uuidString)
            .flatMap { try? JSONDecoder().decode(LocalPosition.self, from: $0) }
    }

    func save(_ book: UUID) {
        if let data = try? JSONEncoder().encode(self) {
            AppDefaults.store.set(data, forKey: "position." + book.uuidString)
        }
    }
}
