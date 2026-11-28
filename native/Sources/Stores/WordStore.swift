#if canImport(AppKit)
import AppKit
#else
import UIKit
#endif
import Foundation
import Observation

/// The word selected in the reader and everything shown about it.
@MainActor
@Observable
final class WordStore {
    private(set) var selection: WordRef?
    private(set) var surface: String?
    private(set) var info: WordInfo?
    private(set) var definition: SystemDefinition?
    private(set) var loading = false
    private(set) var cardState: CardState = .idle
    var error: String?

    enum CardState: Equatable {
        case idle, creating, created, exists
        case failed(String)
    }

    private let core: any CoreService
    private let events: EventHub
    private var tokens: [EventHub.Token] = []
    private var generation = 0
    private var reloadTask: Task<Void, Never>?
    private var cardTimerTask: Task<Void, Never>?
    var targetLanguage = ""

    init(core: any CoreService, events: EventHub) {
        self.core = core
        self.events = events
        tokens.append(
            events.on { [weak self] event in
                guard let self, case .bookUpdated(let id) = event, id == selection?.bookId else { return }
                self.reloadTask?.cancel()
                self.reloadTask = Task { await self.reload() }
            })
    }

    func close() {
        events.cancel(tokens)
        tokens = []
        reloadTask?.cancel()
        reloadTask = nil
        cardTimerTask?.cancel()
        cardTimerTask = nil
        generation += 1
    }

    func select(_ ref: WordRef?, surface: String?) {
        guard ref != selection else { return }
        reloadTask?.cancel()
        reloadTask = nil
        cardTimerTask?.cancel()
        cardTimerTask = nil
        selection = ref
        self.surface = surface
        cardState = .idle
        // Going to another word keeps the last one until the new one loads,
        // so open details do not collapse and reopen in between.
        guard ref != nil else {
            generation += 1
            loading = false
            info = nil
            definition = nil
            return
        }
        reloadTask = Task { await reload() }
    }

    func reload() async {
        generation += 1
        let gen = generation
        guard let ref = selection else {
            loading = false
            return
        }
        loading = true
        defer { if gen == generation { loading = false } }
        do {
            let info = try await core.getWordInfo(
                bookId: ref.bookId, paragraphId: ref.paragraphId, sentenceId: ref.sentence, wordId: ref.word)
            guard !Task.isCancelled, gen == generation else { return }
            self.info = info
            if info == nil { definition = nil }
            if let info, info.hasCard, cardState == .idle { cardState = .exists }
            if let info, !info.isPunctuation {
                await loadDefinition(info, gen: gen)
            }
        } catch {
            guard !Task.isCancelled, gen == generation else { return }
            info = nil
            definition = nil
            self.error = error.userMessage
        }
    }

    private func loadDefinition(_ info: WordInfo, gen: Int) async {
        let word = info.original
        let def = try? await core.getSystemDefinition(
            word: word, sourceLang: info.sourceLanguage.isEmpty ? "en" : info.sourceLanguage,
            targetLang: targetLanguage.isEmpty ? "en" : targetLanguage)
        guard !Task.isCancelled, gen == generation else { return }
        definition = (def ?? nil).map { SystemDefinition(definition: Self.readable($0.definition), transcription: $0.transcription) }
    }

    /// The system dictionary answers in HTML (the web app renders it); shown
    /// here as plain text without invoking WebKit on the main thread.
    nonisolated static func readable(_ definition: String) -> String {
        guard definition.contains("<") else { return definition }
        var text = definition
        for pattern in [
            #"(?is)<head\b[^>]*>[\s\S]*?</head>"#,
            #"(?is)<style\b[^>]*>[\s\S]*?</style>"#,
            #"(?is)<script\b[^>]*>[\s\S]*?</script>"#,
        ] {
            text = text.replacingOccurrences(of: pattern, with: "", options: .regularExpression)
        }
        text = text.replacingOccurrences(of: #"(?is)<br\s*/?>"#, with: "\n", options: .regularExpression)
        text = text.replacingOccurrences(
            of: #"(?is)</(?:p|div|li|tr|h[1-6]|blockquote)>"#, with: "\n", options: .regularExpression)
        text = text.replacingOccurrences(of: #"<[^>]+>"#, with: "", options: .regularExpression)
        text = decodeHTMLEntities(text)
        return text
            .split(whereSeparator: \.isNewline)
            .map { $0.trimmingCharacters(in: .whitespaces) }
            .filter { !$0.isEmpty }
            .joined(separator: "\n")
    }

    private nonisolated static func decodeHTMLEntities(_ string: String) -> String {
        guard string.contains("&") else { return string }
        let named: [(String, String)] = [
            ("&nbsp;", " "),
            ("&lt;", "<"),
            ("&gt;", ">"),
            ("&quot;", "\""),
            ("&apos;", "'"),
            ("&#39;", "'"),
            ("&mdash;", "—"),
            ("&ndash;", "–"),
            ("&hellip;", "…"),
            ("&amp;", "&"),
        ]
        var result = ""
        result.reserveCapacity(string.count)
        var index = string.startIndex
        while index < string.endIndex {
            if string[index] == "&", let semi = string[index...].firstIndex(of: ";") {
                let entity = String(string[index...semi])
                if entity.hasPrefix("&#x") || entity.hasPrefix("&#X") {
                    let hex = entity.dropFirst(3).dropLast()
                    if let code = UInt32(hex, radix: 16), let scalar = UnicodeScalar(code) {
                        result.append(Character(scalar))
                        index = string.index(after: semi)
                        continue
                    }
                } else if entity.hasPrefix("&#") {
                    let dec = entity.dropFirst(2).dropLast()
                    if let code = UInt32(dec, radix: 10), let scalar = UnicodeScalar(code) {
                        result.append(Character(scalar))
                        index = string.index(after: semi)
                        continue
                    }
                } else if let match = named.first(where: { $0.0.caseInsensitiveCompare(entity) == .orderedSame }) {
                    result.append(match.1)
                    index = string.index(after: semi)
                    continue
                }
            }
            result.append(string[index])
            index = string.index(after: index)
        }
        return result
    }

    func createCard() async {
        guard let ref = selection, cardState != .creating else { return }
        cardTimerTask?.cancel()
        cardTimerTask = nil
        cardState = .creating
        do {
            let result = try await core.createCardForWord(
                bookId: ref.bookId, paragraphId: ref.paragraphId, sentenceId: ref.sentence, wordId: ref.word)
            guard selection == ref else { return }
            if result == .alreadyExists {
                cardState = .exists
            } else {
                // A brief check, then the button goes, as in the web app.
                cardState = .created
                cardTimerTask = Task {
                    try? await Task.sleep(for: .milliseconds(1200))
                    guard !Task.isCancelled else { return }
                    if selection == ref, cardState == .created { cardState = .exists }
                }
            }
        } catch {
            guard selection == ref else { return }
            cardState = .failed(error.userMessage)
        }
    }
}
