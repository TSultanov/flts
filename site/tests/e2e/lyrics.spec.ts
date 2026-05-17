import { test, expect, type Page } from '@playwright/test';

const TARGET = 'spa'; // mock config default targetLanguageId
const MODEL = 0; // mock config default model

type NowPlayingFixture = {
    state: 'playing' | 'paused' | 'stopped' | 'notrunning';
    trackId?: string;
    name?: string;
    artist?: string;
    album?: string;
    positionMs?: number;
    durationMs?: number;
};

type LyricsFixture = {
    track_id: string;
    lines: Array<{ time_ms: number | null; text: string }>;
    synced: boolean;
};

type LyricsTranslationFixture = {
    track_id: string;
    target_lang: string;
    model: number;
    lines: Array<{
        translation: string;
        glosses: Array<{ fragment: string; gloss: string; note: string }>;
    }>;
};

async function installPlatform(page: Page, platform: string) {
    // addInitScript runs before any page script, so this is set before
    // App.svelte's module-level `platform()` call.
    await page.addInitScript((p) => {
        (window as any).__mockPlatform = p;
    }, platform);
}

async function emitSpotifyState(page: Page, np: NowPlayingFixture | null) {
    await page.evaluate((state) => {
        (window as any).__mockSpotifyState(state);
    }, np);
}

async function setMockLyrics(
    page: Page,
    trackId: string,
    lyrics: LyricsFixture | null,
) {
    await page.evaluate(
        ({ trackId, lyrics }) => {
            (window as any).__mockLyrics(trackId, lyrics);
        },
        { trackId, lyrics },
    );
}

async function setCachedTranslation(page: Page, t: LyricsTranslationFixture) {
    await page.evaluate((translation) => {
        (window as any).__mockTranslationCache(translation);
    }, t);
}

/// Manually fire `lyrics_translation_done` for a given trackId. The backend
/// now keys translation events by trackId (not requestId) so the frontend
/// filters by content match against whatever's currently playing.
async function fireTranslationDone(
    page: Page,
    trackId: string,
    translation: LyricsTranslationFixture,
) {
    await page.evaluate(
        ({ trackId, translation }) => {
            (window as any).__tauriEmit('lyrics_translation_done', {
                trackId,
                translation,
            });
        },
        { trackId, translation },
    );
}

async function fireTranslationError(
    page: Page,
    trackId: string,
    error: string,
) {
    await page.evaluate(
        ({ trackId, error }) => {
            (window as any).__tauriEmit('lyrics_translation_error', {
                trackId,
                error,
            });
        },
        { trackId, error },
    );
}

const SYNCED_LYRICS: LyricsFixture = {
    track_id: 'spotify:track:sync',
    synced: true,
    lines: [
        { time_ms: 0, text: 'Primera línea' },
        { time_ms: 5000, text: 'Segunda línea' },
        { time_ms: 10000, text: 'Tercera línea' },
    ],
};

function playingState(
    trackId: string,
    positionMs: number,
    overrides: Partial<NowPlayingFixture> = {},
): NowPlayingFixture {
    return {
        state: 'playing',
        trackId,
        name: 'Test Song',
        artist: 'Test Artist',
        album: 'Test Album',
        positionMs,
        durationMs: 200_000,
        ...overrides,
    };
}

