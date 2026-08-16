import fs from 'node:fs';
import net from 'node:net';
import path from 'node:path';
import { test, expect } from '../../real/fixtures';
import type { RealHarness } from '../../real/fixtures';

/**
 * Anki export is driven entirely over the bridge: `sync_anki_now` runs the same
 * `run_pass` the periodic task does, `get_anki_sync_status` is the surface the
 * nav button renders. There is no UI here — the button lives behind a status
 * event the Node-side BridgeClient cannot observe.
 *
 * Exportable cards come from translation, not familiarity: the queue calls
 * `Library::apply_paragraph_to_cards`, which writes one card per eligible lemma
 * under `<configDir>/library/cards/deu-eng/`. So a scripted LLM translation of a
 * one-paragraph book is what makes notes exist to push.
 */

const CARD_DIR = ['library', 'cards', 'deu-eng'];
/**
 * `deck_name(deu, eng)`. Seeded rather than left to `bootstrap`: the sync task
 * lives for the worker's whole session, so an earlier spec's card write may
 * already have flipped its `bootstrapped` flag — and the per-test `anki.reset()`
 * wipes the deck it created. Without the deck, every addNote fails with
 * "deck was not found" and the cards land in the 60s backoff.
 */
const DECK = 'FLTS::Deutsch-English';
const SRC = 'deu';
const TGT = 'eng'; // fixtures' config.targetLanguageId
const MODEL = 1; // Gemini25Flash

type Report = {
  totalCards: number;
  attempted: number;
  succeeded: number;
  failed: number;
  persistentFailures: string[];
};

type Status = {
  state: 'idle' | 'syncing' | 'ok' | 'err' | 'unreachable';
  lastFinishedAtMs: number | null;
  lastError: string | null;
  lastReport: Report | null;
};

type StoredCard = { id: string; anki_data: { state: string } | null };

const sleep = (ms: number) => new Promise((r) => setTimeout(r, ms));

/** Lowercase ASCII only: lemma slug == lemma, so the card id is predictable. */
function nonceLemmas(n: number): string[] {
  const seed = `${Date.now().toString(36)}${Math.random().toString(36).slice(2, 8)}`
    .replace(/[^a-z]/g, 'x');
  return Array.from({ length: n }, (_, i) => `w${seed}${'abcdefgh'[i]}`);
}

const cardIdOf = (lemma: string) => `flts_${SRC}_${TGT}_${lemma}`;

function cardsDir(h: RealHarness): string {
  return path.join(h.configDir, ...CARD_DIR);
}

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

/**
 * Cards outlive the per-test book cleanup and `sync_pass` walks every card on
 * disk, so leftovers from earlier tests in this worker would land in the same
 * batches and skew every report count.
 */
function clearCards(h: RealHarness): void {
  fs.rmSync(cardsDir(h), { recursive: true, force: true });
}

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

/** Import a one-paragraph book, translate it, and wait for the cards on disk. */
async function seedCards(h: RealHarness, lemmas: string[]): Promise<void> {
  const text = lemmas.join(' ');
  await h.llm.seed({
    scripts: [{ matchSubstring: text, translation: translationJson(lemmas) }],
  });
  const bookId = await h.invoke<string>('import_plain_text', {
    title: 'anki-failures',
    text,
    sourceLanguageId: SRC,
  });
  await h.invoke('translate_paragraph', {
    bookId,
    paragraphId: 0,
    model: MODEL,
    useCache: false,
  });
  await expect
    .poll(() => storedCards(h).map((c) => c.id).sort(), { timeout: 30_000 })
    .toEqual(lemmas.map(cardIdOf).sort());
}

/**
 * Cards are saved one by one and every save wakes the sync task, so seeding
 * would otherwise race the test's own syncs. A blanket 503 makes each woken
 * pass die on the `version()` probe — before `sync_pass`, so nothing is pushed
 * and no card enters backoff.
 */
async function blockAnki(h: RealHarness): Promise<void> {
  await h.anki.addRule({ action: { type: 'status', code: 503 } });
}

/**
 * Waits out the woken passes. A stable request log is not enough on its own: a
 * pass that has entered `run_pass` but not yet issued its `version()` probe
 * looks identical to no pass at all, and would then be unblocked by the
 * caller's `clearRules()` and steal the sync the test means to own. `syncing`
 * is set at the top of `run_pass`, before the probe, so it closes that window.
 */
