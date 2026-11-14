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
    /// Folders made in this session that hold no book yet.
    var pendingFolders: [[String]] = []
    var error: String?

    private let events: EventHub
    private(set) var core: (any CoreService)?

    init(events: EventHub) {
        self.events = events
    }

    func attach(_ core: any CoreService) {
        self.core = core
        events.on("library_updated") { [weak self] _ in
            self?.importProgress = nil
            self?.scheduleReload()
        }
        // Emitted per saved paragraph during a chapter translation.
        events.on("book_updated", as: UUID.self) { [weak self] _ in
            self?.scheduleReload()
        }
        events.on("library_import_progress", as: ImportProgress.self) { [weak self] p in
            self?.importProgress = p.done < p.total ? p : nil
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
            books = try await core.invoke("list_books")
            var tree = LibraryFolder.build(books)
            for path in pendingFolders where !tree.allPaths.contains(path) {
                tree = tree.addingEmpty(path)
            }
            self.tree = tree
            loaded = true
        } catch {
            self.error = error.localizedDescription
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
                try await core.call("delete_book", ["bookId": .uuid(id)])
            } catch {
                self.error = error.localizedDescription
            }
        }
        await reload()
    }

    func move(_ ids: [UUID], to path: [String]) async {
        guard let core else { return }
        for id in ids {
            do {
                try await core.call("move_book", ["bookId": .uuid(id), "path": .strings(path)])
            } catch {
                self.error = error.localizedDescription
            }
        }
        pendingFolders.removeAll { $0 == path }
        await reload()
    }

    func createFolder(_ path: [String]) {
        guard !path.isEmpty, !tree.allPaths.contains(path) else { return }
        pendingFolders.append(path)
        tree = tree.addingEmpty(path)
    }

    // MARK: Import

    func parseEpub(at url: URL) async throws -> EpubPreview {
        guard let core else { throw CoreFailure(message: "Not ready") }
        let scoped = url.startAccessingSecurityScopedResource()
        defer { if scoped { url.stopAccessingSecurityScopedResource() } }
        let data = try Data(contentsOf: url)
        let raw: JSONValue = try await core.invoke(
            "parse_epub", ["epubBase64": .string(data.base64EncodedString())])
        let summary = try JSONDecoder().decode(EpubPreview.Summary.self, from: JSONEncoder().encode(raw))
        return EpubPreview(raw: raw, summary: summary)
    }

    @discardableResult
    func importEpub(_ preview: EpubPreview, sourceLanguageId: String) async throws -> UUID {
        guard let core else { throw CoreFailure(message: "Not ready") }
        let id: UUID = try await core.invoke(
            "import_epub", ["book": preview.raw, "sourceLanguageId": .string(sourceLanguageId)])
        await reload()
        return id
    }

    @discardableResult
    func importText(title: String, text: String, sourceLanguageId: String) async throws -> UUID {
        guard let core else { throw CoreFailure(message: "Not ready") }
        let id: UUID = try await core.invoke(
            "import_plain_text",
            ["title": .string(title), "text": .string(text), "sourceLanguageId": .string(sourceLanguageId)])
        await reload()
        return id
    }
}

extension LibraryFolder {
    func addingEmpty(_ target: [String]) -> LibraryFolder {
        guard target.count > path.count, Array(target.prefix(path.count)) == path else { return self }
        var copy = self
        let childPath = Array(target.prefix(path.count + 1))
        if let i = copy.folders.firstIndex(where: { $0.path == childPath }) {
            copy.folders[i] = copy.folders[i].addingEmpty(target)
        } else {
            copy.folders.append(LibraryFolder(path: childPath).addingEmpty(target))
        }
        return copy
    }
}
