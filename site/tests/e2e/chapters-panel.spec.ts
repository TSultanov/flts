import { expect, test } from '@playwright/test';
import { fillerHtml, seedAndOpen } from './helpers/paragraph';

// Covers the BookView chapters-panel redesign:
//  - toggle via the edge handle and the `c` keyboard shortcut
//  - opening the panel must NOT resize the book viewport (overlay, not flex column)
//  - the resize grip persists the panel width across reloads
//  - on narrow viewports, clicking a chapter auto-closes the panel
//  - books with a single chapter render no handle at all

test.describe.configure({ mode: 'parallel' });

const PANEL = '[data-testid="chapters-panel"]';
const HANDLE = '[data-testid="chapters-panel-handle"]';
const GRIP = '[data-testid="chapters-panel-resize"]';

function multiChapterSpec() {
  return {
    chapters: [
      {
        title: 'Chapter 0',
        paragraphs: Array.from({ length: 5 }, (_, i) => ({ html: fillerHtml(i) })),
      },
      {
        title: 'Chapter 1',
        paragraphs: Array.from({ length: 5 }, (_, i) => ({ html: fillerHtml(i + 5) })),
      },
      {
        title: 'Chapter 2',
        paragraphs: Array.from({ length: 5 }, (_, i) => ({ html: fillerHtml(i + 10) })),
      },
    ],
  };
}

test.describe('ChaptersPanel — toggle behavior', () => {
  test.skip(({ browserName }) => browserName === 'firefox', 'chromium + webkit only');

  test('handle toggles the panel open and closed', async ({ page }) => {
const { bookId } = await seedAndOpen(page, multiChapterSpec());
    // Wait until ChapterView has fully mounted; the chapter-container's
    // initial layout phase otherwise races Playwright's actionability check
    // for the handle.
    await page.waitForSelector('.paragraphs-container.is-ready');

    await expect(page.locator(HANDLE)).toBeVisible();
    // Default state is closed.
    await expect(page.locator(HANDLE)).toHaveAttribute('aria-expanded', 'false');
    await expect(page.locator(PANEL)).toHaveAttribute('aria-hidden', 'true');

    await page.locator(HANDLE).click();
    await expect(page.locator(HANDLE)).toHaveAttribute('aria-expanded', 'true');
    await expect(page.locator(PANEL)).toHaveAttribute('aria-hidden', 'false');
    // Each chapter renders an anchor in the panel.
    await expect(
      page.locator(`${PANEL} a[href="/book/${bookId}/1"]`),
    ).toBeVisible();

    await page.locator(HANDLE).click();
    await expect(page.locator(HANDLE)).toHaveAttribute('aria-expanded', 'false');
  });

  test('pressing `c` toggles the panel', async ({ page }) => {
await seedAndOpen(page, multiChapterSpec());

    await expect(page.locator(HANDLE)).toHaveAttribute('aria-expanded', 'false');
    await page.keyboard.press('c');
    await expect(page.locator(HANDLE)).toHaveAttribute('aria-expanded', 'true');
    await page.keyboard.press('c');
    await expect(page.locator(HANDLE)).toHaveAttribute('aria-expanded', 'false');
  });
});

test.describe('ChaptersPanel — overlay does not resize book viewport', () => {
  test.skip(({ browserName }) => browserName === 'firefox', 'chromium + webkit only');

  test('opening the panel leaves .paragraphs-container width unchanged', async ({
    page,
  }) => {
await seedAndOpen(page, multiChapterSpec());
    await page.waitForSelector('.paragraphs-container');

    const beforeWidth = await page.evaluate(() => {
      const c = document.querySelector('.paragraphs-container') as HTMLElement | null;
      return c ? c.clientWidth : -1;
    });
    expect(beforeWidth).toBeGreaterThan(0);

    await page.locator(HANDLE).click();
    await expect(page.locator(HANDLE)).toHaveAttribute('aria-expanded', 'true');

    // Give the CSS transform transition time to settle (180ms) before measuring.
    await page.waitForTimeout(250);

    const afterWidth = await page.evaluate(() => {
      const c = document.querySelector('.paragraphs-container') as HTMLElement | null;
      return c ? c.clientWidth : -1;
    });
    expect(afterWidth).toBe(beforeWidth);
  });
});

