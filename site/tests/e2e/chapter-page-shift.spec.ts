import { expect, test } from "./helpers/test";
import {
  htmlOfSize,
  multipageSpec,
  seedAndOpen,
  segmentsOfHtml,
  wordSegment,
  type ParagraphSegment,
  type SeedParagraph,
} from "./helpers/paragraph";

/**
 * Virtualization must not change a paragraph's height. In the multi-column
 * page flow, a height delta repacks every column after it. A page flip then
 * pulls the incoming page's first paragraph back onto the page the reader
 * just left, where the reader never sees it again.
 */
test.describe("Chapter paging vs virtualization (chromium only)", () => {
  test.skip(({ browserName }) => browserName !== "chromium", "chromium-only");

  const COUNT = 220;

  /** Deterministic prose with uneven word lengths, so lines fill irregularly. */
  function prose(idx: number, words: number): string {
    const bank = [
      "a",
      "the",
      "quick",
      "brown",
      "fox",
      "extraordinarily",
      "jumps",
      "over",
      "lazy",
      "dog",
      "and",
      "then",
      "some",
      "internationalization",
      "of",
      "words",
      "with",
      "varying",
      "length",
      "x",
      "yz",
      "abcdefghij",
    ];
    const out: string[] = [];
    let seed = (idx * 2654435761) % 4294967296;
    for (let i = 0; i < words; i++) {
      seed = (seed * 1103515245 + 12345) % 2147483648;
      out.push(bank[seed % bank.length]);
      if ((i + 1) % 11 === 0) out[out.length - 1] += ".";
    }
    out[out.length - 1] = out[out.length - 1].replace(/\.?$/, ".");
    return out.join(" ");
  }

  const marked = (
    text: string,
    flatIndex: number,
    word: number,
    marks?: ParagraphSegment["marks"],
  ): ParagraphSegment => ({
    ...wordSegment({ flatIndex, sentence: 0, word, text, translation: null }),
    ...(marks ? { marks } : {}),
  });

  /**
   * Mostly one-line paragraphs, so columns pack with almost no slack. A
   * single-line delta then crosses a column boundary instead of being
   * absorbed. The other cases cover markup, entities and an unbreakable word.
   */
  function corpusParagraph(idx: number): SeedParagraph {
    switch (idx % 20) {
      case 4: {
        const html = htmlOfSize(idx, 3);
        return { html, segments: segmentsOfHtml(html) };
      }
      case 9: {
        const html = prose(idx, 40);
        return { html, segments: segmentsOfHtml(html) };
      }
      case 11: {
        const html = "Wait— what?! Yes... no; maybe (perhaps), «truly»: fine.";
        return { html, segments: segmentsOfHtml(html) };
      }
      case 13: {
        const html = `Pneumonoultramicroscopicsilicovolcanoconiosis${idx} follows.`;
        return { html, segments: segmentsOfHtml(html) };
      }
      case 15:
        return {
          html: "<b>Bold</b> and <i>slanted</i> words here.",
          segments: [
            marked("Bold", 0, 0, ["strong"]),
            { kind: "gap", text: " and " },
            marked("slanted", 1, 1, ["emphasis"]),
            { kind: "gap", text: " " },
            marked("words", 2, 2),
            { kind: "gap", text: " " },
            marked("here", 3, 3),
            { kind: "gap", text: "." },
          ],
        };
      case 17:
        return {
          html: "Tom &amp; Jerry at caf&eacute; today.",
          segments: [
            marked("Tom", 0, 0),
            { kind: "gap", text: " & " },
            marked("Jerry", 1, 1),
            { kind: "gap", text: " at " },
            marked("café", 2, 2),
            { kind: "gap", text: " " },
            marked("today", 3, 3),
            { kind: "gap", text: "." },
          ],
        };
      case 19:
        return {
          html: "First line<br>second line here.",
          segments: [
            marked("First", 0, 0),
            { kind: "gap", text: " " },
            marked("line", 1, 1),
            { kind: "break" },
            marked("second", 2, 2),
            { kind: "gap", text: " " },
            marked("line", 3, 3),
            { kind: "gap", text: " " },
            marked("here", 4, 4),
            { kind: "gap", text: "." },
          ],
        };
      default: {
        const html = htmlOfSize(idx, 1);
        return { html, segments: segmentsOfHtml(html) };
      }
    }
  }

  function corpusSpec() {
    const overrides: Record<number, SeedParagraph> = {};
    for (let i = 0; i < COUNT; i++) overrides[i] = corpusParagraph(i);
    return multipageSpec(COUNT, overrides);
  }

  /**
   * One three-line paragraph per column, the rest one line, so the columns
   * hold almost no slack. This is the shape that reproduces the reported
   * page-flip symptom.
   */
  function tightlyPackedSpec() {
    const overrides: Record<number, SeedParagraph> = {};
    for (let i = 0; i < COUNT; i++) {
      const html = htmlOfSize(i, i % 10 === 4 ? 3 : 1);
      overrides[i] = { html, segments: segmentsOfHtml(html) };
    }
    return multipageSpec(COUNT, overrides);
  }

  async function open(
    page: import("@playwright/test").Page,
    spec: ReturnType<typeof corpusSpec>,
  ) {
    await seedAndOpen(page, spec);
    await page.waitForSelector(".paragraphs-container.is-ready");
    await page.waitForTimeout(800);
  }

  for (const width of [1280, 900, 560]) {
    test(`a paragraph's height is the same mounted and virtualized (${width}px)`, async ({
      page,
    }) => {
      test.setTimeout(120000);
      await page.setViewportSize({ width, height: 720 });
      await open(page, corpusSpec());

      // Anchor across the chapter so every paragraph is observed both inside
      // and outside the mount window, then compare the heights it reported.
      const conflicts = await page.evaluate(async (count: number) => {
        const c = document.querySelector(
          ".paragraphs-container",
        ) as HTMLElement;
        const seen = new Map<
          number,
          { mounted: Set<number>; virtual: Set<number> }
        >();

        const sample = () => {
          for (const el of Array.from(
            c.querySelectorAll(".paragraph-wrapper"),
          )) {
            const e = el as HTMLElement;
            const id = Number(e.dataset["paragraphId"]);
            const h = Math.round(e.getBoundingClientRect().height * 10) / 10;
            let entry = seen.get(id);
            if (!entry) {
              entry = { mounted: new Set(), virtual: new Set() };
              seen.set(id, entry);
            }
            (e.querySelector(".word-span") ? entry.mounted : entry.virtual).add(
              h,
            );
          }
        };

        sample();
        for (let target = 0; target < count; target += 15) {
          const el = c.querySelector(
            `.paragraph-wrapper[data-paragraph-id="${target}"]`,
          );
          el?.scrollIntoView({
            behavior: "auto",
            block: "nearest",
            inline: "center",
          });
          await new Promise((r) => setTimeout(r, 400));
          sample();
        }

        const out: Array<{
          id: number;
          mounted: number[];
          virtualized: number[];
        }> = [];
        for (const [id, entry] of seen) {
          if (entry.mounted.size === 0 || entry.virtual.size === 0) continue;
          const m = [...entry.mounted].sort((a, b) => a - b);
          const v = [...entry.virtual].sort((a, b) => a - b);
          if (m.length !== v.length || m.some((h, i) => h !== v[i])) {
            out.push({ id, mounted: m, virtualized: v });
          }
        }
        return out;
      }, COUNT);

      expect(conflicts).toEqual([]);
    });
  }

  // The translate button lives in grid column 1 only while a paragraph is
  // mounted and untranslated; virtualized, that cell is an empty div. If the
  // button is ever taller than the text beside it, mounting alone changes the
  // row height — a second, independent source of the same page shift.
  for (const width of [1280, 560]) {
    test(`the translate button does not change the row height (${width}px)`, async ({
      page,
    }) => {
      test.setTimeout(120000);
      await page.setViewportSize({ width, height: 720 });

      // No segments anywhere: every paragraph shows the button when mounted.
      const overrides: Record<number, SeedParagraph> = {};
      for (let i = 0; i < COUNT; i++) overrides[i] = { html: htmlOfSize(i, 1) };
      await open(page, multipageSpec(COUNT, overrides));

      const heights = await page.evaluate(async () => {
        const c = document.querySelector(
          ".paragraphs-container",
        ) as HTMLElement;
        const at = (id: number) => {
          const e = c.querySelector(
            `.paragraph-wrapper[data-paragraph-id="${id}"]`,
          ) as HTMLElement;
          return {
            height: Math.round(e.getBoundingClientRect().height * 10) / 10,
            hasButton: !!e.querySelector("button.translate"),
          };
        };
        const scrollTo = async (id: number) => {
          c
            .querySelector(`.paragraph-wrapper[data-paragraph-id="${id}"]`)
            ?.scrollIntoView({
              behavior: "auto",
              block: "nearest",
              inline: "center",
            });
          await new Promise((r) => setTimeout(r, 500));
        };
        await scrollTo(120);
        const mounted = at(120);
        await scrollTo(0);
        await new Promise((r) => setTimeout(r, 400));
        const virtualized = at(120);
        return { mounted, virtualized };
      });

      // The button must be present when mounted, or the test proves nothing.
      expect(heights.mounted.hasButton).toBe(true);
      expect(heights.virtualized.hasButton).toBe(false);
      expect(heights.virtualized.height).toBe(heights.mounted.height);
    });
  }

  test("flipping to the next page keeps that page's first paragraph on it", async ({
    page,
  }) => {
    test.setTimeout(120000);
    await open(page, tightlyPackedSpec());

    const shifts = await page.evaluate(async () => {
      const c = document.querySelector(".paragraphs-container") as HTMLElement;

      // Drives the browser's own snap animation, as a swipe or wheel flip does.
      const settle = () =>
        new Promise<void>((res) => {
          let last = -1;
          let still = 0;
          const tick = () => {
            if (c.scrollLeft === last) {
              if (++still > 4) return res();
            } else {
              still = 0;
              last = c.scrollLeft;
            }
            requestAnimationFrame(tick);
          };
          tick();
        });
      // Content-space x, independent of the current scroll.
      const contentLeft = (id: number): number | null => {
        const e = c.querySelector(
          `.paragraph-wrapper[data-paragraph-id="${id}"]`,
        );
        if (!e) return null;
        return Math.round(
          e.getBoundingClientRect().left -
            c.getBoundingClientRect().left +
            c.scrollLeft,
        );
      };
      const pageOf = (id: number) => {
        const l = contentLeft(id);
        return l === null ? null : Math.round(l / c.clientWidth);
      };
      const firstOnPage = (pageIndex: number): number | null => {
        for (const el of Array.from(c.querySelectorAll(".paragraph-wrapper"))) {
          const id = Number((el as HTMLElement).dataset["paragraphId"]);
          if (pageOf(id) === pageIndex) return id;
        }
        return null;
      };

      const out: Array<{
        flip: number;
        paragraphId: number;
        expectedPage: number;
        actualPage: number | null;
      }> = [];
      for (let flip = 0; flip < 12; flip++) {
        const nextPage = Math.round(c.scrollLeft / c.clientWidth) + 1;
        const incoming = firstOnPage(nextPage);
        c.scrollBy({ left: c.clientWidth, behavior: "smooth" });
        await settle();
        // The reflow lands with the mount-window recompute after the flip.
        await new Promise((r) => setTimeout(r, 700));
        if (incoming === null) continue;
        const landedPage = pageOf(incoming);
        if (landedPage !== nextPage) {
          out.push({
            flip,
            paragraphId: incoming,
            expectedPage: nextPage,
            actualPage: landedPage,
          });
        }
      }
      return out;
    });

    // A paragraph that moves back a page after the flip is never rendered on
    // any page the reader visits.
    expect(shifts).toEqual([]);
  });
});
