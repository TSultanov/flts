<script lang="ts">
    import { onMount, onDestroy, getContext } from 'svelte';
    import { platform } from '@tauri-apps/plugin-os';
    import { configStore } from '../config';
    import {
        getTrackLyricsState,
        listenLyricsState,
        spotifyStateStore,
        startSpotifyWatcher,
        stopSpotifyWatcher,
    } from './store';
    import {
        spotifyQueueStore,
        type QueueStoreValue,
        type TrackMeta,
    } from '../spotify/queueStore';
    import type {
        Lyrics,
        LyricsTranslation,
        NowPlaying,
    } from './types';
    import NowPlayingCard from './NowPlayingCard.svelte';
    import LyricsList from './LyricsList.svelte';

    const mainHeight = getContext<{ value: number }>('mainHeight');

    // Detect macOS — on other platforms, render an explanation and bail.
    let isMac = $state(false);
    onMount(() => {
        try {
            isMac = platform() === 'macos';
        } catch {
            isMac = false;
        }
    });

    let nowPlaying: NowPlaying | null = $state(null);
    let stateCleanup: (() => void) | null = null;
    let stateUnsub: (() => void) | null = null;
    let translationCleanup: (() => void) | null = null;
    let queueValue: QueueStoreValue = $state({ snapshot: null, receivedAt: 0 });
    let queueCleanup: (() => void) | null = null;
    let queueUnsub: (() => void) | null = null;

    // Hide "Up next" if the snapshot is older than 30s — the AppleScript
    // watcher might still report Playing while polling has fallen behind, and
    // an outdated next track is worse than no next track.
    const QUEUE_STALE_MS = 30_000;
    let nowTickMs: number = $state(Date.now());
    const nextTrack = $derived.by<TrackMeta | null>(() => {
        const cfg = configStore.current;
        if (cfg && cfg.spotifyShowNextTrack === false) return null;
        if (!queueValue.snapshot) return null;
        if (nowTickMs - queueValue.receivedAt > QUEUE_STALE_MS) return null;
        return queueValue.snapshot.upcoming[0] ?? null;
    });

    let lyrics: Lyrics | null = $state(null);
    let translation: LyricsTranslation | null = $state(null);
    let translationStatus: 'idle' | 'fetching' | 'translating' | 'error' | 'unsupported-track' = $state('idle');
    let translationBytes: number = $state(0);
    let errorMessage: string | null = $state(null);
    let lastDispatchKey: string | null = null;

    type StatusMessage = { text: string; level: 'info' | 'warn' | 'err' };
    const statusMessage = $derived.by<StatusMessage | null>(() => {
        if (translationStatus === 'fetching')
            return { text: 'Fetching lyrics…', level: 'info' };
        if (translationStatus === 'translating')
            return { text: `Translating (${translationBytes} bytes)…`, level: 'info' };
        if (translationStatus === 'unsupported-track')
            return { text: 'No lyrics found for this track on LRClib.', level: 'warn' };
        if (translationStatus === 'error')
            return { text: `Error: ${errorMessage}`, level: 'err' };
        if (lyrics && !lyrics.synced)
            return {
                text: 'Plain lyrics only — karaoke sync unavailable for this track.',
                level: 'warn',
            };
        return null;
    });

    // Spotify's AppleScript position is sampled every ~500ms and the Rust
    // watcher suppresses sub-second deltas to avoid emit storms, so the raw
    // `nowPlaying.positionMs` would stay frozen between events. We extrapolate
    // locally: anchor on the last emitted value + perf clock, advance while
    // state is `playing`. One source of truth → consistent display and sync.
    let livePositionMs: number = $state(0);
    let anchorPositionMs: number = 0;
    let anchorPerfMs: number = 0;
    let positionTicker: ReturnType<typeof setInterval> | undefined;

    $effect(() => {
        // Re-anchor on every Spotify emit (track change, play/pause, seek).
        // Use `== null` to catch both undefined and JSON-null (which the Rust
        // side emits when AppleScript parsing fails) — letting null through
        // would silently coerce to 0 in `null + elapsed` later.
        if (!nowPlaying || nowPlaying.positionMs == null) {
            anchorPositionMs = 0;
            anchorPerfMs = performance.now();
            livePositionMs = 0;
            return;
        }
        anchorPositionMs = nowPlaying.positionMs;
        anchorPerfMs = performance.now();
        livePositionMs = nowPlaying.positionMs;
    });

    async function onTrackChange(np: NowPlaying) {
        if (
            np.state === 'notrunning' ||
            np.state === 'stopped' ||
            !np.trackId ||
            !np.name ||
            !np.artist
        ) {
            lyrics = null;
            translation = null;
            translationStatus = 'idle';
            errorMessage = null;
            lastDispatchKey = null;
            return;
        }

        const cfg = configStore.current;
        if (!cfg || !cfg.targetLanguageId) {
            translationStatus = 'idle';
            return;
        }

        const dispatchKey = `${np.trackId}|${cfg.targetLanguageId}|${cfg.model}`;
        if (dispatchKey === lastDispatchKey) return;
        lastDispatchKey = dispatchKey;

        // Reset view state for the new track and ask the backend for whatever
        // it currently has resolved. This is read-only — the backend's
        // resolver runs continuously based on what's playing and what's in
        // the queue; we never tell it to fetch. Anything not in the bootstrap
        // arrives via lyrics_resolved / lyrics_translation_done events.
        lyrics = null;
        translation = null;
        errorMessage = null;
        translationStatus = 'fetching';
        translationBytes = 0;

        try {
            const state = await getTrackLyricsState({
                trackId: np.trackId,
                targetLang: cfg.targetLanguageId,
                model: cfg.model,
            });
            if (lastDispatchKey !== dispatchKey) return; // stale
            if (state.lyrics) {
                lyrics = state.lyrics;
                if (state.translation) {
                    translation = state.translation;
                    translationStatus = 'idle';
                } else {
                    translationStatus = 'translating';
                }
            }
            // If state.lyrics is null we stay in 'fetching' — the backend
            // may still be resolving the track; the lyrics_resolved event
            // will land soon with either Some lyrics or an explicit null
            // (which we map to 'unsupported-track').
        } catch (e) {
            errorMessage = String(e);
            translationStatus = 'error';
        }
    }

    $effect(() => {
        // Drive on changes to nowPlaying.
        if (!isMac) return;
        if (!nowPlaying) return;
        void onTrackChange(nowPlaying);
    });

    onMount(async () => {
        if (!isMac) return;

        positionTicker = setInterval(() => {
            nowTickMs = Date.now();
            if (nowPlaying?.state !== 'playing') {
                // Keep livePositionMs in sync with the anchor when paused so
                // pausing freezes the display cleanly.
                if (livePositionMs !== anchorPositionMs) {
                    livePositionMs = anchorPositionMs;
                }
                return;
            }
            livePositionMs = anchorPositionMs + (performance.now() - anchorPerfMs);
        }, 100);

        await startSpotifyWatcher();
        const { store, cleanup } = spotifyStateStore();
        stateCleanup = cleanup;
        stateUnsub = store.subscribe((np) => {
            nowPlaying = np;
        });

        const queueState = spotifyQueueStore();
        queueCleanup = queueState.cleanup;
        queueUnsub = queueState.store.subscribe((v) => {
            queueValue = v;
        });

        translationCleanup = await listenLyricsState({
            // Match events to the currently-displayed track by id. Events
            // for any other track (background resolution of the queue) are
            // naturally filtered out here.
            onLyricsResolved: (e) => {
                if (e.trackId !== nowPlaying?.trackId) return;
                if (e.lyrics) {
                    lyrics = e.lyrics;
                    if (translationStatus === 'fetching') {
                        translationStatus = 'translating';
                    }
                } else {
                    translationStatus = 'unsupported-track';
                }
            },
            onProgress: (e) => {
                if (e.trackId === nowPlaying?.trackId) {
                    translationBytes = e.bytes;
                }
            },
            onDone: (e) => {
                if (e.trackId === nowPlaying?.trackId) {
                    translation = e.translation;
                    translationStatus = 'idle';
                }
            },
            onError: (e) => {
                if (e.trackId === nowPlaying?.trackId) {
                    errorMessage = e.error;
                    translationStatus = 'error';
                }
            },
        });
    });

    onDestroy(async () => {
        if (positionTicker) clearInterval(positionTicker);
        if (stateUnsub) stateUnsub();
        if (stateCleanup) stateCleanup();
        if (queueUnsub) queueUnsub();
        if (queueCleanup) queueCleanup();
        if (translationCleanup) translationCleanup();
        try {
            await stopSpotifyWatcher();
        } catch {}
    });
