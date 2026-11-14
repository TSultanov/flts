import SwiftUI

/// The library as its own page: folders and books; opening a book leaves it.
struct LibraryPage: View {
    @Environment(AppModel.self) private var model
    @State private var selection: Set<UUID> = []
    @State private var moving: [BookMeta] = []
    @State private var deleting: [BookMeta] = []
    @State private var creatingFolder = false
    @State private var expanded: Set<[String]> = []

    var body: some View {
        @Bindable var model = model
        Group {
            if model.library.loaded && model.library.books.isEmpty && model.library.pendingFolders.isEmpty {
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
                    FolderContents(folder: model.library.tree, expanded: $expanded)
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
                .onDeleteCommand {
                    deleting = selection.compactMap(model.library.book)
                }
                .frame(maxWidth: 900)
                .frame(maxWidth: .infinity)
            }
        }
        .navigationTitle("Library")
        .toolbar {
            AppToolbarItems()
            ToolbarItemGroup(placement: .primaryAction) {
                Button {
                    creatingFolder = true
                } label: {
                    Label("New Folder", systemImage: "folder.badge.plus")
                }
                .help("New Folder")
                Button {
                    model.showImport = true
                } label: {
                    Label("Import", systemImage: "plus")
                }
                .help("Import a book")
            }
        }
        .sheet(isPresented: Binding(get: { !moving.isEmpty }, set: { if !$0 { moving = [] } })) {
            MoveBookSheet(books: moving)
        }
        .sheet(isPresented: $creatingFolder) {
            NewFolderSheet()
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

    private func open(_ id: UUID) {
        model.selectedBookId = id
    }
}

private struct FolderContents: View {
    let folder: LibraryFolder
    @Binding var expanded: Set<[String]>

    var body: some View {
        ForEach(folder.folders) { child in
            DisclosureGroup(isExpanded: binding(for: child.path)) {
                FolderContents(folder: child, expanded: $expanded)
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
        let paragraphs = book.paragraphsCount == 1 ? "1 paragraph" : "\(book.paragraphsCount) paragraphs"
        let translated = "\(Int((book.translationRatio * 100).rounded()))% translated"
        return "\(chapters) · \(paragraphs) · \(translated)"
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
            Picker("Folder", selection: $target) {
                ForEach(model.library.tree.allPaths, id: \.self) { path in
                    Text(path.isEmpty ? "Library" : path.joined(separator: " › ")).tag(path)
                }
            }
            TextField("Or a new folder inside it", text: $newFolder)
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
        .frame(width: 420)
        .onAppear { target = books.first?.path ?? [] }
    }
}

struct NewFolderSheet: View {
    @Environment(AppModel.self) private var model
    @Environment(\.dismiss) private var dismiss
    @State private var parent: [String] = []
    @State private var name = ""

    var body: some View {
        VStack(alignment: .leading, spacing: 16) {
            Text("New Folder")
                .font(.headline)
            Picker("Inside", selection: $parent) {
                ForEach(model.library.tree.allPaths, id: \.self) { path in
                    Text(path.isEmpty ? "Library" : path.joined(separator: " › ")).tag(path)
                }
            }
            TextField("Name", text: $name)
            Text("A folder is kept once a book is moved into it.")
                .font(.caption)
                .foregroundStyle(.secondary)
            HStack {
                Spacer()
                Button("Cancel") { dismiss() }
                    .keyboardShortcut(.cancelAction)
                Button("Create") {
                    let trimmed = name.trimmingCharacters(in: .whitespacesAndNewlines)
                    model.library.createFolder(parent + [trimmed])
                    dismiss()
                }
                .keyboardShortcut(.defaultAction)
                .disabled(name.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty)
            }
        }
        .padding(20)
        .frame(width: 380)
    }
}
