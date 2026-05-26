import { expect, test } from '@playwright/test';
import {
  fillerHtml,
  fillerSegments,
  seedAndOpen,
  setParagraphTranslation,
} from './helpers/paragraph';

// Covers the per-chapter translation percent rendered in the chapters panel:
//  - 0% when no paragraph in the chapter is translated
//  - rounded percent for partially-translated chapters
//  - label hidden entirely when the chapter is 100% translated
//  - reactive refresh when a paragraph finishes translating mid-session

test.describe.configure({ mode: 'parallel' });

const HANDLE = '[data-testid="chapters-panel-handle"]';
const RATIO = '[data-testid="chapter-translation-ratio"]';

function rowSelector(chapterId: number): string {
  return `[data-testid="chapter-row"][data-chapter-id="${chapterId}"]`;
}

function chapterSpec(paragraphCount: number, translatedCount: number) {
  return {
    title: `Chapter with ${translatedCount}/${paragraphCount} translated`,
    paragraphs: Array.from({ length: paragraphCount }, (_, i) => ({
      html: fillerHtml(i),
      ...(i < translatedCount ? { segments: fillerSegments(i) } : {}),
    })),
  };
}

test.describe('Chapter translation ratio — initial render', () => {
  test.skip(({ browserName }) => browserName === 'firefox', 'chromium + webkit only');

  test('untranslated chapter renders 0%', async ({ page }) => {
    const { bookId } = await seedAndOpen(page, {
      chapters: [chapterSpec(3, 0), chapterSpec(3, 0)],
    });
    void bookId;

    await page.locator(HANDLE).click();

    const ratio = page.locator(`${rowSelector(0)} ${RATIO}`);
    await expect(ratio).toHaveText('0%');
  });

  test('partial chapter renders rounded percent', async ({ page }) => {
    await seedAndOpen(page, {
      chapters: [chapterSpec(4, 2), chapterSpec(3, 1)],
    });

    await page.locator(HANDLE).click();

    await expect(page.locator(`${rowSelector(0)} ${RATIO}`)).toHaveText('50%');
    // 1/3 ≈ 33.333… → toFixed(0) → "33"
    await expect(page.locator(`${rowSelector(1)} ${RATIO}`)).toHaveText('33%');
  });

  test('fully-translated chapter omits the ratio', async ({ page }) => {
    await seedAndOpen(page, {
      chapters: [chapterSpec(3, 3), chapterSpec(3, 0)],
    });

    await page.locator(HANDLE).click();

    // Sanity: the unrelated chapter still gets a ratio label — proves we hide
    // only the 100% row, not the label globally.
    await expect(page.locator(`${rowSelector(1)} ${RATIO}`)).toHaveText('0%');
    await expect(page.locator(`${rowSelector(0)} ${RATIO}`)).toHaveCount(0);
  });
});

test.describe('Chapter translation ratio — reactivity', () => {
  test.skip(({ browserName }) => browserName === 'firefox', 'chromium + webkit only');

  test('ratio refreshes when a paragraph finishes translating', async ({ page }) => {
    // Two chapters so the chapter-panel handle renders (single-chapter books
    // skip the panel entirely — see chapters-panel.spec.ts).
    const { bookId } = await seedAndOpen(page, {
      chapters: [chapterSpec(4, 0), chapterSpec(1, 0)],
    });

    await page.locator(HANDLE).click();
    const ratio = page.locator(`${rowSelector(0)} ${RATIO}`);
    await expect(ratio).toHaveText('0%');

    // Translate paragraph 0 (global id 0 — chapter 0's first paragraph).
    // Mock emits book_updated, which the chapter list Resource subscribes to.
    await setParagraphTranslation(page, bookId, 0, fillerSegments(0));

    await expect(ratio).toHaveText('25%');
  });
});
