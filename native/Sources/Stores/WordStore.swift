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
            events.on("book_updated", as: UUID.self) { [weak self] id in
                guard let self, id == self.selection?.bookId else { return }
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
        info = nil
        definition = nil
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
            let info: WordInfo? = try await core.invoke(
                "get_word_info",
                [
                    "bookId": .uuid(ref.bookId), "paragraphId": .int(ref.paragraphId),
                    "sentenceId": .int(ref.sentence), "wordId": .int(ref.word),
                ], as: WordInfo?.self)
            guard gen == generation else { return }
            self.info = info
            if let info, info.hasCard, cardState == .idle { cardState = .exists }
            if let info, !info.isPunctuation {
                await loadDefinition(info, gen: gen)
            }
        } catch {
            guard gen == generation else { return }
            self.error = error.localizedDescription
        }
    }

    private func loadDefinition(_ info: WordInfo, gen: Int) async {
        let word = info.original
        let def = try? await core.invoke(
            "get_system_definition",
            [
                "word": .string(word), "sourceLang": .string(info.sourceLanguage.isEmpty ? "en" : info.sourceLanguage),
                "targetLang": .string(targetLanguage.isEmpty ? "en" : targetLanguage),
            ], as: SystemDefinition?.self)
        guard gen == generation else { return }
        definition = def ?? nil
    }

    func createCard() async {
        guard let ref = selection else { return }
        cardState = .creating
        do {
            let result: String = try await core.invoke(
                "create_card_for_word",
                [
                    "bookId": .uuid(ref.bookId), "paragraphId": .int(ref.paragraphId),
                    "sentenceId": .int(ref.sentence), "wordId": .int(ref.word),
                ])
            cardState = result == "alreadyExists" ? .exists : .created
        } catch {
            cardState = .failed(error.localizedDescription)
        }
    }
}