</script>

{#if !isMac}
    <div class="unsupported">
        <h2>Spotify lyrics translation is macOS only</h2>
        <p>
            This mode reads the currently-playing track from the local
            Spotify desktop client via AppleScript, which is only available
            on macOS.
        </p>
    </div>
{:else}
    <div class="root" style="height: {mainHeight?.value ?? 700}px;">
        <NowPlayingCard {nowPlaying} {livePositionMs} {nextTrack} />
        {#if statusMessage}
            <div class="status-bar {statusMessage.level}">
                {statusMessage.text}
            </div>
        {/if}
        <div class="lyrics-area">
            <LyricsList {lyrics} {translation} {nowPlaying} {livePositionMs} />
        </div>
    </div>
{/if}

<style>
    .root {
        display: flex;
        flex-direction: column;
        height: 100%;
    }
    .status-bar {
        padding: 8px 16px;
        font-size: 0.85em;
        opacity: 0.9;
        line-height: 1.4;
    }
    .status-bar.warn {
        color: #c08000;
    }
    .status-bar.err {
        color: var(--error-color, #b00020);
    }
    .lyrics-area {
        flex: 1 1 auto;
        min-height: 0;
        overflow: hidden;
    }
    .unsupported {
        padding: 40px;
        max-width: 640px;
        margin: 0 auto;
    }
    h2 {
        margin-top: 0;
    }
</style>
