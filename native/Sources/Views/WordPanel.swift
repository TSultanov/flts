import SwiftUI

/// The selected word at the bottom of the reader: a one-line bar, or (as in
/// the web app) its details in place of the bar. The details grow upward over
/// the page, never resizing it, so pages do not reflow.
struct WordPanel: View {
    let session: BookSession
    @Binding var expanded: Bool
    /// Upper bound the user drags; the panel is shorter when its content is.
    @AppStorage("wordPanelHeight", store: AppDefaults.store) private var height: Double = 320
    @State private var dragStart: Double?
    @State private var contentHeight: Double = 0

    static let barHeight: CGFloat = 44
    #if os(macOS)
    static let handleHeight: CGFloat = 10
    #else
    /// Tall enough to catch a finger.
    static let handleHeight: CGFloat = 24
    #endif

    var body: some View {
        GeometryReader { geo in
            VStack(spacing: 0) {
                Spacer(minLength: 0)
                VStack(spacing: 0) {
                    if expanded, session.word.info != nil {
                        resizeHandle(maxHeight: geo.size.height * 0.75)
                        WordHeader(session: session, expanded: $expanded)
                            .frame(height: Self.barHeight)
                        Divider()
                        ScrollView {
                            WordDetails(session: session)
                                .padding(.horizontal, 16)
                                .padding(.vertical, 14)
                                .onGeometryChange(for: Double.self) { $0.size.height } action: { contentHeight = $0 }
                        }
                        .frame(height: min(contentHeight, max(height, 120), geo.size.height * 0.75))
                        WordFooter(session: session)
                    } else {
                        WordBar(session: session, expanded: $expanded)
                            .frame(height: Self.barHeight)
                    }
                }
                .wordPanelBackground()
                .overlay(alignment: .top) { Divider() }
                .shadow(color: .black.opacity(expanded ? 0.1 : 0), radius: 10, y: -3)
            }
        }
        .animation(.easeOut(duration: 0.18), value: expanded)
    }

    private func resizeHandle(maxHeight: Double) -> some View {
        Capsule()
            .fill(.tertiary)
            .frame(width: 36, height: 4)
            .frame(maxWidth: .infinity)
            .frame(height: Self.handleHeight)
            .contentShape(Rectangle())
            .gesture(
                DragGesture()
                    .onChanged { value in
                        // From the height shown, which is the content's when that is shorter.
                        let start = dragStart ?? min(height, max(contentHeight, 120))
                        dragStart = start
                        height = min(max(start - value.translation.height, 120), maxHeight)
                    }
                    .onEnded { _ in dragStart = nil }
            )
            .resizePointer()
            .accessibilityLabel("Resize word details")
    }
}

/// The word itself, heading its details.
private struct WordHeader: View {
    let session: BookSession
    @Binding var expanded: Bool

    var body: some View {
        let word = session.word
        HStack(spacing: 10) {
            if let info = word.info {
                Text(info.original)
                    .font(.system(.title2, design: .serif).weight(.semibold))
                    .textSelection(.enabled)
                if !info.grammar.originalInitialForm.isEmpty,
                    info.grammar.originalInitialForm != info.original.lowercased()
                {
                    Text("(\(info.grammar.originalInitialForm))")
                        .foregroundStyle(.secondary)
                }
                if let t = word.definition?.transcription, !t.isEmpty {
                    Text(t).foregroundStyle(.secondary)
                }
                Spacer(minLength: 8)
                if case .failed(let message) = word.cardState {
                    Text(message)
                        .font(.caption)
                        .foregroundStyle(.red)
                        .lineLimit(1)
                        .help(message)
                }
                #if os(iOS)
                DictionaryButton(term: info.original)
                #endif
                if info.canCreateCard {
                    CardButton(state: word.cardState, compact: true) {
                        Task { await word.createCard() }
                    }
                }
            } else {
                Spacer()
            }
            Button {
                expanded = false
            } label: {
                Image(systemName: "chevron.down")
                    .frame(width: 20, height: 20)
                    .touchTarget()
            }
            .buttonStyle(.borderless)
            .help("Hide word details (W)")
            .accessibilityLabel("Hide word details")
        }
        .padding(.horizontal, 16)
    }
}

/// The collapsed line: word, dictionary form, translations, note, actions.
private struct WordBar: View {
    let session: BookSession
    @Binding var expanded: Bool

