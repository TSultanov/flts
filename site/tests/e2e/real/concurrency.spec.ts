import { test, expect } from "../../real/fixtures";
import type { RealHarness } from "../../real/fixtures";
import {
  MODEL,
  STREAM_GLOB,
  drained,
  lemmaSets,
  seedBook,
  storedIds,
  storedSegments,
  textOf,
  translationJson,
} from "../../real/spec-helpers";

/**
 * Concurrency: several paragraph translations in flight at once, and other
 * writers (reading state) racing them. All chapter 0 — translated
 * paragraphs in chapter >0 pay for the preceding chapters' summaries.
 *
 * Everything runs over the bridge; there is no UI for the queue's parallelism.
 */

const scriptsFor = (sets: string[][]) =>
  sets.map((lemmas) => ({
    matchSubstring: textOf(lemmas),
    translation: translationJson(lemmas),
  }));

const seed = (h: RealHarness, sets: string[][]) =>
  seedBook(h, sets, "concurrency");

async function streamCallsFor(h: RealHarness, text: string): Promise<number> {
  const reqs = await h.llm.requests();
  return reqs.filter(
    (r) => r.path.endsWith(":streamGenerateContent") && r.body.includes(text),
  ).length;
}

const activityOf = (h: RealHarness, bookId: string, paragraphId: number) =>
  h.invoke<unknown>("get_paragraph_translation_activity", {
    bookId,
    paragraphId,
  });

test.describe("translation queue concurrency", () => {
  test("a stalled lane does not block the rest of the chapter", async ({
    harness,
  }) => {
    test.setTimeout(120_000);
    const original =
      await harness.invoke<Record<string, unknown>>("get_config");
    const sets = lemmaSets(6);
    const texts = sets.map(textOf);
    const held = 3;
    const unheld = [0, 1, 2, 4, 5];

    try {
      // update_config rebuilds the queue, so the new bound takes effect here.
      await harness.invoke("update_config", {
        config: { ...original, translationConcurrency: 3 },
      });
      const bookId = await seed(harness, sets);

      // Path-scoped as well as body-scoped: the chapter summary is a unary call
      // that quotes every paragraph, and stalling it would block everything.
      await harness.llm.addRule({
        matcher: { pathGlob: STREAM_GLOB, bodyContains: texts[held] },
        action: { type: "stall" },
      });
      await harness.invoke("translate_chapter", {
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
      for (const id of unheld)
        expect(await streamCallsFor(harness, texts[id])).toBe(1);

      // reset is the only stall release; it also wipes scripts, so re-seed.
      await harness.llm.reset();
      await harness.llm.seed({ scripts: scriptsFor(sets) });

      // The released request retries on its own, but it can lose the race with
      // the re-seed (or hit the one-off cachedContent 404); re-drive only once
      // the lane is genuinely free.
      await expect
        .poll(
          async () => {
            if ((await storedSegments(harness, bookId, held)) !== null)
              return true;
            if ((await activityOf(harness, bookId, held)) === null) {
              await harness.invoke("translate_paragraph", {
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
      await expect
        .poll(() => drained(harness), { timeout: 30_000 })
        .toEqual([]);
    } finally {
      await harness.invoke("update_config", { config: original });
    }
  });

  test("a retrying paragraph never bleeds into its neighbours", async ({
    harness,
  }) => {
    test.setTimeout(120_000);
    const sets = lemmaSets(6);
    const texts = sets.map(textOf);
    const failing = 2;
    const bookId = await seed(harness, sets);

    await harness.llm.addRule({
      matcher: { pathGlob: STREAM_GLOB, bodyContains: texts[failing] },
      action: { type: "status", code: 503, body: { error: "sim overloaded" } },
      times: 2,
    });
    await harness.invoke("translate_chapter", {
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
      const words = segments.filter((s) => s.kind === "word");
      expect(words.map((w) => w.text)).toEqual(lemmas);
      expect(words.map((w) => w.translation)).toEqual(
        lemmas.map((l) => `t-${l}`),
      );
    }
  });

  test("reading-state writes land intact while translations are saving", async ({
    harness,
  }) => {
    test.setTimeout(120_000);
    const sets = lemmaSets(6);
    const bookId = await seed(harness, sets);

    // Keeps the translations in flight across the reading-state writes.
    await harness.llm.addRule({
      matcher: { pathGlob: STREAM_GLOB },
      action: { type: "delay", ms: 800 },
    });
    await harness.invoke("translate_chapter", {
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
      await harness.invoke("save_book_reading_state", {
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
    expect(
      await harness.invoke("get_book_reading_state", { bookId }),
    ).toMatchObject({
      chapterId: 0,
      paragraphId: 5,
      pageOffset: 5,
    });
    for (const [id, lemmas] of sets.entries()) {
      const words = (await storedSegments(harness, bookId, id))!.filter(
        (s) => s.kind === "word",
      );
      expect(words.map((w) => w.text)).toEqual(lemmas);
    }
  });
});
