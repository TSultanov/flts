import { invoke } from '@tauri-apps/api/core';
import { listen, type UnlistenFn } from '@tauri-apps/api/event';
import { writable, type Readable } from 'svelte/store';

import type {
    LyricsResolved,
    LyricsTranslationDone,
    LyricsTranslationError,
    LyricsTranslationProgress,
    NowPlaying,
    TrackLyricsState,
} from './types';

export async function startSpotifyWatcher(): Promise<void> {
    await invoke('start_spotify_watcher');
}

export async function stopSpotifyWatcher(): Promise<void> {
    await invoke('stop_spotify_watcher');
}

export async function getNowPlaying(): Promise<NowPlaying | null> {
    return (await invoke<NowPlaying | null>('get_now_playing')) ?? null;
}

/// Read-only snapshot of the backend's cached state for `trackId`. Returns
/// `{ lyrics, translation }` with whatever's currently resolved (both can be
/// null). Pure data fetch — never causes the backend to do work; the resolver
/// runs on its own schedule and pushes updates through events.
export async function getTrackLyricsState(args: {
    trackId: string;
    targetLang: string;
    model: number;
}): Promise<TrackLyricsState> {
    return await invoke<TrackLyricsState>('get_track_lyrics_state', {
        trackId: args.trackId,
        targetLang: args.targetLang,
        model: args.model,
    });
}

/// Subscribes to all backend events that describe a track's lyrics+translation
/// lifecycle. Consumers filter by `trackId` against whatever they're showing.
export async function listenLyricsState(handlers: {
    onLyricsResolved?: (e: LyricsResolved) => void;
    onProgress?: (e: LyricsTranslationProgress) => void;
    onDone?: (e: LyricsTranslationDone) => void;
    onError?: (e: LyricsTranslationError) => void;
}): Promise<UnlistenFn> {
    const unlistens: UnlistenFn[] = [];
    if (handlers.onLyricsResolved) {
        unlistens.push(
            await listen<LyricsResolved>('lyrics_resolved', (e) =>
                handlers.onLyricsResolved!(e.payload),
            ),
        );
    }
    if (handlers.onProgress) {
        unlistens.push(
            await listen<LyricsTranslationProgress>(
                'lyrics_translation_progress',
                (e) => handlers.onProgress!(e.payload),
            ),
        );
    }
    if (handlers.onDone) {
        unlistens.push(
            await listen<LyricsTranslationDone>('lyrics_translation_done', (e) =>
                handlers.onDone!(e.payload),
            ),
        );
    }
    if (handlers.onError) {
        unlistens.push(
            await listen<LyricsTranslationError>('lyrics_translation_error', (e) =>
                handlers.onError!(e.payload),
            ),
        );
    }
    return () => {
        unlistens.forEach((u) => u());
    };
}

/// Listens to `spotify_state` and exposes the latest payload as a Svelte
/// readable store. Returns the store plus a teardown function.
export function spotifyStateStore(): {
    store: Readable<NowPlaying | null>;
    cleanup: () => void;
} {
    const inner = writable<NowPlaying | null>(null);
    let unlisten: UnlistenFn | null = null;

    listen<NowPlaying>('spotify_state', (e) => {
        inner.set(e.payload);
    }).then((fn) => {
        unlisten = fn;
    });

    void getNowPlaying().then((np) => inner.set(np));

    return {
        store: { subscribe: inner.subscribe },
        cleanup: () => {
            if (unlisten) {
                unlisten();
                unlisten = null;
            }
        },
    };
}
