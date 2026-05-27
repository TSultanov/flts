import { expect, test, type Page } from '@playwright/test';
import {
  fillerHtml,
  fillerSegments,
  seedAndOpen,
} from './helpers/paragraph';

// Covers the floating "Translate chapter" button rendered on the dark
// frame of ChapterView. The button schedules translation of every
// untranslated paragraph in the open chapter via a single
// `translate_chapter` IPC call:
//  - hidden when the chapter is already 100% translated
//  - visible when the chapter has at least one untranslated paragraph
//  - clicking enqueues every untranslated paragraph; already-translated
//    paragraphs are not re-enqueued
//  - disabled while `canTranslate(chapterId)` is false (waiting on prior
//    chapter summaries); reactivates once summaries advance
//  - hides reactively when the last paragraph lands and the chapter
//    becomes 100% translated

test.describe.configure({ mode: 'parallel' });

const BUTTON = '[data-testid="translate-chapter-button"]';

function chapterSpec(paragraphCount: number, translatedCount: number, offset = 0) {
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
  return page.evaluate(() =>
    (window as any).__test.getTranslateChapterCalls(),
  );
}

async function getTranslateCalls(page: Page): Promise<
  Array<{ bookId: string; paragraphId: number; useCache: boolean; model: unknown }>
> {
  return page.evaluate(() => (window as any).__test.getTranslateCalls());
}

test.describe('translate-chapter button — visibility', () => {
  test.skip(({ browserName }) => browserName === 'firefox', 'chromium + webkit only');

  test('hidden when the open chapter is 100% translated', async ({ page }) => {
    // Two chapters so single-chapter shortcuts don't kick in.
    await seedAndOpen(page, {
      chapters: [chapterSpec(3, 3), chapterSpec(2, 0, 10)],
    });

    await expect(page.locator('.paragraphs-container.is-ready')).toBeVisible();
    await expect(page.locator(BUTTON)).toHaveCount(0);
  });

  test('visible when the open chapter has untranslated paragraphs', async ({ page }) => {
    await seedAndOpen(page, {
      chapters: [chapterSpec(4, 1), chapterSpec(2, 0, 10)],
    });

    await expect(page.locator(BUTTON)).toBeVisible();
  });
});

test.describe('translate-chapter button — click behaviour', () => {
  test.skip(({ browserName }) => browserName === 'firefox', 'chromium + webkit only');

  test('clicking schedules every untranslated paragraph via one translate_chapter call', async ({
    page,
  }) => {
    const { bookId } = await seedAndOpen(page, {
      chapters: [chapterSpec(3, 1), chapterSpec(2, 0, 10)],
      // Provide explicit segments for the untranslated paragraphs so the
      // mock translator actually fills them in (the default 'immediate'
      // config emits paragraph_updated but doesn't set segments).
      translateConfigs: [
        { paragraphId: 1, cfg: { kind: 'immediate', segments: fillerSegments(1) } },
        { paragraphId: 2, cfg: { kind: 'immediate', segments: fillerSegments(2) } },
      ],
    });

    await expect(page.locator(BUTTON)).toBeVisible();
    await page.locator(BUTTON).click();

    // Exactly one server-side fan-out call, scoped to the open chapter,
    // with the count of untranslated paragraphs (3 - 1 = 2).
    await expect
      .poll(async () => (await getTranslateChapterCalls(page)).length)
      .toBe(1);
    const calls = await getTranslateChapterCalls(page);
    expect(calls[0].bookId).toBe(bookId);
    expect(calls[0].chapterId).toBe(0);
    expect(calls[0].enqueuedCount).toBe(2);

    // Per-paragraph translate_paragraph is NOT how this flow works —
    // the fan-out happens on the backend.
    expect(await getTranslateCalls(page)).toHaveLength(0);

    // The two previously-untranslated paragraphs land. The original
    // paragraph 0 was seeded translated and never re-enqueued.
    await expect(page.locator('.paragraph-wrapper button.translate')).toHaveCount(0);
  });

  test('button hides reactively once the last paragraph lands', async ({ page }) => {
    await seedAndOpen(page, {
      chapters: [chapterSpec(2, 1), chapterSpec(2, 0, 10)],
      translateConfigs: [
        { paragraphId: 1, cfg: { kind: 'immediate', segments: fillerSegments(1) } },
      ],
    });

    await expect(page.locator(BUTTON)).toBeVisible();
    await page.locator(BUTTON).click();

    // book_updated → chapter ratio flips to 1 → button unmounts.
    await expect(page.locator(BUTTON)).toHaveCount(0);
  });
});

test.describe('translate-chapter button — summary gating', () => {
  test.skip(({ browserName }) => browserName === 'firefox', 'chromium + webkit only');

  test('disabled when prior-chapter summary is not yet generated', async ({ page }) => {
    // Open chapter 1; canTranslate(1) requires chapter 0's summary,
    // which is not yet generated.
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

    await expect(page.locator('.paragraphs-container.is-ready')).toBeVisible();

    await expect(page.locator(BUTTON)).toBeVisible();
    await expect(page.locator(BUTTON)).toBeDisabled();

    // Advance the summary worker → chapter 0 summary done →
    // canTranslate(1) flips true → button enables.
    await page.evaluate(
      (id) => (window as any).__test.advanceSummaryGeneration(id),
      bookId,
    );
    await expect(page.locator(BUTTON)).toBeEnabled();
  });
});
