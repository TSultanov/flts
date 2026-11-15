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
    var targetLanguage = ""

    init(core: any CoreService, events: EventHub) {
        self.core = core
        self.events = events
        tokens.append(
            events.on { [weak self] event in
                guard let self, case .bookUpdated(let id) = event, id == selection?.bookId else { return }
                Task { await self.reload() }
            })
    }

    func close() {
        events.cancel(tokens)
    }

    func select(_ ref: WordRef?, surface: String?) {
        guard ref != selection else { return }
        selection = ref
        self.surface = surface
        // Going to another word keeps the last one until the new one loads,
        // so open details do not collapse and reopen in between.
        if ref == nil {
            info = nil
            definition = nil
        }
        cardState = .idle
        Task { await reload() }
    }

    func reload() async {
        generation += 1
        let gen = generation
        guard let ref = selection else { return }
        loading = true
        defer { if gen == generation { loading = false } }
        do {
            let info = try await core.getWordInfo(
                bookId: ref.bookId, paragraphId: ref.paragraphId, sentenceId: ref.sentence, wordId: ref.word)
            guard gen == generation else { return }
            self.info = info
            if info == nil { definition = nil }
            if let info, info.hasCard, cardState == .idle { cardState = .exists }
            if let info, !info.isPunctuation {
                await loadDefinition(info, gen: gen)
            }
        } catch {
            guard gen == generation else { return }
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
        guard gen == generation else { return }
        definition = def ?? nil
    }

    func createCard() async {
        guard let ref = selection else { return }
        cardState = .creating
        do {
            let result = try await core.createCardForWord(
                bookId: ref.bookId, paragraphId: ref.paragraphId, sentenceId: ref.sentence, wordId: ref.word)
            if result == .alreadyExists {
                cardState = .exists
            } else {
                // A brief check, then the button goes, as in the web app.
                cardState = .created
                let ref = selection
                Task {
                    try? await Task.sleep(for: .milliseconds(1200))
                    if selection == ref, cardState == .created { cardState = .exists }
                }
            }
        } catch {
            cardState = .failed(error.userMessage)
        }
    }
}
