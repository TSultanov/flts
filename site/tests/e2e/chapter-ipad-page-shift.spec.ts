import { writeFile } from "node:fs/promises";
import { devices, type Page } from "@playwright/test";
import { expect, test } from "./helpers/test";
import { seedAndOpen } from "./helpers/paragraph";
import {
  pageFlipCases,
  pageFlipChapter,
  lastParagraphId as LAST,
} from "../fixtures/ipad-page-flip";

async function lastParagraphLayout(page: Page) {
  return page.locator(".paragraphs-container").evaluate((container, id) => {
    const c = container as HTMLElement;
    const cr = c.getBoundingClientRect();
    const p = c.querySelector(`[data-paragraph-id="${id}"] p`)!;
    const walker = document.createTreeWalker(p, NodeFilter.SHOW_TEXT);
    const rects: DOMRect[] = [];
    while (walker.nextNode()) {
      const node = walker.currentNode as Text;
      if (node.parentElement!.closest(".translation-overlay")) continue;
      for (const match of node.data.matchAll(/\S+/g)) {
        const range = document.createRange();
        range.setStart(node, match.index!);
        range.setEnd(node, match.index! + 1);
        rects.push(range.getBoundingClientRect());
      }
    }
    const column = (r: DOMRect) =>
      Math.floor((r.left - cr.left + c.scrollLeft + 0.01) / cr.width);
    return {
      firstPage: column(rects[0]),
      lastPage: column(rects.at(-1)!),
      firstLineTop: rects[0].top - cr.top,
      height: cr.height,
      pageWidth: cr.width,
      words: rects.length,
      precedingSplits: Array.from(
        c.querySelectorAll<HTMLElement>(".paragraph-wrapper"),
      )
        .slice(-11, -1)
        .filter((el) => el.getBoundingClientRect().width > cr.width * 1.5)
        .map((el) => Number(el.dataset.paragraphId)),
    };
  }, LAST);
}

// Source offsets identify the same glyphs in both renderings, even inside a
// hyphenated word. Translation overlays are outside the source text.
async function paragraphWrap(page: Page, id: number) {
  return page
    .locator(`.paragraph-wrapper[data-paragraph-id="${id}"] p`)
    .evaluate((p) => {
      const c = p.closest<HTMLElement>(".paragraphs-container")!;
      const left = c.getBoundingClientRect().left - c.scrollLeft;
      const walker = document.createTreeWalker(p, NodeFilter.SHOW_TEXT);
      const range = document.createRange();
      const starts: number[] = [];
      let offset = 0;
      let lastColumn = -1;
      let lastTop = -Infinity;
      while (walker.nextNode()) {
        const node = walker.currentNode as Text;
        if (node.parentElement!.closest(".translation-overlay")) continue;
        for (const match of node.data.matchAll(/\S/gu)) {
          range.setStart(node, match.index!);
          range.setEnd(node, match.index! + match[0].length);
          const rect = range.getBoundingClientRect();
          const column = Math.floor((rect.left - left) / c.clientWidth);
          if (column !== lastColumn || Math.abs(rect.top - lastTop) > 1) {
            starts.push(offset + match.index!);
            lastTop = rect.top;
            lastColumn = column;
          }
        }
        offset += node.length;
      }
      return { mounted: !!p.querySelector(".word-span"), starts };
    });
}

async function traceFlip(page: Page, targetPage: number) {
  return page.locator(".paragraphs-container").evaluate(
    async (container, { id, targetPage }) => {
      const c = container as HTMLElement;
      const p = c.querySelector(`[data-paragraph-id="${id}"] p`)!;
      const words = () => {
        const walker = document.createTreeWalker(p, NodeFilter.SHOW_TEXT);
        const out: Array<{ node: Text; offset: number; text: string }> = [];
        while (walker.nextNode()) {
          const node = walker.currentNode as Text;
          if (node.parentElement!.closest(".translation-overlay")) continue;
          for (const match of node.data.matchAll(/\S+/g))
            out.push({ node, offset: match.index!, text: match[0] });
        }
        return out;
      };
      const position = (word: ReturnType<typeof words>[number]) => {
        const range = document.createRange();
        range.setStart(word.node, word.offset);
        range.setEnd(word.node, word.offset + 1);
        const rect = range.getBoundingClientRect();
        const cr = c.getBoundingClientRect();
        return {
          x: rect.left - cr.left + c.scrollLeft,
          y: rect.top - cr.top,
          token: word.text,
          scrollLeft: c.scrollLeft,
          scrollTop: c.scrollTop,
          pageWidth: cr.width,
          pageHeight: cr.height,
        };
      };
      const mounted = () =>
        Array.from(c.querySelectorAll<HTMLElement>(".paragraph-wrapper"))
          .filter((el) => el.querySelector(".word-span"))
          .map((el) => Number(el.dataset["paragraphId"]));
      const incomingRows = () =>
        words()
          .map((word, i) => ({ i, pos: position(word) }))
          .filter(
            ({ pos }) =>
              Math.floor((pos.x + 0.01) / pos.pageWidth) === targetPage,
          )
          .sort((a, b) => a.pos.y - b.pos.y || a.pos.x - b.pos.x);
      const rows = () =>
        incomingRows()
          .filter((r, i, a) => !i || Math.abs(r.pos.y - a[i - 1].pos.y) > 1)
          .map((r) => r.i);
      const initialRows = rows();
      const incoming = incomingRows()[0];
      if (!incoming)
        throw new Error(`No final-paragraph text on page ${targetPage}`);

      const samples = [{ ...incoming.pos, mounted: mounted() }];
      const start = performance.now();
      let lastLeft = c.scrollLeft;
      let still = 0;
      c.scrollTo({ left: targetPage * c.clientWidth, behavior: "smooth" });
      do {
        await new Promise<void>((resolve) =>
          requestAnimationFrame(() => resolve()),
        );
        const sample = {
          ...position(words()[incoming.i]),
          mounted: mounted(),
        };
        samples.push(sample);
        still = sample.scrollLeft === lastLeft ? still + 1 : 0;
        lastLeft = sample.scrollLeft;
        if (still >= 8 && performance.now() - start > 700) break;
      } while (performance.now() - start < 5000);
      return {
        targetPage,
        wordIndex: incoming.i,
        initialRows,
        finalRows: rows(),
        samples,
      };
    },
    { id: LAST, targetPage },
  );
}

