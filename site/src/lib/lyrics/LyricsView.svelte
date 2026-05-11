<script lang="ts">
    import { onMount, onDestroy, getContext } from 'svelte';
    import { platform } from '@tauri-apps/plugin-os';
    import { configStore } from '../config';
    import {
        getLyrics,
        listenLyricsTranslation,
        spotifyStateStore,
        startSpotifyWatcher,
        stopSpotifyWatcher,
        translateLyrics,
    } from './store';
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

    let lyrics: Lyrics | null = $state(null);
    let translation: LyricsTranslation | null = $state(null);
    let translationStatus: 'idle' | 'fetching' | 'translating' | 'error' | 'unsupported-track' = $state('idle');
    let translationBytes: number = $state(0);
    let errorMessage: string | null = $state(null);
    let currentRequestId: number | null = $state(null);
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

        const cfg = $configStore;
        if (!cfg || !cfg.targetLanguageId) {
            translationStatus = 'idle';
            return;
        }

        const dispatchKey = `${np.trackId}|${cfg.targetLanguageId}|${cfg.model}`;
        if (dispatchKey === lastDispatchKey) return;
        lastDispatchKey = dispatchKey;

        // Reset state for new track.
        lyrics = null;
        translation = null;
        errorMessage = null;
        translationStatus = 'fetching';
        translationBytes = 0;

        try {
            const fetched = await getLyrics({
                trackId: np.trackId,
                artist: np.artist,
                title: np.name,
                album: np.album,
                durationS: np.durationMs ? Math.round(np.durationMs / 1000) : undefined,
            });
            if (lastDispatchKey !== dispatchKey) return; // stale
            lyrics = fetched;
            if (!fetched) {
                translationStatus = 'unsupported-track';
                return;
            }
        } catch (e) {
            errorMessage = String(e);
            translationStatus = 'error';
            return;
        }

        translationStatus = 'translating';
        try {
            const reqId = await translateLyrics({
                trackId: np.trackId,
                targetLang: cfg.targetLanguageId,
                model: cfg.model,
            });
            currentRequestId = reqId;
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

        translationCleanup = await listenLyricsTranslation({
            onProgress: (e) => {
                if (currentRequestId === null || e.requestId === currentRequestId) {
                    translationBytes = e.bytes;
                }
            },
            onDone: (e) => {
                if (currentRequestId === null || e.requestId === currentRequestId) {
                    translation = e.translation;
                    translationStatus = 'idle';
                }
            },
            onError: (e) => {
                if (currentRequestId === null || e.requestId === currentRequestId) {
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
        <NowPlayingCard {nowPlaying} {livePositionMs} />
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