    var body: some View {
        let word = session.word
        HStack(spacing: 10) {
            if let info = word.info {
                Text(info.original)
                    .font(.system(.title3, design: .serif).weight(.semibold))
                    .lineLimit(1)
                    .textSelection(.enabled)
                if !info.grammar.originalInitialForm.isEmpty,
                    info.grammar.originalInitialForm != info.original.lowercased()
                {
                    Text("(\(info.grammar.originalInitialForm))")
                        .foregroundStyle(.secondary)
                }
                if let t = word.definition?.transcription, !t.isEmpty {
                    Text(t).foregroundStyle(.secondary)
                }
                if !info.contextualTranslations.isEmpty {
                    Text("—").foregroundStyle(.tertiary)
                    Text(info.contextualTranslations.joined(separator: ", "))
                        .lineLimit(1)
                }
                if !info.note.isEmpty {
                    Text(info.note)
                        .font(.callout)
                        .foregroundStyle(.secondary)
                        .lineLimit(1)
                }
                Spacer(minLength: 8)
                if case .failed(let message) = word.cardState {
                    // Inline, as in the web app, not only in a tooltip.
                    Text(message)
                        .font(.caption)
                        .foregroundStyle(.red)
                        .lineLimit(1)
                        .help(message)
                }
                #if os(iOS)
                DictionaryButton(term: info.original)
                #endif
                if info.canCreateCard {
                    CardButton(state: word.cardState, compact: true) {
                        Task { await word.createCard() }
                    }
                }
            } else if word.selection != nil, word.loading {
                ProgressView().controlSize(.small)
                Spacer()
            } else if word.selection != nil {
                Text(word.surface ?? "")
                    .font(.system(.title3, design: .serif))
                Text("This paragraph is not translated yet.")
                    .foregroundStyle(.secondary)
                Spacer()
            } else if let chapter = session.chapter, chapter.loaded, chapter.untranslatedCount > 0,
                !session.book.canTranslate(chapter.chapterId)
            {
                // Tooltips say this on the Mac; a touch screen has none.
                ProgressView().controlSize(.small)
                Text("Preparing this chapter for translation…")
                    .foregroundStyle(.secondary)
                    .lineLimit(1)
                Spacer()
            } else {
                Text("Select a word to show its translation")
                    .foregroundStyle(.secondary)
                    .lineLimit(1)
                Spacer()
            }
            Button {
                expanded = true
            } label: {
                Image(systemName: "chevron.up")
                    .frame(width: 20, height: 20)
                    .touchTarget()
            }
            .buttonStyle(.borderless)
            .disabled(word.info == nil)
            .help("Show word details (W)")
            .accessibilityLabel("Show word details")
        }
        .padding(.horizontal, 16)
        #if os(iOS)
        // The whole bar opens the details, not only its chevron.
        .contentShape(Rectangle())
        .onTapGesture { if word.info != nil { expanded = true } }
        #endif
    }
}

/// Model and actions for the shown word, under the details.
private struct WordFooter: View {
    let session: BookSession
    @Environment(AppModel.self) private var model
    /// Defaults to the model that made the current translation, as in the web app.
    @State private var retranslateModel = ""
    @Environment(\.horizontalSizeClass) private var sizeClass

    var body: some View {
        let word = session.word
        if let info = word.info {
            HStack {
                if sizeClass != .compact {
                    Text("Translate paragraph again with")
                        .font(.callout)
                        .foregroundStyle(.secondary)
                        .fixedSize()
                }
                Picker("Model", selection: $retranslateModel) {
                    let models = model.config.models(for: model.config.config?.translationProvider ?? .google)
                    if !models.contains(where: { $0.id == retranslateModel }) {
                        Text(retranslateModel).tag(retranslateModel)
                    }
                    ForEach(models) { Text($0.name).tag($0.id) }
                }
                .labelsHidden()
                .controlSize(.small)
                .frame(maxWidth: 220)
                Spacer()
                if let ref = word.selection {
                    let activity = model.activity.activity(ref.bookId, ref.paragraphId)
                    if let activity {
                        ProgressView(
                            value: Double(activity.progressChars), total: Double(max(activity.expectedChars, 1))
                        )
                        .progressViewStyle(.circular)
                        .controlSize(.small)
                    }
                    Button("Translate Again") {
                        let chosen = retranslateModel
                        Task {
                            await session.chapter?.translate(paragraphId: ref.paragraphId, model: chosen, useCache: false)
                        }
                    }
                    .controlSize(.small)
                    .disabled(activity != nil)
                    .help("Translate this paragraph again without the cache")
                }
            }
            .onAppear { retranslateModel = info.translationModel }
            .onChange(of: info.translationModel) { _, m in retranslateModel = m }
            .padding(.horizontal, 16)
            .padding(.vertical, 8)
            .overlay(alignment: .top) { Divider() }
        }
    }
}

