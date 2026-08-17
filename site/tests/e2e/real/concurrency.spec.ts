import fs from 'node:fs';
import path from 'node:path';
import { test, expect } from '../../real/fixtures';
import type { RealHarness } from '../../real/fixtures';

/**
 * Concurrency: several paragraph translations in flight at once, and other
 * writers (anki sync, reading state) racing them. All chapter 0 — translated
 * paragraphs in chapter >0 pay for the preceding chapters' summaries.
 *
 * Everything runs over the bridge; there is no UI for the queue's parallelism.
 */

const MODEL = 1; // Gemini25Flash
const STREAM_GLOB = '*streamGenerateContent*';
const DECK = 'FLTS::Deutsch-English'; // deck_name(deu, eng)
const CARD_DIR = ['library', 'cards', 'deu-eng'];
const LETTERS = 'abcdefgh';

const sleep = (ms: number) => new Promise((r) => setTimeout(r, ms));

/**
 * Lowercase ASCII only: the lemma slug is then the lemma itself, so card ids
 * are predictable. The disk translation cache outlives the config dir, so the
 * seed also has to be unique per test.
 */
function nonceSeed(): string {
  return `${Date.now().toString(36)}${Math.random().toString(36).slice(2, 8)}`.replace(
    /[^a-z]/g,
    'x',
  );
}

