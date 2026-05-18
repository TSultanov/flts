import { expect, test } from '@playwright/test';
import { multipageSpec, paragraphLocator, seedAndOpen } from './helpers/paragraph';

// Regression suite for chapter reading-position restore.
//
// Opening a book with a previously-saved `readingState` should scroll the
// horizontal page-flip container so the saved paragraph lands in view. The
// path runs: LibraryView -> /book/{id} -> BookView fetches readingState ->
// navigates to /book/{id}/{savedChapter} -> ChapterView mounts with
// `initialParagraphId` already set -> ChapterViewModel.startInitialSync
// -> scrollIntoView on the matching wrapper.
//
// IMPORTANT: the regression only reproduces via the library->book flow. A
// direct goto('/book/{id}/0') happens to work because ChapterView mounts
// once with initialParagraphId=null, syncs to paragraph 0, then re-runs
// the sync when readingState arrives — which lazy-mounts the world before
// the scroll fires. Real users click the book from the library, which
// mounts ChapterView a single time with initialParagraphId already
// populated, and that timing is what the refactor broke.
//
// These run on both Chromium and WebKit because WKWebView is the
// production engine.
test.describe('Chapter reading-state restore (multipage)', () => {
  test.skip(({ browserName }) => browserName === 'firefox', 'chromium + webkit only');

  const COUNT = 80;
  const TARGET = 40;
  // Restore goes through one tick() retry inside scrollParagraphIntoView and
  // a subsequent recomputeMountWindow. 3s is comfortably above the worst
  // observed end-to-end latency on either engine.
  const POLL = { timeout: 3000, intervals: [50, 100, 200] } as const;

  async function openBookFromLibrary(
    page: import('@playwright/test').Page,
    bookId: string,
  ) {
    // Same path a real user takes: BookView mounts at /book/{id} with no
    // chapter, fetches readingState, then navigates to /book/{id}/{savedChapter}
    // so ChapterView mounts ONCE with initialParagraphId already set.
    await page.locator(`a[href="/book/${bookId}"]`).first().click();
    await page.waitForSelector('.paragraphs-container');
  }

  // ----- R1 ---------------------------------------------------------------
  test('R1: saved paragraph is in view after opening the book from the library', async ({
    page,
  }) => {
    const { bookId } = await seedAndOpen(
      page,
      multipageSpec(COUNT, {}, {
        readingState: { chapterId: 0, paragraphId: TARGET },
      }),
      { path: '/library' },
    );
    await openBookFromLibrary(page, bookId);

    const target = paragraphLocator(page, TARGET);
    await expect(target).toBeAttached();

    await expect
      .poll(
        async () =>
          page.evaluate((id) => {
            const container = document.querySelector(
              '.paragraphs-container',
            ) as HTMLElement | null;
            const el = document.querySelector(
              `.paragraph-wrapper[data-paragraph-id="${id}"]`,
            ) as HTMLElement | null;
            if (!container || !el) return false;
            const cr = container.getBoundingClientRect();
            const er = el.getBoundingClientRect();
            return er.right > cr.left && er.left < cr.right;
          }, TARGET),
        POLL,
      )
      .toBe(true);
  });

  // ----- R2 ---------------------------------------------------------------
  test('R2: with no saved state the book opens on paragraph 0', async ({ page }) => {
    const { bookId } = await seedAndOpen(page, multipageSpec(COUNT), {
      path: '/library',
    });
    await openBookFromLibrary(page, bookId);

    const first = paragraphLocator(page, 0);
    await expect(first).toBeAttached();

    // Settle a beat in case anything async would scroll us off paragraph 0.
    await page.waitForTimeout(200);

    const scrollLeft = await page.evaluate(() => {
      const c = document.querySelector(
        '.paragraphs-container',
      ) as HTMLElement | null;
      return c ? c.scrollLeft : -1;
    });
    expect(scrollLeft).toBeGreaterThanOrEqual(0);
    expect(scrollLeft).toBeLessThan(50);
  });
});
