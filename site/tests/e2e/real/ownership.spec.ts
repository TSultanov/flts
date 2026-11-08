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

test("a failed flush preserves the working session and configuration", async ({
  harness,
}) => {
  const original = await harness.invoke<Record<string, unknown>>("get_config");
  const bookId = await seedBook(harness, lemmaSets(1), "failed-flush-owner");
  const root = await harness.invoke<string>("get_library_root");
  const invalid = path.join(root, bookId, "translation_zzz_yyy.dat");
  fs.writeFileSync(invalid, "invalid translation metadata");
  try {
    await expect(
      harness.invoke("update_config", {
        config: {
          ...original,
          translationConcurrency: Number(original.translationConcurrency) + 1,
        },
      }),
    ).rejects.toThrow();
    const live = await harness.invoke<Record<string, unknown>>("get_config");
    expect(live.translationConcurrency).toBe(original.translationConcurrency);
    expect(
      JSON.parse(
        fs.readFileSync(path.join(harness.configDir, "config.json"), "utf8"),
      ).translationConcurrency,
    ).toBe(original.translationConcurrency);
    await expect(storedSegments(harness, bookId, 0)).resolves.toBeNull();
  } finally {
    fs.rmSync(invalid, { force: true });
    await harness.invoke("update_config", { config: original });
  }
});

test("an in-flight translation cannot recreate a deleted book", async ({
  harness,
}) => {
  const bookId = await seedBook(harness, lemmaSets(1), "deleted-owner");
  const root = await harness.invoke<string>("get_library_root");
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
  expect(fs.existsSync(path.join(root, bookId))).toBe(false);
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
