import fs from "node:fs";
import path from "node:path";
import { test, expect } from "../../real/fixtures";
import {
  MODEL,
  STREAM_GLOB,
  drained,
  lemmaSets,
  seedBook,
  storedSegments,
} from "../../real/spec-helpers";

type ShelfBook = { id: string; path: string[] };

/**
 * Makes every store commit fail until the returned restore runs. A commit
 * appends in place to this device's shard, creates the shard of a table it has
 * not written yet, or rewrites one through a temporary beside it; read-only
 * files and directories under `<root>/db` refuse all three and leave reads
 * working.
 */
function freezeStore(root: string): () => void {
  const modes: Array<[string, number]> = [];
  const walk = (entry: string) => {
    const stat = fs.statSync(entry);
    modes.push([entry, stat.mode & 0o777]);
    fs.chmodSync(entry, stat.isDirectory() ? 0o555 : 0o444);
    if (stat.isDirectory()) {
      for (const name of fs.readdirSync(entry)) walk(path.join(entry, name));
    }
  };
  walk(path.join(root, "db"));
  return () => {
    for (const [entry, mode] of modes) fs.chmodSync(entry, mode);
  };
}

test("a failed save neither blocks a settings change nor loses the edit", async ({
  harness,
}) => {
  const original = await harness.invoke<Record<string, unknown>>("get_config");
  const bookId = await seedBook(harness, lemmaSets(1), "failed-flush-owner");
  const root = await harness.invoke<string>("get_library_root");
  const changed = Number(original.translationConcurrency) + 1;
  const thaw = freezeStore(root);
  try {
    await expect(
      harness.invoke("move_book", { bookId, path: ["held"] }),
    ).rejects.toThrow();
    // The flush of the still-dirty book fails too; the change applies anyway.
    await harness.invoke("update_config", {
      config: { ...original, translationConcurrency: changed },
    });
    const live = await harness.invoke<Record<string, unknown>>("get_config");
    expect(live.translationConcurrency).toBe(changed);
    await expect(storedSegments(harness, bookId, 0)).resolves.toBeNull();
  } finally {
    thaw();
    // Flushes what stayed dirty.
    await harness.invoke("update_config", { config: original });
  }
  await harness.restartApp();
  const shelf = await harness.invoke<ShelfBook[]>("list_books");
  expect(shelf.find((book) => book.id === bookId)?.path).toEqual(["held"]);
});

test("an in-flight translation cannot recreate a deleted book", async ({
  harness,
}) => {
  const bookId = await seedBook(harness, lemmaSets(1), "deleted-owner");
  await harness.llm.addRule({
    matcher: { pathGlob: STREAM_GLOB },
    action: { type: "stall" },
  });
  await harness.invoke("translate_paragraph", {
    bookId,
    paragraphId: 0,
    model: MODEL,
    useCache: false,
  });
  await expect
    .poll(
      async () =>
        (await harness.llm.requests()).filter((r) =>
          r.path.endsWith(":streamGenerateContent"),
        ).length,
    )
    .toBeGreaterThan(0);
  await harness.invoke("delete_book", { bookId });
  await harness.llm.reset();
  await expect.poll(() => drained(harness), { timeout: 15_000 }).toEqual([]);
  const shelf = await harness.invoke<ShelfBook[]>("list_books");
  expect(shelf.map((book) => book.id)).not.toContain(bookId);
  await expect(
    harness.invoke("translate_paragraph", {
      bookId,
      paragraphId: 0,
      model: MODEL,
      useCache: false,
    }),
  ).rejects.toThrow();
});

test("a large chapter uses bounded admission and shutdown still drains", async ({
  harness,
}) => {
  const original = await harness.invoke<Record<string, unknown>>("get_config");
  await harness.invoke("update_config", {
    config: { ...original, translationConcurrency: 1 },
  });
  try {
    const bookId = await harness.invoke<string>("import_plain_text", {
      title: "bounded-chapter",
      text: Array.from({ length: 1110 }, (_, i) => `queue paragraph ${i}`).join(
        "\n",
      ),
      sourceLanguageId: "deu",
    });
    await harness.llm.addRule({
      matcher: { pathGlob: STREAM_GLOB },
      action: { type: "stall" },
    });
    await harness.invoke("translate_chapter", {
      bookId,
      chapterId: 0,
      model: MODEL,
      useCache: false,
    });
    await expect
      .poll(
        async () =>
          (await harness.llm.requests()).filter((r) =>
            r.path.endsWith(":streamGenerateContent"),
          ).length,
      )
      .toBe(1);
    await expect.poll(async () => (await drained(harness)).length).toBe(1025);
    await expect(
      harness.invoke("translate_paragraph", {
        bookId,
        paragraphId: 1109,
        model: MODEL,
        useCache: false,
      }),
    ).rejects.toThrow(/busy/);
  } finally {
    await harness.invoke("update_config", { config: original });
  }
  await expect.poll(() => drained(harness)).toEqual([]);
});
