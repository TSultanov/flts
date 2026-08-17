import { test, expect } from '../../real/fixtures';
import type { RealHarness } from '../../real/fixtures';
import {
  paragraphLocator,
  seedAndOpen,
  wordSegment,
  type ParagraphSegment,
} from '../helpers/paragraph';

/**
 * Cross-cutting resilience: the three sims programmed together (total outage,
 * mixed health) and the app process itself restarted. Everything is chapter 0 —
 * translated paragraphs in chapter >0 hit the stale summary-ready watch
 * (task-13-report.md).
 */

const TARGET = 'eng'; // fixtures' config.targetLanguageId
const MODEL = 1; // Gemini25Flash
const DECK = 'FLTS::Deutsch-English'; // deck_name(deu, eng)
const STREAM_GLOB = '*streamGenerateContent*';
const LRC = '[00:01.00]Erste Zeile\n[00:05.00]Zweite Zeile';

const sleep = (ms: number) => new Promise((r) => setTimeout(r, ms));

function nonce(): string {
  return `${Date.now().toString(36)}${Math.random().toString(36).slice(2, 8)}`;
}

/** The disk translation cache outlives the config dir, so text must be unique. */
function nonceText(): string {
  return `Der ${nonce()} Vogel singt heute Morgen`;
}

function segmentsOf(text: string): ParagraphSegment[] {
  return text.split(' ').map((token, i) =>
    wordSegment({
      flatIndex: i,
      sentence: 0,
      word: i,
      text: token,
      translation: `t${i}`,
    }),
  );
}

/** Import one already-translated chapter-0 paragraph and open it. */
async function seedTranslated(page: import('@playwright/test').Page, text: string) {
  const { bookId } = await seedAndOpen(page, {
    chapters: [{ paragraphs: [{ html: text, segments: segmentsOf(text) }] }],
  });
  const p = paragraphLocator(page, 0);
  await expect(p.locator('.word-span').first()).toBeAttached({ timeout: 30_000 });
  return { bookId, p, words: await p.locator('.word-span').count() };
}

function nonceTrack() {
  const n = nonce();
  return {
    trackId: `trk-${n}`,
    name: `Song-${n}`,
    artist: `Artist-${n}`,
    album: 'Album',
    durationMs: 210_000,
    targetLang: TARGET,
    model: MODEL,
  };
}

type LyricsState = { lyrics: { lines: Array<{ text: string }> } | null };
type Status = { state: string; lastReport: { failed: number } | null };

/** `run_pass` refuses to queue behind an in-flight pass; that is not a failure. */
async function syncNow(h: RealHarness): Promise<{ failed: number }> {
  const deadline = Date.now() + 30_000;
  for (;;) {
    try {
      return await h.invoke('sync_anki_now');
    } catch (err) {
      if (!String(err).includes('in progress') || Date.now() > deadline) throw err;
      await sleep(100);
    }
  }
}

async function storedSegments(h: RealHarness, bookId: string): Promise<unknown> {
  const rows = await h.invoke<Array<{ id: number; segments?: unknown }>>(
    'get_paragraph_translations_batch',
    { bookId, paragraphIds: [0] },
  );
  return rows.find((r) => r.id === 0)?.segments ?? null;
}

