import { invoke } from '@tauri-apps/api/core';
import { listen, type UnlistenFn } from '@tauri-apps/api/event';
import { writable, type Readable } from 'svelte/store';

export type TrackMeta = {
    id: string;
    name: string;
    artist: string;
    album?: string;
    durationMs: number;
};

export type QueueSnapshot = {
    /// "playlist" | "album" | "artist" | "show" | null. Preload should only
    /// fire for "playlist" / "album" — for the others, the next track is
    /// either undefined (autoplay) or not a song.
    contextType: string | null;
    currentlyPlayingId: string | null;
    upcoming: TrackMeta[];
};

export type SpotifyWebStatus = {
    connected: boolean;
    premiumRequired: boolean;
    lastError: string | null;
};

export async function spotifyWebConnect(clientId: string): Promise<void> {
    await invoke('spotify_web_connect', { clientId });
}

export async function spotifyWebDisconnect(): Promise<void> {
    await invoke('spotify_web_disconnect');
}

export async function spotifyWebStatus(): Promise<SpotifyWebStatus> {
    return await invoke<SpotifyWebStatus>('spotify_web_status');
}

export async function spotifyWebGetQueue(): Promise<QueueSnapshot | null> {
    return (await invoke<QueueSnapshot | null>('spotify_web_get_queue')) ?? null;
}

/// Subscribes to `spotify_queue` and surfaces snapshots plus the timestamp at
/// which they arrived. Consumers can use `receivedAt` to ignore stale snapshots
/// (e.g. "Up next" hides itself when the latest event is >30s old, indicating
/// playback stopped or the watcher fell behind).
export type QueueStoreValue = {
    snapshot: QueueSnapshot | null;
    receivedAt: number;
};

export function spotifyQueueStore(): {
    store: Readable<QueueStoreValue>;
    cleanup: () => void;
} {
    const inner = writable<QueueStoreValue>({ snapshot: null, receivedAt: 0 });
    let unlisten: UnlistenFn | null = null;

    listen<QueueSnapshot | null>('spotify_queue', (e) => {
        inner.set({ snapshot: e.payload ?? null, receivedAt: Date.now() });
    }).then((fn) => {
        unlisten = fn;
    });

    void spotifyWebGetQueue().then((snapshot) =>
        inner.set({ snapshot, receivedAt: snapshot ? Date.now() : 0 }),
    );

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
