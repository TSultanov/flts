import fs from 'node:fs';
import path from 'node:path';
import { test, expect } from '../../real/fixtures';
import type { RealHarness } from '../../real/fixtures';
import {
  DECK,
  MODEL,
  STREAM_GLOB,
  addsOf,
  blockAnki,
  cardIdOf,
  clearCards,
  drained,
  lemmaSets,
  quiesceAnki,
  seedBook,
  storedCards,
  storedIds,
  syncNow,
} from '../../real/spec-helpers';

/**
 * The app process dies mid-work. SIGTERM gets the graceful shutdown path;
 * SIGKILL gets none, so whatever was mid-write is exactly where it stopped.
 * Only the app restarts — the sims keep their state across it, which is what
 * makes the anki case a real duplicate-detection test.
 */

/** `Config::default()`; a restart that lost the config dir would show this. */
const DEFAULT_ANKI_ENDPOINT = 'http://127.0.0.1:8765';

/**
 * Asks the sim over the AnkiConnect wire instead of reading its request log:
 * the log records a request on arrival, before the fault layer runs, so a
 * logged addNote is not yet an accepted one.
 */
async function simNoteIds(h: RealHarness, lemma: string): Promise<number[]> {
  const res = await fetch(h.anki.baseUrl, {
    method: 'POST',
    headers: { 'content-type': 'application/json' },
    body: JSON.stringify({
      action: 'findNotes',
      version: 6,
      params: { query: `tag:${cardIdOf(lemma)}` },
    }),
  });
  const json = (await res.json()) as { result?: number[] };
  return json.result ?? [];
}

/**
 * Library-backed queries answer with an empty list rather than an error before
 * the `eval_config` a relaunch spawns — after the bridge starts listening — has
 * opened the library. So a restart must wait for the book to reappear before
 * asserting anything about it.
 */
async function awaitLibrary(h: RealHarness, bookId: string): Promise<void> {
  await expect
    .poll(
      async () =>
        (await h.invoke<Array<{ id: string }>>('list_books')).map((b) => b.id),
      { timeout: 30_000, intervals: [100] },
    )
    .toContain(bookId);
}

/** Serialize the queue and pace it, so "mid-batch" is a real, wide window. */
async function startPacedChapter(
  h: RealHarness,
  original: Record<string, unknown>,
  sets: string[][],
): Promise<string> {
  await h.invoke('update_config', { config: { ...original, translationConcurrency: 1 } });
  const bookId = await seedBook(h, sets, 'restart-under-load');
  await h.llm.addRule({
    matcher: { pathGlob: STREAM_GLOB },
    action: { type: 'delay', ms: 1200 },
  });
  await h.invoke('translate_chapter', {
    bookId,
    chapterId: 0,
    model: MODEL,
    useCache: false,
  });
  return bookId;
}

/** The ids stored when the batch was partway through — never all of them. */
async function partwayIds(h: RealHarness, bookId: string): Promise<number[]> {
  let ids: number[] = [];
  await expect
    .poll(
      async () => {
        ids = await storedIds(h, bookId, 6);
        return ids.length;
      },
      { timeout: 60_000, intervals: [100] },
    )
    .toBeGreaterThanOrEqual(2);
  expect(ids.length).toBeLessThan(6);
  return ids;
}

/** Post-restart: clear the pacing, finish the chapter, prove the queue drains. */
async function finishChapter(
  h: RealHarness,
  bookId: string,
  before: number[],
): Promise<void> {
  await awaitLibrary(h, bookId);
  // Nothing that was already on disk was lost with the process.
  expect(await storedIds(h, bookId, 6)).toEqual(expect.arrayContaining(before));
  const chapters = await h.invoke<unknown[]>('list_book_chapters', { bookId });
  expect(chapters.length).toBe(1);

  await h.llm.clearRules();
  await h.invoke('translate_chapter', {
    bookId,
    chapterId: 0,
    model: MODEL,
    useCache: false,
  });
  await expect
    .poll(() => storedIds(h, bookId, 6), { timeout: 90_000 })
    .toEqual([0, 1, 2, 3, 4, 5]);
  await expect.poll(() => drained(h), { timeout: 30_000 }).toEqual([]);
}