test.use({
  userAgent: devices["iPad Pro 11"].userAgent,
  isMobile: true,
  hasTouch: true,
  deviceScaleFactor: 2,
  viewport: { width: 820, height: 1180 },
  screen: { width: 820, height: 1180 },
  colorScheme: "light",
  trace: "on",
  video: "on",
});

test.describe("iPad final-paragraph page flips", () => {
  test.skip(
    ({ browserName }) => browserName !== "webkit",
    "Mobile WebKit investigation",
  );

  for (const { name, input } of pageFlipCases) {
    test(`incoming line stays on its page (${name})`, async ({
      page,
    }, info) => {
      test.setTimeout(90000);
      await seedAndOpen(
        page,
        {
          title: "Synthetic long chapter — iPad Air 5 portrait",
          chapters: [pageFlipChapter(input)],
          readingState: { chapterId: 0, paragraphId: LAST - 1, pageOffset: 0 },
        },
        { path: "/library" },
      );
      await page.locator('a[href^="/book/"]').first().click();
      await expect(
        page.locator(".paragraphs-container.is-ready"),
      ).toBeVisible();
      await expect(
        page
          .locator(`[data-paragraph-id="${LAST}"] .translation-overlay`)
          .first(),
      ).toBeAttached();
      const layout = await lastParagraphLayout(page);
      expect(layout.firstPage).toBeGreaterThan(50);
      expect(layout.lastPage).toBeGreaterThan(layout.firstPage);
      expect(layout.words).toBe(1200);
      expect(layout.firstLineTop).toBeGreaterThan(layout.height * 0.8);
      expect(layout.precedingSplits.length).toBeGreaterThanOrEqual(3);

      await page.locator(".paragraphs-container").evaluate((el, column) => {
        el.scrollTo({ left: column * el.clientWidth, behavior: "instant" });
      }, layout.firstPage);
      await page.waitForTimeout(700);
      await page.screenshot({ path: info.outputPath("before-flips.png") });
      const punctuationId = LAST - 4;
      const wrap =
        name === "mixed punctuation"
          ? await paragraphWrap(page, punctuationId)
          : undefined;
      const flip = await traceFlip(page, layout.firstPage + 1);
      const report = info.outputPath("line-trace.json");
      await writeFile(report, JSON.stringify({ layout, flip }, null, 2));
      await info.attach("Line positions and mount windows", {
        path: report,
        contentType: "application/json",
      });
      const screenshot = info.outputPath("after-flips.png");
      await page.screenshot({ path: screenshot });
      await info.attach("Final page", {
        path: screenshot,
        contentType: "image/png",
      });

      if (wrap) {
        expect(wrap.mounted).toBe(true);
        expect(wrap.starts.length).toBeGreaterThan(1);
        expect(await paragraphWrap(page, punctuationId)).toEqual({
          mounted: false,
          starts: wrap.starts,
        });
      }

      const before = flip.samples[0];
      const after = flip.samples.at(-1)!;
      expect(after.scrollLeft).toBeCloseTo(
        flip.targetPage * after.pageWidth,
        0,
      );
      expect(
        flip.samples.every(
          (sample) => sample.scrollTop === 0 && sample.token === before.token,
        ),
      ).toBe(true);
      expect(
        flip.samples.some(
          (sample) =>
            sample.x >= sample.scrollLeft &&
            sample.x < sample.scrollLeft + sample.pageWidth,
        ),
      ).toBe(true);
      expect(before.mounted.some((id) => !after.mounted.includes(id))).toBe(
        true,
      );
      if (flip.finalRows[0] !== flip.initialRows[0]) {
        expect(flip.finalRows[0]).toBe(flip.initialRows[1]);
        expect(after.x).toBeCloseTo(before.x - before.pageWidth, 0);
      }
      expect(
        flip.finalRows[0],
        "The first line seen during the flip must still be first after snap",
      ).toBe(flip.initialRows[0]);
    });
  }
});
