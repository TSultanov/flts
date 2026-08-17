import fs from 'node:fs';
import path from 'node:path';
import { test, expect } from '../../real/fixtures';
import type { RealHarness } from '../../real/fixtures';

/**
 * The app process dies mid-work. SIGTERM gets the graceful shutdown path;
 * SIGKILL gets none, so whatever was mid-write is exactly where it stopped.
 * Only the app restarts — the sims keep their state across it, which is what
 * makes the anki case a real duplicate-detection test.
 */

const MODEL = 1; // Gemini25Flash
const STREAM_GLOB = '*streamGenerateContent*';
const DECK = 'FLTS::Deutsch-English'; // deck_name(deu, eng)
const CARD_DIR = ['library', 'cards', 'deu-eng'];
const LETTERS = 'abcdefgh';
/** `Config::default()`; a restart that lost the config dir would show this. */
const DEFAULT_ANKI_ENDPOINT = 'http://127.0.0.1:8765';

const sleep = (ms: number) => new Promise((r) => setTimeout(r, ms));

/** Lowercase ASCII only (lemma slug == lemma), unique per test (disk cache). */
function lemmaSets(n: number, per = 3): string[][] {
  const seed = `${Date.now().toString(36)}${Math.random().toString(36).slice(2, 8)}`
    .replace(/[^a-z]/g, 'x');
  return Array.from({ length: n }, (_, i) =>
    Array.from({ length: per }, (_, j) => `w${seed}${LETTERS[i]}${LETTERS[j]}`),
  );
}

const textOf = (lemmas: string[]) => lemmas.join(' ');
const cardIdOf = (lemma: string) => `flts_deu_eng_${lemma}`;

/** Gemini's compact translation schema (library/src/book/translation_import.rs). */
function translationJson(lemmas: string[]): unknown {
  return {
    s: [
      {
        ft: 'full-0',
        wl: lemmas.map((w) => ({
          o: w,
          t: [`t-${w}`],
          n: null,
          p: false,
          g: {
            lf: w,
            lt: `t-${w}`,
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

/** One chapter-0 paragraph per lemma set, each with its own scripted answer. */
async function seedBook(h: RealHarness, sets: string[][]): Promise<string> {
  await h.llm.seed({
    scripts: sets.map((lemmas) => ({
      matchSubstring: textOf(lemmas),
      translation: translationJson(lemmas),
    })),
  });
  return h.invoke<string>('import_plain_text', {
    title: 'restart-under-load',
    text: sets.map(textOf).join('\n'),
    sourceLanguageId: 'deu',
  });
}

/** Ids whose translation is on disk, ascending. */
async function storedIds(
  h: RealHarness,
  bookId: string,
  count: number,
): Promise<number[]> {
  const rows = await h.invoke<Array<{ id: number; segments?: unknown }>>(
    'get_paragraph_translations_batch',
    { bookId, paragraphIds: [...Array(count).keys()] },
  );
  return rows
    .filter((r) => r.segments)
    .map((r) => r.id)
    .sort((a, b) => a - b);
}

const drained = (h: RealHarness) =>
  h.invoke<unknown[]>('list_paragraph_translation_activity');

type Report = { totalCards: number; attempted: number; succeeded: number; failed: number };
type Status = { state: string };
type StoredCard = { id: string; anki_data: { state: string } | null };

const cardsDir = (h: RealHarness) => path.join(h.configDir, ...CARD_DIR);

function storedCards(h: RealHarness): StoredCard[] {
  let files: string[];
  try {
    files = fs.readdirSync(cardsDir(h)).filter((f) => f.endsWith('.json'));
  } catch {
    return [];
  }
  return files.map(
    (f) => JSON.parse(fs.readFileSync(path.join(cardsDir(h), f), 'utf8')) as StoredCard,
  );
}

/** `sync_pass` walks every card on disk, so leftovers would skew every count. */
const clearCards = (h: RealHarness) =>
  fs.rmSync(cardsDir(h), { recursive: true, force: true });

/**
 * Two transients, both bounded: `run_pass` refuses to queue behind an in-flight
 * pass, and a just-relaunched app has no sync task until the `eval_config` it
 * spawns *after* the bridge starts listening has run.
 */
const SYNC_TRANSIENTS = ['in progress', 'no anki sync task installed'];

async function syncNow(h: RealHarness): Promise<Report> {
  const deadline = Date.now() + 60_000;
  for (;;) {
    try {
      return await h.invoke<Report>('sync_anki_now');
    } catch (err) {
      const transient = SYNC_TRANSIENTS.some((m) => String(err).includes(m));
      if (!transient || Date.now() > deadline) throw err;
      await sleep(100);
    }
  }
}

/**
 * Same window, read side: library-backed queries answer with an empty list
 * rather than an error before `eval_config` opens the library, so a restart
 * must wait for the book to reappear before asserting anything about it.
 */
async function awaitLibrary(h: RealHarness, bookId: string): Promise<void> {
  await expect
    .poll(
      async () => {
        const books = await h.invoke<Array<{ id: string }>>('list_books');
        return books.some((b) => b.id === bookId);
      },
      { timeout: 30_000, intervals: [100] },
    )
    .toBe(true);
}

/** See anki-failures.spec.ts: card saves wake the sync task behind our backs. */
async function quiesceAnki(h: RealHarness): Promise<void> {
  const deadline = Date.now() + 30_000;
  let last = -1;
  for (;;) {
    const n = (await h.anki.requests()).length;
    const { state } = await h.invoke<Status>('get_anki_sync_status');
    if (n === last && state !== 'syncing') return;
    last = n;
    if (Date.now() > deadline) throw new Error('anki sim never went quiet');
    await sleep(400);
  }
}

function occurrences(hay: string, needle: string): number {
  let count = 0;
  for (let i = hay.indexOf(needle); i !== -1; i = hay.indexOf(needle, i + needle.length)) {
    count++;
  }
  return count;
}

/** `tags` is only ever sent by addNote, and carries exactly the card id. */
async function addsOf(h: RealHarness, lemma: string): Promise<number> {
  const needle = `"tags":["${cardIdOf(lemma)}"]`;
  return (await h.anki.requests()).reduce((acc, r) => acc + occurrences(r.body, needle), 0);
}

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

/** Serialize the queue and pace it, so "mid-batch" is a real, wide window. */
async function startPacedChapter(
  h: RealHarness,
  original: Record<string, unknown>,
  sets: string[][],
): Promise<string> {
  await h.invoke('update_config', { config: { ...original, translationConcurrency: 1 } });
  const bookId = await seedBook(h, sets);
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
    const bookId = await seedBook(harness, sets);

    // Translate with anki blocked: every woken pass dies on the version()
    // probe, so no card is pushed and none enters backoff.
    await harness.anki.addRule({ action: { type: 'status', code: 503 } });
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
    await quiesceAnki(harness);
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
