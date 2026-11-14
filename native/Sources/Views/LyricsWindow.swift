import SwiftUI

struct LyricsWindow: View {
    @Environment(AppModel.self) private var model

    var body: some View {
        let lyrics = model.lyrics
        VStack(spacing: 0) {
            NowPlayingCard()
                .padding()
            Divider()
            if lyrics.nowPlaying?.trackId == nil {
                ContentUnavailableView(
                    "Nothing Playing", systemImage: "music.note",
                    description: Text("Play a song in Spotify to see its lyrics translated."))
            } else if !lyrics.lyricsResolved {
                ProgressView("Finding lyrics…").frame(maxWidth: .infinity, maxHeight: .infinity)
            } else if let current = lyrics.lyrics {
                LyricsList(lyrics: current)
            } else {
                ContentUnavailableView("No Lyrics Found", systemImage: "text.badge.xmark")
            }
            if let error = lyrics.error {
                ErrorBanner(message: error) { lyrics.error = nil }
            }
        }
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
                        ProgressView(value: Double(pos), total: Double(duration))
                            .progressViewStyle(.linear)
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

    private var upNext: TrackMeta? {
        let q = model.lyrics.queue
        guard q?.currentlyPlayingId == model.lyrics.nowPlaying?.trackId else { return nil }
        return q?.upcoming.first
    }
}

private struct TranslationBadge: View {
    let phase: LyricsStore.TranslationPhase

    var body: some View {
        switch phase {
        case .idle: EmptyView()
        case .translating: ProgressView().controlSize(.small).help("Translating lyrics…")
        case .done: Image(systemName: "checkmark.circle").foregroundStyle(.green).help("Translated")
        case .failed(let message):
            Image(systemName: "exclamationmark.triangle").foregroundStyle(.orange).help(message)
        }
    }
}

private struct LyricsList: View {
    let lyrics: Lyrics
    @Environment(AppModel.self) private var model

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
                                isCurrent: index == current
                            )
                            .id(index)
                        }
                    }
                    .padding()
                }
                .onChange(of: current) { _, line in
                    guard let line else { return }
                    withAnimation(.easeInOut(duration: 0.3)) { proxy.scrollTo(line, anchor: .center) }
                }
            }
        }
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

    var body: some View {
        if line.text.trimmingCharacters(in: .whitespaces).isEmpty {
            Spacer().frame(height: 8)
        } else {
            VStack(alignment: .leading, spacing: 4) {
                Text(line.text)
                    .font(.system(.title3, design: .serif).weight(isCurrent ? .semibold : .regular))
                    .foregroundStyle(isCurrent ? .primary : .secondary)
                if let translation, !translation.translation.isEmpty {
                    Text(translation.translation)
                        .font(.callout)
                        .foregroundStyle(.tint)
                }
                if isCurrent, let glosses = translation?.glosses, !glosses.isEmpty {
                    ForEach(glosses, id: \.self) { g in
                        HStack(alignment: .firstTextBaseline, spacing: 6) {
                            Text(g.fragment).font(.caption.weight(.semibold))
                            Text(g.gloss).font(.caption)
                            if !g.note.isEmpty { Text(g.note).font(.caption).foregroundStyle(.secondary) }
                        }
                    }
                }
            }
            .textSelection(.enabled)
            .animation(.easeInOut(duration: 0.2), value: isCurrent)
        }
    }
}
