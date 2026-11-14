import SwiftUI

struct LibrarySidebar: View {
    @Environment(AppModel.self) private var model
    @State private var moving: BookMeta?
    @State private var deleting: BookMeta?
    @State private var creatingFolder = false
    @State private var expanded: Set<[String]> = []

    var body: some View {
        @Bindable var model = model
        List(selection: $model.selectedBookId) {
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
            Section("Library") {
                FolderContents(
                    folder: model.library.tree, expanded: $expanded, moving: $moving, deleting: $deleting)
            }
        }
        .listStyle(.sidebar)
        .overlay {
            if model.library.loaded && model.library.books.isEmpty && model.library.pendingFolders.isEmpty {
                Text("No books yet")
                    .foregroundStyle(.secondary)
            }
        }
        .toolbar {
            ToolbarItemGroup {
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
        .sheet(item: $moving) { book in
            MoveBookSheet(book: book)
        }
        .sheet(isPresented: $creatingFolder) {
            NewFolderSheet()
        }
        .confirmationDialog(
            "Delete “\(deleting?.title ?? "")”?", isPresented: .constant(deleting != nil), titleVisibility: .visible
        ) {
            Button("Delete", role: .destructive) {
                if let book = deleting {
                    Task { await model.library.delete([book.id]) }
                }
                deleting = nil
            }
            Button("Cancel", role: .cancel) { deleting = nil }
        } message: {
            Text("The book and its translations are removed from every synced device.")
        }
    }
}

private struct FolderContents: View {
    let folder: LibraryFolder
    @Binding var expanded: Set<[String]>
    @Binding var moving: BookMeta?
    @Binding var deleting: BookMeta?

    var body: some View {
        ForEach(folder.folders) { child in
            DisclosureGroup(isExpanded: binding(for: child.path)) {
                FolderContents(folder: child, expanded: $expanded, moving: $moving, deleting: $deleting)
            } label: {
                Label(child.name, systemImage: "folder")
            }
        }
        ForEach(folder.books) { book in
            BookRow(book: book)
                .tag(book.id)
                .contextMenu {
                    Button("Move to Folder…") { moving = book }
                    Divider()
                    Button("Delete…", role: .destructive) { deleting = book }
                }
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
        HStack(spacing: 8) {
            Image(systemName: "book.closed")
                .foregroundStyle(.secondary)
            VStack(alignment: .leading, spacing: 2) {
                Text(book.title)
                    .lineLimit(2)
                Text("\(book.chaptersCount) chapters · \(Int((book.translationRatio * 100).rounded()))% translated")
                    .font(.caption)
                    .foregroundStyle(.secondary)
            }
        }
        .padding(.vertical, 2)
        .help(book.title)
    }
}

struct MoveBookSheet: View {
    let book: BookMeta
    @Environment(AppModel.self) private var model
    @Environment(\.dismiss) private var dismiss
    @State private var target: [String] = []
    @State private var newFolder = ""

    var body: some View {
        VStack(alignment: .leading, spacing: 16) {
            Text("Move “\(book.title)”")
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
                    Task { await model.library.move([book.id], to: path) }
                    dismiss()
                }
                .keyboardShortcut(.defaultAction)
            }
        }
        .padding(20)
        .frame(width: 420)
        .onAppear { target = book.path }
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
