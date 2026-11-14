import AppKit
import Foundation
import SwiftUI
import Testing

@testable import FLTS

/// Records calls and replies with canned JSON.
final class FakeCore: CoreService, @unchecked Sendable {
    var replies: [String: String] = [:]
    private(set) var calls: [(String, [String: JSONValue])] = []
    private let lock = NSLock()

    func invokeRaw(_ cmd: String, _ args: [String: JSONValue]) async throws -> Data {
        lock.withLock { calls.append((cmd, args)) }
        guard let reply = lock.withLock({ replies[cmd] }) else { throw CoreFailure(message: "no reply for \(cmd)") }
        return Data(reply.utf8)
    }
}

private func book(_ title: String, _ path: [String]) -> BookMeta {
    BookMeta(id: UUID(), title: title, chaptersCount: 1, paragraphsCount: 1, translationRatio: 0, path: path)
}

@Suite struct LibraryFolderTests {
    @Test func buildsNestedFoldersSorted() {
        let tree = LibraryFolder.build([book("b", ["X", "Y"]), book("a", []), book("c", ["X"])])
        #expect(tree.books.map(\.title) == ["a"])
        #expect(tree.folders.map(\.name) == ["X"])
        #expect(tree.folders[0].books.map(\.title) == ["c"])
        #expect(tree.folders[0].folders[0].books.map(\.title) == ["b"])
        #expect(tree.allPaths == [[], ["X"], ["X", "Y"]])
    }

    @Test func emptyFolderIsAdded() {
        let tree = LibraryFolder.build([]).addingEmpty(["A", "B"])
        #expect(tree.allPaths == [[], ["A"], ["A", "B"]])
    }
}

@MainActor
@Suite struct TranslationActivityTests {
    let id = UUID()

    @Test func progressNeverCreatesAnEntry() {
        let store = TranslationActivityStore(events: EventHub())
        store.progress(.init(bookId: id, paragraphId: 1, requestId: 7, progressChars: 5, expectedChars: 10))
        #expect(store.activity(id, 1) == nil)
    }

    @Test func staleFinishedKeepsNewerRequest() {
        let store = TranslationActivityStore(events: EventHub())
        store.started(.init(bookId: id, paragraphId: 1, requestId: 1, expectedChars: 10))
        store.started(.init(bookId: id, paragraphId: 1, requestId: 2, expectedChars: 10))
        store.finished(.init(bookId: id, paragraphId: 1, requestId: 1, error: nil))
        #expect(store.activity(id, 1)?.requestId == 2)
        store.finished(.init(bookId: id, paragraphId: 1, requestId: 2, error: nil))
        #expect(store.activity(id, 1) == nil)
    }
}

@Suite struct PairingTests {
    @Test func parsesJSONAndBareIds() {
        let json = SyncStore.pairingPayload(deviceId: "ABC", name: "Mac")
        #expect(SyncStore.parsePairing(json)?.deviceId == "ABC")
        #expect(SyncStore.parsePairing(json)?.name == "Mac")
        #expect(SyncStore.parsePairing("  XYZ ")?.deviceId == "XYZ")
        #expect(SyncStore.parsePairing("  ") == nil)
    }
}

@MainActor
@Suite struct ChapterStoreTests {
    @Test func loadsParagraphTexts() async {
        let core = FakeCore()
        let bookId = UUID()
        core.replies["get_book_chapter_paragraph_ids"] = "[0,1]"
        core.replies["get_paragraph_texts_batch"] = """
            {"language":"spa","paragraphs":[{"id":0,"text":"Hola","runs":[],"words":[{"start":0,"len":4,"s":0,"w":0,"f":0,"t":"Hello"}],"translated":true},
             {"id":1,"text":"mundo","runs":[{"start":0,"len":5,"marks":1}],"words":[],"translated":false}]}
            """
        let store = ChapterStore(bookId: bookId, chapterId: 0, core: core, events: EventHub())
        await store.load()
        #expect(store.paragraphIds == [0, 1])
        #expect(store.paragraphs[0]?.words.first?.t == "Hello")
        #expect(store.untranslatedCount == 1)
        #expect(store.language == "es")
    }

