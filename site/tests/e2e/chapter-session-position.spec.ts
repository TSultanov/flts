import { expect, test } from './helpers/test';
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
    // Paragraphs 0..39 in ch0, 40..79 in ch1; saved state points deep into ch0.
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
    await page.locator(`a[href="/book/${bookId}"]`).first().click();
    await page.waitForSelector('.paragraphs-container');

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

    await scrollToParagraph(page, 0);
    // Outlast the 400ms save debounce, which is what updates positionByChapter.
    await page.waitForTimeout(500);

    // Chapter links live inside the collapsible ChaptersPanel.
    await page.locator('[data-testid="chapters-panel-handle"]').click();
    await page.locator(`a[href="/book/${bookId}/1"]`).click();
    await expect(paragraphLocator(page, 40)).toBeAttached();

    // The panel auto-closes on chapter click.
    await page.locator('[data-testid="chapters-panel-handle"]').click();
    await page.locator(`a[href="/book/${bookId}/0"]`).click();
    await expect(paragraphLocator(page, 0)).toBeAttached();

    // In-session position (paragraph 0) must win over the saved state (30).
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
