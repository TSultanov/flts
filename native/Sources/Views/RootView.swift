import SwiftUI

struct RootView: View {
    @Environment(AppModel.self) private var model

    var body: some View {
        switch model.phase {
        case .starting:
            ProgressView("Opening library…")
                .frame(maxWidth: .infinity, maxHeight: .infinity)
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

struct MainView: View {
    @Environment(AppModel.self) private var model
    @State private var session: BookSession?
    @State private var showInspector = true
    @State private var columns = NavigationSplitViewVisibility.all

    var body: some View {
        @Bindable var model = model
        NavigationSplitView(columnVisibility: $columns) {
            LibrarySidebar()
                .navigationSplitViewColumnWidth(min: 200, ideal: 240, max: 360)
        } content: {
            Group {
                if let session {
                    ChaptersList(session: session)
                } else {
                    ContentUnavailableView("No Book Selected", systemImage: "books.vertical")
                }
            }
            .navigationSplitViewColumnWidth(min: 180, ideal: 230, max: 340)
        } detail: {
            Group {
                if let session {
                    BookReaderScreen(session: session, showInspector: $showInspector)
                } else {
                    LibraryOverview()
                }
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
        .task(id: model.selectedBookId) {
            session?.close()
            session = nil
            guard let id = model.selectedBookId, let core = model.core else { return }
            let next = BookSession(bookId: id, core: core, events: model.events)
            session = next
            await next.start()
        }
        .alert(
            "Something went wrong", isPresented: .constant(model.lastError != nil),
            actions: { Button("OK") { model.lastError = nil } },
            message: { Text(model.lastError ?? "") })
    }
}

struct LibraryOverview: View {
    @Environment(AppModel.self) private var model

    var body: some View {
        if model.library.loaded && model.library.books.isEmpty {
            ContentUnavailableView {
                Label("Your Library Is Empty", systemImage: "books.vertical")
            } description: {
                Text("Import an EPUB or paste a text to start reading.")
            } actions: {
                Button("Import Book…") { model.showImport = true }
                    .buttonStyle(.borderedProminent)
            }
        } else {
            ContentUnavailableView("Select a Book", systemImage: "book")
        }
    }
}
