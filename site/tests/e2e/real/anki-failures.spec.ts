import net from "node:net";
import { test, expect } from "../../real/fixtures";
import type { RealHarness } from "../../real/fixtures";
import {
  DECK,
  MODEL,
  SRC,
  addsOf,
  blockAnki,
  cardIdOf,
  clearCards,
  countIn,
  nonceSeed,
  quiesceAnki,
  sleep,
  storedCards,
  syncNow,
  translationJson,
  type Status,
} from "../../real/spec-helpers";

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

/** Lowercase ASCII only: lemma slug == lemma, so the card id is predictable. */
function nonceLemmas(n: number): string[] {
  const seed = nonceSeed();
  return Array.from({ length: n }, (_, i) => `w${seed}${"abcdefgh"[i]}`);
}

/** Import a one-paragraph book, translate it, and wait for the cards on disk. */
async function seedCards(h: RealHarness, lemmas: string[]): Promise<void> {
  const text = lemmas.join(" ");
  await h.llm.seed({
    scripts: [{ matchSubstring: text, translation: translationJson(lemmas) }],
  });
  const bookId = await h.invoke<string>("import_plain_text", {
    title: "anki-failures",
    text,
    sourceLanguageId: SRC,
  });
  await h.invoke("translate_paragraph", {
    bookId,
    paragraphId: 0,
    model: MODEL,
    useCache: false,
  });
  await expect
    .poll(
      () =>
        storedCards(h)
          .map((c) => c.id)
          .sort(),
      { timeout: 30_000 },
    )
    .toEqual(lemmas.map(cardIdOf).sort());
}

async function deadPort(): Promise<number> {
  const srv = net.createServer();
  await new Promise<void>((r) => srv.listen(0, "127.0.0.1", r));
  const port = (srv.address() as net.AddressInfo).port;
  await new Promise<void>((r) => srv.close(() => r()));
  return port;
}

/** Block → seed → drain, leaving the sim clean and every card unsynced. */
async function seedWhileBlocked(
  h: RealHarness,
  lemmas: string[],
): Promise<void> {
  clearCards(h);
  await blockAnki(h);
  await h.anki.seed({ decks: [DECK] });
  await seedCards(h, lemmas);
  await quiesceAnki(h);
  await h.anki.clearRules();
}

const ankiStates = (h: RealHarness) =>
  storedCards(h)
    .map((c) => c.anki_data?.state ?? null)
    .sort();

test.describe("Anki failure injection", () => {
  test("a dead AnkiConnect endpoint is Unreachable and the app stays healthy", async ({
    harness,
  }) => {
    clearCards(harness);
    const original =
      await harness.invoke<Record<string, unknown>>("get_config");
    const port = await deadPort();
    try {
      await harness.invoke("update_config", {
        config: { ...original, ankiEndpoint: `http://127.0.0.1:${port}` },
      });

      await expect(syncNow(harness)).rejects.toThrow(
        /AnkiConnect: HTTP request failed/,
      );

      const status = await harness.invoke<Status>("get_anki_sync_status");
      expect(status.state).toBe("unreachable");
      expect(status.lastError).toBeTruthy();
      expect(status.lastFinishedAtMs).toBeGreaterThan(0);
      // Unreachable means version() never got past the probe: no report at all.
      expect(status.lastReport).toBeNull();

      // The rest of the backend is unaffected by the failing sync task.
      expect(await harness.invoke("list_books")).toEqual([]);
      const live = await harness.invoke<Record<string, unknown>>("get_config");
      expect(live.ankiEndpoint).toBe(`http://127.0.0.1:${port}`);
    } finally {
      await harness.invoke("update_config", { config: original });
    }

    // Pointing back at a live endpoint recovers without a restart.
    const report = await syncNow(harness);
    expect(report.failed).toBe(0);
    expect((await harness.invoke<Status>("get_anki_sync_status")).state).toBe(
      "ok",
    );
  });

  test("a failure after addNote never re-adds the notes it already created", async ({
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
      action: { type: "status", code: 500 },
      times: 1,
    });

    const first = await syncNow(harness);
    expect(first.totalCards).toBe(3);
    expect(first.attempted).toBe(3);
    expect(first.succeeded).toBe(0);
    expect(first.failed).toBe(3);
    // Pinned contract: per-card failures do NOT flip the status surface — a
    // pass that returns Ok is `ok` however many cards inside it failed.
    const afterFirst = await harness.invoke<Status>("get_anki_sync_status");
    expect(afterFirst.state).toBe("ok");
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
    expect(ankiStates(harness)).toEqual(["active", "active", "active"]);
  });

  test("a second sync updates the existing notes instead of re-adding them", async ({
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
    expect(ankiStates(harness)).toEqual(["active", "active", "active"]);
  });

  test("recovers after an AnkiConnect outage and re-syncs without re-adding", async ({
    harness,
  }) => {
    const lemmas = nonceLemmas(3);
    await seedWhileBlocked(harness, lemmas);

    await harness.anki.addRule({ action: { type: "drop" } });
    await expect(syncNow(harness)).rejects.toThrow(/AnkiConnect/);
    expect((await harness.invoke<Status>("get_anki_sync_status")).state).toBe(
      "unreachable",
    );
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
    expect(ankiStates(harness)).toEqual(["active", "active", "active"]);

    const mark = (await harness.anki.requests()).length;
    const settled = await syncNow(harness);
    expect(settled.succeeded).toBe(3);
    expect(await countIn(harness, '"action":"addNote"', mark)).toBe(0);
    expect(await countIn(harness, '"action":"updateNoteFields"', mark)).toBe(3);
  });
});