test.describe('cross-cutting resilience', () => {
  test('no data loss across a total three-service outage', async ({ page, harness }) => {
    const text = nonceText();
    const { bookId, words } = await seedTranslated(page, text);
    expect(words).toBeGreaterThan(0);

    // Everything the app talks to goes dark at once.
    for (const sim of [harness.llm, harness.lrclib, harness.anki]) {
      await sim.addRule({ action: { type: 'drop' } });
    }

    // Save-bearing flows, all expected to fail: navigation re-runs the chapter
    // load (summaries), the re-translate rewrites the paragraph, the sync
    // rewrites card state, the resolve rewrites the lyrics cache.
    await page.goto('/');
    await page.goto(`/book/${bookId}/0`);
    await expect(paragraphLocator(page, 0).locator('.word-span').first()).toBeAttached({
      timeout: 30_000,
    });

    await harness.invoke('translate_paragraph', {
      bookId,
      paragraphId: 0,
      model: MODEL,
      useCache: false,
    });
    await expect(syncNow(harness)).rejects.toThrow(/AnkiConnect/);
    await expect(harness.invoke('e2e_resolve_track', nonceTrack())).rejects.toThrow();
    // translate_paragraph only enqueues; let the doomed pass reach its save path.
    await expect
      .poll(
        async () =>
          (await harness.llm.requests()).filter(
            (r) => r.path.endsWith(':streamGenerateContent') && r.body.includes(text),
          ).length,
        { timeout: 60_000 },
      )
      .toBeGreaterThan(0);
    await sleep(2000);

    for (const sim of [harness.llm, harness.lrclib, harness.anki]) {
      await sim.clearRules();
    }

    // Nothing the outage touched was lost: the translation is still on disk...
    await page.reload();
    const p = paragraphLocator(page, 0);
    await expect(p.locator('.word-span')).toHaveCount(words, { timeout: 30_000 });
    await expect(p.locator('button.translate')).toHaveCount(0);
    expect(await storedSegments(harness, bookId)).not.toBeNull();
    // ...and so is the book.
    const books = await harness.invoke<Array<{ id: string }>>('list_books');
    expect(books.map((b) => b.id)).toContain(bookId);
  });

  test('a stalled LLM blocks neither lyrics nor anki', async ({ harness }) => {
    const track = nonceTrack();
    await harness.lrclib.seed([
      { artist: track.artist, title: track.name, album: track.album, syncedLyrics: LRC },
    ]);
    await harness.anki.seed({ decks: [DECK] });

    const text = nonceText();
    const bookId = await harness.invoke<string>('import_plain_text', {
      title: 'resilience',
      text,
      sourceLanguageId: 'deu',
    });

    try {
      await harness.llm.addRule({
        matcher: { pathGlob: STREAM_GLOB },
        action: { type: 'stall' },
      });
      await harness.invoke('translate_paragraph', {
        bookId,
        paragraphId: 0,
        model: MODEL,
        useCache: false,
      });
      // Only once the socket is actually held open is the test meaningful.
      await expect
        .poll(
          async () =>
            (await harness.llm.requests()).some(
              (r) => r.path.endsWith(':streamGenerateContent') && r.body.includes(text),
            ),
          { timeout: 30_000 },
        )
        .toBe(true);

      const started = Date.now();
      const [state] = await Promise.all([
        harness
          .invoke('e2e_resolve_track', track)
          .then(() =>
            harness.invoke<LyricsState>('get_track_lyrics_state', {
              trackId: track.trackId,
              targetLang: TARGET,
              model: MODEL,
            }),
          ),
        syncNow(harness),
      ]);
      // Both finished promptly — no head-of-line blocking behind the stall.
      expect(Date.now() - started).toBeLessThan(30_000);
      expect(state.lyrics).not.toBeNull();
      expect(state.lyrics!.lines.length).toBe(2);
      expect((await harness.invoke<Status>('get_anki_sync_status')).state).toBe('ok');

      // The stall is still held: nothing resolved it, and nothing was stored.
      expect(await storedSegments(harness, bookId)).toBeNull();
    } finally {
      // reset is the only stall release; a leaked stall would poison later tests.
      await harness.llm.reset();
    }

    // Recovery: with the stall gone the paragraph translates on a re-drive.
    await harness.llm.seed({
      scripts: [{ matchSubstring: text, translation: translationJson(text) }],
    });
    await expect
      .poll(
        async () => {
          if ((await storedSegments(harness, bookId)) !== null) return true;
          await harness
            .invoke('translate_paragraph', {
              bookId,
              paragraphId: 0,
              model: MODEL,
              useCache: false,
            })
            .catch(() => {});
          return false;
        },
        { timeout: 60_000, intervals: [1000] },
      )
      .toBe(true);
  });

  test('translations survive an app restart', async ({ page, harness }) => {
    const text = nonceText();
    const { bookId, words } = await seedTranslated(page, text);

    const oldPort = harness.bridgePort;
    await harness.restartApp();
    // Ephemeral port: a stale injection would leave the page on a dead socket.
    expect(harness.bridgePort).not.toBe(oldPort);

    // Fresh bridge, fresh page: the init script re-injects the new port.
    await page.goto(`/book/${bookId}/0`);
    const p = paragraphLocator(page, 0);
    await expect(p.locator('.word-span')).toHaveCount(words, { timeout: 30_000 });
    await expect(p.locator('button.translate')).toHaveCount(0);
    // The span carries the overlay translation too, hence containsText.
    await expect(p.locator('.word-span').first()).toContainText(text.split(' ')[0]);

    const books = await harness.invoke<Array<{ id: string }>>('list_books');
    expect(books.map((b) => b.id)).toContain(bookId);
    expect(await storedSegments(harness, bookId)).not.toBeNull();
    expect(await harness.invoke('get_config')).toBeTruthy();
    expect(
      (await harness.invoke<Status>('get_anki_sync_status')).state,
    ).toBeTruthy();
  });
});

/** Gemini's compact translation schema (library/src/book/translation_import.rs). */
function translationJson(text: string): unknown {
  return {
    s: [
      {
        ft: 'full-0',
        wl: text.split(' ').map((token, i) => ({
          o: token,
          t: [`t${i}`],
          n: null,
          p: false,
          g: {
            lf: token,
            lt: `t${i}`,
            pos: 'common_noun',
            pl: null,
            pe: null,
            te: null,
            ca: null,
            ot: null,
          },
        })),
      },
    ],
  };
}
