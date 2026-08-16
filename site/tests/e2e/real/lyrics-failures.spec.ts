import { test, expect } from '../../real/fixtures';

/**
 * Lyrics resolution is Spotify-driven in production: the AppleScript watcher /
 * Web poller builds a playback list and calls `resolve_track`. There is no
 * Spotify sim, and `get_track_lyrics_state` is a pure cache read that never
 * touches LRClib — so the pipeline is driven through the bridge-only
 * `e2e_resolve_track` arm, which calls that same `resolve_track`. Assertions
 * are on backend state (`get_track_lyrics_state`) plus LRClib traffic; the
 * lyrics UI is unreachable without a `spotify_state` event, so no UI here.
 */

const TARGET = 'eng'; // fixtures' config.targetLanguageId
const MODEL = 1; // Gemini25Flash — the provider key the fixtures configure

type Track = {
  /** Appears verbatim in the LRClib query — the request-log filter keys on it. */
  nonce: string;
  trackId: string;
  name: string;
  artist: string;
  album: string | null;
  durationMs: number;
};

type LyricsState = {
  lyrics: { trackId?: string; lines: Array<{ text: string }> } | null;
  translation: unknown | null;
};

/** Lyrics cache lives under the per-worker config dir but outlives each test. */
function nonceTrack(): Track {
  const n = `${Date.now().toString(36)}${Math.random().toString(36).slice(2, 8)}`;
  return {
    nonce: n,
    trackId: `trk-${n}`,
    name: `Song-${n}`,
    artist: `Artist-${n}`,
    album: 'Album',
    durationMs: 210_000,
  };
}

const LRC = '[00:01.00]Erste Zeile\n[00:05.00]Zweite Zeile\n[00:09.00]Dritte Zeile';

function resolve(track: Track) {
  const { nonce: _nonce, ...meta } = track;
  return { ...meta, targetLang: TARGET, model: MODEL };
}

function lyricsStateArgs(track: Track) {
  return { trackId: track.trackId, targetLang: TARGET, model: MODEL };
}

/** LRClib hits for this track only — the sim logs the query string. */
async function hitsFor(
  lrclib: { requests: () => Promise<Array<{ path: string; query?: string | null }>> },
  track: Track,
): Promise<number> {
  const reqs = await lrclib.requests();
  return reqs.filter(
    (r) => r.path === '/api/get' && (r.query ?? '').includes(track.nonce),
  ).length;
}

const GET_GLOB = '/api/get';

test.describe('LRClib failure injection', () => {
  test('seeded track resolves to synced lyrics', async ({ harness }) => {
    const track = nonceTrack();
    await harness.lrclib.seed([
      { artist: track.artist, title: track.name, album: track.album, syncedLyrics: LRC },
    ]);

    await harness.invoke('e2e_resolve_track', resolve(track));

    const state = await harness.invoke<LyricsState>(
      'get_track_lyrics_state',
      lyricsStateArgs(track),
    );
    expect(state.lyrics).not.toBeNull();
    expect(state.lyrics!.lines.map((l) => l.text)).toEqual([
      'Erste Zeile',
      'Zweite Zeile',
      'Dritte Zeile',
    ]);
    expect(await hitsFor(harness.lrclib, track)).toBe(1);
  });

  test('404 resolves to no-lyrics without retrying', async ({ harness }) => {
    const track = nonceTrack();
    // Catalog left empty: the sim answers LRClib's real 404 body.

    await harness.invoke('e2e_resolve_track', resolve(track));

    const state = await harness.invoke<LyricsState>(
      'get_track_lyrics_state',
      lyricsStateArgs(track),
    );
    expect(state.lyrics).toBeNull();
    // 404 is terminal (Ok(None)) — it must never reach the retry classifier.
    expect(await hitsFor(harness.lrclib, track)).toBe(1);
  });

  test('two 503s then success: the retry budget covers it', async ({ harness }) => {
    const track = nonceTrack();
    await harness.lrclib.seed([
      { artist: track.artist, title: track.name, album: track.album, syncedLyrics: LRC },
    ]);
    await harness.lrclib.addRule({
      matcher: { pathGlob: GET_GLOB },
      action: { type: 'status', code: 503, body: { error: 'sim overloaded' } },
      times: 2,
    });

    await harness.invoke('e2e_resolve_track', resolve(track));

    const state = await harness.invoke<LyricsState>(
      'get_track_lyrics_state',
      lyricsStateArgs(track),
    );
    expect(state.lyrics).not.toBeNull();
    expect(state.lyrics!.lines.length).toBe(3);
    // LRCLIB_RETRY.max_attempts = 3: two rejected plus the one that stuck.
    expect(await hitsFor(harness.lrclib, track)).toBeGreaterThanOrEqual(3);
  });

  test('malformed JSON fails terminally and leaves the app usable', async ({
    harness,
  }) => {
    const track = nonceTrack();
    await harness.lrclib.seed([
      { artist: track.artist, title: track.name, album: track.album, syncedLyrics: LRC },
    ]);
    await harness.lrclib.addRule({
      matcher: { pathGlob: GET_GLOB },
      action: { type: 'corrupt', mode: 'malformed_json' },
    });

    // Pinned contract: a decode error is NOT transient, so resolve_track
    // surfaces it to its caller instead of retrying or silently swallowing it.
    await expect(harness.invoke('e2e_resolve_track', resolve(track))).rejects.toThrow(
      /error decoding response body|expected|EOF/i,
    );

    const state = await harness.invoke<LyricsState>(
      'get_track_lyrics_state',
      lyricsStateArgs(track),
    );
    expect(state.lyrics).toBeNull();
    expect(await hitsFor(harness.lrclib, track)).toBe(1);

    // Backend still serving: a clean track after the failure resolves normally.
    const ok = nonceTrack();
    await harness.lrclib.clearRules();
    await harness.lrclib.seed([
      { artist: ok.artist, title: ok.name, album: ok.album, syncedLyrics: LRC },
    ]);
    await harness.invoke('e2e_resolve_track', resolve(ok));
    const okState = await harness.invoke<LyricsState>(
      'get_track_lyrics_state',
      lyricsStateArgs(ok),
    );
    expect(okState.lyrics).not.toBeNull();
  });

  test('a slow response still resolves inside the 10s client timeout', async ({
    harness,
  }) => {
    const track = nonceTrack();
    await harness.lrclib.seed([
      { artist: track.artist, title: track.name, album: track.album, syncedLyrics: LRC },
    ]);
    await harness.lrclib.addRule({
      matcher: { pathGlob: GET_GLOB },
      action: { type: 'delay', ms: 2000 },
      times: 1,
    });

    const started = Date.now();
    await harness.invoke('e2e_resolve_track', resolve(track));
    expect(Date.now() - started).toBeGreaterThanOrEqual(1500);

    const state = await harness.invoke<LyricsState>(
      'get_track_lyrics_state',
      lyricsStateArgs(track),
    );
    expect(state.lyrics).not.toBeNull();
    expect(state.lyrics!.lines.length).toBe(3);
    // The delayed attempt was the only one — no timeout, no retry.
    expect(await hitsFor(harness.lrclib, track)).toBe(1);
  });
});
