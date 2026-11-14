import Foundation
import Observation

/// Spotify now-playing with time-synced lyrics and their translation (macOS).
@MainActor
@Observable
final class LyricsStore: CoreBound {
    enum TranslationPhase: Equatable {
        case idle
        case translating(bytes: Int)
        case done
        case failed(String)
    }

    private(set) var nowPlaying: NowPlaying?
    /// When `nowPlaying` arrived, for extrapolating the playback position.
    private(set) var receivedAt = Date()
    private(set) var lyrics: Lyrics?
    private(set) var lyricsResolved = false
    private(set) var translation: LyricsTranslation?
    private(set) var phase: TranslationPhase = .idle
    private(set) var queue: QueueSnapshot?
    private(set) var webStatus: SpotifyWebStatus?
    private(set) var cdpStatus: SpotifyCdpStatus?
    private(set) var loginAgent: SpotifyLoginAgentStatus?
    private(set) var watching = false
    var error: String?

    /// Set by the view from config; used when asking for a translation.
    var targetLanguage = ""
    var model = ""

    private let events: EventHub
    private var core: (any CoreService)?

    init(events: EventHub) {
        self.events = events
    }

    func attach(_ core: any CoreService) {
        self.core = core
        events.on { [weak self] event in
            guard let self else { return }
            switch event {
            case .spotifyState(let np): apply(np)
            case .spotifyQueue(let q): queue = q
            case .lyricsResolved(let e) where e.trackId == nowPlaying?.trackId:
                lyrics = e.lyrics
                lyricsResolved = true
            case .lyricsTranslationProgress(let e) where e.trackId == nowPlaying?.trackId:
                phase = .translating(bytes: e.bytes)
            case .lyricsTranslationDone(let e) where e.trackId == nowPlaying?.trackId:
                translation = e.translation
                phase = .done
            case .lyricsTranslationError(let e) where e.trackId == nowPlaying?.trackId:
                phase = .failed(e.error)
            default: break
            }
        }
    }

    func start() async {
        guard let core, !watching else { return }
        do {
            try await core.startSpotifyWatcher()
            watching = true
            if let np = (try? await core.getNowPlaying()) ?? nil {
                apply(np)
            }
            await refreshSpotifyStatus()
        } catch {
            self.error = error.localizedDescription
        }
    }

    func stop() async {
        guard let core, watching else { return }
        watching = false
        try? await core.stopSpotifyWatcher()
    }

    private func apply(_ np: NowPlaying) {
        let changedTrack = np.trackId != nowPlaying?.trackId
        nowPlaying = np
        receivedAt = Date()
        if changedTrack {
            lyrics = nil
            lyricsResolved = false
            translation = nil
            phase = .idle
            Task { await loadTrack() }
        }
    }

    func loadTrack() async {
        guard let core, let trackId = nowPlaying?.trackId, !targetLanguage.isEmpty else { return }
        do {
            let state = try await core.getTrackLyricsState(trackId: trackId, targetLang: targetLanguage, model: model)
            guard trackId == nowPlaying?.trackId else { return }
            if let lyrics = state.lyrics {
                self.lyrics = lyrics
                lyricsResolved = true
            }
            if let translation = state.translation {
                self.translation = translation
                phase = .done
            } else if state.lyrics != nil {
                phase = .translating(bytes: 0)
            }
        } catch {
            self.error = error.localizedDescription
        }
    }

    /// Playback position now, extrapolated while playing.
    func position(at now: Date) -> Int? {
        guard let np = nowPlaying, let base = np.positionMs else { return nil }
        guard np.state == .playing else { return base }
        let elapsed = Int(now.timeIntervalSince(receivedAt) * 1000)
        return min(base + elapsed, np.durationMs ?? Int.max)
    }

    /// Index of the line playing at `positionMs`, for synced lyrics.
    func currentLine(at positionMs: Int?) -> Int? {
        guard let lyrics, lyrics.synced, let positionMs else { return nil }
        var current: Int?
        for (i, line) in lyrics.lines.enumerated() {
            guard let t = line.timeMs else { continue }
            if t <= positionMs { current = i } else { break }
        }
        return current
    }

    // MARK: Spotify Web / DevTools

    func refreshSpotifyStatus() async {
        guard let core else { return }
        webStatus = try? await core.spotifyWebStatus()
        cdpStatus = try? await core.spotifyCdpStatus()
        loginAgent = try? await core.spotifyLoginAgentStatus()
        queue = ((try? await core.spotifyWebQueue()) ?? nil) ?? queue
    }

    func connectWeb(clientId: String) async {
        await run { try await $0.spotifyWebConnect(clientId: clientId) }
    }

    func disconnectWeb() async {
        await run { try await $0.spotifyWebDisconnect() }
    }

    func restartSpotifyWithDevTools() async {
        await run { _ = try await $0.spotifyRestartWithDevtools() }
    }

    func installLoginAgent() async {
        await run { _ = try await $0.spotifyInstallLoginAgent() }
    }

    func removeLoginAgent() async {
        await run { _ = try await $0.spotifyRemoveLoginAgent() }
    }

    private func run(_ action: (any CoreService) async throws -> Void) async {
        guard let core else { return }
        do {
            try await action(core)
            error = nil
        } catch {
            self.error = error.localizedDescription
        }
        await refreshSpotifyStatus()
    }
}
