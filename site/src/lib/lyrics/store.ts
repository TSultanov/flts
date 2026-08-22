import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import { writable, type Readable } from "svelte/store";

import type {
  LyricsResolved,
  LyricsTranslationDone,
  LyricsTranslationError,
  LyricsTranslationProgress,
  NowPlaying,
  TrackLyricsState,
} from "./types";

export async function startSpotifyWatcher(): Promise<void> {
  await invoke("start_spotify_watcher");
}

export async function stopSpotifyWatcher(): Promise<void> {
  await invoke("stop_spotify_watcher");
}

export async function getNowPlaying(): Promise<NowPlaying | null> {
  return (await invoke<NowPlaying | null>("get_now_playing")) ?? null;
}

/// Read-only snapshot; never makes the backend resolve. Either field can be
/// null, and updates arrive through events.
export async function getTrackLyricsState(args: {
  trackId: string;
  targetLang: string;
  model: string;
}): Promise<TrackLyricsState> {
  return await invoke<TrackLyricsState>("get_track_lyrics_state", {
    trackId: args.trackId,
    targetLang: args.targetLang,
    model: args.model,
  });
}

/// Fires for every track, so consumers must filter by `trackId`.
export async function listenLyricsState(handlers: {
  onLyricsResolved?: (e: LyricsResolved) => void;
  onProgress?: (e: LyricsTranslationProgress) => void;
  onDone?: (e: LyricsTranslationDone) => void;
  onError?: (e: LyricsTranslationError) => void;
}): Promise<UnlistenFn> {
  const unlistens: UnlistenFn[] = [];
  if (handlers.onLyricsResolved) {
    unlistens.push(
      await listen<LyricsResolved>("lyrics_resolved", (e) =>
        handlers.onLyricsResolved!(e.payload),
      ),
    );
  }
  if (handlers.onProgress) {
    unlistens.push(
      await listen<LyricsTranslationProgress>(
        "lyrics_translation_progress",
        (e) => handlers.onProgress!(e.payload),
      ),
    );
  }
  if (handlers.onDone) {
    unlistens.push(
      await listen<LyricsTranslationDone>("lyrics_translation_done", (e) =>
        handlers.onDone!(e.payload),
      ),
    );
  }
  if (handlers.onError) {
    unlistens.push(
      await listen<LyricsTranslationError>("lyrics_translation_error", (e) =>
        handlers.onError!(e.payload),
      ),
    );
  }
  return () => {
    unlistens.forEach((u) => u());
  };
}

/// Returns the store plus a teardown function.
export function spotifyStateStore(): {
  store: Readable<NowPlaying | null>;
  cleanup: () => void;
} {
  const inner = writable<NowPlaying | null>(null);
  let unlisten: UnlistenFn | null = null;

  listen<NowPlaying>("spotify_state", (e) => {
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
