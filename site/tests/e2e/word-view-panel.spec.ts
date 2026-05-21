import { expect, test } from '@playwright/test';
import {
  paragraphLocator,
  seedAndOpen,
  wordSegment,
  wordSpan,
} from './helpers/paragraph';

// Covers the WordView bottom-overlay redesign:
//   - hint text when no selection
//   - peek populates word + translation on selection
//   - expand via button + `w` shortcut
//   - overlay does not resize the book viewport
//   - drag-resize persists across reloads
//   - collapse via button + `w` shortcut

test.describe.configure({ mode: 'parallel' });

const PANEL = '[data-testid="word-view"]';
const PEEK = '[data-testid="word-view-peek"]';
const EXPAND = '[data-testid="word-view-expand"]';
const COLLAPSE = '[data-testid="word-view-collapse"]';
const EXPANDED = '[data-testid="word-view-expanded"]';

async function seedClickableBook(page: import('@playwright/test').Page) {
  const segments = [
    wordSegment({
      flatIndex: 0,
      sentence: 0,
      word: 0,
      text: 'hola',
      translation: 'hello',
    }),
  ];
  // Embed the wordInfo in the seed so it survives page reloads (the init
  // script re-applies the seed on every navigation).
  const { bookId } = await seedAndOpen(page, {
    chapters: [{ paragraphs: [{ html: 'hola', segments }] }],
    wordInfos: [
      {
        paragraphId: 0,
        sentenceId: 0,
        wordId: 0,
        info: {
          original: 'hello',
          contextualTranslations: ['hola', 'hi'],
          fullSentenceTranslation: 'hola',
          note: 'a greeting',
        },
      },
    ],
  });
  return { bookId };
}

test.describe('WordView panel — hint state', () => {
  test.skip(({ browserName }) => browserName === 'firefox', 'chromium + webkit only');

  test('peek shows hint text when no word is selected', async ({ page }) => {
    await seedClickableBook(page);
    await page.waitForSelector('.paragraphs-container.is-ready');

    await expect(page.locator(PEEK)).toBeVisible();
    await expect(page.locator(PEEK)).toContainText('Select a word');
    await expect(page.locator(EXPAND)).toHaveCount(0);
  });
});

test.describe('WordView panel — selection populates peek', () => {
  test.skip(({ browserName }) => browserName === 'firefox', 'chromium + webkit only');

  test('clicking a word fills the peek bar', async ({ page }) => {
    await seedClickableBook(page);
    const p = paragraphLocator(page, 0);
    await wordSpan(p, 0).click();

    const peek = page.locator(PEEK);
    await expect(peek.locator('.peek-word')).toHaveText('hello');
    await expect(peek.locator('.peek-translations')).toHaveText('hola, hi');
    await expect(page.locator(EXPAND)).toBeVisible();
    // Stays collapsed by default — expanded body is absent.
    await expect(page.locator(EXPANDED)).toHaveCount(0);
  });
});

test.describe('WordView panel — expand and collapse', () => {
  test.skip(({ browserName }) => browserName === 'firefox', 'chromium + webkit only');

  test('expand button opens the full form', async ({ page }) => {
    await seedClickableBook(page);
    const p = paragraphLocator(page, 0);
    await wordSpan(p, 0).click();

    await page.locator(EXPAND).click();
    await expect(page.locator(EXPANDED)).toBeVisible();
    await expect(page.locator(EXPANDED).locator('.word-original')).toHaveText('hello');
    // The Note details block is open by default.
    await expect(page.locator(EXPANDED)).toContainText('a greeting');
    // Peek body is replaced by the expanded body while expanded.
    await expect(page.locator(PEEK)).toHaveCount(0);

    await page.locator(COLLAPSE).click();
    await expect(page.locator(EXPANDED)).toHaveCount(0);
    await expect(page.locator(PEEK)).toBeVisible();
  });

  test('`w` shortcut toggles expand', async ({ page }) => {
    await seedClickableBook(page);
    const p = paragraphLocator(page, 0);
    await wordSpan(p, 0).click();

    await page.keyboard.press('w');
    await expect(page.locator(EXPANDED)).toBeVisible();
    await page.keyboard.press('w');
    await expect(page.locator(EXPANDED)).toHaveCount(0);
  });
});

test.describe('WordView panel — overlay does not resize book viewport', () => {
  test.skip(({ browserName }) => browserName === 'firefox', 'chromium + webkit only');

  test('expanding the panel leaves the chapter container width unchanged', async ({
    page,
  }) => {
    await seedClickableBook(page);
    await page.waitForSelector('.paragraphs-container.is-ready');

    const beforeWidth = await page.evaluate(() => {
      const c = document.querySelector('.paragraphs-container') as HTMLElement | null;
      return c ? c.clientWidth : -1;
    });
    expect(beforeWidth).toBeGreaterThan(0);

    const p = paragraphLocator(page, 0);
    await wordSpan(p, 0).click();
    await page.locator(EXPAND).click();
    await expect(page.locator(EXPANDED)).toBeVisible();
    await page.waitForTimeout(250);

    const afterWidth = await page.evaluate(() => {
      const c = document.querySelector('.paragraphs-container') as HTMLElement | null;
      return c ? c.clientWidth : -1;
    });
    expect(afterWidth).toBe(beforeWidth);
  });
});

test.describe('WordView panel — drag resize', () => {
  test.skip(({ browserName }) => browserName !== 'chromium', 'chromium-only — mouse drag');

  test('dragging the top grip changes the height within the session', async ({
    page,
  }) => {
    await seedClickableBook(page);
    const p = paragraphLocator(page, 0);
    await wordSpan(p, 0).click();
    await page.locator(EXPAND).click();
    await expect(page.locator(EXPANDED)).toBeVisible();
    await page.waitForTimeout(250);

    const grip = page.locator('[data-testid="word-view-resize"]');
    const startBox = await grip.boundingBox();
    if (!startBox) throw new Error('resize grip not visible');

    const heightBefore = await page.evaluate(
      () =>
        (
          document.querySelector(
            '[data-testid="word-view"]',
          ) as HTMLElement | null
        )?.clientHeight ?? -1,
    );

    // Drag upward to make the panel taller.
    const startX = startBox.x + startBox.width / 2;
    const startY = startBox.y + startBox.height / 2;
    await page.mouse.move(startX, startY);
    await page.mouse.down();
    await page.mouse.move(startX, startY - 20, { steps: 5 });
    await page.mouse.move(startX, startY - 100, { steps: 10 });
    await page.mouse.up();

    const heightAfter = await page.evaluate(
      () =>
        (
          document.querySelector(
            '[data-testid="word-view"]',
          ) as HTMLElement | null
        )?.clientHeight ?? -1,
    );
    expect(heightAfter).toBeGreaterThan(heightBefore + 80);
  });
});
