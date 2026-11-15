import SwiftUI

/// The library as its own page: folders and books; opening a book leaves it.
struct LibraryPage: View {
    @Environment(AppModel.self) private var model
    @State private var selection: Set<UUID> = []
    @State private var moving: [BookMeta] = []
    @State private var deleting: [BookMeta] = []
    /// The list takes the keyboard on arrival, so arrows and Return work at once.
    @FocusState private var listFocused: Bool

    /// On the Mac one selected book has its menu and Delete key; on iOS
    /// selecting is edit mode, where even one book needs the bar.
    #if os(macOS)
    private static let selectionBarThreshold = 1
    #else
    private static let selectionBarThreshold = 0
    #endif

    var body: some View {
        @Bindable var model = model
        VStack(spacing: 0) {
            // Above the list, so they show even when the library becomes empty.
            if let error = model.library.error {
                NoticeBar(text: error, symbol: "exclamationmark.triangle", tint: .red) { model.library.error = nil }
            }
            if let notice = model.deletedBookNotice {
                NoticeBar(text: notice, symbol: "trash", tint: .secondary) { model.deletedBookNotice = nil }
            }
            content
        }
        .modifier(LibraryChrome(
            moving: $moving, deleting: $deleting, selection: $selection))
    }

    @ViewBuilder
    private var content: some View {
        @Bindable var model = model
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
                List(selection: $selection) {
                    if let progress = model.library.importProgress {
                        Section {
                            ProgressView(value: Double(progress.done), total: Double(max(progress.total, 1))) {
                                Text("Importing library…")
                            } currentValueLabel: {
                                Text("\(progress.done) of \(progress.total)")
                            }
                            .padding(.vertical, 4)
                        }
                    }
                    FolderContents(
                        folder: model.library.tree,
                        expanded: Binding(
                            get: { model.library.expandedFolders }, set: { model.library.expandedFolders = $0 }))
                }
                .listStyle(.inset)
                .contextMenu(forSelectionType: UUID.self) { ids in
                    let books = ids.compactMap(model.library.book)
                    if !books.isEmpty {
                        if books.count == 1 { Button("Open") { open(books[0].id) } }
                        Divider()
                        Button(books.count == 1 ? "Move to Folder…" : "Move \(books.count) Books to Folder…") {
                            moving = books
                        }
                        Button(books.count == 1 ? "Delete…" : "Delete \(books.count) Books…", role: .destructive) {
                            deleting = books
                        }
                    }
                } primaryAction: { ids in
                    if ids.count == 1, let id = ids.first { open(id) }
                }
                .onDeleteKey {
                    deleting = selection.compactMap(model.library.book)
                }
                .safeAreaInset(edge: .bottom) {
                    if selection.count > Self.selectionBarThreshold {
                        SelectionBar(
                            count: selection.count,
                            move: { moving = selection.compactMap(model.library.book) },
                            delete: { deleting = selection.compactMap(model.library.book) },
                            clear: { selection = [] })
                    }
                }
                .focused($listFocused)
                .defaultFocus($listFocused, true)
                .task {
                    // Once the list is in the window; earlier the request is dropped.
                    try? await Task.sleep(for: .milliseconds(150))
                    listFocused = true
                }
                .frame(maxWidth: 900)
                .frame(maxWidth: .infinity)
        }
    }

    private func open(_ id: UUID) {
        model.selectedBookId = id
    }
}

/// The library page's title, toolbar, sheets and confirmations.
private struct LibraryChrome: ViewModifier {
    @Environment(AppModel.self) private var model
    @Binding var moving: [BookMeta]
    @Binding var deleting: [BookMeta]
    @Binding var selection: Set<UUID>

    func body(content: Content) -> some View {
        content
            .navigationTitle("Library")
                .toolbar { LibraryToolbarItems() }
                .sheet(isPresented: Binding(get: { !moving.isEmpty }, set: { if !$0 { moving = [] } })) {
                MoveBookSheet(books: moving)
            }
                .confirmationDialog(
                deleting.count == 1 ? "Delete “\(deleting[0].title)”?" : "Delete \(deleting.count) books?",
                isPresented: Binding(get: { !deleting.isEmpty }, set: { if !$0 { deleting = [] } }),
                titleVisibility: .visible
            ) {
                Button("Delete", role: .destructive) {
                    let ids = deleting.map(\.id)
                    Task { await model.library.delete(ids) }
                    selection.subtract(ids)
                    deleting = []
                }
                Button("Cancel", role: .cancel) { deleting = [] }
            } message: {
                Text("The book and its translations are removed from every synced device.")
            }
    }
}

/// The library's toolbar. Also shown while the library opens, so the toolbar
/// is not rebuilt when it does.
struct LibraryToolbarItems: ToolbarContent {
    var enabled = true
    @Environment(AppModel.self) private var model

