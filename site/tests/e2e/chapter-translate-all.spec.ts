import { type Page } from "@playwright/test";
import { expect, test } from "./helpers/test";
import { isRealMode } from "./helpers/backend-mode";
import { fillerHtml, fillerSegments, seedAndOpen } from "./helpers/paragraph";

// The floating "Translate chapter" button: one `translate_chapter` IPC call
// fans out to every untranslated paragraph. Visible only below 100%, disabled
// while `canTranslate(chapterId)` is false.

test.describe.configure({ mode: "parallel" });

const BUTTON = '[data-testid="translate-chapter-button"]';

function chapterSpec(
  paragraphCount: number,
  translatedCount: number,
  offset = 0,
) {
  return {
    title: `Chapter with ${translatedCount}/${paragraphCount} translated`,
    paragraphs: Array.from({ length: paragraphCount }, (_, i) => ({
      html: fillerHtml(i + offset),
      ...(i < translatedCount ? { segments: fillerSegments(i + offset) } : {}),
    })),
  };
}

async function getTranslateChapterCalls(page: Page): Promise<
  Array<{
    bookId: string;
    chapterId: number;
    useCache: boolean;
    model: unknown;
    enqueuedCount: number;
  }>
> {
  return page.evaluate(() => (window as any).__test.getTranslateChapterCalls());
}

async function getTranslateCalls(page: Page): Promise<
  Array<{
    bookId: string;
    paragraphId: number;
    useCache: boolean;
    model: unknown;
  }>
> {
  return page.evaluate(() => (window as any).__test.getTranslateCalls());
}

test.describe("translate-chapter button — visibility", () => {
  test.skip(
    ({ browserName }) => browserName === "firefox",
    "chromium + webkit only",
  );

  test("hidden when the open chapter is 100% translated", async ({ page }) => {
    await seedAndOpen(page, {
      chapters: [chapterSpec(3, 3), chapterSpec(2, 0, 10)],
    });

    await expect(page.locator(".paragraphs-container.is-ready")).toBeVisible();
    await expect(page.locator(BUTTON)).toHaveCount(0);
  });

  test("visible when the open chapter has untranslated paragraphs", async ({
    page,
  }) => {
    await seedAndOpen(page, {
      chapters: [chapterSpec(4, 1), chapterSpec(2, 0, 10)],
    });

    await expect(page.locator(BUTTON)).toBeVisible();
  });
});

test.describe("translate-chapter button — click behaviour", () => {
  test.skip(
    ({ browserName }) => browserName === "firefox",
    "chromium + webkit only",
  );

  test("clicking schedules every untranslated paragraph via one translate_chapter call", async ({
    page,
  }) => {
    test.skip(isRealMode(), "mock-only __test.getTranslateChapterCalls");
    const { bookId } = await seedAndOpen(page, {
      chapters: [chapterSpec(3, 1), chapterSpec(2, 0, 10)],
      // The default 'immediate' config emits paragraph_updated without setting
      // segments, so they must be explicit.
      translateConfigs: [
        {
          paragraphId: 1,
          cfg: { kind: "immediate", segments: fillerSegments(1) },
        },
        {
          paragraphId: 2,
          cfg: { kind: "immediate", segments: fillerSegments(2) },
        },
      ],
    });

    await expect(page.locator(BUTTON)).toBeVisible();
    await page.locator(BUTTON).click();

    // One fan-out call, scoped to the open chapter, counting 3 - 1 = 2.
    await expect
      .poll(async () => (await getTranslateChapterCalls(page)).length)
      .toBe(1);
    const calls = await getTranslateChapterCalls(page);
    expect(calls[0].bookId).toBe(bookId);
    expect(calls[0].chapterId).toBe(0);
    expect(calls[0].enqueuedCount).toBe(2);

    // The fan-out is server-side; no per-paragraph calls.
    expect(await getTranslateCalls(page)).toHaveLength(0);

    // Paragraph 0 was seeded translated and must not be re-enqueued.
    await expect(
      page.locator(".paragraph-wrapper button.translate"),
    ).toHaveCount(0);
  });

  test("button hides reactively once the last paragraph lands", async ({
    page,
  }) => {
    await seedAndOpen(page, {
      chapters: [chapterSpec(2, 1), chapterSpec(2, 0, 10)],
      translateConfigs: [
        {
          paragraphId: 1,
          cfg: { kind: "immediate", segments: fillerSegments(1) },
        },
      ],
    });

    await expect(page.locator(BUTTON)).toBeVisible();
    await page.locator(BUTTON).click();

    await expect(page.locator(BUTTON)).toHaveCount(0);
  });
});

test.describe("translate-chapter button — summary gating", () => {
  test.skip(
    ({ browserName }) => browserName === "firefox",
    "chromium + webkit only",
  );

  test("disabled when prior-chapter summary is not yet generated", async ({
    page,
  }) => {
    // Needs a half-generated summaryStatus, which the real backend never has.
    test.skip(isRealMode(), "summaryStatus seeding is mock-only");
    // canTranslate(1) requires chapter 0's summary, still ungenerated.
    const bookId = `test-book-summary-gating-${Date.now()}`;
    await seedAndOpen(
      page,
      {
        bookId,
        chapters: [chapterSpec(2, 2), chapterSpec(3, 0, 10)],
        summaryStatus: {
          generated: [false, false],
          activelyGenerating: 0,
        },
      },
      { path: `/book/${bookId}/1` },
    );

    await expect(page.locator(".paragraphs-container.is-ready")).toBeVisible();

    await expect(page.locator(BUTTON)).toBeVisible();
    await expect(page.locator(BUTTON)).toBeDisabled();

    // Advancing the worker completes chapter 0's summary.
    await page.evaluate(
      (id) => (window as any).__test.advanceSummaryGeneration(id),
      bookId,
    );
    await expect(page.locator(BUTTON)).toBeEnabled();
  });
});