    /// `book_updated` comes once per saved paragraph while a chapter
    /// translates; with unchanged ids it must not refetch the chapter.
    @Test func bookUpdatedWithSameIdsDoesNotRefetch() async throws {
        let core = FakeCore()
        let bookId = UUID()
        let events = EventHub()
        core.replies["get_book_chapter_paragraph_ids"] = "[0,1]"
        core.replies["get_paragraph_texts_batch"] = #"{"language":"spa","paragraphs":[]}"#
        let store = ChapterStore(bookId: bookId, chapterId: 0, core: core, events: events)
        await store.load()
        let before = core.calls.filter { $0.0 == "get_paragraph_texts_batch" }.count
        let payload = Data("\"\(bookId.uuidString.lowercased())\"".utf8)
        for _ in 0..<20 { events.deliver("book_updated", payload) }
        try await Task.sleep(for: .milliseconds(200))
        #expect(core.calls.filter { $0.0 == "get_paragraph_texts_batch" }.count == before)
        events.deliver("paragraph_updated", Data(#"{"bookId":"\#(bookId.uuidString.lowercased())","paragraphId":1}"#.utf8))
        try await Task.sleep(for: .milliseconds(200))
        #expect(core.calls.filter { $0.0 == "get_paragraph_texts_batch" }.count == before + 1)
    }
}

@MainActor
@Suite struct ChapterLayoutTests {
    private func paragraph(_ id: Int, _ text: String) -> ParagraphText {
        var words: [ParagraphText.Word] = []
        var offset = 0
        for (i, w) in text.split(separator: " ").enumerated() {
            let len = (String(w) as NSString).length
            words.append(.init(start: offset, len: len, s: 0, w: i, f: i, t: "t\(i)"))
            offset += len + 1
        }
        return ParagraphText(id: id, text: text, runs: [], words: words, translated: true)
    }

    @Test func paginatesAndRoundTripsAnchors() {
        let layout = ChapterTextLayout()
        let long = Array(repeating: "lorem ipsum dolor sit amet", count: 40).joined(separator: " ")
        let paragraphs = Dictionary(uniqueKeysWithValues: (0..<20).map { ($0, paragraph($0, long)) })
        layout.setContent(order: Array(0..<20), paragraphs: paragraphs, typography: ReaderTypography())
        layout.layout(width: 400, pageHeight: 500)
        #expect(layout.pages.count > 10)
        for (i, page) in layout.pages.enumerated() {
            #expect(page.bottom - page.top <= 500 + 0.5, "page \(i) too tall")
            if i > 0 { #expect(page.top >= layout.pages[i - 1].bottom - 0.5) }
        }
        for page in [0, 3, layout.pages.count - 1] {
            let anchor = layout.anchor(forPage: page)!
            #expect(layout.page(forParagraph: anchor.paragraphId, pageOffset: anchor.pageOffset) == page)
        }
    }

    @Test func hitTestFindsWords() {
        let layout = ChapterTextLayout()
        layout.setContent(order: [0], paragraphs: [0: paragraph(0, "alpha beta gamma")], typography: ReaderTypography())
        layout.layout(width: 600, pageHeight: 500)
        for index in 0..<3 {
            let range = layout.wordRange(paragraphId: 0, wordIndex: index)!
            let frame = layout.frames(of: range).first!
            let hit = layout.hit(at: CGPoint(x: frame.midX, y: frame.maxY - 6))
            #expect(hit?.wordIndex == index)
        }
    }

    @Test func replacingAParagraphKeepsOthers() {
        let layout = ChapterTextLayout()
        layout.setContent(
            order: [0, 1], paragraphs: [0: paragraph(0, "one two"), 1: paragraph(1, "three")],
            typography: ReaderTypography())
        layout.replace([paragraph(0, "uno dos tres cuatro")])
        #expect(layout.storage.string == "uno dos tres cuatro\nthree")
        #expect(layout.ranges[1] == NSRange(location: 20, length: 5))
    }
}

/// Renders the reader offscreen to `FLTS_SNAPSHOT_DIR` for eyeballing; skipped otherwise.
@MainActor
@Suite struct ReaderSnapshot {
    @Test func renderSpread() throws {
        guard let dir = ProcessInfo.processInfo.environment["FLTS_SNAPSHOT_DIR"] else { return }
        let words = "El gato negro duerme sobre la mesa mientras la lluvia cae despacio en la ciudad antigua".split(separator: " ")
        var paragraphs: [Int: ParagraphText] = [:]
        for id in 0..<12 {
            let text = (0..<(20 + id * 7)).map { String(words[($0 + id) % words.count]) }.joined(separator: " ") + "."
            var ws: [ParagraphText.Word] = []
            var offset = 0
            for (i, w) in text.dropLast().split(separator: " ").enumerated() {
                let len = (String(w) as NSString).length
                ws.append(.init(start: offset, len: len, s: 0, w: i, f: i, t: "tr-\(w.lowercased())"))
                offset += len + 1
            }
            let translated = id % 3 != 2
            paragraphs[id] = ParagraphText(
                id: id, text: text, runs: id == 1 ? [.init(start: 0, len: 9, marks: 1)] : [],
                words: translated ? ws : [], translated: translated)
        }
        let canvas = ReaderCanvas(frame: NSRect(x: 0, y: 0, width: 1200, height: 760))
        canvas.appearance = NSAppearance(named: .aqua)
        canvas.setContent(order: Array(0..<12), paragraphs: paragraphs, typography: ReaderTypography(), anchor: nil)
        canvas.layoutSubtreeIfNeeded()
        canvas.layout()
        canvas.selected = (0, 3)
        canvas.revealed = [.init(paragraphId: 0, wordIndex: 3), .init(paragraphId: 0, wordIndex: 10), .init(paragraphId: 1, wordIndex: 2)]
        let rep = try #require(canvas.bitmapImageRepForCachingDisplay(in: canvas.bounds))
        canvas.cacheDisplay(in: canvas.bounds, to: rep)
        let png = try #require(rep.representation(using: .png, properties: [:]))
        try png.write(to: URL(fileURLWithPath: dir).appendingPathComponent("reader.png"))
    }
}

/// Timings for the reader gate; run with `FLTS_PERF=1` (prints, never fails on time).
@MainActor
@Suite struct ReaderPerf {
    private func heavyChapter() -> [Int: ParagraphText] {
        let vocab = "el gato negro duerme sobre la mesa mientras la lluvia cae despacio en la ciudad antigua y nadie sabe cuándo volverá".split(separator: " ")
        var out: [Int: ParagraphText] = [:]
        var total = 0
        for id in 0..<320 {
            // Mostly short, some up to ~400 words, ~11.3k words in all.
            let count = id % 23 == 0 ? 400 : 20 + (id * 13) % 30
            total += count
            var text = ""
            var ws: [ParagraphText.Word] = []
            for i in 0..<count {
                if i > 0 { text += " " }
                let w = String(vocab[(id + i * 5) % vocab.count])
                ws.append(.init(start: (text as NSString).length, len: (w as NSString).length, s: i / 12, w: i % 12, f: i, t: "t\(i)"))
                text += w
            }
            out[id] = ParagraphText(id: id, text: text + ".", runs: [], words: ws, translated: true)
        }
        return out
    }

    private func ms(_ d: Duration) -> String {
        String(format: "%.1f ms", Double(d.components.attoseconds) / 1e15 + Double(d.components.seconds) * 1000)
    }

    private func draw(_ canvas: ReaderCanvas) {
        let rep = canvas.bitmapImageRepForCachingDisplay(in: canvas.bounds)!
        canvas.cacheDisplay(in: canvas.bounds, to: rep)
    }

    @Test func gate() async {
        guard ProcessInfo.processInfo.environment["FLTS_PERF"] != nil else { return }
        let paragraphs = heavyChapter()
        let order = Array(0..<320)
        let clock = ContinuousClock()
        var report: [String] = []

        do {
            let t = ChapterTextLayout()
            let build = clock.measure { t.setContent(order: order, paragraphs: paragraphs, typography: ReaderTypography()) }
            let lay = clock.measure { t.layoutManager.ensureLayout(for: t.layoutManager.documentRange) }
            let all = clock.measure { t.layout(width: 520, pageHeight: 700) }
            report.append("split: build \(ms(build)), ensureLayout(400w) \(ms(lay)), relayout+paginate(520w) \(ms(all))")
            let changed = (200..<220).map { id -> ParagraphText in
                let p = paragraphs[id]!
                return ParagraphText(id: id, text: p.text + " fin", runs: [], words: p.words, translated: true)
            }
            let keep = t.lineAnchor(forPage: 60)
            let rep = clock.measure { t.replace(changed) }
            let ens = clock.measure { t.layoutManager.ensureLayout(for: t.layoutManager.documentRange) }
            let rel = clock.measure { t.layout(width: 520, pageHeight: 700, breakAt: keep) }
            report.append("update split: replace \(ms(rep)), ensureLayout \(ms(ens)), layout(collect+paginate) \(ms(rel))")
            for (h, j) in [(false, true), (true, false), (false, false)] {
                var ty = ReaderTypography()
                ty.hyphenate = h
                ty.justify = j
                let v = ChapterTextLayout()
                v.setContent(order: order, paragraphs: paragraphs, typography: ty)
                let d = clock.measure { v.layoutManager.ensureLayout(for: v.layoutManager.documentRange) }
                report.append("ensureLayout hyphenate=\(h) justify=\(j): \(ms(d))")
            }
        }
        func waitFull(_ canvas: ReaderCanvas) async {
            guard canvas.isProvisional else { return }
            await withCheckedContinuation { (c: CheckedContinuation<Void, Never>) in
                canvas.onFullLayout = {
                    canvas.onFullLayout = nil
                    c.resume()
                }
            }
        }
        for run in ["first", "again"] {
            for anchor in [nil, (240, 0)] as [(Int, Int)?] {
                let canvas = ReaderCanvas(frame: NSRect(x: 0, y: 0, width: 1280, height: 800))
                let start = clock.now
                canvas.setContent(
                    order: order, paragraphs: paragraphs, typography: ReaderTypography(),
                    anchor: anchor.map { ReadingAnchor(paragraphId: $0.0, pageOffset: $0.1) })
                canvas.layout()
                draw(canvas)
                let reveal = clock.now - start
                let shownTop = canvas.text.lineAnchor(forPage: canvas.spreadStart)
                await waitFull(canvas)
                let full = clock.now - start
                let stayed = canvas.text.lineAnchor(forPage: canvas.spreadStart) == shownTop
                report.append("\(anchor == nil ? "open" : "restore 3/4") \(run): reveal \(ms(reveal)), full \(ms(full)), \(canvas.pageCount) pages, page \(canvas.spreadStart), view kept: \(stayed)")
            }
        }

        let canvas = ReaderCanvas(frame: NSRect(x: 0, y: 0, width: 1280, height: 800))
        canvas.setContent(order: order, paragraphs: paragraphs, typography: ReaderTypography(), anchor: ReadingAnchor(paragraphId: 240, pageOffset: 0))
        canvas.layout()
        await waitFull(canvas)
        var flips: [Duration] = []
        for _ in 0..<20 {
            flips.append(clock.measure {
                canvas.go(toPage: canvas.spreadStart + 2, animated: false)
                draw(canvas)
            })
        }
        report.append("flip worst: \(ms(flips.max()!)), median \(ms(flips.sorted()[10]))")

        var taps: [Duration] = []
        for p in 240..<250 {
            taps.append(clock.measure {
                canvas.selected = (p, 3)
                canvas.revealed.insert(.init(paragraphId: p, wordIndex: 3))
                draw(canvas)
            })
        }
        report.append("tap worst: \(ms(taps.max()!))")

        let anchorBefore = canvas.currentAnchor()
        let changed = (200..<220).map { id -> ParagraphText in
            let p = paragraphs[id]!
            return ParagraphText(id: id, text: p.text + " fin", runs: [], words: p.words, translated: true)
        }
        // Worst single run-loop turn while a 20-paragraph burst is applied.
        canvas.replace(changed)
        let burstStart = clock.now
        await withCheckedContinuation { (c: CheckedContinuation<Void, Never>) in
            canvas.onChangesApplied = {
                canvas.onChangesApplied = nil
                c.resume()
            }
        }
        let burst = clock.now - burstStart
        let single = clock.measure {
            canvas.replace(Array(changed.prefix(ReaderCanvas.changesPerTurn)))
        }
        let oneTurnStart = clock.now
        await withCheckedContinuation { (c: CheckedContinuation<Void, Never>) in
            canvas.onChangesApplied = {
                canvas.onChangesApplied = nil
                c.resume()
            }
        }
        let oneTurn = clock.now - oneTurnStart + single
        draw(canvas)
        report.append("update 20 paragraphs: whole burst \(ms(burst)) in \(20 / ReaderCanvas.changesPerTurn) turns, one turn (5 paragraphs) \(ms(oneTurn)), anchor kept: \(canvas.currentAnchor()?.paragraphId == anchorBefore?.paragraphId)")

        var resizes: [Duration] = []
        canvas.inLiveResizeOverride = true
        for w in stride(from: 1280, through: 900, by: -20) {
            resizes.append(clock.measure {
                canvas.setFrameSize(NSSize(width: CGFloat(w), height: 800))
                canvas.layout()
                draw(canvas)
            })
        }
        canvas.inLiveResizeOverride = false
        let settleStart = clock.now
        canvas.viewDidEndLiveResize()
        await waitFull(canvas)
        let settle = clock.now - settleStart
        report.append("resize settle (full layout): \(ms(settle))")
        report.append("resize step worst: \(ms(resizes.max()!)), median \(ms(resizes.sorted()[resizes.count / 2]))")

        let text = report.joined(separator: "\n")
        print("READER-PERF\n" + text)
        if let dir = ProcessInfo.processInfo.environment["FLTS_SNAPSHOT_DIR"] {
            try? text.write(toFile: dir + "/reader-perf.txt", atomically: true, encoding: .utf8)
        }
    }
}

/// The real core on a seeded library (`FLTS_SMOKE_CONFIG_DIR`), rendered
/// offscreen; skipped otherwise. Starts the core, so it runs once per process.
@MainActor
@Suite struct AppSmoke {
    @Test func opensLibraryAndReader() async throws {
        guard let dir = ProcessInfo.processInfo.environment["FLTS_SMOKE_CONFIG_DIR"] else { return }
        setenv("FLTS_CONFIG_DIR", dir, 1)
        setenv("FLTS_DISABLE_SYNC", "1", 1)
        setenv("FLTS_DISABLE_KEYRING", "1", 1)
        let model = AppModel()
        model.start()
        guard case .running = model.phase else {
            Issue.record("core did not start: \(model.phase)")
            return
        }
        for _ in 0..<200 where !(model.library.loaded && model.config.loaded) {
            try await Task.sleep(for: .milliseconds(50))
        }
        #expect(model.config.isConfigured)
        let book = try #require(model.library.books.first)
        model.selectedBookId = book.id

        let window = NSWindow(
            contentRect: NSRect(x: 0, y: 0, width: 1400, height: 860), styleMask: [.titled, .resizable],
            backing: .buffered, defer: false)
        window.appearance = NSAppearance(named: .aqua)
        let host = NSHostingView(rootView: RootView().environment(model))
        window.contentView = host
        try await Task.sleep(for: .seconds(3))

        // Tap a word through the canvas like a click would.
        let canvas = try #require(host.firstDescendant(of: ReaderCanvas.self))
        #expect(canvas.pageCount > 1)
        #expect((canvas.accessibilityValue() as? String)?.isEmpty == false)
        let p = try #require(canvas.text.paragraphs.values.first { $0.translated && !$0.words.isEmpty })
        canvas.selected = (p.id, 1)
        canvas.revealed = [.init(paragraphId: p.id, wordIndex: 1), .init(paragraphId: p.id, wordIndex: 4)]
        canvas.delegate?.reader(canvas, selected: p.id, wordIndex: 1)
        try await Task.sleep(for: .seconds(1.5))

        // System dictionary: the core hops to the main thread through the shell.
        let core = try #require(model.core)
        let definition = try await core.invoke(
            "get_system_definition", ["word": "gato", "sourceLang": "spa", "targetLang": "eng"],
            as: SystemDefinition?.self)
        print("SMOKE-DICT \(definition.map { String($0.definition.prefix(60)) } ?? "none")")

        // Anki card for the selected word.
        let session = BookSession(bookId: book.id, core: core, events: model.events)
        await session.start()
        let first = try #require(session.chapter?.paragraphs.values.first { $0.translated && !$0.words.isEmpty })
        let w = first.words[0]
        session.word.select(WordRef(bookId: book.id, paragraphId: first.id, sentence: w.s, word: w.w, flatIndex: w.f), surface: nil)
        for _ in 0..<100 where session.word.info == nil { try await Task.sleep(for: .milliseconds(30)) }
        #expect(session.word.info != nil)
        if session.word.info?.canCreateCard == true {
            await session.word.createCard()
            #expect(session.word.cardState == .created || session.word.cardState == .exists)
        }
        session.close()

        // Import, move into a folder, delete.
        let id = try await model.library.importText(title: "Smoke", text: "Hola mundo.\n\nAdiós.", sourceLanguageId: "spa")
        #expect(model.library.book(id) != nil)
        await model.library.move([id], to: ["Pruebas"])
        #expect(model.library.book(id)?.path == ["Pruebas"])
        await model.library.delete([id])
        #expect(model.library.book(id) == nil)

        if let out = ProcessInfo.processInfo.environment["FLTS_SNAPSHOT_DIR"] {
            let rep = try #require(host.bitmapImageRepForCachingDisplay(in: host.bounds))
            host.cacheDisplay(in: host.bounds, to: rep)
            try rep.representation(using: .png, properties: [:])?.write(
                to: URL(fileURLWithPath: out).appendingPathComponent("app.png"))
        }
        await model.shutdown()
    }
}

extension NSView {
    func firstDescendant<T: NSView>(of type: T.Type) -> T? {
        for sub in subviews {
            if let match = sub as? T ?? sub.firstDescendant(of: type) { return match }
        }
        return nil
    }
}

@MainActor
@Suite struct ReaderPositionTests {
    private func chapter() -> [Int: ParagraphText] {
        let vocab = "uno dos tres cuatro cinco seis siete ocho nueve diez".split(separator: " ")
        var out: [Int: ParagraphText] = [:]
        for id in 0..<120 {
            let n = 15 + (id * 17) % 140
            let text = (0..<n).map { String(vocab[($0 + id) % vocab.count]) }.joined(separator: " ")
            out[id] = ParagraphText(id: id, text: text, runs: [], words: [], translated: true)
        }
        return out
    }

    private func open(_ anchor: ReadingAnchor?, _ paragraphs: [Int: ParagraphText]) async -> ReaderCanvas {
        let canvas = ReaderCanvas(frame: NSRect(x: 0, y: 0, width: 1100, height: 760))
        canvas.setContent(order: Array(0..<120), paragraphs: paragraphs, typography: ReaderTypography(), anchor: anchor)
        canvas.layout()
        if canvas.isProvisional {
            await withCheckedContinuation { (c: CheckedContinuation<Void, Never>) in
                canvas.onFullLayout = {
                    canvas.onFullLayout = nil
                    c.resume()
                }
            }
        }
        return canvas
    }

    /// Saving and reopening must land on the same top line, from any page.
    @Test func savedPositionReopensInPlace() async {
        let paragraphs = chapter()
        let first = await open(nil, paragraphs)
        for page in stride(from: 0, to: first.pageCount, by: 7) {
            first.go(toPage: page, animated: false)
            guard let saved = first.currentAnchor(), let top = first.text.lineAnchor(forPage: page) else { continue }
            let reopened = await open(saved, paragraphs)
            #expect(reopened.text.lineAnchor(forPage: reopened.spreadStart) == top, "page \(page)")
            // Another device's position (no line) lands in the right paragraph.
            let synced = await open(ReadingAnchor(paragraphId: saved.paragraphId, pageOffset: saved.pageOffset), paragraphs)
            #expect(synced.currentAnchor()?.paragraphId == saved.paragraphId, "page \(page)")
        }
    }
}

/// First launch on an empty config dir (`FLTS_SMOKE_EMPTY_DIR`): onboarding,
/// then settings written through the core.
@MainActor
@Suite struct FirstLaunchSmoke {
    @Test func onboardsAndSavesConfig() async throws {
        guard let dir = ProcessInfo.processInfo.environment["FLTS_SMOKE_EMPTY_DIR"] else { return }
        setenv("FLTS_CONFIG_DIR", dir, 1)
        setenv("FLTS_DISABLE_SYNC", "1", 1)
        setenv("FLTS_DISABLE_KEYRING", "1", 1)
        let model = AppModel()
        model.start()
        for _ in 0..<200 where !(model.config.loaded && !model.config.languages.isEmpty) {
            try await Task.sleep(for: .milliseconds(50))
        }
        #expect(!model.config.isConfigured)
        #expect(model.library.books.isEmpty)
        await model.config.update {
            $0.targetLanguageId = "eng"
            $0.setApiKey("test-key", for: $0.translationProvider)
        }
        #expect(model.config.isConfigured)
        await model.config.reload()
        #expect(model.config.config?.targetLanguageId == "eng")
        let saved = try String(contentsOfFile: dir + "/config.json", encoding: .utf8)
        #expect(saved.contains("\"targetLanguageId\": \"eng\"") || saved.contains("\"targetLanguageId\":\"eng\""))
        await model.shutdown()
    }
}

/// The embedded Syncthing engine in this process (`FLTS_SMOKE_SYNC_DIR`):
/// sync comes up, and quitting returns within the bound.
@MainActor
@Suite struct SyncSmoke {
    @Test func syncStartsAndShutsDown() async throws {
        guard let dir = ProcessInfo.processInfo.environment["FLTS_SMOKE_SYNC_DIR"] else { return }
        setenv("FLTS_CONFIG_DIR", dir, 1)
        setenv("FLTS_DISABLE_KEYRING", "1", 1)
        unsetenv("FLTS_DISABLE_SYNC")
        let model = AppModel()
        model.start()
        for _ in 0..<100 where !model.config.loaded { try await Task.sleep(for: .milliseconds(50)) }
        await model.sync.setEnabled(true)
        var states: [SyncState] = []
        for _ in 0..<300 {
            await model.sync.refresh()
            if states.last != model.sync.status.state { states.append(model.sync.status.state) }
            if model.sync.status.state == .online { break }
            try await Task.sleep(for: .milliseconds(100))
        }
        print("SYNC-STATES \(states) device \(model.sync.status.deviceId ?? "-")")
        #expect(model.sync.status.state == .online)
        await model.sync.refreshDevices()
        #expect(model.sync.thisDevice?.deviceId.isEmpty == false)
        if let peer = ProcessInfo.processInfo.environment["FLTS_SMOKE_PEER_ID"] {
            await model.sync.add(deviceId: peer, name: "Peer")
            #expect(model.sync.error == nil)
            #expect(model.sync.devices.contains { $0.deviceId == peer })
            await model.sync.remove(deviceId: peer)
            #expect(!model.sync.devices.contains { $0.deviceId == peer })
        }
        let clock = ContinuousClock()
        let quit = await clock.measure { await model.shutdown() }
        print("SYNC-SHUTDOWN \(quit)")
        #expect(quit < .seconds(12))
    }
}

@MainActor
@Suite struct LyricsStoreTests {
    @Test func followsTrackAndTranslation() async throws {
        let core = FakeCore()
        let events = EventHub()
        core.replies["start_spotify_watcher"] = "null"
        core.replies["get_now_playing"] = #"{"state":"playing","trackId":"t1","name":"Song","artist":"A","album":null,"positionMs":15000,"durationMs":200000}"#
        core.replies["get_track_lyrics_state"] = #"{"lyrics":{"track_id":"t1","lines":[{"time_ms":0,"text":"uno"},{"time_ms":10000,"text":"dos"},{"time_ms":20000,"text":"tres"}],"synced":true},"translation":null}"#
        core.replies["spotify_web_status"] = #"{"connected":false,"premiumRequired":false,"lastError":null}"#
        core.replies["spotify_cdp_status"] = #"{"available":false,"port":9222,"hint":"off"}"#
        core.replies["spotify_login_agent_status"] = #"{"installed":false,"path":"/x"}"#
        core.replies["spotify_web_get_queue"] = "null"
        let store = LyricsStore(events: events)
        store.attach(core)
        store.targetLanguage = "eng"
        await store.start()
        for _ in 0..<50 where store.lyrics == nil { try await Task.sleep(for: .milliseconds(10)) }
        #expect(store.lyrics?.lines.count == 3)
        #expect(store.currentLine(at: store.position(at: store.receivedAt)) == 1)
        #expect(store.phase == .translating(bytes: 0))

        events.deliver("lyrics_translation_done", Data(#"{"trackId":"t1","translation":{"track_id":"t1","target_lang":"eng","model":"m","lines":[{"translation":"one","glosses":[]},{"translation":"two","glosses":[]},{"translation":"three","glosses":[]}]}}"#.utf8))
        #expect(store.translation?.lines[1].translation == "two")
        #expect(store.phase == .done)

        // A stale track's events are ignored.
        events.deliver("lyrics_translation_error", Data(#"{"trackId":"old","error":"x"}"#.utf8))
        #expect(store.phase == .done)
    }
}

/// Counts calls on the way to the real core.
final class CountingCore: CoreService, @unchecked Sendable {
    let inner: any CoreService
    private let lock = NSLock()
    private var counts: [String: Int] = [:]

    init(_ inner: any CoreService) { self.inner = inner }

    func count(_ cmd: String) -> Int { lock.withLock { counts[cmd, default: 0] } }

    func invokeRaw(_ cmd: String, _ args: [String: JSONValue]) async throws -> Data {
        lock.withLock { counts[cmd, default: 0] += 1 }
        return try await inner.invokeRaw(cmd, args)
    }
}

/// Native translate and EPUB import against the real core and the LLM sim
/// (`FLTS_SMOKE_TRANSLATE_DIR`, `FLTS_SMOKE_LLM_URL`, `FLTS_SMOKE_TEXT`,
/// `FLTS_SMOKE_EPUB`); skipped otherwise.
@MainActor
@Suite struct TranslateSmoke {
    @Test func translatesAChapterThroughEvents() async throws {
        let env = ProcessInfo.processInfo.environment
        guard let dir = env["FLTS_SMOKE_TRANSLATE_DIR"], let llm = env["FLTS_SMOKE_LLM_URL"],
            let textFile = env["FLTS_SMOKE_TEXT"]
        else { return }
        setenv("FLTS_CONFIG_DIR", dir, 1)
        setenv("FLTS_GEMINI_BASE_URL", llm, 1)
        setenv("FLTS_DISABLE_SYNC", "1", 1)
        setenv("FLTS_DISABLE_KEYRING", "1", 1)
        let model = AppModel()
        model.start()
        for _ in 0..<200 where !(model.config.loaded && model.library.loaded) {
            try await Task.sleep(for: .milliseconds(50))
        }
        let core = CountingCore(try #require(model.core))

        if let epub = env["FLTS_SMOKE_EPUB"] {
            let preview = try await model.library.parseEpub(at: URL(fileURLWithPath: epub))
            print("SMOKE-EPUB title \(preview.summary.title) lang \(preview.summary.language ?? "-")")
            #expect(!preview.summary.title.isEmpty)
            let lang = await model.config.suggestedSourceLanguage(for: preview.summary.language)
            #expect(lang == "spa")
            let id = try await model.library.importEpub(preview, sourceLanguageId: lang)
            let chapters: [ChapterMeta] = try await core.invoke("list_book_chapters", ["bookId": .uuid(id)])
            print("SMOKE-EPUB chapters \(chapters.map(\.title))")
            #expect(chapters.contains { $0.title == "Dos" })
        }

        let text = try String(contentsOfFile: textFile, encoding: .utf8)
        let bookId = try await model.library.importText(title: "Traducir", text: text, sourceLanguageId: "spa")
        let book = BookStore(bookId: bookId, core: core, events: model.events)
        await book.load()
        let chapter = ChapterStore(bookId: bookId, chapterId: 0, core: core, events: model.events)
        await chapter.load()
        let total = chapter.paragraphIds.count
        #expect(chapter.untranslatedCount == total)
        let fetchesBefore = core.count("get_paragraph_texts_batch")

        await book.translateChapter(0, model: model.config.config?.model ?? "")
        for _ in 0..<600 where chapter.untranslatedCount > 0 {
            try await Task.sleep(for: .milliseconds(50))
        }
        #expect(chapter.untranslatedCount == 0, "only events can have updated the store")
        try await Task.sleep(for: .milliseconds(500))
        let fetches = core.count("get_paragraph_texts_batch") - fetchesBefore
        print("SMOKE-TRANSLATE paragraphs \(total), text fetches \(fetches), list_books \(core.count("list_books"))")
        #expect(fetches <= total + 3)
        #expect(chapter.paragraphs.values.allSatisfy { !$0.words.isEmpty })
        #expect(model.activity.entries.isEmpty)
        chapter.close()
        book.close()
        await model.shutdown()
    }
}

@MainActor
@Suite struct HitTestGate {
    /// Long words so lines wrap and hyphenate; every piece of a split word must hit it.
    private func chapter() -> [Int: ParagraphText] {
        let vocab = "desafortunadamente extraordinariamente gato incomprensiblemente la responsabilidades y sol internacionalización".split(separator: " ")
        var out: [Int: ParagraphText] = [:]
        for id in 0..<80 {
            var text = ""
            var ws: [ParagraphText.Word] = []
            for i in 0..<(25 + id % 40) {
                if i > 0 { text += " " }
                let w = String(vocab[(id * 3 + i) % vocab.count])
                ws.append(.init(start: (text as NSString).length, len: (w as NSString).length, s: 0, w: i, f: i, t: "t"))
                text += w
            }
            out[id] = ParagraphText(id: id, text: text + ".", runs: [], words: ws, translated: true)
        }
        return out
    }

    private func check(_ canvas: ReaderCanvas, taps: inout Int, misses: inout [String], split: inout Int) {
        var rng = SystemRandomNumberGenerator()
        for page in canvas.spreadStart..<min(canvas.spreadStart + canvas.columns, canvas.pageCount) {
            let band = canvas.text.pages[page]
            var candidates: [(Int, Int, [CGRect])] = []
            for (pid, p) in canvas.text.paragraphs {
                for i in p.words.indices {
                    guard let r = canvas.text.wordRange(paragraphId: pid, wordIndex: i) else { continue }
                    let frames = canvas.text.frames(of: r).filter { $0.minY >= band.top - 0.5 && $0.maxY <= band.bottom + 0.5 }
                    if !frames.isEmpty { candidates.append((pid, i, frames)) }
                }
            }
            for _ in 0..<12 where !candidates.isEmpty {
                let (pid, i, frames) = candidates[Int.random(in: 0..<candidates.count, using: &rng)]
                for f in frames {
                    let v = canvas.viewPoint(CGPoint(x: f.midX, y: f.maxY - canvas.text.typography.fontSize * 0.6), onPage: page)!
                    taps += 1
                    if frames.count > 1 { split += 1 }
                    let hit = canvas.hit(v)
                    if hit?.paragraphId != pid || hit?.wordIndex != i { misses.append("p\(pid) w\(i) got \(String(describing: hit))") }
                }
            }
        }
    }

    @Test func randomTapsHitTheirWord() async {
        let paragraphs = chapter()
        var taps = 0
        var split = 0
        var misses: [String] = []
        // Provisional layout right after opening deep, then the full one.
        let canvas = ReaderCanvas(frame: NSRect(x: 0, y: 0, width: 1100, height: 760))
        canvas.setContent(order: Array(0..<80), paragraphs: paragraphs, typography: ReaderTypography(), anchor: ReadingAnchor(paragraphId: 50, pageOffset: 0))
        canvas.layout()
        #expect(canvas.isProvisional)
        check(canvas, taps: &taps, misses: &misses, split: &split)
        await withCheckedContinuation { (c: CheckedContinuation<Void, Never>) in
            canvas.onFullLayout = {
                canvas.onFullLayout = nil
                c.resume()
            }
        }
        for page in stride(from: 0, to: canvas.pageCount, by: max(canvas.pageCount / 8, 1)) {
            canvas.go(toPage: page, animated: false)
            check(canvas, taps: &taps, misses: &misses, split: &split)
        }
        print("HIT-GATE taps \(taps), split-word taps \(split), misses \(misses.count)")
        #expect(taps >= 100)
        #expect(split > 0)
        #expect(misses.isEmpty, "\(misses.prefix(5))")
    }
}

@MainActor
@Suite struct ReaderSelectionTests {
    private func event(_ type: NSEvent.EventType, _ point: CGPoint, in window: NSWindow) -> NSEvent {
        NSEvent.mouseEvent(
            with: type, location: point, modifierFlags: [], timestamp: 0, windowNumber: window.windowNumber,
            context: nil, eventNumber: 0, clickCount: 1, pressure: 1)!
    }

    @Test func dragSelectsAndClickReveals() throws {
        let text = "alpha beta gamma delta epsilon zeta eta theta"
        var words: [ParagraphText.Word] = []
        var offset = 0
        for (i, w) in text.split(separator: " ").enumerated() {
            words.append(.init(start: offset, len: w.count, s: 0, w: i, f: i, t: "t\(i)"))
            offset += w.count + 1
        }
        let window = NSWindow(contentRect: NSRect(x: 0, y: 0, width: 900, height: 600), styleMask: [.titled], backing: .buffered, defer: false)
        let canvas = ReaderCanvas(frame: NSRect(x: 0, y: 0, width: 900, height: 600))
        window.contentView = canvas
        canvas.setContent(order: [0], paragraphs: [0: ParagraphText(id: 0, text: text, runs: [], words: words, translated: true)], typography: ReaderTypography(), anchor: nil)
        canvas.layout()

        func windowPoint(word: Int) -> CGPoint {
            let frame = canvas.text.frames(of: canvas.text.wordRange(paragraphId: 0, wordIndex: word)!).first!
            let view = canvas.viewPoint(CGPoint(x: frame.midX, y: frame.maxY - 8), onPage: 0)!
            return canvas.convert(view, to: nil)
        }

        // Drag from "beta" to "delta": a selection, no word tap.
        canvas.mouseDown(with: event(.leftMouseDown, windowPoint(word: 1), in: window))
        canvas.mouseDragged(with: event(.leftMouseDragged, windowPoint(word: 3), in: window))
        canvas.mouseUp(with: event(.leftMouseUp, windowPoint(word: 3), in: window))
        let selected = try #require(canvas.selectedText)
        #expect(selected.contains("gamma"))
        #expect(canvas.revealed.isEmpty)

        // A plain click selects the word and reveals its translation; selection clears.
        canvas.mouseDown(with: event(.leftMouseDown, windowPoint(word: 4), in: window))
        canvas.mouseUp(with: event(.leftMouseUp, windowPoint(word: 4), in: window))
        #expect(canvas.selectedText == nil)
        #expect(canvas.revealed.contains(.init(paragraphId: 0, wordIndex: 4)))
    }
}

/// Chapter open including the fetch from the real core (`FLTS_SMOKE_HEAVY_DIR`),
/// comparable with the web reader's "text shown"; prints, never fails on time.
@MainActor
@Suite struct OpenTiming {
    @Test func fetchAndFirstPaint() async throws {
        guard let dir = ProcessInfo.processInfo.environment["FLTS_SMOKE_HEAVY_DIR"] else { return }
        setenv("FLTS_CONFIG_DIR", dir, 1)
        setenv("FLTS_DISABLE_SYNC", "1", 1)
        setenv("FLTS_DISABLE_KEYRING", "1", 1)
        let model = AppModel()
        model.start()
        for _ in 0..<200 where !model.library.loaded { try await Task.sleep(for: .milliseconds(50)) }
        let book = try #require(model.library.books.first)
        let core = try #require(model.core)
        let clock = ContinuousClock()
        var lines: [String] = []
        for anchor in [nil, 240] as [Int?] {
            for run in 0..<5 {
                let start = clock.now
                let chapter = ChapterStore(bookId: book.id, chapterId: 0, core: core, events: model.events)
                await chapter.load()
                let fetched = clock.now - start
                let canvas = ReaderCanvas(frame: NSRect(x: 0, y: 0, width: 1280, height: 800))
                canvas.setContent(
                    order: chapter.paragraphIds, paragraphs: chapter.paragraphs, typography: ReaderTypography(),
                    anchor: anchor.map { ReadingAnchor(paragraphId: $0, pageOffset: 0) })
                canvas.layout()
                let rep = canvas.bitmapImageRepForCachingDisplay(in: canvas.bounds)!
                canvas.cacheDisplay(in: canvas.bounds, to: rep)
                let shown = clock.now - start
                lines.append("\(anchor == nil ? "start" : "3/4") run \(run): \(chapter.paragraphIds.count) paragraphs, fetched \(fetched), text shown \(shown)")
                chapter.close()
            }
        }
        print("OPEN-TIMING\n" + lines.joined(separator: "\n"))
        await model.shutdown()
    }
}
