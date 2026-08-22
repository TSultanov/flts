import { expect, test } from "./helpers/test";
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
} from "./helpers/paragraph";

// Lazy-loading regressions: everything a user must still see when paragraphs
// virtualize on scroll.
test.describe("ParagraphView (multipage, chromium only)", () => {
  test.skip(({ browserName }) => browserName !== "chromium", "chromium-only");

  const COUNT = 80;

  test("M1: 80 paragraphs render in order; chapter is genuinely long", async ({
    page,
  }) => {
    await seedAndOpen(page, multipageSpec(COUNT));

    const ids = await page.evaluate(() => {
      const wrappers = document.querySelectorAll(
        ".paragraphs-container .paragraph-wrapper",
      );
      return Array.from(wrappers).map(
        (c) => (c as HTMLElement).dataset["paragraphId"],
      );
    });
    expect(ids).toEqual(Array.from({ length: COUNT }, (_, i) => String(i)));

    // Confirms a real long-scroll target: >50 viewports wide.
    const ratio = await page.evaluate(() => {
      const c = document.querySelector(".paragraphs-container") as HTMLElement;
      return c.scrollWidth / c.clientWidth;
    });
    expect(ratio).toBeGreaterThan(50);
  });

  test("M2: translate a middle paragraph (40) after scrolling to it", async ({
    page,
  }) => {
    const segments = [
      wordSegment({
        flatIndex: 0,
        sentence: 0,
        word: 0,
        text: "hola",
        translation: "hello",
      }),
    ];

    const { bookId } = await seedAndOpen(
      page,
      multipageSpec(
        COUNT,
        {},
        {
          translateConfigs: [
            { paragraphId: 40, cfg: { kind: "immediate", segments } },
          ],
        },
      ),
    );

    const initialScrollLeft = await page.evaluate(() => {
      return (document.querySelector(".paragraphs-container") as HTMLElement)
        .scrollLeft;
    });
    expect(initialScrollLeft).toBeLessThan(50); // starts near paragraph 0

    await scrollToParagraph(page, 40);
    const p40 = paragraphLocator(page, 40);
    await translateButton(p40).click();

    await expectTranslated(p40);
    await expect(p40.locator(".word-span")).toHaveText("hola");

    const calls = await getTranslateCalls(page);
    expect(calls).toHaveLength(1);
    expect(calls[0]).toMatchObject({ bookId, paragraphId: 40 });
  });

  test("M3: spinner persists across scroll-away-and-back during a long translation", async ({
    page,
  }) => {
    await seedAndOpen(
      page,
      multipageSpec(
        COUNT,
        {},
        {
          translateConfigs: [
            {
              paragraphId: 40,
              cfg: {
                kind: "progress",
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
                    text: "multipage done",
                    translation: null,
                  }),
                ],
              },
            },
          ],
        },
      ),
    );

    const p40 = paragraphLocator(page, 40);
    await scrollToParagraph(page, 40);
    await translateButton(p40).click();

    await expect(p40.locator(".circular-progress")).toBeVisible();
    await expect(translateButton(p40)).toBeDisabled();

    await scrollToParagraph(page, 0);
    await scrollToParagraph(page, 79);
    await scrollToParagraph(page, 40);

    await expect(p40.locator(".circular-progress")).toBeVisible();
    await expect(translateButton(p40)).toBeDisabled();

    await expectTranslated(p40);
    await expect(p40.locator(".circular-progress")).toHaveCount(0);
    await expect(p40.getByText("multipage done")).toBeVisible();
  });

  test("M4: translation completing while scrolled away still lands on return", async ({
    page,
  }) => {
    await seedAndOpen(
      page,
      multipageSpec(
        COUNT,
        {},
        {
          translateConfigs: [
            {
              paragraphId: 40,
              cfg: {
                kind: "progress",
                steps: [
                  { progress: 50, total: 100, delayMs: 300 },
                  { progress: 100, total: 100, delayMs: 300 },
                ],
                segments: [
                  wordSegment({
                    flatIndex: 0,
                    sentence: 0,
                    word: 0,
                    text: "finished while away",
                    translation: null,
                  }),
                ],
              },
            },
          ],
        },
      ),
    );

    const p40 = paragraphLocator(page, 40);
    await scrollToParagraph(page, 40);
    await translateButton(p40).click();
    await expect(p40.locator(".circular-progress")).toBeVisible();

    await scrollToParagraph(page, 0);
    await page.waitForTimeout(900);

    await scrollToParagraph(page, 40);
    await expect(p40.locator(".circular-progress")).toHaveCount(0);
    await expectTranslated(p40);
    await expect(p40.getByText("finished while away")).toBeVisible();
  });

  test("M5: auto-show annotations apply on scroll-into-view and persist across churn", async ({
    page,
  }) => {
    const segmentsFor = (prefix: string, autoShow: number[]) =>
      [0, 1, 2].flatMap((i) => [
        ...(i > 0 ? [{ kind: "gap" as const, html: " " }] : []),
        wordSegment({
          flatIndex: i,
          sentence: 0,
          word: i,
          text: `${prefix}-${i}`,
          translation: `t${prefix.replace("w", "")}-${i}`,
          familiarity: autoShow.includes(i) ? 0 : 1,
        }),
      ]);
    const segments40 = segmentsFor("w40", [0, 2]);
    const segments65 = segmentsFor("w65", [1]);

    await seedAndOpen(
      page,
      multipageSpec(COUNT, {
        40: { segments: segments40 },
        65: { segments: segments65 },
      }),
    );

    const p40 = paragraphLocator(page, 40);
    const p65 = paragraphLocator(page, 65);

    const overlay = (p: ReturnType<typeof paragraphLocator>, i: number) =>
      wordSpan(p, i).locator(".translation-overlay");

    // Overlays only exist inside the mount window.
    await scrollToParagraph(page, 40);
    await expect(overlay(p40, 0)).toHaveCount(1);
    await expect(overlay(p40, 2)).toHaveCount(1);
    await expect(overlay(p40, 1)).toHaveCount(0);

    await scrollToParagraph(page, 65);
    await expect(overlay(p65, 1)).toHaveCount(1);
    await expect(overlay(p65, 0)).toHaveCount(0);
    await expect(overlay(p65, 2)).toHaveCount(0);

    // Scroll churn must not lose the overlays.
    await scrollToParagraph(page, 0);
    await scrollToParagraph(page, 40);
    await expect(overlay(p40, 0)).toHaveCount(1);
    await expect(overlay(p40, 2)).toHaveCount(1);
    await scrollToParagraph(page, 65);
    await expect(overlay(p65, 1)).toHaveCount(1);
  });

  test("M6: two in-flight translations stay in their own lanes", async ({
    page,
  }) => {
    await seedAndOpen(
      page,
      multipageSpec(
        COUNT,
        {},
        {
          translateConfigs: [
            {
              paragraphId: 10,
              cfg: {
                kind: "progress",
                steps: [
                  { progress: 50, total: 100, delayMs: 600 },
                  { progress: 100, total: 100, delayMs: 600 },
                ],
                segments: [
                  wordSegment({
                    flatIndex: 0,
                    sentence: 0,
                    word: 0,
                    text: "p10 done",
                    translation: null,
                  }),
                ],
              },
            },
            {
              paragraphId: 65,
              cfg: {
                kind: "progress",
                steps: [
                  { progress: 50, total: 100, delayMs: 600 },
                  { progress: 100, total: 100, delayMs: 600 },
                ],
                segments: [
                  wordSegment({
                    flatIndex: 0,
                    sentence: 0,
                    word: 0,
                    text: "p65 done",
                    translation: null,
                  }),
                ],
              },
            },
          ],
        },
      ),
    );

    const p10 = paragraphLocator(page, 10);
    const p65 = paragraphLocator(page, 65);

    await scrollToParagraph(page, 10);
    await translateButton(p10).click();
    await expect(p10.locator(".circular-progress")).toBeVisible();

    await scrollToParagraph(page, 65);
    await translateButton(p65).click();
    await expect(p65.locator(".circular-progress")).toBeVisible();

    // The two translations are independent.
    await scrollToParagraph(page, 10);
    await expect(p10.locator(".circular-progress")).toBeVisible();

    await expectTranslated(p10);
    await expect(p10.getByText("p10 done")).toBeVisible();

    await scrollToParagraph(page, 65);
    await expectTranslated(p65);
    await expect(p65.getByText("p65 done")).toBeVisible();

    const calls = await getTranslateCalls(page);
    expect(calls).toHaveLength(2);
    expect(calls.map((c) => c.paragraphId).sort((a, b) => a - b)).toEqual([
      10, 65,
    ]);
  });

  // The shared fixture tiles every paragraph with segments, as the backend
  // does, so mounted and unmounted layouts match and the mount window is
  // decided by viewport distance rather than a size delta between branches.

  function allTranslatedSpec() {
    const overrides: Record<
      number,
      { segments: ReturnType<typeof fillerSegments> }
    > = {};
    for (let i = 0; i < COUNT; i++) {
      overrides[i] = { segments: fillerSegments(i) };
    }
    return multipageSpec(COUNT, overrides);
  }

  test("L1: far paragraphs render no WordSpans on initial load", async ({
    page,
  }) => {
    await seedAndOpen(page, allTranslatedSpec());

    await expectWordSpansMounted(page, 0);

    // Load lands at paragraph 0, so 40 and 79 must have no WordSpans.
    await expectWordSpansUnmounted(page, 40);
    await expectWordSpansUnmounted(page, 79);

    // Unmounted leaves only the plain original <p>: no translate button.
    await expect(translateButton(paragraphLocator(page, 40))).toHaveCount(0);
    await expect(translateButton(paragraphLocator(page, 79))).toHaveCount(0);
  });

  test("L1b: untranslated far paragraphs also drop the translate button", async ({
    page,
  }) => {
    await seedAndOpen(page, multipageSpec(COUNT));

    await expect(translateButton(paragraphLocator(page, 0))).toHaveCount(1);

    await expect(translateButton(paragraphLocator(page, 40))).toHaveCount(0);
    await expect(translateButton(paragraphLocator(page, 79))).toHaveCount(0);

    await scrollToParagraph(page, 40);
    await expect(translateButton(paragraphLocator(page, 40))).toHaveCount(1);
  });

  test("L2: scroll moves the mount window symmetrically", async ({ page }) => {
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

  test("L3: scroll across mount-window boundaries does not jump position", async ({
    page,
  }) => {
    await seedAndOpen(page, allTranslatedSpec());

    // Mid-chapter, so mount-window boundaries can be crossed both ways.
    await scrollToParagraph(page, 40);

    // Paragraph 38 sits just inside the mount window; stepping across the
    // window edge must move it smoothly, since a mount/unmount cascade that
    // resized siblings would shift it non-monotonically.
    const samples = await page.evaluate(async () => {
      const container = document.querySelector(
        ".paragraphs-container",
      ) as HTMLElement;
      const ref = container.querySelector(
        '.paragraph-wrapper[data-paragraph-id="42"]',
      ) as HTMLElement;
      const pageWidth = container.clientWidth;
      const startScroll = container.scrollLeft;
      const out: Array<{ scrollLeft: number; refLeft: number }> = [];
      for (let i = 0; i <= 20; i++) {
        container.scrollLeft = startScroll + (pageWidth * 2 * i) / 20;
        await new Promise((r) => setTimeout(r, 30));
        out.push({
          scrollLeft: container.scrollLeft,
          refLeft: ref.getBoundingClientRect().left,
        });
      }
      return out;
    });

    expect(samples.length).toBe(21);
    for (let i = 1; i < samples.length; i++) {
      expect(samples[i].scrollLeft).toBeGreaterThanOrEqual(
        samples[i - 1].scrollLeft - 1,
      );
    }
    // Scrolling right must move the ref leftward, monotonically.
    for (let i = 1; i < samples.length; i++) {
      const delta = samples[i].refLeft - samples[i - 1].refLeft;
      expect(delta).toBeLessThanOrEqual(2);
    }
  });

  test("L4: re-mounted paragraph restores its auto-shown overlays", async ({
    page,
  }) => {
    // Words 0 and 2 auto-show, word 1 stays hidden: a contrast that must
    // survive the unmount/remount cycle.
    const segments50 = fillerSegments(50).map((seg) => {
      if (seg.kind === "word" && seg.flatIndex < 3) {
        return {
          ...seg,
          translation: `tr-${seg.flatIndex}`,
          familiarity: seg.flatIndex === 1 ? 1 : 0,
        };
      }
      return seg;
    });
    await seedAndOpen(
      page,
      multipageSpec(COUNT, {
        50: { segments: segments50 },
      }),
    );

    // Overlays must return after the unmount round trip.
    await scrollToParagraph(page, 50);
    const p50 = paragraphLocator(page, 50);
    await expect(wordSpan(p50, 0).locator(".translation-overlay")).toHaveCount(
      1,
    );
    await expect(wordSpan(p50, 2).locator(".translation-overlay")).toHaveCount(
      1,
    );

    await scrollToParagraph(page, 0);
    await expectWordSpansUnmounted(page, 50);

    await scrollToParagraph(page, 50);
    await expect(wordSpan(p50, 0).locator(".translation-overlay")).toHaveCount(
      1,
    );
    await expect(wordSpan(p50, 2).locator(".translation-overlay")).toHaveCount(
      1,
    );
    await expect(wordSpan(p50, 1).locator(".translation-overlay")).toHaveCount(
      0,
    );
  });

  test("L5: selection survives an unmount/remount cycle", async ({ page }) => {
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
    // The selection lives in ChapterView, so it must survive re-mount.
    await expect(wordSpan(p40, 1)).toHaveClass(/selected/);
  });

  test("L6: translation completing on an unmounted paragraph still renders on return", async ({
    page,
  }) => {
    const { bookId } = await seedAndOpen(page, multipageSpec(COUNT));

    await scrollToParagraph(page, 40);
    await setTranslateConfig(page, bookId, 40, {
      kind: "progress",
      steps: [
        { progress: 50, total: 100, delayMs: 300 },
        { progress: 100, total: 100, delayMs: 300 },
      ],
      segments: [
        wordSegment({
          flatIndex: 0,
          sentence: 0,
          word: 0,
          text: "late mount",
          translation: null,
        }),
      ],
    });
    await translateButton(paragraphLocator(page, 40)).click();
    await expect(
      paragraphLocator(page, 40).locator(".circular-progress"),
    ).toBeVisible();

    // Paragraph 40 must unmount while its translation is still running.
    await scrollToParagraph(page, 0);
    await page.waitForTimeout(900);

    await scrollToParagraph(page, 40);
    await expectTranslated(paragraphLocator(page, 40));
    await expect(
      paragraphLocator(page, 40).getByText("late mount"),
    ).toBeVisible();
  });
});
