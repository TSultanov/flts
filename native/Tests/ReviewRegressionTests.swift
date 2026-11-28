import AppKit
import Foundation
import Testing

@testable import FLTS

@MainActor
private final class AsyncGate {
    private var entered = 0
    private var enterWaiters: [(Int, CheckedContinuation<Void, Never>)] = []
    private var paused: [CheckedContinuation<Void, Never>] = []

    func waitForEntry(_ count: Int = 1) async {
        if entered >= count { return }
        await withCheckedContinuation { cont in
            enterWaiters.append((count, cont))
        }
    }

    func pause() async {
        entered += 1
        let ready = enterWaiters.filter { entered >= $0.0 }
        enterWaiters.removeAll { entered >= $0.0 }
        for (_, cont) in ready { cont.resume() }
        await withCheckedContinuation { paused.append($0) }
    }

    func releaseNext() {
        guard !paused.isEmpty else { return }
        paused.removeFirst().resume()
    }

    func releaseAll() {
        let pending = paused
        paused.removeAll()
        for cont in pending { cont.resume() }
    }
}

@MainActor
@Suite struct ReviewRegressionTests {
    private func paragraph(_ id: Int, _ text: String, translated: Bool = true) -> ParagraphText {
        var words: [WordRange] = []
        var offset = 0
        for (i, w) in text.split(separator: " ").enumerated() {
            let len = (String(w) as NSString).length
            words.append(WordRange(start: offset, len: len, sentence: 0, word: i, flatIndex: i, translation: "t\(i)"))
            offset += len + 1
        }
        return ParagraphText(id: id, text: text, runs: [], words: words, translated: translated)
    }

    private func chapterParagraphs(count: Int = 120) -> [Int: ParagraphText] {
        let vocab = "uno dos tres cuatro cinco seis siete ocho nueve diez".split(separator: " ")
        var out: [Int: ParagraphText] = [:]
        for id in 0..<count {
            let n = 15 + (id * 17) % 140
            let text = (0..<n).map { String(vocab[($0 + id) % vocab.count]) }.joined(separator: " ")
            out[id] = paragraph(id, text)
        }
        return out
    }

