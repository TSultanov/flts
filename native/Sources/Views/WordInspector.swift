import SwiftUI

struct WordInspector: View {
    let session: BookSession
    @Environment(AppModel.self) private var model

    var body: some View {
        let word = session.word
        Group {
            if word.selection == nil {
                ContentUnavailableView(
                    "No Word Selected", systemImage: "text.cursor",
                    description: Text("Click a word to see its translation and grammar."))
            } else if let info = word.info {
                ScrollView {
                    WordDetails(info: info, session: session)
                        .padding()
                }
            } else if word.loading {
                ProgressView().frame(maxWidth: .infinity, maxHeight: .infinity)
            } else {
                ContentUnavailableView(
                    word.surface ?? "", systemImage: "character.bubble",
                    description: Text(word.error ?? "This paragraph is not translated yet."))
            }
        }
    }
}

private struct WordDetails: View {
    let info: WordInfo
    let session: BookSession
    @Environment(AppModel.self) private var model

    var body: some View {
        let word = session.word
        VStack(alignment: .leading, spacing: 16) {
            VStack(alignment: .leading, spacing: 4) {
                Text(info.original)
                    .font(.system(.title, design: .serif))
                    .textSelection(.enabled)
                if !info.grammar.originalInitialForm.isEmpty, info.grammar.originalInitialForm != info.original {
                    Text(info.grammar.originalInitialForm)
                        .font(.title3)
                        .foregroundStyle(.secondary)
                        .textSelection(.enabled)
                }
                if let def = word.definition, let t = def.transcription, !t.isEmpty {
                    Text(t).font(.callout).foregroundStyle(.secondary)
                }
            }

            if !info.contextualTranslations.isEmpty {
                Section("Translations") {
                    VStack(alignment: .leading, spacing: 4) {
                        ForEach(info.contextualTranslations, id: \.self) { t in
                            Text(t).font(.title3)
                        }
                    }
                    .textSelection(.enabled)
                }
            }

            if !info.note.isEmpty {
                Section("Note") {
                    Text(info.note).textSelection(.enabled)
                }
            }

            Section("Grammar") {
                Grid(alignment: .leading, horizontalSpacing: 12, verticalSpacing: 4) {
                    row("Part of speech", info.grammar.partOfSpeech)
                    row("Dictionary form", info.grammar.targetInitialForm)
                    row("Number", info.grammar.plurality)
                    row("Person", info.grammar.person)
                    row("Tense", info.grammar.tense)
                    row("Case", info.grammar.case)
                    row("Other", info.grammar.other)
                }
            }

            Section("Sentence") {
                Text(info.fullSentenceTranslation)
                    .textSelection(.enabled)
            }

            if info.canCreateCard {
                CardButton(state: word.cardState) {
                    Task { await word.createCard() }
                }
            }

            if let def = word.definition, !def.definition.isEmpty {
                Section("Dictionary") {
                    Text(def.definition)
                        .font(.callout)
                        .textSelection(.enabled)
                }
            }

            HStack {
                Text(info.translationModel)
                    .font(.caption)
                    .foregroundStyle(.tertiary)
                Spacer()
                if let ref = word.selection {
                    Button("Retranslate") {
                        let modelName = model.config.config?.model ?? info.translationModel
                        Task {
                            await session.chapter?.translate(
                                paragraphId: ref.paragraphId, model: modelName, useCache: false)
                        }
                    }
                    .controlSize(.small)
                    .disabled(model.activity.activity(ref.bookId, ref.paragraphId) != nil)
                    .help("Translate this paragraph again without the cache")
                }
            }
        }
        .frame(maxWidth: .infinity, alignment: .leading)
    }

    @ViewBuilder
    private func row(_ label: String, _ value: String?) -> some View {
        if let value, !value.isEmpty {
            GridRow {
                Text(label).foregroundStyle(.secondary)
                Text(value).textSelection(.enabled)
            }
        }
    }
}

private struct Section<Content: View>: View {
    let title: String
    @ViewBuilder let content: Content

    init(_ title: String, @ViewBuilder content: () -> Content) {
        self.title = title
        self.content = content()
    }

    var body: some View {
        VStack(alignment: .leading, spacing: 6) {
            Text(title)
                .font(.caption.weight(.semibold))
                .foregroundStyle(.secondary)
                .textCase(.uppercase)
            content
        }
    }
}

private struct CardButton: View {
    let state: WordStore.CardState
    let action: () -> Void

    var body: some View {
        switch state {
        case .idle:
            Button(action: action) {
                Label("Add Anki Card", systemImage: "rectangle.stack.badge.plus")
            }
        case .creating:
            ProgressView().controlSize(.small)
        case .created:
            Label("Card added", systemImage: "checkmark.circle.fill").foregroundStyle(.green)
        case .exists:
            Label("Card exists", systemImage: "checkmark.circle").foregroundStyle(.secondary)
        case .failed(let message):
            VStack(alignment: .leading) {
                Button(action: action) {
                    Label("Add Anki Card", systemImage: "rectangle.stack.badge.plus")
                }
                Text(message).font(.caption).foregroundStyle(.red)
            }
        }
    }
}