test.describe('ChaptersPanel — resize persistence', () => {
  test.skip(({ browserName }) => browserName !== 'chromium', 'chromium-only — mouse drag');

  test('dragging the grip changes the width and survives a reload', async ({
    page,
  }) => {
const { bookId } = await seedAndOpen(page, multiChapterSpec());
    await page.locator(HANDLE).click();
    await expect(page.locator(HANDLE)).toHaveAttribute('aria-expanded', 'true');
    await page.waitForTimeout(250);

    const grip = page.locator(GRIP);
    const startBox = await grip.boundingBox();
    if (!startBox) throw new Error('resize grip not visible');

    const startX = startBox.x + startBox.width / 2;
    const startY = startBox.y + startBox.height / 2;
    const targetX = startX + 80;

    await page.mouse.move(startX, startY);
    await page.mouse.down();
    // Intermediate move helps Chromium register the drag as a pointer-capture
    // sequence rather than a click.
    await page.mouse.move(startX + 20, startY, { steps: 5 });
    await page.mouse.move(targetX, startY, { steps: 10 });
    await page.mouse.up();

    const widthAfterDrag = await page.evaluate(
      () =>
        (document.querySelector(
          '[data-testid="chapters-panel"]',
        ) as HTMLElement | null)?.clientWidth ?? -1,
    );
    expect(widthAfterDrag).toBeGreaterThan(260);

    // Reload through the library so BookView remounts via the natural path.
    // We use page.goto with the same path to force a hard reload; the
    // localStorage persists across reloads on the same origin.
    await page.goto(`/book/${bookId}/0`);
    await expect(page.locator(HANDLE)).toBeVisible();
    // Open the panel again (default closed); width should reflect the saved value.
    await page.locator(HANDLE).click();
    await page.waitForTimeout(250);

    const widthAfterReload = await page.evaluate(
      () =>
        (document.querySelector(
          '[data-testid="chapters-panel"]',
        ) as HTMLElement | null)?.clientWidth ?? -1,
    );
    expect(Math.abs(widthAfterReload - widthAfterDrag)).toBeLessThanOrEqual(2);
  });
});

test.describe('ChaptersPanel — auto-close on chapter selection', () => {
  test.skip(({ browserName }) => browserName !== 'chromium', 'chromium-only');

  test('clicking a chapter closes the panel', async ({ page }) => {
    const { bookId } = await seedAndOpen(page, multiChapterSpec());

    await page.locator(HANDLE).click();
    await expect(page.locator(HANDLE)).toHaveAttribute('aria-expanded', 'true');

    await page.locator(`${PANEL} a[href="/book/${bookId}/1"]`).click();
    await expect(page).toHaveURL(new RegExp(`/book/${bookId}/1$`));
    await expect(page.locator(HANDLE)).toHaveAttribute('aria-expanded', 'false');
  });
});

test.describe('ChaptersPanel — single-chapter books render no panel', () => {
  test.skip(({ browserName }) => browserName === 'firefox', 'chromium + webkit only');

  test('handle is absent when there is only one chapter', async ({ page }) => {
await seedAndOpen(page, {
      chapters: [
        {
          title: 'Only Chapter',
          paragraphs: Array.from({ length: 5 }, (_, i) => ({ html: fillerHtml(i) })),
        },
      ],
    });
    await page.waitForSelector('.paragraphs-container');
    await expect(page.locator(HANDLE)).toHaveCount(0);
    await expect(page.locator(PANEL)).toHaveCount(0);
  });
});
