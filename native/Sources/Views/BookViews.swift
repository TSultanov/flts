import SwiftUI

struct ChaptersList: View {
    let session: BookSession
    /// The row the keyboard is on; a chapter opens on Return or a click.
    @State private var highlighted: Int?
    @FocusState private var focused: Bool

    private func open(_ id: Int) {
        Task {
            await session.open(chapter: id)
            // As in the web app, picking a chapter closes the list (also when
            // it is the open one); closing hands the keys back to the reader.
            session.controls.hideChapters?()
            session.controls.focus()
        }
    }

    var body: some View {
        let book = session.book
        List(selection: $highlighted) {
            ForEach(book.chapters) { chapter in
                ChapterRow(
                    chapter: chapter, summaryReady: summaryReady(chapter.id),
                    summarizing: book.summary?.activelyGenerating == chapter.id)
                    .tag(chapter.id)
                    .contentShape(Rectangle())
                    .simultaneousGesture(TapGesture().onEnded { open(chapter.id) })
            }
        }
        .focused($focused)
        .onAppear {
            highlighted = book.currentChapter
            // The list takes the keys while it is up: arrows move, Return opens.
            Task {
                try? await Task.sleep(for: .milliseconds(120))
                focused = true
            }
        }
        .onKeyPress(.return) {
            if let highlighted { open(highlighted) }
            return .handled
        }
        .onKeyPress(.escape) {
            session.controls.hideChapters?()
            return .handled
        }
        .onKeyPress(characters: CharacterSet(charactersIn: "cC"), phases: .down) { _ in
            session.controls.toggleChapters?()
            return .handled
        }
        .overlay {
            if !book.loaded {
                ProgressView()
            } else if book.chapters.isEmpty {
                Text("No chapters").foregroundStyle(.secondary)
            }
        }
    }

    private func summaryReady(_ chapter: Int) -> Bool? {
        guard let s = session.book.summary, chapter < s.generated.count else { return nil }
        return s.generated[chapter]
    }
}

private struct ChapterRow: View {
    let chapter: ChapterMeta
    let summaryReady: Bool?
    let summarizing: Bool

    var body: some View {
        HStack(spacing: 8) {
            Text(chapter.title.isEmpty ? "<no title>" : chapter.title)
                .lineLimit(2)
                .foregroundStyle(summaryReady == false ? .secondary : .primary)
            Spacer(minLength: 4)
            if summarizing {
                ProgressView()
                    .controlSize(.mini)
                    .help("Summarizing this chapter for translation context")
            }
            // As in the web reader: only unfinished chapters show a share.
            if chapter.translationRatio < 1 {
                Text("\(Int((chapter.translationRatio * 100).rounded()))%")
                    .font(.caption.monospacedDigit())
                    .foregroundStyle(.secondary)
            }
        }
        .padding(.vertical, 2)
    }
}

struct BookReaderScreen: View {
    let session: BookSession
    @Binding var wordExpanded: Bool
    let toggleChapters: () -> Void
    @Environment(AppModel.self) private var model
    private let prefs = ReaderPreferences.shared

    var body: some View {
        Group {
            if let chapter = session.chapter, chapter.loaded {
                ZStack(alignment: .bottom) {
                    ReaderView(
                        book: session.book, chapter: chapter, word: session.word, activity: model.activity,
                        controls: session.controls, typography: prefs.typography(language: chapter.language),
                        model: model.config.config?.model ?? "",
                        revision: chapter.revision, activityEntries: model.activity.entries
                    )
                    .id(ChapterIdentity(book: session.book.bookId, chapter: chapter.chapterId))
                    .padding(.bottom, WordPanel.barHeight)
                    WordPanel(session: session, expanded: $wordExpanded)
                }
                .onAppear {
                    // With no word chosen there are no details to show.
                    session.controls.toggleWordPanel = {
                        if wordExpanded || session.word.info != nil { wordExpanded.toggle() }
                    }
                    session.controls.toggleChapters = toggleChapters
                    session.controls.collapseWordPanel = {
                        guard wordExpanded else { return false }
                        wordExpanded = false
                        return true
                    }
                }
                // The panel folds away with its word.
                .onChange(of: session.word.selection) { _, ref in
                    if ref == nil { wordExpanded = false }
                }
            } else if session.chapter != nil || !session.book.loaded {
                ProgressView()
                    .frame(maxWidth: .infinity, maxHeight: .infinity)
            } else {
                if session.book.chapters.isEmpty {
                    ContentUnavailableView("This Book Could Not Be Opened", systemImage: "exclamationmark.triangle")
                } else {
                    ContentUnavailableView("Select a Chapter", systemImage: "list.bullet")
                }
            }
        }
        .overlay(alignment: .top) {
            if let error = session.chapter?.error {
                ErrorBanner(message: error) { session.chapter?.error = nil }
            } else if let failure = model.activity.lastFailure {
                ErrorBanner(message: "Translation failed: \(failure)") { model.activity.clearFailure() }
            } else if let error = session.book.error ?? session.word.error {
                ErrorBanner(message: error) {
                    session.book.error = nil
                    session.word.error = nil
                }
            }
        }
        // The book names the window; the chapter is the subtitle when there is one to name.
        .bookTitle(model.library.book(session.book.bookId)?.title ?? "", chapter: chapterTitle)
        .toolbar { toolbar }
        #if os(iOS)
        .background { ReaderKeys(controls: session.controls) }
        #endif
        .focusedSceneValue(\.readerControls, session.controls)
        .focusedSceneValue(
            \.bookActions,
            BookActions(
                translateChapter: translateChapter, toggleWordPanel: { session.controls.toggleWordPanel?() },
                toggleChapters: toggleChapters, hasChapters: session.book.chapters.count > 1,
                chaptersShown: session.controls.chaptersShown, wordDetailsShown: wordExpanded))
        .onChange(of: model.config.targetLanguage?.id) { _, id in
            session.word.targetLanguage = id ?? ""
        }
        .onAppear { session.word.targetLanguage = model.config.config?.targetLanguageId ?? "" }
    }

