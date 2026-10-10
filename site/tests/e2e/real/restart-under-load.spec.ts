import fs from "node:fs";
import path from "node:path";
import { test, expect } from "../../real/fixtures";
import type { RealHarness } from "../../real/fixtures";
import {
  MODEL,
  STREAM_GLOB,
  drained,
  lemmaSets,
  seedBook,
  storedIds,
} from "../../real/spec-helpers";

/**
 * The app process dies mid-work. SIGTERM gets the graceful shutdown path;
 * SIGKILL gets none, so whatever was mid-write is exactly where it stopped.
 * Only the app restarts — the sims keep their state across it.
 */

/** `Config::default()`; a restart that lost the config dir would show this. */
const DEFAULT_ANKI_ENDPOINT = "http://127.0.0.1:8765";

/**
 * First call after a relaunch, deliberately unpolled: library-backed commands
 * await the readiness gate, so an answer is a *configured* answer.
 */
async function expectLibraryHas(h: RealHarness, bookId: string): Promise<void> {
  const ids = (await h.invoke<Array<{ id: string }>>("list_books")).map(
    (b) => b.id,
  );
  expect(ids).toContain(bookId);
}

/** Serialize the queue and pace it, so "mid-batch" is a real, wide window. */
async function startPacedChapter(
  h: RealHarness,
  original: Record<string, unknown>,
  sets: string[][],
): Promise<string> {
  await h.invoke("update_config", {
    config: { ...original, translationConcurrency: 1 },
  });
  const bookId = await seedBook(h, sets, "restart-under-load");
  await h.llm.addRule({
    matcher: { pathGlob: STREAM_GLOB },
    action: { type: "delay", ms: 1200 },
  });
  await h.invoke("translate_chapter", {
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
  await expectLibraryHas(h, bookId);
  // Nothing that was already on disk was lost with the process.
  expect(await storedIds(h, bookId, 6)).toEqual(expect.arrayContaining(before));
  const chapters = await h.invoke<unknown[]>("list_book_chapters", { bookId });
  expect(chapters.length).toBe(1);

  await h.llm.clearRules();
  await h.invoke("translate_chapter", {
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

test.describe("restart under load", () => {
  test("SIGTERM mid-batch loses nothing and the chapter finishes after relaunch", async ({
    harness,
  }) => {
    test.setTimeout(120_000);
    const original =
      await harness.invoke<Record<string, unknown>>("get_config");
    try {
      const bookId = await startPacedChapter(harness, original, lemmaSets(6));
      const before = await partwayIds(harness, bookId);

      const oldPort = harness.bridgePort;
      await harness.restartApp();
      expect(harness.bridgePort).not.toBe(oldPort);

      await finishChapter(harness, bookId, before);
    } finally {
      await harness.invoke("update_config", { config: original });
    }
  });

  test("SIGKILL mid-batch leaves the config intact and the chapter finishable", async ({
    harness,
  }) => {
    test.setTimeout(120_000);
    const original =
      await harness.invoke<Record<string, unknown>>("get_config");
    try {
      const bookId = await startPacedChapter(harness, original, lemmaSets(6));
      const before = await partwayIds(harness, bookId);

      await harness.restartApp({ signal: "SIGKILL" });

      // The atomic config save survived an un-graceful death: nothing was
      // quarantined as unparseable...
      expect(
        fs.existsSync(path.join(harness.configDir, "config.json.corrupt")),
      ).toBe(false);
      // ...and the harness config is still the live one, not Config::default().
      const live = await harness.invoke<Record<string, unknown>>("get_config");
      expect(live.ankiEndpoint).toBe(original.ankiEndpoint);
      expect(live.ankiEndpoint).not.toBe(DEFAULT_ANKI_ENDPOINT);
      expect(live.translationConcurrency).toBe(1);

      await finishChapter(harness, bookId, before);
    } finally {
      await harness.invoke("update_config", { config: original });
    }
  });
});
