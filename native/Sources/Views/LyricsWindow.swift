import SwiftUI

struct LyricsWindow: View {
    @Environment(AppModel.self) private var model

    var body: some View {
        let lyrics = model.lyrics
        VStack(spacing: 0) {
            NowPlayingCard()
                .padding()
            Divider()
            if lyrics.nowPlaying?.state == .notRunning {
                ContentUnavailableView(
                    "Spotify Is Not Running", systemImage: "music.note",
                    description: Text("Open Spotify and play a song to see its lyrics translated."))
            } else if lyrics.nowPlaying?.trackId == nil || lyrics.nowPlaying?.state == .stopped {
                ContentUnavailableView(
                    "Spotify Is Stopped", systemImage: "music.note",
                    description: Text("Play a song in Spotify to see its lyrics translated."))
            } else if !lyrics.lyricsResolved {
                ProgressView("Fetching lyrics…").frame(maxWidth: .infinity, maxHeight: .infinity)
            } else if let current = lyrics.lyrics {
                if current.lines.allSatisfy({ $0.text.trimmingCharacters(in: .whitespaces).isEmpty }) {
                    ContentUnavailableView("Lyrics Are Empty", systemImage: "text.badge.xmark")
                } else {
                    if !current.synced {
                        Label("Plain lyrics only — karaoke sync unavailable", systemImage: "exclamationmark.triangle")
                            .font(.callout)
                            .foregroundStyle(.orange)
                            .padding(.top, 8)
                    }
                    LyricsList(lyrics: current)
                }
            } else {
                ContentUnavailableView(
                    "No Lyrics Found", systemImage: "text.badge.xmark",
                    description: Text("LRCLIB has no lyrics for this track."))
            }
            StatusLine(phase: lyrics.phase)
            if let error = lyrics.error {
                ErrorBanner(message: error) { lyrics.error = nil }
            }
        }
        .background(Color(nsColor: .windowBackgroundColor))
        .task {
            lyrics.targetLanguage = model.config.config?.targetLanguageId ?? ""
            lyrics.model = model.config.config?.model ?? ""
            await lyrics.start()
        }
        .onDisappear { Task { await lyrics.stop() } }
        .onChange(of: model.config.config?.targetLanguageId) { _, id in
            lyrics.targetLanguage = id ?? ""
            Task { await lyrics.loadTrack() }
        }
        // Translations are cached per track, language and model.
        .onChange(of: model.config.config?.model) { _, m in
            lyrics.model = m ?? ""
            Task { await lyrics.loadTrack() }
        }
    }
}

private struct NowPlayingCard: View {
    @Environment(AppModel.self) private var model

    var body: some View {
        let lyrics = model.lyrics
        let np = lyrics.nowPlaying
        TimelineView(.periodic(from: .now, by: 1)) { context in
            HStack(spacing: 12) {
                Image(systemName: np?.state == .playing ? "play.circle.fill" : "pause.circle")
                    .font(.largeTitle)
                    .foregroundStyle(.tint)
                VStack(alignment: .leading, spacing: 2) {
                    Text(np?.name ?? "Spotify").font(.headline).lineLimit(1)
                    Text([np?.artist, np?.album].compactMap { $0 }.joined(separator: " — "))
                        .font(.subheadline).foregroundStyle(.secondary).lineLimit(1)
                    if let duration = np?.durationMs, duration > 0, let pos = lyrics.position(at: context.date) {
                        HStack {
                            ProgressView(value: Double(pos), total: Double(duration))
                                .progressViewStyle(.linear)
                            Text("\(Self.time(pos)) / \(Self.time(duration))")
                                .font(.caption.monospacedDigit())
                                .foregroundStyle(.secondary)
                        }
                    }
                }
                Spacer()
                TranslationBadge(phase: lyrics.phase)
            }
        }
        if model.config.config?.spotifyShowNextTrack == true, let next = upNext {
            HStack {
                Text("Up next").font(.caption).foregroundStyle(.secondary)
                Text("\(next.name) — \(next.artist)").font(.caption).lineLimit(1)
                Spacer()
            }
            .padding(.top, 6)
        }
    }

    static func time(_ ms: Int) -> String {
        let s = max(ms, 0) / 1000
        return "\(s / 60):" + String(format: "%02d", s % 60)
    }

