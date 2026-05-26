import { expect, test, type Page } from '@playwright/test';
import { fillerHtml, seedAndOpen } from './helpers/paragraph';

// Covers the chapter-summary status surface:
//  - chapters whose summary isn't generated are visually dim
//  - the chapter currently being summarized shows a small spinner
//  - paragraphs' translate buttons are disabled until summaries for all
//    PRIOR chapters are ready (chapter 0 is always translatable)
//
// The summary worker is fully mocked from the browser-mode harness — no
// Tauri binary is involved here.

test.describe.configure({ mode: 'parallel' });

const PANEL = '[data-testid="chapters-panel"]';
const HANDLE = '[data-testid="chapters-panel-handle"]';
const ROW = '[data-testid="chapter-row"]';
const SPINNER = '[data-testid="summary-spinner"]';

function multiChapterSpec(summaryStatus?: {
  generated: boolean[];
  activelyGenerating?: number | null;
}) {
  return {
    chapters: [
      {
        title: 'Chapter 0',
        paragraphs: Array.from({ length: 3 }, (_, i) => ({ html: fillerHtml(i) })),
      },
      {
        title: 'Chapter 1',
        paragraphs: Array.from({ length: 3 }, (_, i) => ({ html: fillerHtml(i + 3) })),
      },
      {
        title: 'Chapter 2',
        paragraphs: Array.from({ length: 3 }, (_, i) => ({ html: fillerHtml(i + 6) })),
      },
    ],
    summaryStatus,
  };
}

function rowFor(page: Page, chapterId: number) {
  return page.locator(`${PANEL} ${ROW}[data-chapter-id="${chapterId}"]`);
}

function spinnerInRow(page: Page, chapterId: number) {
  return rowFor(page, chapterId).locator(SPINNER);
}

async function openPanel(page: Page) {
  await page.waitForSelector('.paragraphs-container.is-ready');
  await page.locator(HANDLE).click();
  await expect(page.locator(HANDLE)).toHaveAttribute('aria-expanded', 'true');
}

async function navigateToChapter(page: Page, bookId: string, chapterId: number) {
  await page.locator(`${PANEL} a[href="/book/${bookId}/${chapterId}"]`).click();
  await page.waitForURL(new RegExp(`/book/${bookId}/${chapterId}$`));
  await page.waitForSelector('.paragraphs-container.is-ready');
}

