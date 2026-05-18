import { expect, test } from '@playwright/test';
import {
  getTranslationsBatchCalls,
  seedAndOpen,
  wordSegment,
} from './helpers/paragraph';

// Chromium-only — the regression is in Svelte reactivity / DOM measurement,
// not in any browser-specific layout behaviour.
test.describe.configure({ mode: 'parallel' });

test.describe('Chapter initial translation batch (chromium only)', () => {
  test.skip(({ browserName }) => browserName !== 'chromium', 'chromium-only');

  test('opening a chapter does not enqueue translations for the whole chapter on initial mount', async ({
    page,
  }) => {
    // Large enough that "the whole chapter" is clearly different from
    // "the visible window". The regression we're guarding against:
    // #recomputeMountWindow running against empty paragraph wrappers
    // (before any originals arrive) classifies ~every paragraph as
    // mounted, which feeds the translations queue with the whole
    // chapter — back-pressuring the originals queue on the shared
    // book.lock() in the Rust backend.
    //
    // Each paragraph carries substantial text so a loaded wrapper has
    // realistic height. Without this, every wrapper (loaded or not) is
    // a single line and the geometric mount window catches almost all
    // of them in both broken and fixed paths.
    const N = 80;
    const bodyText = (
      'Lorem ipsum dolor sit amet, consectetur adipiscing elit. ' +
      'Sed do eiusmod tempor incididunt ut labore et dolore magna ' +
      'aliqua. Ut enim ad minim veniam, quis nostrud exercitation ' +
      'ullamco laboris nisi ut aliquip ex ea commodo consequat. ' +
      'Duis aute irure dolor in reprehenderit in voluptate velit ' +
      'esse cillum dolore eu fugiat nulla pariatur.'
    ).repeat(2);
    const paragraphs = Array.from({ length: N }, (_, i) => ({
      html: `<p>${bodyText} (paragraph ${i})</p>`,
      segments: [
        wordSegment({
          flatIndex: 0,
          sentence: 0,
          word: 0,
          text: `w${i}`,
          translation: `t${i}`,
        }),
      ],
    }));

    await seedAndOpen(page, { chapters: [{ paragraphs }] });

    await expect(page.locator('.paragraphs-container.is-ready')).toBeVisible();

    // Wait for at least one translations batch to land. The mount-window
    // computation that triggers it runs inside a rAF after the originals
    // batch resolves; allow a moment for that chain to flush.
    await expect
      .poll(async () => (await getTranslationsBatchCalls(page)).length, {
        timeout: 5000,
      })
      .toBeGreaterThan(0);

    const calls = await getTranslationsBatchCalls(page);
    const totalQueued = new Set(calls.flatMap((c) => c.paragraphIds)).size;
    // Healthy: the eager visible-window enqueue (sized to the container's
    // clientHeight at open time, target ± ~paragraphsPerPage) plus the
    // mount-window recompute. At default Playwright viewport that lands
    // in the 15-25 range. 40 is comfortably above the healthy case and
    // well below the broken case (whole-chapter enqueue ~N=80).
    expect(totalQueued).toBeLessThanOrEqual(40);
  });
});