    private func open(
        _ anchor: ReadingAnchor?, _ paragraphs: [Int: ParagraphText], frame: NSRect = NSRect(x: 0, y: 0, width: 1100, height: 760)
    ) async -> ReaderCanvas {
        let canvas = ReaderCanvas(frame: frame)
        let order = paragraphs.keys.sorted()
        canvas.setContent(order: order, paragraphs: paragraphs, typography: ReaderTypography(), anchor: anchor)
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

    @Test func oddSpreadBlankLeftClickDoesNotCrashAndDrainPreservesPairing() async throws {
        let paragraphs = chapterParagraphs()
        let first = await open(nil, paragraphs)
        try #require(first.columns == 2)

        var canvas = first
        var foundOdd = false
        for pid in 5..<110 {
            let reopened = await open(ReadingAnchor(paragraphId: pid, pageOffset: 0), paragraphs)
            if reopened.spreadStart % 2 == 1 {
                canvas = reopened
                foundOdd = true
                break
            }
        }
        #expect(foundOdd)
        for _ in 0..<canvas.pageCount where canvas.spreadStart > 0 {
            canvas.previousSpread()
        }
        #expect(canvas.spreadStart == -1)

        // Blank left column of an odd first spread must return nil cleanly rather than indexing pages[-1].
        #expect(canvas.nearestColumnPoint(CGPoint(x: 10, y: 100)) == nil)
        #expect(canvas.nearestColumnPoint(CGPoint(x: canvas.columnX(1) + 10, y: 100))?.0 == 1)
        _ = canvas.offset(at: CGPoint(x: 10, y: 100))

        final class DelegateRecorder: ReaderCanvasDelegate {
            var movedPages: [Int] = []
            func reader(_ canvas: ReaderCanvas, selected paragraphId: Int, wordIndex: Int) {}
            func reader(_ canvas: ReaderCanvas, translate paragraphId: Int, useCache: Bool) {}
            func reader(_ canvas: ReaderCanvas, movedTo page: Int, of count: Int) { movedPages.append(page) }
            func readerTranslationProgress(_ canvas: ReaderCanvas, paragraphId: Int) -> Double? { nil }
            func readerToggle(_ canvas: ReaderCanvas, key: String) -> Bool { false }
            func readerCanTranslate(_ canvas: ReaderCanvas) -> Bool { true }
            func readerDeselected(_ canvas: ReaderCanvas) {}
        }

        let delegate = DelegateRecorder()
        canvas.delegate = delegate
        canvas.replace([paragraph(0, "texto actualizado en el primer parrafo")])
        await withCheckedContinuation { (c: CheckedContinuation<Void, Never>) in
            canvas.onChangesApplied = {
                canvas.onChangesApplied = nil
                c.resume()
            }
        }
        #expect(canvas.spreadStart == -1)
        #expect(delegate.movedPages.allSatisfy { $0 >= 0 })
    }

    @Test func detachCancelsPendingFullLayoutAndSlide() async throws {
        let paragraphs = chapterParagraphs(count: 140)
        let canvas = ReaderCanvas(frame: NSRect(x: 0, y: 0, width: 1100, height: 760))
        canvas.setContent(
            order: Array(0..<140), paragraphs: paragraphs, typography: ReaderTypography(),
            anchor: ReadingAnchor(paragraphId: 80, pageOffset: 0))
        canvas.layout()
        #expect(canvas.isProvisional)
        canvas.slide(to: -120)
        #expect(canvas.slide == -120)
        let genBefore = canvas.generation

        canvas.detachFromWindow()
        #expect(canvas.slide == 0)
        #expect(canvas.generation > genBefore)
        try await Task.sleep(for: .milliseconds(250))
        #expect(canvas.isProvisional)
    }

    @Test func wordStoreDeselectClearsLoadingAndCreateCardDoesNotBleed() async throws {
        let core = FakeCore()
        let bookId = UUID()
        let infoGate = AsyncGate()
        let cardGate = AsyncGate()

        func makeInfo(_ word: String) -> WordInfo {
            WordInfo(
                original: word, note: "", isPunctuation: false, contextualTranslations: ["tr"],
                grammar: Grammar(
                    originalInitialForm: word, targetInitialForm: "tr", partOfSpeech: "noun", plurality: nil,
                    person: nil, tense: nil, grammaticalCase: nil, other: nil),
                fullSentenceTranslation: "Sentence.", translationModel: "m", sourceLanguage: "spa",
                canCreateCard: true, hasCard: false)
        }

        core.getSystemDefinitionHandler = { _, _, _ in nil }
        core.getWordInfoHandler = { _, pid, _, wid in
            if wid == 0 {
                await infoGate.pause()
                return makeInfo("uno")
            }
            return makeInfo("dos")
        }
        core.createCardForWordHandler = { _, _, _, _ in
            await cardGate.pause()
            return .created
        }

        let store = WordStore(core: core, events: EventHub())
        let ref1 = WordRef(bookId: bookId, paragraphId: 0, sentence: 0, word: 0, flatIndex: 0)
        let ref2 = WordRef(bookId: bookId, paragraphId: 0, sentence: 0, word: 1, flatIndex: 1)

        // Deselecting while load is in flight immediately clears `loading`.
        store.select(ref1, surface: "uno")
        await infoGate.waitForEntry(1)
        #expect(store.loading)
        store.select(nil, surface: nil)
        #expect(!store.loading)
        infoGate.releaseAll()
        try await Task.sleep(for: .milliseconds(30))
        #expect(!store.loading)
        #expect(store.info == nil)

        // Switching words while `createCardForWord` is in flight does not bleed cardState onto the second word.
        core.getWordInfoHandler = { _, _, _, wid in makeInfo(wid == 0 ? "uno" : "dos") }
        store.select(ref1, surface: "uno")
        for _ in 0..<50 where store.info?.original != "uno" {
            try await Task.sleep(for: .milliseconds(10))
        }
        #expect(store.info?.original == "uno")

        let cardTask = Task { await store.createCard() }
        await cardGate.waitForEntry(1)
        #expect(store.cardState == .creating)

        store.select(ref2, surface: "dos")
        #expect(store.cardState == .idle)
        cardGate.releaseAll()
        await cardTask.value
        #expect(store.selection == ref2)
        #expect(store.cardState == .idle)
        store.close()
    }

    @Test func wordStoreReadableStripsHtmlWithoutWebKit() {
        let html = """
            <style>body { color: red; }</style><script>alert(1)</script>
            <p>Hello &amp; <b>world</b></p><br/><div class="x">Line &lt;2&gt; &#39;ok&#39; &#x2019;s&nbsp;end</div>
            """
        let clean = WordStore.readable(html)
        #expect(clean == "Hello & world\nLine <2> 'ok' ’s end")
    }

    @Test func configStoreConcurrentUpdatesSerializeWithoutClobbering() async throws {
        let core = FakeCore()
        var current = AppConfig(
            targetLanguageId: "eng", translationProvider: .google, geminiApiKey: "k", openaiApiKey: nil,
            deepseekApiKey: nil, zaiApiKey: nil, openrouterApiKey: nil, model: "m0", libraryPath: nil,
            spotifyClientId: nil, spotifyPreloadCount: 1, spotifyShowNextTrack: true, ankiEndpoint: nil,
            ankiApiKey: nil, ankiSyncEnabled: false, syncEnabled: false, syncDeviceName: nil, translationConcurrency: 4)
        let gate = AsyncGate()
        var updateCalls = 0
        var shouldFailFirst = false

        core.getLanguagesHandler = { [] }
        core.getTranslationProvidersHandler = { [] }
        core.getModelsHandler = { [] }
        core.getLibraryRootHandler = { "/tmp/lib" }
        core.getConfigHandler = { current }
        core.updateConfigHandler = { next in
            updateCalls += 1
            let call = updateCalls
            if call == 1 {
                await gate.pause()
                if shouldFailFirst { throw CoreError.Failed(message: "boom") }
            }
            current = next
        }

        let store = ConfigStore(events: EventHub())
        store.attach(core)
        for _ in 0..<50 where !store.loaded { try await Task.sleep(for: .milliseconds(10)) }

        let t1 = Task { await store.update { $0.model = "m1" } }
        await gate.waitForEntry(1)
        let t2 = Task { await store.update { $0.translationConcurrency = 9 } }
        try await Task.sleep(for: .milliseconds(20))
        gate.releaseNext()
        await t1.value
        await t2.value
        #expect(store.config?.model == "m1")
        #expect(store.config?.translationConcurrency == 9)

        // A failing first update must not clobber a serialized second update.
        updateCalls = 0
        shouldFailFirst = true
        let f1 = Task { await store.update { $0.model = "bad" } }
        await gate.waitForEntry(2)
        let f2 = Task { await store.update { $0.translationConcurrency = 12 } }
        try await Task.sleep(for: .milliseconds(20))
        gate.releaseNext()
        await f1.value
        await f2.value
        #expect(store.config?.model == "m1")
        #expect(store.config?.translationConcurrency == 12)
        #expect(store.error == "boom")
    }

    @Test func syncStoreRunIgnoresReentrantCalls() async throws {
        let core = FakeCore()
        let gate = AsyncGate()
        core.getSyncStatusHandler = { .disabled }
        core.syncThisDeviceHandler = { ThisDevice(deviceId: "DEV", name: "Mac") }
        core.syncListDevicesHandler = { [] }
        core.syncListPendingHandler = { [] }
        core.syncSetEnabledHandler = { _ in await gate.pause() }
        core.syncAddDeviceHandler = { _, _ in }

        let store = SyncStore(events: EventHub())
        store.attach(core)
        let t1 = Task { await store.setEnabled(true) }
        await gate.waitForEntry(1)
        #expect(store.busy)
        await store.add(deviceId: "PEER", name: "Peer")
        #expect(core.count("syncAddDevice") == 0)
        gate.releaseAll()
        await t1.value
        #expect(!store.busy)
    }

    @Test func lyricsStoreClampsNegativePositionAndDropsStaleLoad() async throws {
        let core = FakeCore()
        let events = EventHub()
        let gate = AsyncGate()
        var loads = 0

        core.startSpotifyWatcherHandler = {}
        core.spotifyWebStatusHandler = { SpotifyWebStatus(connected: false, premiumRequired: false, lastError: nil) }
        core.spotifyCdpStatusHandler = { SpotifyCdpStatus(available: false, port: 9222, hint: nil) }
        core.spotifyLoginAgentStatusHandler = { SpotifyLoginAgentStatus(installed: false, path: "/p") }
        core.spotifyWebQueueHandler = { nil }
        core.getNowPlayingHandler = {
            NowPlaying(state: .playing, trackId: "t1", name: "Song", artist: "Artist", album: nil, positionMs: 1000, durationMs: 60000)
        }
        core.getTrackLyricsStateHandler = { id, lang, model in
            loads += 1
            let call = loads
            if call == 1 { await gate.pause() }
            return TrackLyricsState(
                lyrics: Lyrics(trackId: id, lines: [LyricsLine(timeMs: 0, text: "hola")], synced: true),
                translation: LyricsTranslation(
                    trackId: id, targetLang: lang, model: model,
                    lines: [LyricsLineTranslation(translation: "tr-\(lang)", glosses: [])]))
        }

        let store = LyricsStore(events: events)
        store.attach(core)
        store.targetLanguage = "eng"
        store.model = "m1"
        await store.start()

        // Negative elapsed time must clamp to >= 0.
        #expect(store.position(at: store.receivedAt.addingTimeInterval(-5)) == 0)

        await gate.waitForEntry(1)
        store.targetLanguage = "fra"
        let fresh = Task { await store.loadTrack() }
        await fresh.value
        #expect(store.translation?.targetLang == "fra")

        gate.releaseAll()
        try await Task.sleep(for: .milliseconds(30))
        #expect(store.translation?.targetLang == "fra")
    }
}
