import SwiftUI
import UniformTypeIdentifiers

struct ImportSheet: View {
    enum Kind: String, CaseIterable, Identifiable {
        case epub = "EPUB"
        case text = "Text"
        var id: String { rawValue }
    }

    @Environment(\.dismiss) private var dismiss
    @State private var kind = Kind.epub

    var body: some View {
        VStack(spacing: 0) {
            Picker("", selection: $kind) {
                ForEach(Kind.allCases) { Text($0.rawValue).tag($0) }
            }
            .pickerStyle(.segmented)
            .labelsHidden()
            .frame(width: 220)
            .padding()
            Divider()
            // Both stay alive, so switching does not discard a parsed book or typed text.
            ZStack {
                EpubImport(close: { dismiss() })
                    .opacity(kind == .epub ? 1 : 0)
                    .disabled(kind != .epub)
                TextImport(close: { dismiss() })
                    .opacity(kind == .text ? 1 : 0)
                    .disabled(kind != .text)
            }
        }
        .sheetSize(width: 560, height: 520)
    }
}

private struct EpubImport: View {
    let close: () -> Void
    @Environment(AppModel.self) private var model
    @State private var picking = false
    @State private var preview: EpubPreview?
    @State private var chapters: [EpubChapterSummary] = []
    /// Chosen chapter indices; empty chapters cannot be chosen.
    @State private var chosen: Set<Int> = []
    @State private var language = "eng"
    @State private var busy = false
    @State private var error: String?

    var body: some View {
        VStack(alignment: .leading, spacing: 12) {
            if let preview {
                Text(preview.title()).font(.title2.weight(.semibold))
                HStack {
                    Text("Select chapters to import").foregroundStyle(.secondary)
                    Spacer()
                    Button(chosen.count == importable.count ? "Select None" : "Select All") {
                        chosen = chosen.count == importable.count ? [] : Set(importable)
                    }
                    .buttonStyle(.borderless)
                }
                List {
                    ForEach(Array(chapters.enumerated()), id: \.offset) { index, chapter in
                        ChapterChoice(
                            preview: preview, index: index, chapter: chapter,
                            isOn: Binding(
                                get: { chosen.contains(index) },
                                set: { on in if on { chosen.insert(index) } else { chosen.remove(index) } }))
                    }
                }
                .borderedList()
                LanguagePicker(title: "Book language", selection: $language)
            } else {
                Spacer()
                VStack(spacing: 12) {
                    Image(systemName: "book.pages").font(.system(size: 40)).foregroundStyle(.secondary)
                    Button("Choose EPUB…") { picking = true }
                        .controlSize(.large)
                    if busy { ProgressView().controlSize(.small) }
                }
                .frame(maxWidth: .infinity)
                Spacer()
            }
            if let error {
                Text(error).foregroundStyle(.red).font(.callout).textSelection(.enabled)
            }
            HStack {
                if preview != nil {
                    Button("Choose Another…") { picking = true }
                }
                Spacer()
                if busy, preview != nil { ProgressView().controlSize(.small) }
                Button("Cancel", action: close).keyboardShortcut(.cancelAction)
                Button("Import") { Task { await importBook() } }
                    .keyboardShortcut(.defaultAction)
                    .disabled(preview == nil || busy || chosen.isEmpty)
            }
        }
        .padding()
        .fileImporter(isPresented: $picking, allowedContentTypes: [UTType(filenameExtension: "epub") ?? .data]) {
            result in
            guard case .success(let url) = result else { return }
            Task { await parse(url) }
        }
        .dropDestination(for: URL.self) { urls, _ in
            guard let url = urls.first(where: { $0.pathExtension.lowercased() == "epub" }) else { return false }
            Task { await parse(url) }
            return true
        }
    }

    private var importable: [Int] {
        chapters.indices.filter { chapters[$0].paragraphCount > 0 }
    }

    private func parse(_ url: URL) async {
        busy = true
        error = nil
        defer { busy = false }
        // A file picked outside the app's container is readable only inside this scope (iOS).
        let scoped = url.startAccessingSecurityScopedResource()
        defer { if scoped { url.stopAccessingSecurityScopedResource() } }
        do {
            let parsed = try await model.library.parseEpub(at: url)
            language = model.config.suggestedSourceLanguage(for: parsed.language())
            chapters = parsed.chapters()
            chosen = Set(importable)
            preview = parsed
        } catch {
            log.error("EPUB parse failed: \(error.userMessage, privacy: .public)")
            self.error = "Could not load this file. Please choose a valid EPUB."
        }
    }

    private func importBook() async {
        guard let preview else { return }
        busy = true
        defer { busy = false }
        do {
            _ = try await model.library.importEpub(preview, sourceLanguageId: language, chapters: chosen.sorted())
            // As in the web app: back to the library, where the book now is.
            model.selectedBookId = nil
            close()
        } catch {
            self.error = error.userMessage
        }
    }
}

/// One chapter: a checkbox (none for an empty chapter) that expands to its text.
private struct ChapterChoice: View {
    let preview: EpubPreview
    let index: Int
    let chapter: EpubChapterSummary
    @Binding var isOn: Bool
    @State private var open = false

