import SwiftUI

struct ChaptersList: View {
    let session: BookSession
    @Environment(AppModel.self) private var model

    var body: some View {
        let book = session.book
        let selection = Binding<Int?>(
            get: { book.currentChapter },
            set: { id in
                guard let id else { return }
                Task { await session.open(chapter: id) }
            })
        List(selection: selection) {
            ForEach(book.chapters) { chapter in
                ChapterRow(chapter: chapter, summaryReady: summaryReady(chapter.id))
                    .tag(chapter.id)
            }
        }
        .overlay {
            if !book.loaded {
                ProgressView()
            } else if book.chapters.isEmpty {
                Text("No chapters").foregroundStyle(.secondary)
            }
        }
        .navigationTitle(model.library.book(book.bookId)?.title ?? "")
    }

    private func summaryReady(_ chapter: Int) -> Bool? {
        guard let s = session.book.summary, chapter < s.generated.count else { return nil }
        return s.generated[chapter]
    }
}

private struct ChapterRow: View {
    let chapter: ChapterMeta
    let summaryReady: Bool?

    var body: some View {
        HStack(spacing: 8) {
            TranslationRing(ratio: chapter.translationRatio)
            Text(chapter.title.isEmpty ? "Chapter \(chapter.id + 1)" : chapter.title)
                .lineLimit(2)
        }
        .padding(.vertical, 2)
        .help("\(Int((chapter.translationRatio * 100).rounded()))% translated")
    }
}

struct TranslationRing: View {
    let ratio: Double

    var body: some View {
        ZStack {
            Circle().stroke(.quaternary, lineWidth: 2)
            Circle()
                .trim(from: 0, to: ratio)
                .stroke(ratio >= 1 ? Color.green : Color.accentColor, style: StrokeStyle(lineWidth: 2, lineCap: .round))
                .rotationEffect(.degrees(-90))
        }
        .frame(width: 12, height: 12)
    }
}

struct BookReaderScreen: View {
    let session: BookSession
    @Binding var showInspector: Bool
    @Environment(AppModel.self) private var model
    private let prefs = ReaderPreferences.shared

    var body: some View {
        Group {
            if let chapter = session.chapter, chapter.loaded {
                ReaderView(
                    book: session.book, chapter: chapter, word: session.word, activity: model.activity,
                    controls: session.controls, typography: prefs.typography(language: chapter.language), model: model.config.config?.model ?? "",
                    revision: chapter.revision, activityEntries: model.activity.entries
                )
                .id(ChapterIdentity(book: session.book.bookId, chapter: chapter.chapterId))
                .overlay(alignment: .top) {
                    if let error = chapter.error {
                        ErrorBanner(message: error) { chapter.error = nil }
                    } else if let failure = model.activity.lastFailure {
                        ErrorBanner(message: "Translation failed: \(failure)") { model.activity.clearFailure() }
                    } else if let error = session.book.error {
                        ErrorBanner(message: error) { session.book.error = nil }
                    }
                }
            } else if session.chapter != nil || !session.book.loaded {
                ProgressView()
                    .frame(maxWidth: .infinity, maxHeight: .infinity)
            } else {
                ContentUnavailableView("Select a Chapter", systemImage: "list.bullet")
            }
        }
        .navigationTitle(chapterTitle)
        .navigationSubtitle(model.library.book(session.book.bookId)?.title ?? "")
        .toolbar { toolbar }
        .inspector(isPresented: $showInspector) {
            WordInspector(session: session)
                .inspectorColumnWidth(min: 240, ideal: 300, max: 420)
        }
        .focusedSceneValue(\.readerControls, session.controls)
        .focusedSceneValue(
            \.bookActions,
            BookActions(translateChapter: translateChapter, toggleInspector: { showInspector.toggle() }))
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
        guard let id = session.book.currentChapter,
            let chapter = session.book.chapters.first(where: { $0.id == id })
        else { return "" }
        return chapter.title.isEmpty ? "Chapter \(id + 1)" : chapter.title
    }

    private func translateChapter() {
        guard let id = session.book.currentChapter else { return }
        let modelName = model.config.config?.model ?? ""
        Task { await session.book.translateChapter(id, model: modelName) }
    }

    @ToolbarContentBuilder
    private var toolbar: some ToolbarContent {
        ToolbarItemGroup(placement: .navigation) {
            Button {
                session.controls.previous()
            } label: {
                Label("Previous Page", systemImage: "chevron.left")
            }
            .disabled(session.controls.page == 0)
            Button {
                session.controls.next()
            } label: {
                Label("Next Page", systemImage: "chevron.right")
            }
            .disabled(session.controls.page + 1 >= session.controls.pageCount)
        }
        ToolbarItemGroup(placement: .primaryAction) {
            if let chapter = session.chapter, chapter.loaded, chapter.untranslatedCount > 0 {
                Button(action: translateChapter) {
                    Label("Translate Chapter", systemImage: "character.bubble")
                }
                .help("Translate the \(chapter.untranslatedCount) untranslated paragraphs in this chapter")
            }
            Button {
                showInspector.toggle()
            } label: {
                Label("Word Panel", systemImage: "sidebar.right")
            }
            .help("Show or hide the word panel")
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
