import SwiftUI

/// The library as its own page: folders and books; opening a book leaves it.
struct LibraryPage: View {
    @Environment(AppModel.self) private var model
    @State private var selection: Set<UUID> = []
    @State private var moving: [BookMeta] = []
    @State private var deleting: [BookMeta] = []
    /// The list takes the keyboard on arrival, so arrows and Return work at once.
    @FocusState private var listFocused: Bool

    #if os(iOS)
    /// Owned here and handed down, so the page sees what the Edit button sets.
    @State private var editMode = EditMode.inactive
    #endif

    /// On the Mac one selected book has its menu and Delete key, so the bar is
    /// for several. On iOS selecting is edit mode: even one book needs the bar
    /// there, and there is none outside it.
    private var selectedBooks: [BookMeta] {
        selection.compactMap(model.library.book)
    }

    /// The folder the keyboard is on, when it is the only thing selected.
    private var selectedFolder: LibraryFolder? {
        guard selection.count == 1, let id = selection.first else { return nil }
        return model.library.tree.allFolders.first { $0.selectionID == id }
    }

    /// Right opens the selected folder, Left closes it, Return toggles it.
    private func folderKey(open: Bool?) -> KeyPress.Result {
        guard let folder = selectedFolder else { return .ignored }
        set(folder, open: open)
        return .handled
    }

    private func set(_ folder: LibraryFolder, open: Bool?) {
        var expanded = model.library.expandedFolders
        if open ?? !expanded.contains(folder.path) { expanded.insert(folder.path) } else { expanded.remove(folder.path) }
        model.library.expandedFolders = expanded
    }

    private var showsSelectionBar: Bool {
        #if os(macOS)
        selectedBooks.count > 1
        #else
        editMode.isEditing && !selectedBooks.isEmpty
        #endif
    }

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
        #if os(iOS)
        .environment(\.editMode, $editMode)
        #endif
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
                    guard ids.count == 1, let id = ids.first else { return }
                    // A book opens; a folder opens or closes (on iOS a tap on its
                    // row arrives here, not through the selection).
                    if model.library.book(id) != nil {
                        open(id)
                    } else if let folder = model.library.tree.allFolders.first(where: { $0.selectionID == id }) {
                        set(folder, open: nil)
                    }
                }
                .onKeyPress(.rightArrow) { folderKey(open: true) }
                .onKeyPress(.leftArrow) { folderKey(open: false) }
                .onKeyPress(.return) { folderKey(open: nil) }
                .onDeleteKey {
                    deleting = selectedBooks
                }
                .safeAreaInset(edge: .bottom) {
                    if showsSelectionBar {
                        SelectionBar(
                            count: selectedBooks.count,
                            move: { moving = selectedBooks },
                            delete: { deleting = selectedBooks },
                            clear: { selection = [] })
                    }
                }
                #if os(iOS)
                // Leaving edit mode drops the selection it made.
                .onChange(of: editMode.isEditing) { _, editing in
                    if !editing { selection = [] }
                }
                #endif
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
            // Selectable on the Mac, so the arrow keys reach a folder, open or
            // closed. On iOS a selectable row would swallow the tap that opens it.
            #if os(macOS)
            .tag(child.selectionID)
            #endif
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

/// Picks a destination in the folder tree; new folders exist only here until
/// a book is moved into them (a folder is just its books' path).
struct MoveBookSheet: View {
    let books: [BookMeta]
    @Environment(AppModel.self) private var model
    @Environment(\.dismiss) private var dismiss
    @State private var target: [String]
    @State private var created: [[String]] = []
    @State private var naming = false
    @State private var newName = ""
    @FocusState private var focus: Field?

    private enum Field { case tree, name }

    init(books: [BookMeta]) {
        self.books = books
        let first = books.first?.path ?? []
        _target = State(initialValue: books.allSatisfy { $0.path == first } ? first : [])
    }

    var body: some View {
        #if os(macOS)
        VStack(alignment: .leading, spacing: 14) {
            VStack(alignment: .leading, spacing: 2) {
                Text("Move to Folder").font(.headline)
                Text(books.count == 1 ? books[0].title : "\(books.count) books").font(.subheadline).foregroundStyle(.secondary).lineLimit(1)
            }
            tree
                .frame(height: 260)
                .background(Color(nsColor: .textBackgroundColor), in: RoundedRectangle(cornerRadius: 8))
                .overlay { RoundedRectangle(cornerRadius: 8).strokeBorder(.separator) }
            footer
            HStack {
                Spacer()
                Button("Cancel") { dismiss() }
                    .keyboardShortcut(.cancelAction)
                // Return in the name field adds the folder; it must not also move.
                Button("Move", action: move)
                    .keyboardShortcut(naming ? nil : .defaultAction)
                    .disabled(alreadyThere)
            }
        }
        .padding(20)
        .frame(width: 440)
        .onAppear { focus = .tree }
        #else
        NavigationStack {
            VStack(spacing: 0) {
                tree
                Divider()
                footer.padding(.horizontal, 16).padding(.vertical, 10)
            }
            .navigationTitle(subject)
            .navigationBarTitleDisplayMode(.inline)
            .toolbar {
                ToolbarItem(placement: .cancellationAction) { Button("Cancel") { dismiss() } }
                ToolbarItem(placement: .confirmationAction) { Button("Move", action: move).disabled(alreadyThere) }
            }
        }
        .presentationDetents([.medium, .large])
        #endif
    }