/** `n` paragraphs of `per` pairwise-distinct nonce lemmas. */
function lemmaSets(n: number, per = 3): string[][] {
  const seed = nonceSeed();
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

const scriptsFor = (sets: string[][]) =>
  sets.map((lemmas) => ({
    matchSubstring: textOf(lemmas),
    translation: translationJson(lemmas),
  }));

/** One chapter-0 paragraph per lemma set, each with its own scripted answer. */
async function seedBook(h: RealHarness, sets: string[][]): Promise<string> {
  await h.llm.seed({ scripts: scriptsFor(sets) });
  return h.invoke<string>('import_plain_text', {
    title: 'concurrency',
    text: sets.map(textOf).join('\n'),
    sourceLanguageId: 'deu',
  });
}

type WordSegment = { kind: string; text: string; translation: string | null };

async function storedSegments(
  h: RealHarness,
  bookId: string,
  paragraphId: number,
): Promise<WordSegment[] | null> {
  const rows = await h.invoke<Array<{ id: number; segments?: WordSegment[] | null }>>(
    'get_paragraph_translations_batch',
    { bookId, paragraphIds: [paragraphId] },
  );
  return rows.find((r) => r.id === paragraphId)?.segments ?? null;
}

/** Ids whose translation is on disk, ascending. */
async function storedIds(
  h: RealHarness,
  bookId: string,
  count: number,
): Promise<number[]> {
  const ids = [...Array(count).keys()];
  const rows = await h.invoke<Array<{ id: number; segments?: unknown }>>(
    'get_paragraph_translations_batch',
    { bookId, paragraphIds: ids },
  );
  return rows
    .filter((r) => r.segments)
    .map((r) => r.id)
    .sort((a, b) => a - b);
}

/** Streaming translation traffic only; summaries are unary `:generateContent`. */
async function streamCallsFor(h: RealHarness, text: string): Promise<number> {
  const reqs = await h.llm.requests();
  return reqs.filter(
    (r) => r.path.endsWith(':streamGenerateContent') && r.body.includes(text),
  ).length;
}

const activityOf = (h: RealHarness, bookId: string, paragraphId: number) =>
  h.invoke<unknown>('get_paragraph_translation_activity', { bookId, paragraphId });

const drained = (h: RealHarness) =>
  h.invoke<unknown[]>('list_paragraph_translation_activity');

// --- anki (spec: sync racing translation writes) ---

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

/** `run_pass` refuses to queue behind an in-flight pass; that is not a failure. */
async function syncNow(h: RealHarness): Promise<Report> {
  const deadline = Date.now() + 60_000;
  for (;;) {
    try {
      return await h.invoke<Report>('sync_anki_now');
    } catch (err) {
      if (!String(err).includes('in progress') || Date.now() > deadline) throw err;
      await sleep(100);
    }
  }
}

/**
 * Every card save wakes the sync task, so a test's own syncs would otherwise
 * race the seeding. See anki-failures.spec.ts for the full argument.
 */
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

/** Sub-actions ride inside `multi` envelopes, so only body occurrences count. */
async function countIn(h: RealHarness, needle: string, from = 0): Promise<number> {
  const reqs = (await h.anki.requests()).slice(from);
  return reqs.reduce((acc, r) => acc + occurrences(r.body, needle), 0);
}

/** `tags` is only ever sent by addNote, and carries exactly the card id. */
const addsOf = (h: RealHarness, lemma: string, from = 0) =>
  countIn(h, `"tags":["${cardIdOf(lemma)}"]`, from);

test.describe('translation queue concurrency', () => {
  test('a stalled lane does not block the rest of the chapter', async ({ harness }) => {
    test.setTimeout(120_000);
    const original = await harness.invoke<Record<string, unknown>>('get_config');
    const sets = lemmaSets(6);
    const texts = sets.map(textOf);
    const held = 3;
    const unheld = [0, 1, 2, 4, 5];

    try {
      // update_config rebuilds the queue, so the new bound takes effect here.
      await harness.invoke('update_config', {
        config: { ...original, translationConcurrency: 3 },
      });
      const bookId = await seedBook(harness, sets);

      // Path-scoped as well as body-scoped: the chapter summary is a unary call
      // that quotes every paragraph, and stalling it would block everything.
      await harness.llm.addRule({
        matcher: { pathGlob: STREAM_GLOB, bodyContains: texts[held] },
        action: { type: 'stall' },
      });
      await harness.invoke('translate_chapter', {
        bookId,
        chapterId: 0,
        model: MODEL,
        useCache: false,
      });

      await expect
        .poll(() => storedIds(harness, bookId, 6), { timeout: 90_000 })
        .toEqual(unheld);
      // The held lane is still occupied, not abandoned.
      expect(await activityOf(harness, bookId, held)).not.toBeNull();
      // Counted before the reset, which wipes the request log.
      for (const id of unheld) expect(await streamCallsFor(harness, texts[id])).toBe(1);

      // reset is the only stall release; it also wipes scripts, so re-seed.
      await harness.llm.reset();
      await harness.llm.seed({ scripts: scriptsFor(sets) });

      // The released request retries on its own, but it can lose the race with
      // the re-seed (or hit the one-off cachedContent 404); re-drive only once
      // the lane is genuinely free.
      await expect
        .poll(
          async () => {
            if ((await storedSegments(harness, bookId, held)) !== null) return true;
            if ((await activityOf(harness, bookId, held)) === null) {
              await harness.invoke('translate_paragraph', {
                bookId,
                paragraphId: held,
                model: MODEL,
                useCache: false,
              });
            }
            return false;
          },
          { timeout: 60_000, intervals: [500] },
        )
        .toBe(true);
      await expect.poll(() => drained(harness), { timeout: 30_000 }).toEqual([]);
    } finally {
      await harness.invoke('update_config', { config: original });
    }
  });

  test('a retrying paragraph never bleeds into its neighbours', async ({ harness }) => {
    test.setTimeout(120_000);
    const sets = lemmaSets(6);
    const texts = sets.map(textOf);
    const failing = 2;
    const bookId = await seedBook(harness, sets);

    await harness.llm.addRule({
      matcher: { pathGlob: STREAM_GLOB, bodyContains: texts[failing] },
      action: { type: 'status', code: 503, body: { error: 'sim overloaded' } },
      times: 2,
    });
    await harness.invoke('translate_chapter', {
      bookId,
      chapterId: 0,
      model: MODEL,
      useCache: false,
    });

    await expect
      .poll(() => storedIds(harness, bookId, 6), { timeout: 90_000 })
      .toEqual([0, 1, 2, 3, 4, 5]);
    await expect.poll(() => drained(harness), { timeout: 30_000 }).toEqual([]);

    // Two rejections plus the attempt that stuck; the retries stayed on their
    // own lane.
    expect(await streamCallsFor(harness, texts[failing])).toBe(3);
    for (const id of [0, 1, 3, 4, 5]) {
      expect(await streamCallsFor(harness, texts[id])).toBe(1);
    }

    // Every paragraph got its own script's answer, not a neighbour's.
    for (const [id, lemmas] of sets.entries()) {
      const segments = (await storedSegments(harness, bookId, id))!;
      const words = segments.filter((s) => s.kind === 'word');
      expect(words.map((w) => w.text)).toEqual(lemmas);
      expect(words.map((w) => w.translation)).toEqual(lemmas.map((l) => `t-${l}`));
    }
  });

  test('an anki sync in flight never double-adds the cards written under it', async ({
    harness,
  }) => {
    test.setTimeout(120_000);
    const sets = lemmaSets(6);
    const early = [0, 1, 2];
    const late = [3, 4, 5];
    const allLemmas = sets.flat();

    clearCards(harness);
    await harness.anki.seed({ decks: [DECK] });
    const bookId = await seedBook(harness, sets);

    // Seed the early cards with anki blocked, so the version() probe fails
    // before sync_pass and nothing is pushed or put in backoff.
    await harness.anki.addRule({ action: { type: 'status', code: 503 } });
    for (const id of early) {
      await harness.invoke('translate_paragraph', {
        bookId,
        paragraphId: id,
        model: MODEL,
        useCache: false,
      });
    }
    await expect
      .poll(() => storedCards(harness).length, { timeout: 60_000 })
      .toBe(early.length * 3);
    await quiesceAnki(harness);
    await harness.anki.clearRules();

    // Held at the version() probe, which run_pass reaches after flipping the
    // status to syncing — a wide, cheap window (a blanket delay would instead
    // pay 1.5s on every one of the pass's ~30 requests).
    await harness.anki.addRule({
      matcher: { bodyContains: '"action":"version"' },
      action: { type: 'delay', ms: 2000 },
    });
    const inFlight = syncNow(harness);
    await expect
      .poll(() => harness.invoke<Status>('get_anki_sync_status').then((s) => s.state), {
        timeout: 30_000,
      })
      .toBe('syncing');

    for (const id of late) {
      await harness.invoke('translate_paragraph', {
        bookId,
        paragraphId: id,
        model: MODEL,
        useCache: false,
      });
    }
    let overlapped = false;
    await expect
      .poll(
        async () => {
          const n = storedCards(harness).length;
          if (n === allLemmas.length) {
            overlapped =
              (await harness.invoke<Status>('get_anki_sync_status')).state === 'syncing';
          }
          return n;
        },
        { timeout: 60_000, intervals: [100] },
      )
      .toBe(allLemmas.length);
    // The late cards really did land under a running pass.
    expect(overlapped).toBe(true);

    expect((await inFlight).failed).toBe(0);

    await harness.anki.clearRules();
    // A card save during a pass leaves a pending wake, so a follow-up pass
    // fires on its own; let it finish before taking the mark.
    await quiesceAnki(harness);

    const mark = (await harness.anki.requests()).length;
    const second = await syncNow(harness);
    expect(second.totalCards).toBe(allLemmas.length);
    expect(second.failed).toBe(0);

    // The settled pass re-adds nothing: the early cards are updates only.
    for (const lemma of sets.slice(0, 3).flat()) {
      expect(await addsOf(harness, lemma, mark)).toBe(0);
    }
    // And no card was ever added twice, whichever pass picked it up.
    for (const lemma of allLemmas) expect(await addsOf(harness, lemma)).toBe(1);
    expect(storedCards(harness).map((c) => c.anki_data?.state ?? null)).toEqual(
      allLemmas.map(() => 'active'),
    );
  });

  test('reading-state writes land intact while translations are saving', async ({
    harness,
  }) => {
    test.setTimeout(120_000);
    const sets = lemmaSets(6);
    const bookId = await seedBook(harness, sets);

    // Keeps the translations in flight across the reading-state writes.
    await harness.llm.addRule({
      matcher: { pathGlob: STREAM_GLOB },
      action: { type: 'delay', ms: 800 },
    });
    await harness.invoke('translate_chapter', {
      bookId,
      chapterId: 0,
      model: MODEL,
      useCache: false,
    });
    // Only meaningful once the queue is actually working.
    await expect
      .poll(() => drained(harness).then((a) => a.length), { timeout: 30_000 })
      .toBeGreaterThan(0);

    for (const paragraphId of [1, 2, 3, 4, 5]) {
      await harness.invoke('save_book_reading_state', {
        bookId,
        chapterId: 0,
        paragraphId,
        pageOffset: paragraphId,
      });
    }

    await expect
      .poll(() => storedIds(harness, bookId, 6), { timeout: 90_000 })
      .toEqual([0, 1, 2, 3, 4, 5]);
    await expect.poll(() => drained(harness), { timeout: 30_000 }).toEqual([]);

    // Neither writer clobbered the other: last write wins, translations intact.
    // Read over the bridge rather than through the reader — mounting the
    // chapter view saves its own position over this one.
    expect(await harness.invoke('get_book_reading_state', { bookId })).toMatchObject({
      chapterId: 0,
      paragraphId: 5,
      pageOffset: 5,
    });
    for (const [id, lemmas] of sets.entries()) {
      const words = (await storedSegments(harness, bookId, id))!.filter(
        (s) => s.kind === 'word',
      );
      expect(words.map((w) => w.text)).toEqual(lemmas);
    }
  });
});
