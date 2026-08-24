import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import { writable, type Readable } from "svelte/store";

export type TrackMeta = {
  id: string;
  name: string;
  artist: string;
  album?: string;
  durationMs: number;
};

export type QueueSnapshot = {
  /// Preload only for "playlist"/"album"; elsewhere the next track is
  /// either autoplay-undefined or not a song.
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
  await invoke("spotify_web_connect", { clientId });
}

export async function spotifyWebDisconnect(): Promise<void> {
  await invoke("spotify_web_disconnect");
}

export async function spotifyWebStatus(): Promise<SpotifyWebStatus> {
  return await invoke<SpotifyWebStatus>("spotify_web_status");
}

/**
 * Status of the Spotify DevTools bridge: the desktop app must run with
 * `--remote-debugging-port` for first-party lyrics (fetched inside Spotify's
 * own webview, so no credentials of ours are involved).
 */
export type SpotifyCdpStatus = {
  available: boolean;
  port: number;
  hint: string | null;
};

export async function spotifyCdpStatus(): Promise<SpotifyCdpStatus> {
  return await invoke<SpotifyCdpStatus>("spotify_cdp_status");
}

/** Relaunch Spotify with the DevTools flag (user-initiated, restores session). */
export async function spotifyRestartWithDevtools(): Promise<SpotifyCdpStatus> {
  return await invoke<SpotifyCdpStatus>("spotify_restart_with_devtools");
}

export async function spotifyWebGetQueue(): Promise<QueueSnapshot | null> {
  return (await invoke<QueueSnapshot | null>("spotify_web_get_queue")) ?? null;
}

/// `receivedAt` lets consumers drop stale snapshots — a watcher that fell
/// behind still reports the old queue.
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

  listen<QueueSnapshot | null>("spotify_queue", (e) => {
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