test.describe('chapter-summary status — visual + gating', () => {
  test.skip(({ browserName }) => browserName === 'firefox', 'chromium + webkit only');

  test('all summaries ready: no dim, no spinner, buttons enabled', async ({
    page,
  }) => {
    const { bookId } = await seedAndOpen(
      page,
      multiChapterSpec({
        generated: [true, true, true],
        activelyGenerating: null,
      }),
    );
    await openPanel(page);

    for (const chapterId of [0, 1, 2]) {
      await expect(rowFor(page, chapterId)).not.toHaveClass(/dim/);
      await expect(spinnerInRow(page, chapterId)).toHaveCount(0);
    }

    await navigateToChapter(page, bookId, 0);
    await expect(
      page.locator('.paragraph-wrapper button.translate').first(),
    ).toBeEnabled();
  });

  test('no summaries yet: all dim, spinner on chapter 0, only ch0 translatable', async ({
    page,
  }) => {
    const { bookId } = await seedAndOpen(
      page,
      multiChapterSpec({
        generated: [false, false, false],
        activelyGenerating: 0,
      }),
    );
    await openPanel(page);

    for (const chapterId of [0, 1, 2]) {
      await expect(rowFor(page, chapterId)).toHaveClass(/dim/);
    }
    await expect(spinnerInRow(page, 0)).toHaveCount(1);
    await expect(spinnerInRow(page, 1)).toHaveCount(0);
    await expect(spinnerInRow(page, 2)).toHaveCount(0);

    // Chapter 0 needs no prior summary, so its buttons are enabled even
    // when its own summary hasn't been generated yet.
    await navigateToChapter(page, bookId, 0);
    await expect(
      page.locator('.paragraph-wrapper button.translate').first(),
    ).toBeEnabled();

    // Chapter 1 requires chapter 0's summary.
    await page.locator(HANDLE).click();
    await navigateToChapter(page, bookId, 1);
    await expect(
      page.locator('.paragraph-wrapper button.translate').first(),
    ).toBeDisabled();
  });

  test('advance event un-dims and moves spinner, re-gates translate', async ({
    page,
  }) => {
    const { bookId } = await seedAndOpen(
      page,
      multiChapterSpec({
        generated: [false, false, false],
        activelyGenerating: 0,
      }),
    );
    await openPanel(page);

    await expect(spinnerInRow(page, 0)).toHaveCount(1);

    await page.evaluate(
      (id) => (window as any).__test.advanceSummaryGeneration(id),
      bookId,
    );

    await expect(rowFor(page, 0)).not.toHaveClass(/dim/);
    await expect(spinnerInRow(page, 0)).toHaveCount(0);
    await expect(rowFor(page, 1)).toHaveClass(/dim/);
    await expect(spinnerInRow(page, 1)).toHaveCount(1);
    await expect(rowFor(page, 2)).toHaveClass(/dim/);
    await expect(spinnerInRow(page, 2)).toHaveCount(0);

    // Chapter 1 is now translatable (chapter 0's summary ready).
    await navigateToChapter(page, bookId, 1);
    await expect(
      page.locator('.paragraph-wrapper button.translate').first(),
    ).toBeEnabled();

    // Chapter 2 still blocked (chapter 1's summary not ready).
    await page.locator(HANDLE).click();
    await navigateToChapter(page, bookId, 2);
    await expect(
      page.locator('.paragraph-wrapper button.translate').first(),
    ).toBeDisabled();
  });

  test('"done" event clears the spinner and undims everything', async ({
    page,
  }) => {
    const { bookId } = await seedAndOpen(
      page,
      multiChapterSpec({
        generated: [true, true, false],
        activelyGenerating: 2,
      }),
    );
    await openPanel(page);

    await expect(rowFor(page, 2)).toHaveClass(/dim/);
    await expect(spinnerInRow(page, 2)).toHaveCount(1);

    await page.evaluate(
      (id) => (window as any).__test.advanceSummaryGeneration(id),
      bookId,
    );

    for (const chapterId of [0, 1, 2]) {
      await expect(rowFor(page, chapterId)).not.toHaveClass(/dim/);
      await expect(spinnerInRow(page, chapterId)).toHaveCount(0);
    }

    await navigateToChapter(page, bookId, 2);
    await expect(
      page.locator('.paragraph-wrapper button.translate').first(),
    ).toBeEnabled();
  });

  test('spinner is visually rendered (non-zero, non-white pixels)', async ({
    page,
  }) => {
    await seedAndOpen(
      page,
      multiChapterSpec({
        generated: [false, false, false],
        activelyGenerating: 0,
      }),
    );
    await openPanel(page);
    const spinner = spinnerInRow(page, 0);
    await expect(spinner).toHaveCount(1);

    // Bounding box must have real size.
    const box = await spinner.boundingBox();
    expect(box).not.toBeNull();
    expect(box!.width).toBeGreaterThan(2);
    expect(box!.height).toBeGreaterThan(2);

    // Sample a few pixels from the screenshot and assert at least one is
    // not pure white — i.e. the SVG stroke is actually painted, not the
    // invisible white-on-white case we just fixed.
    const png = await spinner.screenshot();
    // Save the screenshot for manual inspection.
    await page.screenshot({
      path: 'test-results/chapters-panel-spinner.png',
      fullPage: false,
    });
    // PNG signature is 8 bytes; we just need any non-white opaque byte
    // somewhere. A simple heuristic: look for any byte < 200 in the
    // image data portion (anything reasonably darker than pure white).
    let hasDarkPixel = false;
    for (let i = 100; i < png.length; i++) {
      if (png[i] < 200) {
        hasDarkPixel = true;
        break;
      }
    }
    expect(hasDarkPixel).toBe(true);
  });

  test('"failed" event clears spinner but leaves remaining chapters dim', async ({
    page,
  }) => {
    const { bookId } = await seedAndOpen(
      page,
      multiChapterSpec({
        generated: [true, false, false],
        activelyGenerating: 1,
      }),
    );
    await openPanel(page);

    await expect(rowFor(page, 1)).toHaveClass(/dim/);
    await expect(spinnerInRow(page, 1)).toHaveCount(1);

    // setSummaryStatus with activelyGenerating: null and not-all-done
    // emits a "failed" event in the mock.
    await page.evaluate(
      (id) =>
        (window as any).__test.setSummaryStatus(id, {
          generated: [true, false, false],
          activelyGenerating: null,
        }),
      bookId,
    );

    await expect(rowFor(page, 0)).not.toHaveClass(/dim/);
    await expect(rowFor(page, 1)).toHaveClass(/dim/);
    await expect(rowFor(page, 2)).toHaveClass(/dim/);
    await expect(spinnerInRow(page, 0)).toHaveCount(0);
    await expect(spinnerInRow(page, 1)).toHaveCount(0);
    await expect(spinnerInRow(page, 2)).toHaveCount(0);

    // Chapter 1's prior (chapter 0) IS generated, so its paragraphs are
    // still translatable — the failure only affects chapters that need
    // chapter 1's summary as a prior, i.e. chapter 2.
    await navigateToChapter(page, bookId, 1);
    await expect(
      page.locator('.paragraph-wrapper button.translate').first(),
    ).toBeEnabled();

    await page.locator(HANDLE).click();
    await navigateToChapter(page, bookId, 2);
    await expect(
      page.locator('.paragraph-wrapper button.translate').first(),
    ).toBeDisabled();
  });
});
