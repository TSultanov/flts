import { expect, test } from '@playwright/test';
import {
  expectTranslated,
  expectWordSpansMounted,
  expectWordSpansUnmounted,
  fillerSegments,
  getTranslateCalls,
  multipageSpec,
  paragraphLocator,
  scrollToParagraph,
  seedAndOpen,
  setTranslateConfig,
  translateButton,
  wordSegment,
  wordSpan,
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
    const segments = [
      wordSegment({
        flatIndex: 0,
        sentence: 0,
        word: 0,
        text: 'hola',
        translation: 'hello',
      }),
    ];

    const { bookId } = await seedAndOpen(
      page,
      multipageSpec(COUNT, {}, {
        translateConfigs: [
          { paragraphId: 40, cfg: { kind: 'immediate', segments } },
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
              segments: [
                wordSegment({
                  flatIndex: 0,
                  sentence: 0,
                  word: 0,
                  text: 'multipage done',
                  translation: null,
                }),
              ],
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
              segments: [
                wordSegment({
                  flatIndex: 0,
                  sentence: 0,
                  word: 0,
                  text: 'finished while away',
                  translation: null,
                }),
              ],
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
    const segmentsFor = (prefix: string) =>
      [0, 1, 2].flatMap((i) => [
        ...(i > 0 ? [{ kind: 'gap' as const, html: ' ' }] : []),
        wordSegment({
          flatIndex: i,
          sentence: 0,
          word: i,
          text: `${prefix}-${i}`,
          translation: `t${prefix.replace('w', '')}-${i}`,
        }),
      ]);
    const segments40 = segmentsFor('w40');
    const segments65 = segmentsFor('w65');

    await seedAndOpen(
      page,
      multipageSpec(COUNT, {
        40: { segments: segments40, visibleWords: [0, 2] },
        65: { segments: segments65, visibleWords: [1] },
      }),
    );

    const p40 = paragraphLocator(page, 40);
    const p65 = paragraphLocator(page, 65);

    const overlay = (p: ReturnType<typeof paragraphLocator>, i: number) =>
      wordSpan(p, i).locator('.translation-overlay');

    // With lazy mount the paragraph must be within the mount window before
    // overlays exist — scroll there first, then assert.
    await scrollToParagraph(page, 40);
    await expect(overlay(p40, 0)).toHaveCount(1);
    await expect(overlay(p40, 2)).toHaveCount(1);
    await expect(overlay(p40, 1)).toHaveCount(0);

    await scrollToParagraph(page, 65);
    await expect(overlay(p65, 1)).toHaveCount(1);
    await expect(overlay(p65, 0)).toHaveCount(0);
    await expect(overlay(p65, 2)).toHaveCount(0);

    // Scroll churn must not lose overlays once we return to the annotated
    // paragraph.
    await scrollToParagraph(page, 0);
    await scrollToParagraph(page, 40);
    await expect(overlay(p40, 0)).toHaveCount(1);
    await expect(overlay(p40, 2)).toHaveCount(1);
    await scrollToParagraph(page, 65);
    await expect(overlay(p65, 1)).toHaveCount(1);
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
              segments: [
                wordSegment({
                  flatIndex: 0,
                  sentence: 0,
                  word: 0,
                  text: 'p10 done',
                  translation: null,
                }),
              ],
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
              segments: [
                wordSegment({
                  flatIndex: 0,
                  sentence: 0,
                  word: 0,
                  text: 'p65 done',
                  translation: null,
                }),
              ],
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

  // ===== Lazy-mount regression suite (L1–L6) ===============================
  //
  // The shared fixture pre-translates every paragraph with segments that tile
  // the entire original filler text, so mounted vs unmounted rendering
  // produces near-identical layout. This mirrors how the real backend emits
  // segments and keeps the mount-window decision driven by viewport distance
  // rather than artificial size deltas between the two render branches.

  function allTranslatedSpec() {
    const overrides: Record<number, { segments: ReturnType<typeof fillerSegments> }> = {};
    for (let i = 0; i < COUNT; i++) {
      overrides[i] = { segments: fillerSegments(i) };
    }
    return multipageSpec(COUNT, overrides);
  }

  // ----- L1 ----------------------------------------------------------------

  test('L1: far paragraphs render no WordSpans on initial load', async ({ page }) => {
    await seedAndOpen(page, allTranslatedSpec());

    // Wait for the chapter to be ready so the mount-window compute has fired.
    await expectWordSpansMounted(page, 0);

    // Initial load lands at paragraph 0. Far paragraphs (40, 79) must not
    // have mounted WordSpans.
    await expectWordSpansUnmounted(page, 40);
    await expectWordSpansUnmounted(page, 79);

    // Far paragraphs also must not carry a translate button — only the
    // plain original <p> survives in the unmounted state.
    await expect(translateButton(paragraphLocator(page, 40))).toHaveCount(0);
    await expect(translateButton(paragraphLocator(page, 79))).toHaveCount(0);
  });

  // ----- L1b ---------------------------------------------------------------

  test('L1b: untranslated far paragraphs also drop the translate button', async ({ page }) => {
    // No pre-translation anywhere: every paragraph starts in state A.
    await seedAndOpen(page, multipageSpec(COUNT));

    // Near paragraphs keep the translate button (state A in-window).
    await expect(translateButton(paragraphLocator(page, 0))).toHaveCount(1);

    // Far paragraphs lose it even though they're untranslated.
    await expect(translateButton(paragraphLocator(page, 40))).toHaveCount(0);
    await expect(translateButton(paragraphLocator(page, 79))).toHaveCount(0);

    // Scrolling them back into the mount window restores the button.
    await scrollToParagraph(page, 40);
    await expect(translateButton(paragraphLocator(page, 40))).toHaveCount(1);
  });

  // ----- L2 ----------------------------------------------------------------

  test('L2: scroll moves the mount window symmetrically', async ({ page }) => {
    await seedAndOpen(page, allTranslatedSpec());

    await scrollToParagraph(page, 40);
    await expectWordSpansMounted(page, 40);
    await expectWordSpansMounted(page, 38);
    await expectWordSpansMounted(page, 42);
    await expectWordSpansUnmounted(page, 0);
    await expectWordSpansUnmounted(page, 79);

    await scrollToParagraph(page, 60);
    await expectWordSpansMounted(page, 60);
    await expectWordSpansMounted(page, 58);
    await expectWordSpansMounted(page, 62);
    await expectWordSpansUnmounted(page, 40);
    await expectWordSpansUnmounted(page, 79);
  });

  // ----- L3 ----------------------------------------------------------------

  test('L3: scroll across mount-window boundaries does not jump position', async ({ page }) => {
    await seedAndOpen(page, allTranslatedSpec());

    // Land near the middle so we have plenty of room to cross mount-window
    // boundaries in both directions.
    await scrollToParagraph(page, 40);

    // Capture the offset of a paragraph that is JUST inside the mount window
    // (paragraph 38, two before visible). Then step the scroll forward a few
    // small amounts, each crossing the mount-window edge. The reference
    // paragraph's relative position must change smoothly — a mount/unmount
    // cascade that resized siblings would push it sideways non-monotonically.
    const samples = await page.evaluate(async () => {
      const container = document.querySelector('.paragraphs-container') as HTMLElement;
      const ref = container.querySelector(
        '.paragraph-wrapper[data-paragraph-id="42"]',
      ) as HTMLElement;
      const pageWidth = container.clientWidth;
      const startScroll = container.scrollLeft;
      const out: Array<{ scrollLeft: number; refLeft: number }> = [];
      for (let i = 0; i <= 20; i++) {
        container.scrollLeft = startScroll + (pageWidth * 2 * i) / 20;
        // One paint cycle is sufficient — no need to wait many frames.
        await new Promise((r) => setTimeout(r, 30));
        out.push({
          scrollLeft: container.scrollLeft,
          refLeft: ref.getBoundingClientRect().left,
        });
      }
      return out;
    });

    expect(samples.length).toBe(21);
    // scrollLeft must increase monotonically (we asked for monotone targets).
    for (let i = 1; i < samples.length; i++) {
      expect(samples[i].scrollLeft).toBeGreaterThanOrEqual(samples[i - 1].scrollLeft - 1);
    }
    // refLeft = ref.rectLeft. As we scroll right, the ref's viewport-relative
    // position must decrease monotonically (it moves leftward). Any sudden
    // mount/unmount-induced reflow would break this monotony.
    for (let i = 1; i < samples.length; i++) {
      const delta = samples[i].refLeft - samples[i - 1].refLeft;
      // Allow ~2px tolerance for sub-pixel rounding.
      expect(delta).toBeLessThanOrEqual(2);
    }
  });

  // ----- L4 ----------------------------------------------------------------

  test('L4: re-mounted paragraph restores its visible-word overlays', async ({ page }) => {
    // Attach translations to the first few words so the visibleWords annotation
    // has overlay text to display.
    const segments50 = fillerSegments(50).map((seg) => {
      if (seg.kind === 'word' && seg.flatIndex < 3) {
        return { ...seg, translation: `tr-${seg.flatIndex}` };
      }
      return seg;
    });
    await seedAndOpen(
      page,
      multipageSpec(COUNT, {
        50: { segments: segments50, visibleWords: [0, 2] },
      }),
    );

    // Visit 50, leave (it unmounts), come back. Overlays must be present
    // again after the round trip.
    await scrollToParagraph(page, 50);
    const p50 = paragraphLocator(page, 50);
    await expect(wordSpan(p50, 0).locator('.translation-overlay')).toHaveCount(1);
    await expect(wordSpan(p50, 2).locator('.translation-overlay')).toHaveCount(1);

    await scrollToParagraph(page, 0);
    await expectWordSpansUnmounted(page, 50);

    await scrollToParagraph(page, 50);
    await expect(wordSpan(p50, 0).locator('.translation-overlay')).toHaveCount(1);
    await expect(wordSpan(p50, 2).locator('.translation-overlay')).toHaveCount(1);
    await expect(wordSpan(p50, 1).locator('.translation-overlay')).toHaveCount(0);
  });

  // ----- L5 ----------------------------------------------------------------

  test('L5: selection survives an unmount/remount cycle', async ({ page }) => {
    const segments40 = fillerSegments(40);
    await seedAndOpen(
      page,
      multipageSpec(COUNT, { 40: { segments: segments40 } }),
    );

    await scrollToParagraph(page, 40);
    const p40 = paragraphLocator(page, 40);
    await wordSpan(p40, 1).click();
    await expect(wordSpan(p40, 1)).toHaveClass(/selected/);

    await scrollToParagraph(page, 0);
    await expectWordSpansUnmounted(page, 40);

    await scrollToParagraph(page, 40);
    // The selection lives in ChapterView, so after re-mount the same word
    // must reappear highlighted.
    await expect(wordSpan(p40, 1)).toHaveClass(/selected/);
  });

  // ----- L6 ----------------------------------------------------------------

  test('L6: translation completing on an unmounted paragraph still renders on return', async ({
    page,
  }) => {
    const { bookId } = await seedAndOpen(page, multipageSpec(COUNT));

    // Scroll to 40 so it's in the mount window, kick off a slow translate.
    await scrollToParagraph(page, 40);
    await setTranslateConfig(page, bookId, 40, {
      kind: 'progress',
      steps: [
        { progress: 50, total: 100, delayMs: 300 },
        { progress: 100, total: 100, delayMs: 300 },
      ],
      segments: [
        wordSegment({
          flatIndex: 0,
          sentence: 0,
          word: 0,
          text: 'late mount',
          translation: null,
        }),
      ],
    });
    await translateButton(paragraphLocator(page, 40)).click();
    await expect(paragraphLocator(page, 40).locator('.circular-progress')).toBeVisible();

    // Scroll away so paragraph 40 unmounts WHILE the translation is still
    // running. Wait past completion in the background.
    await scrollToParagraph(page, 0);
    await page.waitForTimeout(900);

    // Return: WordSpans must render with the late-arriving segments.
    await scrollToParagraph(page, 40);
    await expectTranslated(paragraphLocator(page, 40));
    await expect(paragraphLocator(page, 40).getByText('late mount')).toBeVisible();
  });
});
