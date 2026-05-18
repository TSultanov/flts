import { expect, test } from '@playwright/test';
import {
  expectTranslated,
  getTranslateCalls,
  multipageSpec,
  paragraphLocator,
  scrollToParagraph,
  seedAndOpen,
  translateButton,
  wordSpan,
  wordSpanHtml,
} from './helpers/paragraph';

// Lazy-loading regression suite. With the current (eager-mount) implementation
// these all pass trivially because no paragraph ever unmounts. Once paragraphs
// virtualize on scroll, this file catches any user-visible regression.
test.describe('ParagraphView (multipage, chromium only)', () => {
  test.skip(({ browserName }) => browserName !== 'chromium', 'chromium-only');

  const COUNT = 80;

  // ----- M1 ----------------------------------------------------------------

  test('M1: 80 paragraphs render in order; chapter is genuinely long', async ({ page }) => {
    await seedAndOpen(page, multipageSpec(COUNT));

    // All wrappers present and in order.
    const ids = await page.evaluate(() => {
      const container = document.querySelector('.paragraphs-container');
      if (!container) return [];
      return Array.from(container.children).map((c) =>
        (c as HTMLElement).dataset['paragraphId'],
      );
    });
    expect(ids).toEqual(Array.from({ length: COUNT }, (_, i) => String(i)));

    // The chapter spans >50 viewports of horizontal scroll — this confirms
    // we have a real long-scroll target, not a 5-paragraph "chapter".
    const ratio = await page.evaluate(() => {
      const c = document.querySelector('.paragraphs-container') as HTMLElement;
      return c.scrollWidth / c.clientWidth;
    });
    expect(ratio).toBeGreaterThan(50);
  });

  // ----- M2 ----------------------------------------------------------------

  test('M2: translate a middle paragraph (40) after scrolling to it', async ({ page }) => {
    const translation = wordSpanHtml({
      flatIndex: 0,
      paragraph: 40,
      sentence: 0,
      word: 0,
      text: 'hola',
      translation: 'hello',
    });

    const { bookId } = await seedAndOpen(
      page,
      multipageSpec(COUNT, {}, {
        translateConfigs: [
          { paragraphId: 40, cfg: { kind: 'immediate', translation } },
        ],
      }),
    );

    const initialScrollLeft = await page.evaluate(() => {
      return (document.querySelector('.paragraphs-container') as HTMLElement).scrollLeft;
    });
    expect(initialScrollLeft).toBeLessThan(50); // starts near paragraph 0

    await scrollToParagraph(page, 40);
    const p40 = paragraphLocator(page, 40);
    await translateButton(p40).click();

    await expectTranslated(p40);
    await expect(p40.locator('.word-span')).toHaveText('hola');

    const calls = await getTranslateCalls(page);
    expect(calls).toHaveLength(1);
    expect(calls[0]).toMatchObject({ bookId, paragraphId: 40 });
  });

  // ----- M3 ----------------------------------------------------------------

  test('M3: spinner persists across scroll-away-and-back during a long translation', async ({
    page,
  }) => {
    await seedAndOpen(
      page,
      multipageSpec(COUNT, {}, {
        translateConfigs: [
          {
            paragraphId: 40,
            cfg: {
              kind: 'progress',
              steps: [
                { progress: 25, total: 100, delayMs: 700 },
                { progress: 75, total: 100, delayMs: 700 },
                { progress: 100, total: 100, delayMs: 700 },
              ],
              translation: '<span>multipage done</span>',
            },
          },
        ],
      }),
    );

    const p40 = paragraphLocator(page, 40);
    await scrollToParagraph(page, 40);
    await translateButton(p40).click();

    // Spinner visible, button disabled before we navigate away.
    await expect(p40.locator('.circular-progress')).toBeVisible();
    await expect(translateButton(p40)).toBeDisabled();

    // Scroll churn — visit both ends then return.
    await scrollToParagraph(page, 0);
    await scrollToParagraph(page, 79);
    await scrollToParagraph(page, 40);

    // Spinner state survived. Button is still disabled.
    await expect(p40.locator('.circular-progress')).toBeVisible();
    await expect(translateButton(p40)).toBeDisabled();

    // Eventually finishes.
    await expectTranslated(p40);
    await expect(p40.locator('.circular-progress')).toHaveCount(0);
    await expect(p40.getByText('multipage done')).toBeVisible();
  });

  // ----- M4 ----------------------------------------------------------------

  test('M4: translation completing while scrolled away still lands on return', async ({
    page,
  }) => {
    await seedAndOpen(
      page,
      multipageSpec(COUNT, {}, {
        translateConfigs: [
          {
            paragraphId: 40,
            cfg: {
              kind: 'progress',
              steps: [
                { progress: 50, total: 100, delayMs: 300 },
                { progress: 100, total: 100, delayMs: 300 },
              ],
              translation: '<span>finished while away</span>',
            },
          },
        ],
      }),
    );

    const p40 = paragraphLocator(page, 40);
    await scrollToParagraph(page, 40);
    await translateButton(p40).click();
    await expect(p40.locator('.circular-progress')).toBeVisible();

    // Immediately scroll away, then wait past the translation's completion.
    await scrollToParagraph(page, 0);
    await page.waitForTimeout(900);

    await scrollToParagraph(page, 40);
    await expect(p40.locator('.circular-progress')).toHaveCount(0);
    await expectTranslated(p40);
    await expect(p40.getByText('finished while away')).toBeVisible();
  });

  // ----- M5 ----------------------------------------------------------------

  test('M5: visible-words annotations apply on scroll-into-view and persist across churn', async ({
    page,
  }) => {
    const translation40 = [0, 1, 2]
      .map((i) =>
        wordSpanHtml({
          flatIndex: i,
          paragraph: 40,
          sentence: 0,
          word: i,
          text: `w40-${i}`,
          translation: `t40-${i}`,
        }),
      )
      .join(' ');
    const translation65 = [0, 1, 2]
      .map((i) =>
        wordSpanHtml({
          flatIndex: i,
          paragraph: 65,
          sentence: 0,
          word: i,
          text: `w65-${i}`,
          translation: `t65-${i}`,
        }),
      )
      .join(' ');

    await seedAndOpen(
      page,
      multipageSpec(COUNT, {
        40: { translation: translation40, visibleWords: [0, 2] },
        65: { translation: translation65, visibleWords: [1] },
      }),
    );

    const p40 = paragraphLocator(page, 40);
    const p65 = paragraphLocator(page, 65);

    // Initially at paragraph 0 — observer hasn't fired on 40 or 65.
    await expect(wordSpan(p40, 0)).not.toHaveClass(/\bshow-translation\b/);
    await expect(wordSpan(p40, 2)).not.toHaveClass(/\bshow-translation\b/);
    await expect(wordSpan(p65, 1)).not.toHaveClass(/\bshow-translation\b/);

    // Scroll to 40 → its [0, 2] light up; [1] does not.
    await scrollToParagraph(page, 40);
    await expect(wordSpan(p40, 0)).toHaveClass(/\bshow-translation\b/);
    await expect(wordSpan(p40, 2)).toHaveClass(/\bshow-translation\b/);
    await expect(wordSpan(p40, 1)).not.toHaveClass(/\bshow-translation\b/);

    // Scroll to 65 → its [1] lights up.
    await scrollToParagraph(page, 65);
    await expect(wordSpan(p65, 1)).toHaveClass(/\bshow-translation\b/);
    await expect(wordSpan(p65, 0)).not.toHaveClass(/\bshow-translation\b/);
    await expect(wordSpan(p65, 2)).not.toHaveClass(/\bshow-translation\b/);

    // Scroll away and back — both stay annotated.
    await scrollToParagraph(page, 0);
    await scrollToParagraph(page, 40);
    await expect(wordSpan(p40, 0)).toHaveClass(/\bshow-translation\b/);
    await expect(wordSpan(p40, 2)).toHaveClass(/\bshow-translation\b/);
    await scrollToParagraph(page, 65);
    await expect(wordSpan(p65, 1)).toHaveClass(/\bshow-translation\b/);
  });

  // ----- M6 ----------------------------------------------------------------

  test('M6: two in-flight translations stay in their own lanes', async ({ page }) => {
    await seedAndOpen(
      page,
      multipageSpec(COUNT, {}, {
        translateConfigs: [
          {
            paragraphId: 10,
            cfg: {
              kind: 'progress',
              steps: [
                { progress: 50, total: 100, delayMs: 600 },
                { progress: 100, total: 100, delayMs: 600 },
              ],
              translation: '<span>p10 done</span>',
            },
          },
          {
            paragraphId: 65,
            cfg: {
              kind: 'progress',
              steps: [
                { progress: 50, total: 100, delayMs: 600 },
                { progress: 100, total: 100, delayMs: 600 },
              ],
              translation: '<span>p65 done</span>',
            },
          },
        ],
      }),
    );

    const p10 = paragraphLocator(page, 10);
    const p65 = paragraphLocator(page, 65);

    await scrollToParagraph(page, 10);
    await translateButton(p10).click();
    await expect(p10.locator('.circular-progress')).toBeVisible();

    await scrollToParagraph(page, 65);
    await translateButton(p65).click();
    await expect(p65.locator('.circular-progress')).toBeVisible();

    // Scroll back to 10. Its spinner is still up — the two are independent.
    await scrollToParagraph(page, 10);
    await expect(p10.locator('.circular-progress')).toBeVisible();

    // Wait for both to land.
    await expectTranslated(p10);
    await expect(p10.getByText('p10 done')).toBeVisible();

    await scrollToParagraph(page, 65);
    await expectTranslated(p65);
    await expect(p65.getByText('p65 done')).toBeVisible();

    const calls = await getTranslateCalls(page);
    expect(calls).toHaveLength(2);
    expect(calls.map((c) => c.paragraphId).sort((a, b) => a - b)).toEqual([10, 65]);
  });
});
