import { expect, test } from '@playwright/test';
import {
  fillerHtml,
  paragraphLocator,
  scrollToParagraph,
  seedAndOpen,
} from './helpers/paragraph';

test.describe.configure({ mode: 'parallel' });

test.describe('Chapter session position (chromium only)', () => {
  test.skip(({ browserName }) => browserName !== 'chromium', 'chromium-only');

  test('navigating away from a chapter and back lands on the in-session position, not the original saved one', async ({
    page,
  }) => {
    // Two chapters, paragraphs 0..39 in ch0 and 40..79 in ch1. Saved state
    // points deep into ch0 so the restore-to-paragraph-30 path is exercised.
    const ch0Paragraphs = Array.from({ length: 40 }, (_, i) => ({
      html: fillerHtml(i),
    }));
    const ch1Paragraphs = Array.from({ length: 40 }, (_, i) => ({
      html: fillerHtml(i + 40),
    }));
    const SAVED_PARAGRAPH = 30;

    const { bookId } = await seedAndOpen(
      page,
      {
        chapters: [
          { title: 'Chapter 0', paragraphs: ch0Paragraphs },
          { title: 'Chapter 1', paragraphs: ch1Paragraphs },
        ],
        readingState: { chapterId: 0, paragraphId: SAVED_PARAGRAPH },
      },
      { path: '/library' },
    );
    // Open the book — BookView mounts, navigates to /book/:id/0 (saved chapter).
    await page.locator(`a[href="/book/${bookId}"]`).first().click();
    await page.waitForSelector('.paragraphs-container');

    // 1. Restore lands on the saved paragraph.
    await expect(paragraphLocator(page, SAVED_PARAGRAPH)).toBeAttached();
    const POLL = { timeout: 3000, intervals: [50, 100, 200] } as const;
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
          }, SAVED_PARAGRAPH),
        POLL,
      )
      .toBe(true);

    // 2. Scroll to paragraph 0 (start of chapter 0).
    await scrollToParagraph(page, 0);
    // 3. Wait long enough for ChapterViewModel's 400 ms-debounced save to
    //    fire — that's the signal that BookView's positionByChapter map has
    //    captured the new ch0 position.
    await page.waitForTimeout(500);

    // 4. Switch to chapter 1 via the sidebar link.
    //    Chapters live inside the collapsible ChaptersPanel; open it first.
    await page.locator('[data-testid="chapters-panel-handle"]').click();
    await page.locator(`a[href="/book/${bookId}/1"]`).click();
    await expect(paragraphLocator(page, 40)).toBeAttached();

    // 5. Switch back to chapter 0. The panel auto-closes on chapter click,
    //    so re-open it.
    await page.locator('[data-testid="chapters-panel-handle"]').click();
    await page.locator(`a[href="/book/${bookId}/0"]`).click();
    await expect(paragraphLocator(page, 0)).toBeAttached();

    // 6. The visible page must be paragraph 0 (where we left ch0 in this
    //    session), NOT paragraph 30 (the original cross-session saved state).
    await expect
      .poll(
        async () =>
          page.evaluate(() => {
            const container = document.querySelector(
              '.paragraphs-container',
            ) as HTMLElement | null;
            const el0 = document.querySelector(
              `.paragraph-wrapper[data-paragraph-id="0"]`,
            ) as HTMLElement | null;
            if (!container || !el0) return false;
            const cr = container.getBoundingClientRect();
            const er = el0.getBoundingClientRect();
            return er.right > cr.left && er.left < cr.right;
          }),
        POLL,
      )
      .toBe(true);

    // And paragraph 30 must NOT be in the visible viewport on this visit.
    const p30Visible = await page.evaluate(() => {
      const container = document.querySelector(
        '.paragraphs-container',
      ) as HTMLElement | null;
      const el = document.querySelector(
        `.paragraph-wrapper[data-paragraph-id="30"]`,
      ) as HTMLElement | null;
      if (!container || !el) return false;
      const cr = container.getBoundingClientRect();
      const er = el.getBoundingClientRect();
      return er.right > cr.left && er.left < cr.right;
    });
    expect(p30Visible).toBe(false);
  });
});