    private var upNext: TrackMeta? {
        let q = model.lyrics.queue
        guard q?.currentlyPlayingId == model.lyrics.nowPlaying?.trackId else { return nil }
        return q?.upcoming.first
    }
}

/// The translation's progress or failure, in words.
private struct StatusLine: View {
    let phase: LyricsStore.TranslationPhase

    var body: some View {
        switch phase {
        case .translating(let bytes):
            Text(bytes > 0 ? "Translating (\(bytes) bytes)…" : "Translating…")
                .font(.caption).foregroundStyle(.secondary).padding(6)
        case .failed(let message):
            // Provider errors can be pages of JSON; the full text is in the tooltip.
            Label("Translation failed", systemImage: "exclamationmark.triangle")
                .font(.caption).foregroundStyle(.red).padding(6)
                .help(message)
        default:
            EmptyView()
        }
    }
}

private struct TranslationBadge: View {
    let phase: LyricsStore.TranslationPhase

    var body: some View {
        switch phase {
        case .idle: EmptyView()
        case .translating: ProgressView().controlSize(.small).help("Translating lyrics…")
        case .done: Image(systemName: "checkmark.circle").foregroundStyle(.green).help("Translated")
        // Said once, in words, under the lyrics.
        case .failed: EmptyView()
        }
    }
}

private struct LyricsList: View {
    let lyrics: Lyrics
    @Environment(AppModel.self) private var model
    @State private var visible: [Int] = []

    var body: some View {
        let store = model.lyrics
        ScrollViewReader { proxy in
            TimelineView(.periodic(from: .now, by: 0.25)) { context in
                let current = store.currentLine(at: store.position(at: context.date))
                ScrollView {
                    LazyVStack(alignment: .leading, spacing: 14) {
                        ForEach(Array(lyrics.lines.enumerated()), id: \.offset) { index, line in
                            LyricsLineView(
                                line: line,
                                translation: translation(at: index),
                                isCurrent: index == current,
                                // Plain lyrics have no current line; dimming them all reads as disabled.
                                dimOthers: lyrics.synced
                            )
                            .id(index)
                        }
                    }
                    .scrollTargetLayout()
                    .padding()
                }
                .onScrollTargetVisibilityChange(idType: Int.self) { ids in visible = ids.sorted() }
                .onChange(of: current) { _, line in
                    guard let line, !inMiddleHalf(line) else { return }
                    withAnimation(.easeInOut(duration: 0.3)) { proxy.scrollTo(line, anchor: .center) }
                }
            }
        }
    }

    /// Re-center only when the line leaves the middle half of the view, as in
    /// the web app, so the list does not scroll on every line.
    private func inMiddleHalf(_ line: Int) -> Bool {
        guard visible.count >= 4, let index = visible.firstIndex(of: line) else { return false }
        return index >= visible.count / 4 && index < visible.count - visible.count / 4
    }

    private func translation(at index: Int) -> LyricsLineTranslation? {
        guard let t = model.lyrics.translation, index < t.lines.count else { return nil }
        return t.lines[index]
    }
}

private struct LyricsLineView: View {
    let line: LyricsLine
    let translation: LyricsLineTranslation?
    let isCurrent: Bool
    let dimOthers: Bool

    var body: some View {
        if line.text.trimmingCharacters(in: .whitespaces).isEmpty {
            Text("¶").foregroundStyle(.quaternary)
        } else {
            VStack(alignment: .leading, spacing: 4) {
                Text(line.text)
                    .font(.system(.title3, design: .serif).weight(isCurrent ? .semibold : .regular))
                    .foregroundStyle(isCurrent || !dimOthers ? .primary : .secondary)
                if let translation, !translation.translation.isEmpty {
                    Text(translation.translation)
                        .font(.callout)
                        .foregroundStyle(.tint)
                }
                if let glosses = translation?.glosses, !glosses.isEmpty {
                    ForEach(glosses, id: \.self) { g in
                        Text(g.note.isEmpty ? "\(g.fragment) — \(g.gloss)" : "\(g.fragment) — \(g.gloss) (\(g.note))")
                            .font(.caption)
                            .foregroundStyle(isCurrent ? .primary : .secondary)
                    }
                }
            }
            .textSelection(.enabled)
            .animation(.easeInOut(duration: 0.2), value: isCurrent)
        }
    }
}
