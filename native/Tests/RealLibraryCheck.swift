import Foundation
import Testing

@testable import FLTS

/// Opens a library through the app's own bindings and dumps what they report,
/// to compare with the Tauri app's view of the same data. Off unless
/// `FLTS_TEST_REAL_DUMP` names an output file; `FLTS_CONFIG_DIR` picks the
/// library (use a copy).
@MainActor
struct RealLibraryCheck {
    @Test(.enabled(if: ProcessInfo.processInfo.environment["FLTS_TEST_REAL_DUMP"]?.isEmpty == false))
    func dump() async throws {
        let env = ProcessInfo.processInfo.environment
        try #require(env["FLTS_CONFIG_DIR"]?.isEmpty == false, "refusing to open the default library")
        let core = try CoreClient.start(events: EventHub())
        var books: [BookMeta]?
        for _ in 0..<240 where books == nil {
            books = try? await core.listBooks()
            if books == nil { try await Task.sleep(for: .milliseconds(250)) }
        }
        var out: [[String: Any]] = []
        for book in try #require(books, "the library never opened") {
            let chapters = try await core.listBookChapters(bookId: book.id)
            var firstIds: [Int] = []
            var texts = 0
            if let first = chapters.first {
                firstIds = try await core.getBookChapterParagraphIds(bookId: book.id, chapterId: first.id)
                // The reader's own query, so real paragraphs go through the typed decoding.
                texts = try await core.getParagraphTexts(bookId: book.id, paragraphIds: Array(firstIds.prefix(200))).paragraphs.count
            }
            #expect(texts == min(firstIds.count, 200), "paragraph texts missing")
            #expect(chapters.count == book.chaptersCount, "chapter count differs from the book's own")
            out.append([
                "id": book.id.uuidString.lowercased(), "title": book.title, "path": book.path,
                "chapters": chapters.count, "ratio": Int((book.translationRatio * 1000).rounded()),
                "firstChapterParagraphs": firstIds.count,
            ])
        }
        // A sample of raw paragraph text, escaped, for looking at odd rendering.
        if let sample = env["FLTS_TEST_REAL_SAMPLE"], !sample.isEmpty, let match = env["FLTS_TEST_REAL_SAMPLE_BOOK"],
            let book = books?.first(where: { $0.title.contains(match) })
        {
            var lines: [String] = []
            for chapter in try await core.listBookChapters(bookId: book.id).suffix(8).prefix(3) {
                let ids = try await core.getBookChapterParagraphIds(bookId: book.id, chapterId: chapter.id)
                for p in try await core.getParagraphTexts(bookId: book.id, paragraphIds: Array(ids.prefix(12))).paragraphs {
                    lines.append("\(chapter.id) \(p.id) translated=\(p.translated) " + p.text.debugDescription)
                }
            }
            try lines.joined(separator: "\n").write(toFile: sample, atomically: true, encoding: .utf8)
        }
        out.sort { ($0["id"] as! String) < ($1["id"] as! String) }
        let data = try JSONSerialization.data(withJSONObject: out, options: [.sortedKeys])
        try data.write(to: URL(fileURLWithPath: env["FLTS_TEST_REAL_DUMP"]!))
        await core.shutdown()
    }
}
