import Foundation
import Observation

/// A folder in the library tree, built from each book's `path`.
struct LibraryFolder: Identifiable, Hashable {
    let path: [String]
    var folders: [LibraryFolder] = []
    var books: [BookMeta] = []

    var id: String { "folder:" + path.joined(separator: "/") }
    var name: String { path.last ?? "Library" }

    static func build(_ books: [BookMeta]) -> LibraryFolder {
        var root = LibraryFolder(path: [])
        for book in books {
            root.insert(book, at: book.path[...])
        }
        root.sort()
        return root
    }

    private mutating func insert(_ book: BookMeta, at rest: ArraySlice<String>) {
        guard let head = rest.first else {
            books.append(book)
            return
        }
        let childPath = path + [head]
        if let i = folders.firstIndex(where: { $0.path == childPath }) {
            folders[i].insert(book, at: rest.dropFirst())
        } else {
            var child = LibraryFolder(path: childPath)
            child.insert(book, at: rest.dropFirst())
            folders.append(child)
        }
    }

    private mutating func sort() {
        folders.sort { $0.name.localizedStandardCompare($1.name) == .orderedAscending }
        books.sort { $0.title.localizedStandardCompare($1.title) == .orderedAscending }
        for i in folders.indices { folders[i].sort() }
    }

    /// Every folder path, depth first, for move pickers.
    var allPaths: [[String]] {
        [path] + folders.flatMap(\.allPaths)
    }
}

@MainActor
@Observable
final class LibraryStore: CoreBound {
    private(set) var books: [BookMeta] = []
    private(set) var tree = LibraryFolder(path: [])
    private(set) var loaded = false
    private(set) var importProgress: ImportProgress?
    var error: String?
    /// Open folders, kept here so they survive a visit to a book.
    var expandedFolders: Set<[String]> = []

    private let events: EventHub
    private(set) var core: (any CoreService)?

    init(events: EventHub) {
        self.events = events
    }

    func attach(_ core: any CoreService) {
        self.core = core
        events.on { [weak self] event in
            switch event {
            case .libraryUpdated:
                self?.importProgress = nil
                self?.scheduleReload()
            // Emitted per saved paragraph during a chapter translation.
            case .bookUpdated:
                self?.scheduleReload()
            case .libraryImportProgress(let p):
                self?.importProgress = p.done < p.total ? p : nil
            default:
                break
            }
        }
        Task { await reload() }
    }

    private var reloadScheduled = false

    /// One `list_books` per burst of events.
    private func scheduleReload() {
        guard !reloadScheduled else { return }
        reloadScheduled = true
        Task {
            try? await Task.sleep(for: .milliseconds(300))
            reloadScheduled = false
            await reload()
        }
    }

    func reload() async {
        guard let core else { return }
        do {
            books = try await core.listBooks()
            tree = LibraryFolder.build(books)
            loaded = true
        } catch {
            self.error = error.userMessage
        }
    }

    func book(_ id: UUID?) -> BookMeta? {
        guard let id else { return nil }
        return books.first { $0.id == id }
    }

    func delete(_ ids: [UUID]) async {
        guard let core else { return }
        for id in ids {
            do {
                try await core.deleteBook(bookId: id)
            } catch {
                self.error = error.userMessage
            }
        }
        await reload()
    }

    func move(_ ids: [UUID], to path: [String]) async {
        guard let core else { return }
        for id in ids {
            do {
                try await core.moveBook(bookId: id, path: path)
            } catch {
                self.error = error.userMessage
            }
        }
        await reload()
    }

    // MARK: Import

    func parseEpub(at url: URL) async throws -> EpubPreview {
        guard let core else { throw CoreError.Failed(message: "Not ready") }
        let scoped = url.startAccessingSecurityScopedResource()
        defer { if scoped { url.stopAccessingSecurityScopedResource() } }
        return try await core.parseEpub(path: url.path)
    }

    @discardableResult
    func importEpub(_ preview: EpubPreview, sourceLanguageId: String, chapters: [Int]? = nil) async throws -> UUID {
        guard let core else { throw CoreError.Failed(message: "Not ready") }
        let all = Array(preview.chapters().indices)
        let id = try await core.importEpub(
            preview: preview, sourceLanguageId: sourceLanguageId, chapters: chapters ?? all)
        await reload()
        return id
    }

    @discardableResult
    func importText(title: String, text: String, sourceLanguageId: String) async throws -> UUID {
        guard let core else { throw CoreError.Failed(message: "Not ready") }
        let id = try await core.importPlainText(title: title, text: text, sourceLanguageId: sourceLanguageId)
        await reload()
        return id
    }
}