    private var subject: String {
        books.count == 1 ? "Move “\(books[0].title)”" : "Move \(books.count) books"
    }

    private var footer: some View {
        HStack(spacing: 12) {
            Button {
                naming = true
            } label: {
                Label("New Folder", systemImage: "folder.badge.plus")
            }
            .disabled(naming)
            Spacer(minLength: 8)
            Text("Move to: /" + destination.joined(separator: "/"))
                .font(.callout)
                .foregroundStyle(.secondary)
                .lineLimit(1)
                .truncationMode(.head)
                .accessibilityIdentifier("move-destination")
        }
    }

    private var tree: some View {
        ScrollViewReader { proxy in
            ScrollView {
                VStack(alignment: .leading, spacing: 2) {
                    ForEach(paths, id: \.self) { path in
                        folderRow(path).id(path)
                        if naming && path == target { nameRow.id("naming") }
                    }
                }
                .padding(6)
            }
            .onChange(of: naming) { if naming { proxy.scrollTo("naming", anchor: .center) } }
            .onChange(of: target) { proxy.scrollTo(target) }
            .onAppear { proxy.scrollTo(target, anchor: .center) }
        }
        #if os(macOS)
        .focusable()
        .focusEffectDisabled()
        .focused($focus, equals: .tree)
        .onKeyPress(.upArrow) { step(-1) }
        .onKeyPress(.downArrow) { step(1) }
        #endif
    }

    private func folderRow(_ path: [String]) -> some View {
        let selected = path == target
        return Button {
            target = path
        } label: {
            HStack(spacing: 8) {
                Image(systemName: path.isEmpty ? "books.vertical" : "folder")
                    .foregroundStyle(selected ? Color.white : Color.accentColor)
                    .frame(width: 22)
                Text(path.last ?? "Library").lineLimit(1)
                Spacer(minLength: 8)
                if books.allSatisfy({ $0.path == path }) {
                    Text("Current").font(.caption).opacity(0.7)
                }
                if selected {
                    Image(systemName: "checkmark").font(.caption.weight(.semibold))
                }
            }
            .padding(.leading, indent(path.count))
            .padding(.trailing, 10)
            .frame(minHeight: Self.rowHeight)
            .foregroundStyle(selected ? Color.white : Color.primary)
            .background(selected ? Color.accentColor : Color.clear, in: RoundedRectangle(cornerRadius: 6))
            .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
        .accessibilityIdentifier("move-folder:" + path.joined(separator: "/"))
        .accessibilityAddTraits(selected ? .isSelected : [])
    }

    private var nameRow: some View {
        HStack(spacing: 8) {
            Image(systemName: "folder.badge.plus")
                .foregroundStyle(.secondary)
                .frame(width: 22)
            TextField("New folder name", text: $newName)
                .textFieldStyle(.roundedBorder)
                .focused($focus, equals: .name)
                .onSubmit(commitName)
                .accessibilityIdentifier("new-folder-name")
                #if os(macOS)
                .onExitCommand(perform: cancelName)
                #endif
            Button(action: cancelName) {
                Image(systemName: "xmark.circle.fill")
            }
            .buttonStyle(.plain)
            .foregroundStyle(.secondary)
            .accessibilityLabel("Cancel New Folder")
        }
        .padding(.leading, indent(target.count + 1))
        .padding(.trailing, 10)
        .frame(minHeight: Self.rowHeight)
        // Focus set before the field is in the window is lost.
        .onAppear { DispatchQueue.main.async { focus = .name } }
    }

    #if os(macOS)
    private static let rowHeight: CGFloat = 28
    #else
    private static let rowHeight: CGFloat = 44
    #endif

    private func indent(_ depth: Int) -> CGFloat { CGFloat(depth) * 18 + 8 }

    /// Every folder, the new ones included, in the library's order.
    private var paths: [[String]] {
        let all = Set(model.library.tree.allPaths).union(created)
        func walk(_ parent: [String]) -> [[String]] {
            let children = all
                .filter { $0.count == parent.count + 1 && Array($0.dropLast()) == parent }
                .sorted { $0[parent.count].localizedStandardCompare($1[parent.count]) == .orderedAscending }
            return [parent] + children.flatMap(walk)
        }
        return walk([])
    }

    private var pendingName: String {
        naming ? newName.trimmingCharacters(in: .whitespacesAndNewlines) : ""
    }

    /// A name still being typed counts, so Move does not drop it.
    private var destination: [String] {
        pendingName.isEmpty ? target : target + [pendingName]
    }

    private var alreadyThere: Bool {
        books.allSatisfy { $0.path == destination }
    }

    private func commitName() {
        let path = destination
        if !paths.contains(path) { created.append(path) }
        target = path
        cancelName()
    }

    private func cancelName() {
        naming = false
        newName = ""
        focus = .tree
    }

    private func step(_ by: Int) -> KeyPress.Result {
        let all = paths
        guard let i = all.firstIndex(of: target), all.indices.contains(i + by) else { return .handled }
        target = all[i + by]
        return .handled
    }

    private func move() {
        let path = destination
        Task { await model.library.move(books.map(\.id), to: path) }
        dismiss()
    }
}