    var body: some ToolbarContent {
        #if os(iOS)
        // Selecting several books (to move or delete them) needs edit mode on iOS.
        ToolbarItem(placement: .topBarLeading) {
            EditButton().disabled(!enabled)
        }
        #endif
        AppToolbarItems()
        ToolbarItemGroup(placement: .primaryAction) {
            Button {
                model.showImport = true
            } label: {
                Label("Import", systemImage: "plus")
            }
            .help("Import a book")
            .disabled(!enabled)
        }
    }
}

/// A dismissible line above the library list.
private struct NoticeBar: View {
    let text: String
    let symbol: String
    let tint: Color
    let dismiss: () -> Void

    var body: some View {
        HStack {
            Label(text, systemImage: symbol).foregroundStyle(tint)
            Spacer()
            Button("Dismiss", action: dismiss).buttonStyle(.borderless)
        }
        .padding(.horizontal, 16)
        .padding(.vertical, 8)
        .background(.bar)
        .overlay(alignment: .bottom) { Divider() }
    }
}

private struct FolderContents: View {
    let folder: LibraryFolder
    @Binding var expanded: Set<[String]>

    var body: some View {
        ForEach(folder.folders) { child in
            DisclosureGroup(isExpanded: binding(for: child.path)) {
                // A full level in, so a folder's books do not read as top-level ones.
                FolderContents(folder: child, expanded: $expanded)
                    .padding(.leading, 20)
            } label: {
                Label(child.name, systemImage: "folder")
                    .font(.headline)
            }
        }
        ForEach(folder.books) { book in
            BookRow(book: book)
                .tag(book.id)
        }
    }

    private func binding(for path: [String]) -> Binding<Bool> {
        Binding(
            get: { expanded.contains(path) },
            set: { open in
                if open { expanded.insert(path) } else { expanded.remove(path) }
            })
    }
}

private struct BookRow: View {
    let book: BookMeta

    var body: some View {
        HStack(spacing: 12) {
            Image(systemName: "book.closed")
                .font(.title2)
                .foregroundStyle(.secondary)
                .frame(width: 28)
            VStack(alignment: .leading, spacing: 3) {
                Text(book.title)
                    .font(.body.weight(.medium))
                    .lineLimit(2)
                Text(counts)
                    .font(.caption)
                    .foregroundStyle(.secondary)
            }
            Spacer()
        }
        .padding(.vertical, 4)
        .help(book.title)
    }

    private var counts: String {
        let chapters = book.chaptersCount == 1 ? "1 chapter" : "\(book.chaptersCount) chapters"
        // As in the web app: a finished book shows no share.
        guard book.translationRatio < 1 else { return chapters }
        return "\(chapters) · \(Int((book.translationRatio * 100).rounded()))% translated"
    }
}

/// Shown while several books are selected, so batch actions are visible.
private struct SelectionBar: View {
    let count: Int
    let move: () -> Void
    let delete: () -> Void
    let clear: () -> Void

    var body: some View {
        HStack {
            Text("\(count) selected").foregroundStyle(.secondary)
            Spacer()
            Button("Move…", action: move)
            Button("Delete…", role: .destructive, action: delete)
            Button("Clear Selection", action: clear)
        }
        .padding(.horizontal, 16)
        .padding(.vertical, 8)
        .background(.bar)
        .overlay(alignment: .top) { Divider() }
    }
}

struct MoveBookSheet: View {
    let books: [BookMeta]
    @Environment(AppModel.self) private var model
    @Environment(\.dismiss) private var dismiss
    @State private var target: [String] = []
    @State private var newFolder = ""

    var body: some View {
        VStack(alignment: .leading, spacing: 16) {
            Text(books.count == 1 ? "Move “\(books[0].title)”" : "Move \(books.count) books")
                .font(.headline)
            #if os(macOS)
            folderPicker
            #else
            // Outside a form, an iOS picker shows only its value.
            HStack {
                Text("Folder")
                Spacer()
                folderPicker
            }
            #endif
            TextField("Or a new folder inside it", text: $newFolder)
                #if os(iOS)
                .textFieldStyle(.roundedBorder)
                #endif
            Text("Move to: " + destinationLabel)
                .font(.callout)
                .foregroundStyle(.secondary)
            HStack {
                Spacer()
                Button("Cancel") { dismiss() }
                    .keyboardShortcut(.cancelAction)
                Button("Move") {
                    let name = newFolder.trimmingCharacters(in: .whitespacesAndNewlines)
                    let path = name.isEmpty ? target : target + [name]
                    Task { await model.library.move(books.map(\.id), to: path) }
                    dismiss()
                }
                .keyboardShortcut(.defaultAction)
            }
        }
        .padding(20)
        .sheetSize(width: 420)
    }

    private var folderPicker: some View {
        Picker("Folder", selection: $target) {
            ForEach(model.library.tree.allPaths, id: \.self) { path in
                Text(path.isEmpty ? "Library" : path.joined(separator: " › ")).tag(path)
            }
        }
    }

    private var destinationLabel: String {
        let name = newFolder.trimmingCharacters(in: .whitespacesAndNewlines)
        let path = name.isEmpty ? target : target + [name]
        return "/" + path.joined(separator: "/")
    }
}