    private struct ChapterIdentity: Hashable {
        let book: UUID
        let chapter: Int
    }

    private var chapterTitle: String {
        guard session.book.chapters.count > 1, let id = session.book.currentChapter,
            let chapter = session.book.chapters.first(where: { $0.id == id })
        else { return "" }
        return chapter.title
    }

    private func translateChapter() {
        guard let id = session.book.currentChapter, session.book.canTranslate(id) else { return }
        let modelName = model.config.config?.model ?? ""
        Task { await session.book.translateChapter(id, model: modelName) }
    }

    @ToolbarContentBuilder
    private var toolbar: some ToolbarContent {
        ToolbarItemGroup(placement: .primaryAction) {
            if let chapter = session.chapter, chapter.loaded, chapter.untranslatedCount > 0 {
                let ready = session.book.canTranslate(chapter.chapterId)
                Button(action: translateChapter) {
                    Label("Translate Chapter", systemImage: "character.bubble")
                }
                .disabled(!ready)
                .help(ready
                    ? "Translate the \(chapter.untranslatedCount) untranslated paragraphs in this chapter"
                    : "Waiting for chapter summaries…")
            }
            #if os(iOS)
            ReadingMenu()
            #endif
        }
    }
}

struct ErrorBanner: View {
    let message: String
    let dismiss: () -> Void

    var body: some View {
        HStack(alignment: .firstTextBaseline) {
            Image(systemName: "exclamationmark.triangle.fill")
                .foregroundStyle(.yellow)
            Text(message)
                .textSelection(.enabled)
                .lineLimit(3)
            Spacer()
            Button("Dismiss", action: dismiss)
                .buttonStyle(.borderless)
        }
        .padding(10)
        .background(.regularMaterial, in: RoundedRectangle(cornerRadius: 8))
        .padding()
    }
}

#if os(iOS)
/// Text size and appearance from the reader (the Mac has menu commands).
private struct ReadingMenu: View {
    @AppStorage(Appearance.key, store: AppDefaults.store) private var appearance = Appearance.system
    private let prefs = ReaderPreferences.shared

    var body: some View {
        Menu {
            ControlGroup {
                Button {
                    prefs.fontSize -= 1
                } label: {
                    Label("Smaller Text", systemImage: "textformat.size.smaller")
                }
                Button {
                    prefs.fontSize += 1
                } label: {
                    Label("Larger Text", systemImage: "textformat.size.larger")
                }
            }
            Button("Default Size") { prefs.fontSize = ReaderPreferences.defaultFontSize }
            Picker("Appearance", selection: $appearance) {
                ForEach(Appearance.allCases) { Text($0.title).tag($0) }
            }
        } label: {
            Label("Text Size and Appearance", systemImage: "textformat.size")
        }
        // Stays open while the size is stepped.
        .menuActionDismissBehavior(.disabled)
    }
}
#endif

#if os(iOS)
/// A hardware keyboard's reader keys, as shortcuts on invisible buttons, so
/// nothing has to hold keyboard focus.
private struct ReaderKeys: View {
    let controls: ReaderControls

    var body: some View {
        Group {
            key(.rightArrow) { controls.next() }
            key(.downArrow) { controls.next() }
            key(.pageDown) { controls.next() }
            key(.space) { controls.next() }
            key(.leftArrow) { controls.previous() }
            key(.upArrow) { controls.previous() }
            key(.pageUp) { controls.previous() }
            key(.space, .shift) { controls.previous() }
            key(.home) { controls.first() }
            key(.end) { controls.last() }
            key("w") { controls.toggleWordPanel?() }
            key("c") { controls.toggleChapters?() }
            key(.escape) {
                if controls.chaptersShown { controls.hideChapters?() } else { _ = controls.collapseWordPanel?() }
            }
        }
        .opacity(0)
        .accessibilityHidden(true)
    }

    private func key(_ key: KeyEquivalent, _ modifiers: EventModifiers = [], _ action: @escaping () -> Void) -> some View {
        Button("", action: action).keyboardShortcut(key, modifiers: modifiers)
    }
}
#endif
