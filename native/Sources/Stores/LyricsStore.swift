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
        events.on("spotify_state", as: NowPlaying.self) { [weak self] np in
            self?.apply(np)
        }
        events.on("lyrics_resolved", as: LyricsResolvedEvent.self) { [weak self] e in
            guard let self, e.trackId == nowPlaying?.trackId else { return }
            lyrics = e.lyrics
            lyricsResolved = true
        }
        events.on("lyrics_translation_progress", as: LyricsProgressEvent.self) { [weak self] e in
            guard let self, e.trackId == nowPlaying?.trackId else { return }
            phase = .translating(bytes: e.bytes)
        }
        events.on("lyrics_translation_done", as: LyricsDoneEvent.self) { [weak self] e in
            guard let self, e.trackId == nowPlaying?.trackId else { return }
            translation = e.translation
            phase = .done
        }
        events.on("lyrics_translation_error", as: LyricsErrorEvent.self) { [weak self] e in
            guard let self, e.trackId == nowPlaying?.trackId else { return }
            phase = .failed(e.error)
        }
        events.on("spotify_queue", as: QueueSnapshot.self) { [weak self] q in
            self?.queue = q
        }
    }

    func start() async {
        guard let core, !watching else { return }
        do {
            try await core.call("start_spotify_watcher")
            watching = true
            if let np: NowPlaying = try? await core.invoke("get_now_playing", as: NowPlaying?.self) ?? nil {
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
        try? await core.call("stop_spotify_watcher")
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
            let state: TrackLyricsState = try await core.invoke(
                "get_track_lyrics_state",
                ["trackId": .string(trackId), "targetLang": .string(targetLanguage), "model": .string(model)])
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
            guard let t = line.time_ms else { continue }
            if t <= positionMs { current = i } else { break }
        }
        return current
    }

    // MARK: Spotify Web / DevTools

    func refreshSpotifyStatus() async {
        guard let core else { return }
        webStatus = try? await core.invoke("spotify_web_status")
        cdpStatus = try? await core.invoke("spotify_cdp_status")
        loginAgent = try? await core.invoke("spotify_login_agent_status")
        queue = (try? await core.invoke("spotify_web_get_queue", as: QueueSnapshot?.self)) ?? queue
    }

    func connectWeb(clientId: String) async {
        await run("spotify_web_connect", ["clientId": .string(clientId)])
    }

    func disconnectWeb() async {
        await run("spotify_web_disconnect")
    }

    func restartSpotifyWithDevTools() async {
        await run("spotify_restart_with_devtools")
    }

    func installLoginAgent() async {
        await run("spotify_install_login_agent")
    }

    func removeLoginAgent() async {
        await run("spotify_remove_login_agent")
    }

    private func run(_ cmd: String, _ args: [String: JSONValue] = [:]) async {
        guard let core else { return }
        do {
            try await core.call(cmd, args)
            error = nil
        } catch {
            self.error = error.localizedDescription
        }
        await refreshSpotifyStatus()
    }
}