async function quiesce(h: RealHarness): Promise<void> {
  const deadline = Date.now() + 30_000;
  let last = -1;
  for (;;) {
    const n = (await h.anki.requests()).length;
    const { state } = await h.invoke<Status>('get_anki_sync_status');
    if (n === last && state !== 'syncing') return;
    last = n;
    if (Date.now() > deadline) throw new Error('anki sim never went quiet');
    await sleep(1500);
  }
}

/** `run_pass` refuses to queue behind an in-flight pass; that is not a failure. */
async function syncNow(h: RealHarness): Promise<Report> {
  const deadline = Date.now() + 30_000;
  for (;;) {
    try {
      return await h.invoke<Report>('sync_anki_now');
    } catch (err) {
      if (!String(err).includes('in progress') || Date.now() > deadline) throw err;
      await sleep(100);
    }
  }
}

function occurrences(hay: string, needle: string): number {
  let count = 0;
  for (let i = hay.indexOf(needle); i !== -1; i = hay.indexOf(needle, i + needle.length)) {
    count++;
  }
  return count;
}

/**
 * Sub-actions are wrapped in `multi` batches, so a request count says nothing —
 * only occurrences inside the bodies do.
 */
async function countIn(h: RealHarness, needle: string, from = 0): Promise<number> {
  const reqs = (await h.anki.requests()).slice(from);
  return reqs.reduce((acc, r) => acc + occurrences(r.body, needle), 0);
}

/** `tags` is only ever sent by addNote, and carries exactly the card id. */
const addsOf = (h: RealHarness, lemma: string, from = 0) =>
  countIn(h, `"tags":["${cardIdOf(lemma)}"]`, from);

async function deadPort(): Promise<number> {
  const srv = net.createServer();
  await new Promise<void>((r) => srv.listen(0, '127.0.0.1', r));
  const port = (srv.address() as net.AddressInfo).port;
  await new Promise<void>((r) => srv.close(() => r()));
  return port;
}

/** Block → seed → drain, leaving the sim clean and every card unsynced. */
async function seedWhileBlocked(h: RealHarness, lemmas: string[]): Promise<void> {
  clearCards(h);
  await blockAnki(h);
  await h.anki.seed({ decks: [DECK] });
  await seedCards(h, lemmas);
  await quiesce(h);
  await h.anki.clearRules();
}

const ankiStates = (h: RealHarness) =>
  storedCards(h)
    .map((c) => c.anki_data?.state ?? null)
    .sort();

