import { expect, test } from './helpers/test';
import {
  paragraphLocator,
  seedAndOpen,
  wordSegment,
  wordSpan,
} from './helpers/paragraph';

test.describe('Tap to reveal translations — config', () => {
  test('checkbox defaults off and persists via update_config', async ({ page }) => {
    await page.goto('/config');

    const checkbox = page.getByTestId('tap-to-reveal');
    await expect(checkbox).toBeVisible();
    await expect(checkbox).not.toBeChecked();

    await checkbox.check();
    await page.locator('#save').click();

    const persisted = await page.evaluate(
      () =>
        (window as any).__test.getConfig() as {
          tapToRevealTranslations?: boolean;
        },
    );
    expect(persisted.tapToRevealTranslations).toBe(true);
  });
});

test.describe.configure({ mode: 'parallel' });

test.describe('Tap to reveal translations — reader (chromium only)', () => {
  test.skip(({ browserName }) => browserName !== 'chromium', 'chromium-only');

  test('familiarity 0 word has no underline and no overlay until tap', async ({
    page,
  }) => {
    await seedAndOpen(page, {
      config: { tapToRevealTranslations: true },
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
    expect(opacity).toBe('');
    await expect(span).toHaveCSS('text-decoration-line', 'none');
    await expect(span.locator('.translation-overlay')).toHaveCount(0);

    await span.click();
    await expect(span.locator('.translation-overlay')).toBeVisible();
  });

  test('half-learned underline is also suppressed', async ({ page }) => {
    await seedAndOpen(page, {
      config: { tapToRevealTranslations: true },
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
    const opacity = await span.evaluate((el) =>
      (el as HTMLElement).style.getPropertyValue('--familiarity-opacity'),
    );
    expect(opacity).toBe('');
    await expect(span).toHaveCSS('text-decoration-line', 'none');
    await expect(span.locator('.translation-overlay')).toHaveCount(0);
  });

  test('default config still auto-shows familiarity 0 overlay', async ({
    page,
  }) => {
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
    const opacity = await span.evaluate((el) =>
      (el as HTMLElement).style.getPropertyValue('--familiarity-opacity'),
    );
    expect(opacity).toBe('1');
  });
});
