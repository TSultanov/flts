import { expect, test } from './helpers/test';
import {
  emitCardsUpdated,
  paragraphLocator,
  seedAndOpen,
  setParagraphTranslationSilent,
  wordSegment,
  wordSpan,
} from './helpers/paragraph';

// Chromium only: Svelte reactivity and inline custom properties are
// engine-invariant, so extra engines cost CI time for no signal.
test.describe.configure({ mode: 'parallel' });

test.describe('Anki familiarity (chromium only)', () => {
  test.skip(({ browserName }) => browserName !== 'chromium', 'chromium-only');

  test('A1: dormant word renders no familiarity opacity', async ({ page }) => {
    await seedAndOpen(page, {
      chapters: [
        {
          paragraphs: [
            {
              html: 'hola',
              segments: [
                wordSegment({
                  flatIndex: 0,
                  sentence: 0,
                  word: 0,
                  text: 'hola',
                  translation: 'hello',
                }),
              ],
            },
          ],
        },
      ],
    });
    const span = wordSpan(paragraphLocator(page, 0), 0);
    await expect(span).toBeVisible();
    const opacity = await span.evaluate((el) =>
      (el as HTMLElement).style.getPropertyValue('--familiarity-opacity'),
    );
    expect(opacity).toBe('');
    await expect(span.locator('.translation-overlay')).toHaveCount(0);
  });

  test('A2: familiarity 0 → full underline + auto-shown overlay', async ({ page }) => {
    await seedAndOpen(page, {
      chapters: [
        {
          paragraphs: [
            {
              html: 'hola',
              segments: [
                wordSegment({
                  flatIndex: 0,
                  sentence: 0,
                  word: 0,
                  text: 'hola',
                  translation: 'hello',
                  familiarity: 0,
                }),
              ],
            },
          ],
        },
      ],
    });
    const span = wordSpan(paragraphLocator(page, 0), 0);
    await expect(span).toBeVisible();
    const opacity = await span.evaluate((el) =>
      (el as HTMLElement).style.getPropertyValue('--familiarity-opacity'),
    );
    expect(opacity).toBe('1');
    await expect(span.locator('.translation-overlay')).toBeVisible();
  });

  test('A3: familiarity 0.5 → half underline, no auto-overlay', async ({ page }) => {
    await seedAndOpen(page, {
      chapters: [
        {
          paragraphs: [
            {
              html: 'hola',
              segments: [
                wordSegment({
                  flatIndex: 0,
                  sentence: 0,
                  word: 0,
                  text: 'hola',
                  translation: 'hello',
                  familiarity: 0.5,
                }),
              ],
            },
          ],
        },
      ],
    });
    const span = wordSpan(paragraphLocator(page, 0), 0);
    await expect(span).toBeVisible();
    const opacity = await span.evaluate((el) =>
      (el as HTMLElement).style.getPropertyValue('--familiarity-opacity'),
    );
    expect(opacity).toBe('0.5');
    await expect(span.locator('.translation-overlay')).toHaveCount(0);
  });

  test('A4: familiarity 1 → invisible underline, no overlay', async ({ page }) => {
    await seedAndOpen(page, {
      chapters: [
        {
          paragraphs: [
            {
              html: 'hola',
              segments: [
                wordSegment({
                  flatIndex: 0,
                  sentence: 0,
                  word: 0,
                  text: 'hola',
                  translation: 'hello',
                  familiarity: 1,
                }),
              ],
            },
          ],
        },
      ],
    });
    const span = wordSpan(paragraphLocator(page, 0), 0);
    await expect(span).toBeVisible();
    const opacity = await span.evaluate((el) =>
      (el as HTMLElement).style.getPropertyValue('--familiarity-opacity'),
    );
    expect(opacity).toBe('0');
    await expect(span.locator('.translation-overlay')).toHaveCount(0);
  });

  test('B1: clicking an auto-shown word keeps the overlay visible', async ({ page }) => {
    await seedAndOpen(page, {
      chapters: [
        {
          paragraphs: [
            {
              html: 'hola',
              segments: [
                wordSegment({
                  flatIndex: 0,
                  sentence: 0,
                  word: 0,
                  text: 'hola',
                  translation: 'hello',
                  familiarity: 0,
                }),
              ],
            },
          ],
        },
      ],
    });
    const span = wordSpan(paragraphLocator(page, 0), 0);
    await expect(span.locator('.translation-overlay')).toBeVisible();

    // Click on a visible word is a no-op for visibility — it never hides.
    await span.click();
    await expect(span.locator('.translation-overlay')).toBeVisible();

    await span.click();
    await expect(span.locator('.translation-overlay')).toBeVisible();
  });

  test('B2: click faded word reveals overlay; further clicks keep it shown', async ({ page }) => {
    await seedAndOpen(page, {
      chapters: [
        {
          paragraphs: [
            {
              html: 'hola',
              segments: [
                wordSegment({
                  flatIndex: 0,
                  sentence: 0,
                  word: 0,
                  text: 'hola',
                  translation: 'hello',
                  familiarity: 0.7,
                }),
              ],
            },
          ],
        },
      ],
    });
    const span = wordSpan(paragraphLocator(page, 0), 0);
    await expect(span.locator('.translation-overlay')).toHaveCount(0);

    await span.click();
    await expect(span.locator('.translation-overlay')).toBeVisible();

    // Second click does not hide — reveal is add-only.
    await span.click();
    await expect(span.locator('.translation-overlay')).toBeVisible();
  });

  test('C1: cards_updated refreshes familiarity opacity in place', async ({ page }) => {
    const { bookId } = await seedAndOpen(page, {
      chapters: [
        {
          paragraphs: [
            {
              html: 'hola',
              segments: [
                wordSegment({
                  flatIndex: 0,
                  sentence: 0,
                  word: 0,
                  text: 'hola',
                  translation: 'hello',
                  familiarity: 0,
                }),
              ],
            },
          ],
        },
      ],
    });
    const span = wordSpan(paragraphLocator(page, 0), 0);
    await expect(span.locator('.translation-overlay')).toBeVisible();
    const before = await span.evaluate((el) =>
      (el as HTMLElement).style.getPropertyValue('--familiarity-opacity'),
    );
    expect(before).toBe('1');

    await setParagraphTranslationSilent(page, bookId, 0, [
      wordSegment({
        flatIndex: 0,
        sentence: 0,
        word: 0,
        text: 'hola',
        translation: 'hello',
        familiarity: 1,
      }),
    ]);
    await emitCardsUpdated(page);

    await expect
      .poll(
        () =>
          span.evaluate((el) =>
            (el as HTMLElement).style.getPropertyValue('--familiarity-opacity'),
          ),
        { timeout: 2000 },
      )
      .toBe('0');
    await expect(span.locator('.translation-overlay')).toHaveCount(0);
  });

  test('C2: cards_updated refresh does not blink to original text', async ({ page }) => {
    const { bookId } = await seedAndOpen(page, {
      chapters: [
        {
          paragraphs: [
            {
              html: 'hola',
              segments: [
                wordSegment({
                  flatIndex: 0,
                  sentence: 0,
                  word: 0,
                  text: 'hola',
                  translation: 'hello',
                  familiarity: 0,
                }),
              ],
            },
          ],
        },
      ],
    });

    await setParagraphTranslationSilent(page, bookId, 0, [
      wordSegment({
        flatIndex: 0,
        sentence: 0,
        word: 0,
        text: 'hola',
        translation: 'hello',
        familiarity: 0.5,
      }),
    ]);
    await emitCardsUpdated(page);

    // Across the 500ms debounce + refetch, segments must never dip to null:
    // .word-span stays mounted and the .original fallback never appears.
    const samples = await page.evaluate(async () => {
      const out: Array<{ spans: number; original: number }> = [];
      const start = performance.now();
      while (performance.now() - start < 800) {
        out.push({
          spans: document.querySelectorAll('.word-span').length,
          original: document.querySelectorAll('.paragraph-wrapper p.original').length,
        });
        await new Promise((r) => setTimeout(r, 50));
      }
      return out;
    });
    for (const s of samples) {
      expect(s.spans).toBe(1);
      expect(s.original).toBe(0);
    }
  });
});