test.describe('Anki failure injection', () => {
  test('a dead AnkiConnect endpoint is Unreachable and the app stays healthy', async ({
    harness,
  }) => {
    clearCards(harness);
    const original = await harness.invoke<Record<string, unknown>>('get_config');
    const port = await deadPort();
    try {
      await harness.invoke('update_config', {
        config: { ...original, ankiEndpoint: `http://127.0.0.1:${port}` },
      });

      await expect(syncNow(harness)).rejects.toThrow(/AnkiConnect: HTTP request failed/);

      const status = await harness.invoke<Status>('get_anki_sync_status');
      expect(status.state).toBe('unreachable');
      expect(status.lastError).toBeTruthy();
      expect(status.lastFinishedAtMs).toBeGreaterThan(0);
      // Unreachable means version() never got past the probe: no report at all.
      expect(status.lastReport).toBeNull();

      // The rest of the backend is unaffected by the failing sync task.
      expect(await harness.invoke('list_books')).toEqual([]);
      const live = await harness.invoke<Record<string, unknown>>('get_config');
      expect(live.ankiEndpoint).toBe(`http://127.0.0.1:${port}`);
    } finally {
      await harness.invoke('update_config', { config: original });
    }

    // Pointing back at a live endpoint recovers without a restart.
    const report = await syncNow(harness);
    expect(report.failed).toBe(0);
    expect((await harness.invoke<Status>('get_anki_sync_status')).state).toBe('ok');
  });

  test('a failure after addNote never re-adds the notes it already created', async ({
    harness,
  }) => {
    // The retry is gated on the 60s linear backoff, which is real wall clock.
    test.setTimeout(180_000);
    const lemmas = nonceLemmas(3);
    await seedWhileBlocked(harness, lemmas);

    // notesInfo is the pull that follows the addNote batch: the notes are
    // already in Anki, but every card is recorded as failed.
    await harness.anki.addRule({
      matcher: { bodyContains: '"action":"notesInfo"' },
      action: { type: 'status', code: 500 },
      times: 1,
    });

    const first = await syncNow(harness);
    expect(first.totalCards).toBe(3);
    expect(first.attempted).toBe(3);
    expect(first.succeeded).toBe(0);
    expect(first.failed).toBe(3);
    // Pinned contract: per-card failures do NOT flip the status surface — a
    // pass that returns Ok is `ok` however many cards inside it failed.
    const afterFirst = await harness.invoke<Status>('get_anki_sync_status');
    expect(afterFirst.state).toBe('ok');
    expect(afterFirst.lastReport?.failed).toBe(3);

    for (const lemma of lemmas) expect(await addsOf(harness, lemma)).toBe(1);
    expect(ankiStates(harness)).toEqual([null, null, null]);

    // Backoff: the immediate retry skips every failed card.
    const second = await syncNow(harness);
    expect(second.totalCards).toBe(3);
    expect(second.attempted).toBe(0);

    await sleep(62_000);

    const mark = (await harness.anki.requests()).length;
    const third = await syncNow(harness);
    expect(third.attempted).toBe(3);
    expect(third.succeeded).toBe(3);
    expect(third.failed).toBe(0);
    // Convergence went through updateNoteFields — never a second addNote.
    expect(await countIn(harness, '"action":"updateNoteFields"', mark)).toBe(3);
    for (const lemma of lemmas) expect(await addsOf(harness, lemma)).toBe(1);
    expect(ankiStates(harness)).toEqual(['active', 'active', 'active']);
  });

  test('a second sync updates the existing notes instead of re-adding them', async ({
    harness,
  }) => {
    const lemmas = nonceLemmas(3);
    await seedWhileBlocked(harness, lemmas);

    const first = await syncNow(harness);
    expect(first.totalCards).toBe(3);
    expect(first.succeeded).toBe(3);
    for (const lemma of lemmas) expect(await addsOf(harness, lemma)).toBe(1);

    const mark = (await harness.anki.requests()).length;
    const second = await syncNow(harness);
    expect(second.succeeded).toBe(3);

    expect(await countIn(harness, '"action":"addNote"', mark)).toBe(0);
    expect(await countIn(harness, '"action":"updateNoteFields"', mark)).toBe(3);
    for (const lemma of lemmas) expect(await addsOf(harness, lemma)).toBe(1);
    expect(ankiStates(harness)).toEqual(['active', 'active', 'active']);
  });

  test('recovers after an AnkiConnect outage and re-syncs without re-adding', async ({
    harness,
  }) => {
    const lemmas = nonceLemmas(3);
    await seedWhileBlocked(harness, lemmas);

    await harness.anki.addRule({ action: { type: 'drop' } });
    await expect(syncNow(harness)).rejects.toThrow(/AnkiConnect/);
    expect((await harness.invoke<Status>('get_anki_sync_status')).state).toBe('unreachable');
    // The outage died on the version() probe, so no card is in backoff and the
    // recovery does not have to wait one out.
    expect(ankiStates(harness)).toEqual([null, null, null]);

    // reset drops the rules and the sim's collection — deck included.
    await harness.anki.reset();
    await harness.anki.seed({ decks: [DECK] });

    const recovered = await syncNow(harness);
    expect(recovered.totalCards).toBe(3);
    expect(recovered.succeeded).toBe(3);
    expect(recovered.failed).toBe(0);
    for (const lemma of lemmas) expect(await addsOf(harness, lemma)).toBe(1);
    expect(ankiStates(harness)).toEqual(['active', 'active', 'active']);

    const mark = (await harness.anki.requests()).length;
    const settled = await syncNow(harness);
    expect(settled.succeeded).toBe(3);
    expect(await countIn(harness, '"action":"addNote"', mark)).toBe(0);
    expect(await countIn(harness, '"action":"updateNoteFields"', mark)).toBe(3);
  });
});
