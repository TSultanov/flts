import { expect, test } from "./helpers/test";
import {
  paragraphLocator,
  type ParagraphSegment,
  seedAndOpen,
  wordSegment,
} from "./helpers/paragraph";

// Pins where a translation overlay sits relative to the glyphs it annotates.
// The overlay is positioned against the WordSpan's box, so any change to that
// box's display or height moves it. No other test in the suite detects that.
test.describe("Word overlay geometry (chromium only)", () => {
  test.skip(({ browserName }) => browserName !== "chromium", "chromium-only");

  const spec = {
    chapters: [
      {
        paragraphs: [
          {
            html: "hola mundo",
            segments: [
              wordSegment({
                flatIndex: 0,
                sentence: 0,
                word: 0,
                text: "hola",
                translation: "hello",
                familiarity: 0,
              }),
              { kind: "gap" as const, text: " " },
              wordSegment({
                flatIndex: 1,
                sentence: 0,
                word: 1,
                text: "mundo",
                translation: "world",
                familiarity: 0,
              }),
            ],
          },
        ],
      },
    ],
  };

  /** Overlay box measured against the word's glyph box, not against the span. */
  async function measure(page: import("@playwright/test").Page) {
    return paragraphLocator(page, 0).evaluate((el) => {
      const span = el.querySelector<HTMLElement>(
        '.word-span[data-flat-index="0"]',
      )!;
      const overlay = span.querySelector<HTMLElement>(".translation-overlay")!;
      // The glyph run, which is what the reader aligns the overlay against.
      const textNode = Array.from(span.childNodes).find(
        (n) => n.nodeType === Node.TEXT_NODE,
      )!;
      const range = document.createRange();
      range.selectNodeContents(textNode);
      const text = range.getBoundingClientRect();
      const o = overlay.getBoundingClientRect();
      const round = (n: number) => Math.round(n * 10) / 10;
      return {
        topFromText: round(o.top - text.top),
        leftFromText: round(o.left - text.left),
        height: round(o.height),
        widthRatio: round(o.width / text.width),
      };
    });
  }

  test("overlay sits in the leading above its word (desktop)", async ({
    page,
  }) => {
    await page.setViewportSize({ width: 1280, height: 720 });
    await seedAndOpen(page, spec);
    await expect(
      paragraphLocator(page, 0).locator(".translation-overlay").first(),
    ).toBeVisible();

    const m = await measure(page);
    // Overlay rides in the half-leading above the glyphs, exactly as wide.
    expect(m).toEqual({
      topFromText: -6,
      leftFromText: 0,
      height: 9.7,
      widthRatio: 1,
    });
  });

  test("overlay sits in the leading above its word (narrow)", async ({
    page,
  }) => {
    await page.setViewportSize({ width: 560, height: 720 });
    await seedAndOpen(page, spec);
    await expect(
      paragraphLocator(page, 0).locator(".translation-overlay").first(),
    ).toBeVisible();

    const m = await measure(page);
    // Overlay rides in the half-leading above the glyphs, exactly as wide.
    expect(m).toEqual({
      topFromText: -6,
      leftFromText: 0,
      height: 9.7,
      widthRatio: 1,
    });
  });

  /** `hyphens: auto` splits this word, and its span, across two lines. */
  function hyphenatedSpec(fillerCount: number) {
    const long = "antidisestablishmentarianism";
    const words = [
      ...Array.from({ length: fillerCount }, () => "filler"),
      long,
      "tail",
      "words",
      "after",
      "the",
      "long",
      "one",
    ];
    const segments = words.flatMap((text, i) => {
      const segs: ParagraphSegment[] = [
        wordSegment({
          flatIndex: i,
          sentence: 0,
          word: i,
          text,
          translation: text === long ? "largo" : null,
          familiarity: 0,
        }),
      ];
      if (i < words.length - 1) segs.push({ kind: "gap", text: " " });
      return segs;
    });
    return {
      chapters: [{ paragraphs: [{ html: words.join(" "), segments }] }],
    };
  }

  async function measureSplit(page: import("@playwright/test").Page) {
    return paragraphLocator(page, 0).evaluate((el) => {
      const span = Array.from(
        el.querySelectorAll<HTMLElement>(".word-span"),
      ).find((s) => s.textContent!.includes("antidis"))!;
      const overlay = span.querySelector<HTMLElement>(".translation-overlay")!;
      const fragments = Array.from(span.getClientRects());
      const widest = fragments.reduce((a, b) => (b.width > a.width ? b : a));
      const o = overlay.getBoundingClientRect();
      const round = (n: number) => Math.round(n * 10) / 10;
      return {
        fragmentCount: fragments.length,
        leftFromWidest: round(o.left - widest.left),
        topFromWidest: round(o.top - widest.top),
        widthRatio: round(o.width / widest.width),
      };
    });
  }

  // The overlay covers one whole fragment, not a sliver of the first line.
  for (const [label, fillerCount] of [
    ["first line", 10],
    ["second line", 12],
  ] as const) {
    test(`overlay stays visible over a word hyphenated onto two lines (widest fragment on the ${label})`, async ({
      page,
    }) => {
      await page.setViewportSize({ width: 560, height: 720 });
      await seedAndOpen(page, hyphenatedSpec(fillerCount));
      await expect(
        paragraphLocator(page, 0).locator(".translation-overlay").first(),
      ).toBeVisible();

      const m = await measureSplit(page);
      expect(m.fragmentCount).toBe(2);
      expect(m).toEqual({
        fragmentCount: 2,
        leftFromWidest: 0,
        topFromWidest: 0,
        widthRatio: 1,
      });
    });
  }

  // The overrides must come off when the word rejoins one line.
  test("overlay returns to the whole word once it stops being hyphenated", async ({
    page,
  }) => {
    await page.setViewportSize({ width: 560, height: 720 });
    await seedAndOpen(page, hyphenatedSpec(12));
    await expect(
      paragraphLocator(page, 0).locator(".translation-overlay").first(),
    ).toBeVisible();
    expect((await measureSplit(page)).fragmentCount).toBe(2);

    await page.setViewportSize({ width: 1280, height: 720 });
    await expect
      .poll(async () => (await measureSplit(page)).fragmentCount)
      .toBe(1);
    expect(await measureSplit(page)).toEqual({
      fragmentCount: 1,
      leftFromWidest: 0,
      topFromWidest: 0,
      widthRatio: 1,
    });
  });
});
