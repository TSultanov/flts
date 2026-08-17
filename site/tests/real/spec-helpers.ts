// Shared real-tier spec helpers: seeding books through the real pipeline,
// reading what the backend stored, and driving/observing the Anki sync.

import fs from 'node:fs';
import path from 'node:path';
import type { RealHarness } from './fixtures';

export const MODEL = 1; // Gemini25Flash
export const SRC = 'deu';
export const TGT = 'eng'; // fixtures' config.targetLanguageId
/**
 * `deck_name(deu, eng)`. Specs must seed it: the sync task's `bootstrapped`
 * flag lives for the worker's whole session, while the per-test `anki.reset()`
 * wipes the deck. Without it every addNote fails into the 60s backoff.
 */
export const DECK = 'FLTS::Deutsch-English';
export const CARD_DIR = ['library', 'cards', `${SRC}-${TGT}`];
/** Streaming translation traffic only; summaries are unary `:generateContent`. */
export const STREAM_GLOB = '*streamGenerateContent*';

export const sleep = (ms: number) => new Promise((r) => setTimeout(r, ms));

export function occurrences(hay: string, needle: string): number {
  let count = 0;
  for (let i = hay.indexOf(needle); i !== -1; i = hay.indexOf(needle, i + needle.length)) {
    count++;
  }
  return count;
}

/**
 * Lowercase ASCII only: the lemma slug is then the lemma itself, so card ids
 * are predictable. Unique per call because the disk translation cache outlives
 * the per-test config dir.
 */
export function nonceSeed(): string {
  return `${Date.now().toString(36)}${Math.random().toString(36).slice(2, 8)}`.replace(
    /[^a-z]/g,
    'x',
  );
}

const LETTERS = 'abcdefgh';

/** `n` paragraphs of `per` pairwise-distinct nonce lemmas. */
export function lemmaSets(n: number, per = 3): string[][] {
  const seed = nonceSeed();
  return Array.from({ length: n }, (_, i) =>
    Array.from({ length: per }, (_, j) => `w${seed}${LETTERS[i]}${LETTERS[j]}`),
  );
}

export const textOf = (lemmas: string[]) => lemmas.join(' ');

/** Gemini's compact translation schema (library/src/book/translation_import.rs). */
export function translationJson(lemmas: string[]): unknown {
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
export async function seedBook(
  h: RealHarness,
  sets: string[][],
  title: string,
): Promise<string> {
  await h.llm.seed({
    scripts: sets.map((lemmas) => ({
      matchSubstring: textOf(lemmas),
      translation: translationJson(lemmas),
    })),
  });
  return h.invoke<string>('import_plain_text', {
    title,
    text: sets.map(textOf).join('\n'),
    sourceLanguageId: SRC,
  });
}

export type WordSegment = { kind: string; text: string; translation: string | null };

export async function storedSegments(
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
export async function storedIds(
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

export const drained = (h: RealHarness) =>
  h.invoke<unknown[]>('list_paragraph_translation_activity');

export type Report = {
  totalCards: number;
  attempted: number;
  succeeded: number;
  failed: number;
  persistentFailures: string[];
};

export type Status = {
  state: 'idle' | 'syncing' | 'ok' | 'err' | 'unreachable';
  lastFinishedAtMs: number | null;
  lastError: string | null;
  lastReport: Report | null;
};

export type StoredCard = { id: string; anki_data: { state: string } | null };

export const cardIdOf = (lemma: string) => `flts_${SRC}_${TGT}_${lemma}`;

export const cardsDir = (h: RealHarness) => path.join(h.configDir, ...CARD_DIR);

export function storedCards(h: RealHarness): StoredCard[] {
  let files: string[];
  try {
    files = fs.readdirSync(cardsDir(h)).filter((f) => f.endsWith('.json'));
  } catch (err) {
    // No card has been written yet; anything else is a real read failure.
    if ((err as NodeJS.ErrnoException).code !== 'ENOENT') throw err;
    return [];
  }
  return files.map(
    (f) => JSON.parse(fs.readFileSync(path.join(cardsDir(h), f), 'utf8')) as StoredCard,
  );
}

/**
 * Cards outlive the per-test book cleanup and `sync_pass` walks every card on
 * disk, so leftovers from earlier tests in this worker would land in the same
 * batches and skew every report count.
 */
export const clearCards = (h: RealHarness) =>
  fs.rmSync(cardsDir(h), { recursive: true, force: true });

/**
 * Every card save wakes the sync task, so seeding would race the test's own
 * syncs. A blanket 503 kills each woken pass on the `version()` probe, before
 * `sync_pass`, so nothing is pushed and no card enters backoff.
 */
export const blockAnki = (h: RealHarness) =>
  h.anki.addRule({ action: { type: 'status', code: 503 } });

/**
 * Retries only "already in progress": `run_pass` refuses to queue behind an
 * in-flight pass, and every card save wakes one. "No sync task installed" must
 * still fail — the readiness gate makes it impossible.
 */
export async function syncNow(h: RealHarness): Promise<Report> {
  const deadline = Date.now() + 60_000;
  for (;;) {
    try {
      return await h.invoke<Report>('sync_anki_now');
    } catch (err) {
      const transient = String(err).includes('anki sync already in progress');
      if (!transient || Date.now() > deadline) throw err;
      await sleep(100);
    }
  }
}

/**
 * Waits out the woken passes. A stable request log alone is not enough: a pass
 * inside `run_pass` but not yet at its `version()` probe looks identical to no
 * pass, and the caller's `clearRules()` would release it into the sync the test
 * means to own. `syncing` is set before the probe, so it closes that window.
 */
export async function quiesceAnki(h: RealHarness, intervalMs = 1500): Promise<void> {
  const deadline = Date.now() + 30_000;
  let last = -1;
  for (;;) {
    const n = (await h.anki.requests()).length;
    const { state } = await h.invoke<Status>('get_anki_sync_status');
    if (n === last && state !== 'syncing') return;
    last = n;
    if (Date.now() > deadline) throw new Error('anki sim never went quiet');
    await sleep(intervalMs);
  }
}

/**
 * Sub-actions are wrapped in `multi` batches, so a request count says nothing —
 * only occurrences inside the bodies do.
 */
export async function countIn(
  h: RealHarness,
  needle: string,
  from = 0,
): Promise<number> {
  const reqs = (await h.anki.requests()).slice(from);
  return reqs.reduce((acc, r) => acc + occurrences(r.body, needle), 0);
}

/** `tags` is only ever sent by addNote, and carries exactly the card id. */
export const addsOf = (h: RealHarness, lemma: string, from = 0) =>
  countIn(h, `"tags":["${cardIdOf(lemma)}"]`, from);