test.describe('Spotify lyrics translation mode', () => {
    test.describe('Platform gating', () => {
        test('nav shows the Lyrics link on macOS', async ({ page }) => {
            await installPlatform(page, 'macos');
            await page.goto('/library');
            await expect(
                page.locator('nav').getByRole('link', { name: 'Lyrics' }),
            ).toBeVisible();
        });

        test('nav hides the Lyrics link on non-macOS', async ({ page }) => {
            await installPlatform(page, 'linux');
            await page.goto('/library');
            await expect(
                page.locator('nav').getByRole('link', { name: 'Lyrics' }),
            ).toHaveCount(0);
        });

        test('renders an explanation when /lyrics is opened on non-macOS', async ({
            page,
        }) => {
            await installPlatform(page, 'linux');
            await page.goto('/lyrics');
            await expect(
                page.getByText('Spotify lyrics translation is macOS only'),
            ).toBeVisible();
        });
    });

    test.describe('Now Playing card', () => {
        test.beforeEach(async ({ page }) => {
            await installPlatform(page, 'macos');
            await page.goto('/lyrics');
        });

        test('shows "Spotify is not running" when no state has arrived', async ({
            page,
        }) => {
            await expect(page.getByText('Spotify is not running')).toBeVisible();
        });

        test('shows "Spotify is stopped" for stopped state', async ({ page }) => {
            await emitSpotifyState(page, { state: 'stopped' });
            await expect(page.getByText('Spotify is stopped')).toBeVisible();
        });

        test('shows track metadata + position + duration', async ({ page }) => {
            await emitSpotifyState(page, playingState('spotify:track:abc', 30_000));
            await expect(page.getByText('Test Song')).toBeVisible();
            await expect(page.getByText('Test Artist')).toBeVisible();
            await expect(page.getByText('Test Album')).toBeVisible();
            await expect(page.getByText('0:30 / 3:20')).toBeVisible();
        });

        test('time advances while state is playing', async ({ page }) => {
            await page.clock.install();
            await emitSpotifyState(page, playingState('spotify:track:abc', 30_000));
            await expect(page.getByText('0:30 / 3:20')).toBeVisible();
            await page.clock.runFor(2_100);
            await expect(page.getByText('0:32 / 3:20')).toBeVisible();
        });

        test('time stays frozen while paused', async ({ page }) => {
            await page.clock.install();
            await emitSpotifyState(page, {
                ...playingState('spotify:track:abc', 30_000),
                state: 'paused',
            });
            await expect(page.getByText('0:30 / 3:20')).toBeVisible();
            await page.clock.runFor(2_500);
            await expect(page.getByText('0:30 / 3:20')).toBeVisible();
        });
    });

    test.describe('Lyrics rendering', () => {
        test.beforeEach(async ({ page }) => {
            await installPlatform(page, 'macos');
            await page.goto('/lyrics');
        });

        test('highlights the line whose time_ms is closest under livePositionMs', async ({
            page,
        }) => {
            await setMockLyrics(page, SYNCED_LYRICS.track_id, SYNCED_LYRICS);
            await emitSpotifyState(
                page,
                playingState(SYNCED_LYRICS.track_id, 0),
            );

            const first = page.getByText('Primera línea').locator('..');
            const second = page.getByText('Segunda línea').locator('..');

            await expect(first).toHaveClass(/active/);
            await expect(second).not.toHaveClass(/active/);
        });

        test('moves the active line as Spotify position advances', async ({
            page,
        }) => {
            await setMockLyrics(page, SYNCED_LYRICS.track_id, SYNCED_LYRICS);
            await emitSpotifyState(
                page,
                playingState(SYNCED_LYRICS.track_id, 0),
            );

            await expect(
                page.getByText('Primera línea').locator('..'),
            ).toHaveClass(/active/);

            // Bump position past the second line's timestamp (5000ms).
            await emitSpotifyState(
                page,
                playingState(SYNCED_LYRICS.track_id, 5_500),
            );
            await expect(
                page.getByText('Segunda línea').locator('..'),
            ).toHaveClass(/active/);
            await expect(
                page.getByText('Primera línea').locator('..'),
            ).not.toHaveClass(/active/);
        });

        test('shows a warning and no active class for plain (unsynced) lyrics', async ({
            page,
        }) => {
            const trackId = 'spotify:track:plain';
            await setMockLyrics(page, trackId, {
                track_id: trackId,
                synced: false,
                lines: [
                    { time_ms: null, text: 'Línea uno' },
                    { time_ms: null, text: 'Línea dos' },
                ],
            });
            // The plain-lyrics warning is gated behind the translation status
            // reaching `idle`. Cache a translation so the status doesn't stick
            // at `translating` and mask the warning.
            await setCachedTranslation(page, {
                track_id: trackId,
                target_lang: TARGET,
                model: MODEL,
                lines: [
                    { translation: 'Line one', glosses: [] },
                    { translation: 'Line two', glosses: [] },
                ],
            });
            await emitSpotifyState(page, playingState(trackId, 0));

            await expect(
                page.getByText(/Plain lyrics only/),
            ).toBeVisible();
            await expect(
                page.getByText('Línea uno').locator('..'),
            ).not.toHaveClass(/active/);
        });

        test('shows "no lyrics found" status when LRClib returns null', async ({
            page,
        }) => {
            const trackId = 'spotify:track:missing';
            await setMockLyrics(page, trackId, null);
            await emitSpotifyState(page, playingState(trackId, 0));

            await expect(
                page.getByText('No lyrics found for this track on LRClib.'),
            ).toBeVisible();
        });
    });

    test.describe('Translation flow', () => {
        test.beforeEach(async ({ page }) => {
            await installPlatform(page, 'macos');
            await page.goto('/lyrics');
        });

        test('cache hit: translation appears immediately, status bar absent', async ({
            page,
        }) => {
            const trackId = 'spotify:track:cached';
            await setMockLyrics(page, trackId, {
                track_id: trackId,
                synced: true,
                lines: [{ time_ms: 0, text: 'Hola' }],
            });
            await setCachedTranslation(page, {
                track_id: trackId,
                target_lang: TARGET,
                model: MODEL,
                lines: [
                    {
                        translation: 'Hello',
                        glosses: [
                            { fragment: 'Hola', gloss: 'hello', note: '' },
                        ],
                    },
                ],
            });

            await emitSpotifyState(page, playingState(trackId, 0));

            await expect(page.locator('.translation')).toHaveText('Hello');
            await expect(page.locator('.gloss')).toHaveText('hello');
            await expect(page.locator('.status-bar')).toHaveCount(0);
        });

        test('cache miss: shows in-flight bar, then translation when done event fires', async ({
            page,
        }) => {
            const trackId = 'spotify:track:inflight';
            await setMockLyrics(page, trackId, {
                track_id: trackId,
                synced: true,
                lines: [{ time_ms: 0, text: 'Adiós' }],
            });
            // No cached translation and no pre-registered response → the mock's
            // translate_lyrics emits nothing, leaving the UI in 'translating'.

            await emitSpotifyState(page, playingState(trackId, 0));

            await expect(page.getByText(/Translating \(\d+ bytes\)/)).toBeVisible();

            await fireTranslationDone(page, trackId, {
                track_id: trackId,
                target_lang: TARGET,
                model: MODEL,
                lines: [
                    { translation: 'Goodbye', glosses: [] },
                ],
            });

            await expect(page.getByText('Goodbye')).toBeVisible();
            await expect(page.locator('.status-bar')).toHaveCount(0);
        });

        test('error event surfaces in the status bar', async ({ page }) => {
            const trackId = 'spotify:track:err';
            await setMockLyrics(page, trackId, {
                track_id: trackId,
                synced: true,
                lines: [{ time_ms: 0, text: 'Test' }],
            });

            await emitSpotifyState(page, playingState(trackId, 0));
            await expect(page.getByText(/Translating/)).toBeVisible();

            await fireTranslationError(page, trackId, 'rate limit hit');

            const statusBar = page.locator('.status-bar');
            await expect(statusBar).toContainText('Error: rate limit hit');
            await expect(statusBar).toHaveClass(/err/);
        });

        test('track change clears stale translation and re-fetches', async ({
            page,
        }) => {
            // Track A: cached translation.
            const trackA = 'spotify:track:A';
            await setMockLyrics(page, trackA, {
                track_id: trackA,
                synced: true,
                lines: [{ time_ms: 0, text: 'Uno' }],
            });
            await setCachedTranslation(page, {
                track_id: trackA,
                target_lang: TARGET,
                model: MODEL,
                lines: [{ translation: 'One', glosses: [] }],
            });

            await emitSpotifyState(page, playingState(trackA, 0));
            await expect(page.getByText('One')).toBeVisible();

            // Track B: lyrics registered, no cache → goes in-flight.
            const trackB = 'spotify:track:B';
            await setMockLyrics(page, trackB, {
                track_id: trackB,
                synced: true,
                lines: [{ time_ms: 0, text: 'Dos' }],
            });
            await emitSpotifyState(page, playingState(trackB, 0));

            // Stale "One" disappears; track B's original line is shown.
            await expect(page.getByText('Dos')).toBeVisible();
            await expect(page.getByText('One')).toHaveCount(0);
            await expect(page.getByText(/Translating/)).toBeVisible();
        });
    });

    test.describe('Status bar visibility', () => {
        test('is absent when synced lyrics are cached and playing', async ({
            page,
        }) => {
            await installPlatform(page, 'macos');
            await page.goto('/lyrics');

            const trackId = 'spotify:track:idle';
            await setMockLyrics(page, trackId, {
                track_id: trackId,
                synced: true,
                lines: [{ time_ms: 0, text: 'Sí' }],
            });
            await setCachedTranslation(page, {
                track_id: trackId,
                target_lang: TARGET,
                model: MODEL,
                lines: [{ translation: 'Yes', glosses: [] }],
            });
            await emitSpotifyState(page, playingState(trackId, 0));

            // Wait for the translation to land via the cache path.
            await expect(page.getByText('Yes')).toBeVisible();
            await expect(page.locator('.status-bar')).toHaveCount(0);
        });
    });
});