    var body: some View {
        DisclosureGroup(isExpanded: $open) {
            VStack(alignment: .leading, spacing: 6) {
                ForEach(Array(preview.chapterParagraphs(index: index).prefix(40).enumerated()), id: \.offset) { _, p in
                    Text(p).font(.system(.callout, design: .serif)).foregroundStyle(.secondary)
                }
            }
            .padding(.vertical, 4)
        } label: {
            HStack {
                Toggle(isOn: $isOn) {
                    Text(chapter.title.isEmpty ? "<no title>" : chapter.title).lineLimit(1)
                }
                .disabled(chapter.paragraphCount == 0)
                Spacer()
                Text(chapter.paragraphCount == 0 ? "empty" : "\(chapter.paragraphCount) ¶")
                    .foregroundStyle(.secondary)
                    .monospacedDigit()
            }
        }
    }
}

private struct TextImport: View {
    let close: () -> Void
    @Environment(AppModel.self) private var model
    @State private var title = ""
    @State private var text = ""
    @State private var language = "eng"
    @State private var busy = false
    @State private var error: String?

    var body: some View {
        VStack(alignment: .leading, spacing: 12) {
            TextField("Title", text: $title)
                .textFieldStyle(.roundedBorder)
            TextEditor(text: $text)
                .font(.system(.body, design: .serif))
                .scrollContentBackground(.hidden)
                .padding(4)
                .background(.background, in: RoundedRectangle(cornerRadius: 6))
                .overlay(RoundedRectangle(cornerRadius: 6).stroke(.quaternary))
            LanguagePicker(title: "Text language", selection: $language)
            if let error {
                Text(error).foregroundStyle(.red).font(.callout)
            }
            HStack {
                Spacer()
                Button("Cancel", action: close).keyboardShortcut(.cancelAction)
                Button("Import") { Task { await importText() } }
                    .keyboardShortcut(.defaultAction)
                    .disabled(busy || title.trimmingCharacters(in: .whitespaces).isEmpty
                        || text.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty)
            }
        }
        .padding()
    }

    private func importText() async {
        busy = true
        defer { busy = false }
        do {
            _ = try await model.library.importText(
                title: title.trimmingCharacters(in: .whitespaces), text: text, sourceLanguageId: language)
            model.selectedBookId = nil
            close()
        } catch {
            self.error = error.userMessage
        }
    }
}

struct LanguagePicker: View {
    let title: String
    @Binding var selection: String
    @Environment(AppModel.self) private var model

    var body: some View {
        #if os(macOS)
        picker
        #else
        // Outside a form, an iOS picker shows only its value.
        HStack {
            Text(title)
            Spacer()
            picker
        }
        #endif
    }

    private var picker: some View {
        Picker(title, selection: $selection) {
            ForEach(model.config.languages) { lang in
                Text(lang.label).tag(lang.id)
            }
        }
    }
}

struct OnboardingView: View {
    @Environment(AppModel.self) private var model
    @State private var tried = false

    var body: some View {
        VStack(spacing: 14) {
            HStack(spacing: 12) {
                Image(systemName: "character.book.closed")
                    .font(.system(size: 34))
                    .foregroundStyle(.tint)
                Text("Welcome to FLTS")
                    .font(.largeTitle.weight(.semibold))
            }
            Text("Pick the language you want translations in and a translation provider.")
                .foregroundStyle(.secondary)
            Form {
                TranslationSettings(essentialsOnly: true)
            }
            .formStyle(.grouped)
            // About its content's height, so Continue sits right under the fields.
            // iOS rows are taller, and the keyboard needs the form to scroll.
            #if os(macOS)
            .frame(maxWidth: 520, maxHeight: 165)
            #else
            .frame(maxWidth: 520, maxHeight: 260)
            .scrollContentBackground(.hidden)
            #endif
            Button("Continue") {
                // Fields save on Return or losing focus; ending editing commits the last one.
                Platform.endEditing()
                tried = true
            }
            .keyboardShortcut(.defaultAction)
            .controlSize(.large)
            .buttonStyle(.borderedProminent)
            if tried, let missing {
                Text(missing)
                    .foregroundStyle(.red)
                    .font(.callout)
            }
            #if os(macOS)
            Text("You can change these later in Settings (⌘,).")
                .font(.caption)
                .foregroundStyle(.secondary)
            #else
            Text("You can change these later in Settings.")
                .font(.caption)
                .foregroundStyle(.secondary)
            #endif
        }
        .padding(24)
        .frame(maxWidth: .infinity, maxHeight: .infinity)
    }

    /// Why the app cannot start yet; nil once it can (the page then goes away).
    private var missing: String? {
        guard let config = model.config.config else { return nil }
        if config.targetLanguageId.isEmpty { return "Choose the language to translate into." }
        if !config.hasApiKeyForProvider {
            return "Paste an API key for \(model.config.provider(config.translationProvider)?.name ?? "the provider")."
        }
        return nil
    }
}
