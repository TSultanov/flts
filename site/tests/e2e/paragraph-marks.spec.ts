import { expect, test } from "./helpers/test";
import {
  fillerHtml,
  multipageSpec,
  paragraphLocator,
  scrollToParagraph,
  seedAndOpen,
  segmentsOfHtml,
  wordSegment,
  type ParagraphSegment,
} from "./helpers/paragraph";

// Inline emphasis reaches the reader as structured marks on segments, so the
// mounted and virtualized renderings put the same characters under the same
// wrappers. Raw tags cannot do this: each {@html} fragment parses on its own,
// so a <b> that spans segments renders as an empty element and loses its word.
test.describe("Paragraph marks (chromium only)", () => {
  test.skip(({ browserName }) => browserName !== "chromium", "chromium-only");

  const markedSegments: ParagraphSegment[] = [
    {
      ...wordSegment({
        flatIndex: 0,
        sentence: 0,
        word: 0,
        text: "Bold",
        translation: null,
      }),
      marks: ["strong"],
    },
    { kind: "gap", text: " " },
    {
      ...wordSegment({
        flatIndex: 1,
        sentence: 0,
        word: 1,
        text: "Slanted",
        translation: null,
      }),
      marks: ["emphasis"],
    },
    { kind: "gap", text: " and " },
    wordSegment({
      flatIndex: 2,
      sentence: 0,
      word: 2,
      text: "plain",
      translation: null,
    }),
    { kind: "break" },
    { kind: "gap", text: "Tom & Jerry at café" },
  ];

  const spec = () => ({
    chapters: [
      {
        paragraphs: [
          {
            html: "<b>Bold</b> <i>Slanted</i> and plain<br>Tom &amp; Jerry at caf&eacute;",
            segments: markedSegments,
          },
        ],
      },
    ],
  });

  test("a translated paragraph renders its emphasis", async ({ page }) => {
    await seedAndOpen(page, spec());
    const p = paragraphLocator(page, 0);
    await expect(p).toBeAttached();

    const weights = await p.evaluate((el) => {
      const weightOf = (text: string) => {
        const span = Array.from(el.querySelectorAll(".word-span")).find(
          (s) => s.textContent?.trim() === text,
        );
        if (!span) return null;
        const cs = getComputedStyle(span);
        return { weight: Number(cs.fontWeight), style: cs.fontStyle };
      };
      return {
        bold: weightOf("Bold"),
        slanted: weightOf("Slanted"),
        plain: weightOf("plain"),
        breaks: el.querySelectorAll("br").length,
      };
    });

    expect(weights.bold?.weight).toBeGreaterThanOrEqual(600);
    expect(weights.slanted?.style).toBe("italic");
    expect(weights.plain?.weight).toBeLessThan(600);
    expect(weights.plain?.style).toBe("normal");
    expect(weights.breaks).toBe(1);
  });

  test("virtualizing a paragraph changes nothing it renders", async ({
    page,
  }) => {
    const COUNT = 80;
    const MARKED = 40;
    const overrides: Record<
      number,
      { html: string; segments: ParagraphSegment[] }
    > = {};
    for (let i = 0; i < COUNT; i++) {
      const html = fillerHtml(i);
      overrides[i] = { html, segments: segmentsOfHtml(html) };
    }
    overrides[MARKED] = {
      html: "<b>Bold</b> <i>Slanted</i> and plain<br>Tom &amp; Jerry at caf&eacute;",
      segments: markedSegments,
    };
    await seedAndOpen(page, multipageSpec(COUNT, overrides));

    const p = paragraphLocator(page, MARKED);
    await expect(p).toBeAttached();

    // Structure the reader can see: the text, and the wrappers around it.
    // WordSpans exist only while mounted, so they are not part of the compare.
    const snapshot = () =>
      p.evaluate((el) => {
        const para = el.querySelector("p")!;
        return {
          text: para.textContent,
          tags: Array.from(para.querySelectorAll("*"))
            .filter(
              (e) =>
                !e.classList.contains("word-span") &&
                !e.classList.contains("translation-overlay"),
            )
            .map((e) => e.tagName.toLowerCase()),
        };
      });

    await scrollToParagraph(page, MARKED);
    await page.waitForTimeout(300);
    const mounted = await snapshot();

    await scrollToParagraph(page, 0);
    await page.waitForTimeout(600);
    await expect(p.locator(".word-span")).toHaveCount(0);
    const virtualized = await snapshot();

    expect(virtualized).toEqual(mounted);
  });
});
