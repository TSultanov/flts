import SwiftUI

struct RootView: View {
    @Environment(AppModel.self) private var model

    var body: some View {
        switch model.phase {
        case .starting:
            ProgressView("Opening library…")
                .frame(maxWidth: .infinity, maxHeight: .infinity)
                .navigationTitle("Library")
                .toolbar { LibraryToolbarItems(enabled: false, newFolder: {}) }
        case .failed(let message):
            ContentUnavailableView {
                Label("FLTS could not start", systemImage: "exclamationmark.triangle")
            } description: {
                Text(message).textSelection(.enabled)
            }
        case .running:
            if model.config.loaded && !model.config.isConfigured {
                OnboardingView()
            } else {
                MainView()
            }
        }
    }
}

/// The open book: chapters, the chapter being read and the selected word.
@MainActor
@Observable
final class BookSession {
    let book: BookStore
    let word: WordStore
    private(set) var chapter: ChapterStore?
    let controls = ReaderControls()
    private let core: any CoreService
    private let events: EventHub

    init(bookId: UUID, core: any CoreService, events: EventHub) {
        self.core = core
        self.events = events
        book = BookStore(bookId: bookId, core: core, events: events)
        word = WordStore(core: core, events: events)
    }

    func start() async {
        await book.load()
        if let id = book.currentChapter { await open(chapter: id) }
    }

    func open(chapter id: Int) async {
        guard chapter?.chapterId != id else { return }
        chapter?.close()
        book.currentChapter = id
        word.select(nil, surface: nil)
        let store = ChapterStore(bookId: book.bookId, chapterId: id, core: core, events: events)
        chapter = store
        await store.load()
    }

    func close() {
        chapter?.close()
        book.close()
        word.close()
    }
}

/// The window's content: the library page, or the open book.
struct MainView: View {
    @Environment(AppModel.self) private var model

    var body: some View {
        @Bindable var model = model
        Group {
            if let id = model.selectedBookId {
                BookPage(bookId: id)
                    .id(id)
            } else {
                LibraryPage()
            }
        }
        .sheet(isPresented: $model.showImport) {
            ImportSheet()
        }
        .onChange(of: model.library.books) { _, books in
            // A remembered book from another library (or deleted elsewhere).
            if model.library.loaded, let id = model.selectedBookId, !books.contains(where: { $0.id == id }) {
                model.selectedBookId = nil
            }
        }
        .alert(
            "Something went wrong", isPresented: .constant(model.lastError != nil),
            actions: { Button("OK") { model.lastError = nil } },
            message: { Text(model.lastError ?? "") })
    }
}

/// An open book: the reader, with the chapter list as a sidebar hidden by
/// default and the word panel along the bottom.
struct BookPage: View {
    let bookId: UUID
    @Environment(AppModel.self) private var model
    @State private var session: BookSession?
    @State private var columns = NavigationSplitViewVisibility.detailOnly
    @State private var wordExpanded = false

    @ViewBuilder
    private var reader: some View {
        if let session {
            BookReaderScreen(
                session: session, wordExpanded: $wordExpanded,
                toggleChapters: {
                    guard !singleChapter else { return }
                    columns = columns == .detailOnly ? .all : .detailOnly
                })
                .onAppear {
                    session.controls.hideChapters = { columns = .detailOnly }
                }
        } else {
            ProgressView().frame(maxWidth: .infinity, maxHeight: .infinity)
        }
    }

    @ToolbarContentBuilder
    private var toolbarItems: some ToolbarContent {
        ToolbarItem(placement: .navigation) {
            Button {
                model.selectedBookId = nil
            } label: {
                Label("Library", systemImage: "chevron.backward")
            }
            .help("Back to the library (⌥⌘L)")
        }
        AppToolbarItems()
    }

    private var singleChapter: Bool {
        guard let session, session.book.loaded else { return false }
        return session.book.chapters.count <= 1
    }

    var body: some View {
        Group {
            // A one-chapter book has no chapter list at all (as in the web app),
            // so no split view and no sidebar toggle.
            if singleChapter {
                reader
                    .toolbar { toolbarItems }
            } else {
                NavigationSplitView(columnVisibility: $columns) {
                    Group {
                        if let session {
                            ChaptersList(session: session)
                        } else {
                            ProgressView()
                        }
                    }
                    .navigationSplitViewColumnWidth(min: 180, ideal: 240, max: 360)
                } detail: {
                    reader
                        .toolbar { toolbarItems }
                }
            }
        }
        .onChange(of: columns) { _, c in session?.controls.chaptersShown = c != .detailOnly }
        .task {
            guard let core = model.core else { return }
            let next = BookSession(bookId: bookId, core: core, events: model.events)
            session = next
            await next.start()
        }
        .onDisappear { session?.close() }
    }
}