test.describe('restart under load', () => {
  test('SIGTERM mid-batch loses nothing and the chapter finishes after relaunch', async ({
    harness,
  }) => {
    test.setTimeout(120_000);
    const original = await harness.invoke<Record<string, unknown>>('get_config');
    try {
      const bookId = await startPacedChapter(harness, original, lemmaSets(6));
      const before = await partwayIds(harness, bookId);

      const oldPort = harness.bridgePort;
      await harness.restartApp();
      expect(harness.bridgePort).not.toBe(oldPort);

      await finishChapter(harness, bookId, before);
    } finally {
      await harness.invoke('update_config', { config: original });
    }
  });

  test('SIGKILL mid-batch leaves the config intact and the chapter finishable', async ({
    harness,
  }) => {
    test.setTimeout(120_000);
    const original = await harness.invoke<Record<string, unknown>>('get_config');
    try {
      const bookId = await startPacedChapter(harness, original, lemmaSets(6));
      const before = await partwayIds(harness, bookId);

      await harness.restartApp({ signal: 'SIGKILL' });

      // The atomic config save survived an un-graceful death: nothing was
      // quarantined as unparseable...
      expect(fs.existsSync(path.join(harness.configDir, 'config.json.corrupt'))).toBe(
        false,
      );
      // ...and the harness config is still the live one, not Config::default().
      const live = await harness.invoke<Record<string, unknown>>('get_config');
      expect(live.ankiEndpoint).toBe(original.ankiEndpoint);
      expect(live.ankiEndpoint).not.toBe(DEFAULT_ANKI_ENDPOINT);
      expect(live.translationConcurrency).toBe(1);

      await finishChapter(harness, bookId, before);
    } finally {
      await harness.invoke('update_config', { config: original });
    }
  });

  test('SIGKILL between addNote and its state pull never re-adds the note', async ({
    harness,
  }) => {
    test.setTimeout(120_000);
    const sets = lemmaSets(2);
    const lemmas = sets.flat();

    clearCards(harness);
    await harness.anki.seed({ decks: [DECK] });
    const bookId = await seedBook(harness, sets, 'restart-under-load');

    // Translate with anki blocked: every woken pass dies on the version()
    // probe, so no card is pushed and none enters backoff.
    await blockAnki(harness);
    for (const paragraphId of sets.keys()) {
      await harness.invoke('translate_paragraph', {
        bookId,
        paragraphId,
        model: MODEL,
        useCache: false,
      });
    }
    await expect
      .poll(() => storedCards(harness).length, { timeout: 60_000 })
      .toBe(lemmas.length);
    await quiesceAnki(harness, 400);
    await harness.anki.clearRules();

    // Hold the pass at the state pull that follows the first addNote, so the
    // kill lands with the note in Anki and nothing recorded locally. `times`
    // keeps it off the pass the relaunched app fires on its own.
    await harness.anki.addRule({
      matcher: { bodyContains: '"action":"notesInfo"' },
      action: { type: 'delay', ms: 5000 },
      times: 1,
    });
    // The bridge socket dies with the app, so this invoke can never settle.
    const dying = harness.invoke('sync_anki_now').catch(() => undefined);

    // Acceptance, not arrival: ask the sim what it actually holds. The pass
    // pushes every addNote before it pulls state, so all six land before the
    // held notesInfo — and none of them is recorded locally yet.
    await expect
      .poll(
        async () => {
          const found = await Promise.all(lemmas.map((l) => simNoteIds(harness, l)));
          return found.filter((ids) => ids.length === 1).length;
        },
        { timeout: 30_000, intervals: [100] },
      )
      .toBe(lemmas.length);

    await harness.restartApp({ signal: 'SIGKILL' });
    await dying;
    // The kill really did land before the state pull: notes in Anki, nothing
    // on disk that could tell the next pass so.
    expect(storedCards(harness).map((c) => c.anki_data)).toEqual(
      lemmas.map(() => null),
    );
    await awaitLibrary(harness, bookId);
    await harness.anki.clearRules();

    const report = await syncNow(harness);
    expect(report.totalCards).toBe(lemmas.length);
    expect(report.failed).toBe(0);
    expect(storedCards(harness).map((c) => c.anki_data?.state ?? null)).toEqual(
      lemmas.map(() => 'active'),
    );
    // The pre-kill note was found, not duplicated: dedup goes through findNotes,
    // not through card state the kill never got to write.
    for (const lemma of lemmas) {
      expect(await addsOf(harness, lemma)).toBe(1);
      expect((await simNoteIds(harness, lemma)).length).toBe(1);
    }
  });
});