struct WordDetails: View {
    let session: BookSession
    @Environment(AppModel.self) private var model
    @Environment(\.horizontalSizeClass) private var sizeClass

    var body: some View {
        if let info = session.word.info {
            Group {
                // Side by side where there is room; stacked on a phone or a narrow split.
                if sizeClass == .compact {
                    VStack(alignment: .leading, spacing: 14) {
                        meaning(info)
                        reference(info)
                    }
                } else {
                    Grid(alignment: .topLeading, horizontalSpacing: 40, verticalSpacing: 14) {
                        GridRow {
                            meaning(info).frame(maxWidth: 560, alignment: .leading)
                            reference(info)
                        }
                    }
                }
            }
            .frame(maxWidth: .infinity, alignment: .leading)
        }
    }

    private func meaning(_ info: WordInfo) -> some View {
        VStack(alignment: .leading, spacing: 12) {
            DetailSection("Translations") {
                Text(info.contextualTranslations.joined(separator: ", "))
                    .font(.title3)
                    .textSelection(.enabled)
            }
            if !info.note.isEmpty {
                DetailSection("Note") { Text(info.note).textSelection(.enabled) }
            }
            DetailSection("Sentence") {
                Text(info.fullSentenceTranslation).textSelection(.enabled)
            }
        }
    }

    private func reference(_ info: WordInfo) -> some View {
        VStack(alignment: .leading, spacing: 12) {
            DetailSection("Grammar") {
                Grid(alignment: .leading, horizontalSpacing: 12, verticalSpacing: 4) {
                    row("Part of speech", info.grammar.partOfSpeech)
                    row("Initial form", info.grammar.originalInitialForm)
                    row("Number", info.grammar.plurality)
                    row("Person", info.grammar.person)
                    row("Tense", info.grammar.tense)
                    row("Case", info.grammar.grammaticalCase)
                    row("Other", info.grammar.other)
                }
            }
            .frame(minWidth: 220, alignment: .leading)
            if let def = session.word.definition, !def.definition.isEmpty {
                DetailSection("Dictionary") {
                    Text(def.definition).font(.callout).textSelection(.enabled)
                }
            }
        }
    }

    @ViewBuilder
    private func row(_ label: String, _ value: String?) -> some View {
        if let value, !value.isEmpty {
            GridRow {
                Text(label).foregroundStyle(.secondary)
                // The model's tags come as "common_noun".
                Text(value.replacingOccurrences(of: "_", with: " ")).textSelection(.enabled)
            }
        }
    }
}

private struct DetailSection<Content: View>: View {
    let title: String
    @ViewBuilder let content: Content

    init(_ title: String, @ViewBuilder content: () -> Content) {
        self.title = title
        self.content = content()
    }

    var body: some View {
        VStack(alignment: .leading, spacing: 4) {
            Text(title)
                .font(.caption.weight(.semibold))
                .foregroundStyle(.secondary)
                .textCase(.uppercase)
            content
        }
    }
}

struct CardButton: View {
    let state: WordStore.CardState
    var compact = false
    let action: () -> Void

    var body: some View {
        switch state {
        case .idle, .failed:
            let button = Button(action: action) {
                Label("Add to Anki", systemImage: "rectangle.stack.badge.plus")
            }
            .buttonStyle(.borderless)
            .help(failureMessage ?? "Add an Anki card for this word")
            .foregroundStyle(failureMessage == nil ? Color.primary : Color.red)
            if compact {
                button.labelStyle(.iconOnly)
            } else {
                button.labelStyle(.titleAndIcon)
            }
        case .creating:
            ProgressView().controlSize(.small)
        case .created:
            Image(systemName: "checkmark.circle.fill").foregroundStyle(.green).help("Card added")
        case .exists:
            EmptyView()
        }
    }

    private var failureMessage: String? {
        if case .failed(let message) = state { message } else { nil }
    }
}

#if os(iOS)
/// Opens the system dictionary for the word, as the web app does on iOS.
private struct DictionaryButton: View {
    let term: String
    @State private var shown = false

    var body: some View {
        Button {
            shown = true
        } label: {
            Image(systemName: "book").touchTarget()
        }
        .buttonStyle(.borderless)
        .accessibilityLabel("Show system dictionary")
        .sheet(isPresented: $shown) {
            DictionarySheet(term: term).ignoresSafeArea()
        }
    }
}
#endif
