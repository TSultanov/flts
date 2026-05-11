import { invoke } from '@tauri-apps/api/core';
import { listen, type UnlistenFn } from '@tauri-apps/api/event';
import { writable, type Readable } from 'svelte/store';

import type {
    Lyrics,
    LyricsTranslationDone,
    LyricsTranslationError,
    LyricsTranslationProgress,
    NowPlaying,
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

export async function getLyrics(args: {
    trackId: string;
    artist: string;
    title: string;
    album?: string;
    durationS?: number;
}): Promise<Lyrics | null> {
    return (
        (await invoke<Lyrics | null>('get_lyrics', {
            trackId: args.trackId,
            artist: args.artist,
            title: args.title,
            album: args.album,
            durationS: args.durationS,
        })) ?? null
    );
}

export async function translateLyrics(args: {
    trackId: string;
    targetLang: string;
    model: number;
}): Promise<number> {
    return await invoke<number>('translate_lyrics', {
        trackId: args.trackId,
        targetLang: args.targetLang,
        model: args.model,
    });
}

/// Subscribes to all three lyrics translation events and routes them to the
/// caller. Returns a teardown function that detaches the listeners.
export async function listenLyricsTranslation(handlers: {
    onProgress?: (e: LyricsTranslationProgress) => void;
    onDone?: (e: LyricsTranslationDone) => void;
    onError?: (e: LyricsTranslationError) => void;
}): Promise<UnlistenFn> {
    const unlistens: UnlistenFn[] = [];
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
