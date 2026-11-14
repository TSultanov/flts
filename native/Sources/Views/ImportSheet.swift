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
            switch kind {
            case .epub: EpubImport(close: { dismiss() })
            case .text: TextImport(close: { dismiss() })
            }
        }
        .frame(width: 560, height: 520)
    }
}

private struct EpubImport: View {
    let close: () -> Void
    @Environment(AppModel.self) private var model
    @State private var picking = false
    @State private var preview: EpubPreview?
    @State private var language = "eng"
    @State private var busy = false
    @State private var error: String?

    var body: some View {
        VStack(alignment: .leading, spacing: 12) {
            if let preview {
                let chapters = preview.chapters()
                Text(preview.title()).font(.title2.weight(.semibold))
                Text("\(chapters.count) chapters").foregroundStyle(.secondary)
                List(Array(chapters.enumerated()), id: \.offset) { _, chapter in
                    HStack {
                        Text(chapter.title.isEmpty ? "Untitled" : chapter.title).lineLimit(1)
                        Spacer()
                        Text("\(chapter.paragraphCount) ¶").foregroundStyle(.secondary).monospacedDigit()
                    }
                }
                .listStyle(.bordered)
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
                Button("Cancel", action: close).keyboardShortcut(.cancelAction)
                Button("Import") { Task { await importBook() } }
                    .keyboardShortcut(.defaultAction)
                    .disabled(preview == nil || busy)
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

    private func parse(_ url: URL) async {
        busy = true
        error = nil
        defer { busy = false }
        do {
            let parsed = try await model.library.parseEpub(at: url)
            language = model.config.suggestedSourceLanguage(for: parsed.language())
            preview = parsed
        } catch {
            self.error = error.localizedDescription
        }
    }

    private func importBook() async {
        guard let preview else { return }
        busy = true
        defer { busy = false }
        do {
            let id = try await model.library.importEpub(preview, sourceLanguageId: language)
            model.selectedBookId = id
            close()
        } catch {
            self.error = error.localizedDescription
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
            let id = try await model.library.importText(
                title: title.trimmingCharacters(in: .whitespaces), text: text, sourceLanguageId: language)
            model.selectedBookId = id
            close()
        } catch {
            self.error = error.localizedDescription
        }
    }
}

struct LanguagePicker: View {
    let title: String
    @Binding var selection: String
    @Environment(AppModel.self) private var model

    var body: some View {
        Picker(title, selection: $selection) {
            ForEach(model.config.languages) { lang in
                Text(lang.label).tag(lang.id)
            }
        }
    }
}

struct OnboardingView: View {
    @Environment(AppModel.self) private var model

    var body: some View {
        VStack(spacing: 20) {
            Image(systemName: "character.book.closed")
                .font(.system(size: 48))
                .foregroundStyle(.tint)
            Text("Welcome to FLTS")
                .font(.largeTitle.weight(.semibold))
            Text("Pick the language you read in and a translation provider to get started.")
                .foregroundStyle(.secondary)
            Form {
                TranslationSettings()
            }
            .formStyle(.grouped)
            .frame(maxWidth: 520, maxHeight: 360)
        }
        .padding(40)
        .frame(maxWidth: .infinity, maxHeight: .infinity)
    }
}
