import { writeFile } from "node:fs/promises";
import type { Page, TestInfo } from "@playwright/test";
import { expect, test } from "./helpers/test";
import { seedAndOpen, segmentsOfHtml } from "./helpers/paragraph";

async function openRepro(page: Page, segmented: boolean) {
  await seedAndOpen(page, {
    title: "Synthetic paragraph pagination",
    chapters: [
      {
        paragraphs: [22, 30, 2].map((count, index) => {
          const html = Array.from(
            { length: count },
            () =>
              `Synthetic paragraph ${index} describes a quiet garden beside the river.`,
          ).join(" ");
          return {
            html,
            ...(segmented ? { segments: segmentsOfHtml(html) } : {}),
          };
        }),
      },
    ],
  });
  await expect(page.locator(".paragraphs-container.is-ready")).toBeVisible();
  await expect(page.locator(".paragraph-wrapper")).toHaveCount(3);
  if (segmented) {
    await expect(
      page.locator('[data-paragraph-id="1"] .word-span').first(),
    ).toBeAttached();
  }
}

async function geometry(page: Page) {
  return page.locator(".paragraphs-container").evaluate((container) => {
    const c = container as HTMLElement;
    const cr = c.getBoundingClientRect();
    return {
      pageWidth: cr.width,
      pageHeight: cr.height,
      scrollLeft: c.scrollLeft,
      paragraphs: Array.from(
        c.querySelectorAll<HTMLElement>(".paragraph-wrapper"),
      ).map((el) => {
        const p = el.querySelector("p")!;
        const range = document.createRange();
        range.selectNodeContents(p);
        const rects = Array.from(range.getClientRects());
        const column = (rect: DOMRect) =>
          Math.floor((rect.left - cr.left + c.scrollLeft) / cr.width);
        const button = el
          .querySelector("button.translate")
          ?.getBoundingClientRect();
        return {
          firstColumn: column(rects[0]),
          firstLineTop: rects[0].top - cr.top,
          textColumns: [...new Set(rects.map(column))],
          height: el.getBoundingClientRect().height,
          lineHeight: parseFloat(getComputedStyle(p).lineHeight),
          marginBottom: parseFloat(getComputedStyle(el).marginBottom),
          lastLineVisible:
            rects.at(-1)!.left >= cr.left && rects.at(-1)!.right <= cr.right,
          button: button
            ? {
                column: column(button),
                top: button.top - cr.top,
                right: button.right - cr.left + c.scrollLeft,
              }
            : null,
          textLeft: rects[0].left - cr.left + c.scrollLeft,
        };
      }),
    };
  });
}

async function capture(page: Page, info: TestInfo, name: string) {
  const layout = await geometry(page);
  const measurements = info.outputPath(`${name}.json`);
  await writeFile(measurements, JSON.stringify(layout, null, 2));
  await info.attach(`${name}-geometry`, {
    path: measurements,
    contentType: "application/json",
  });
  const screenshot = info.outputPath(`${name}.png`);
  await page.screenshot({ path: screenshot });
  await info.attach(name, { path: screenshot, contentType: "image/png" });
  return layout;
}

for (const width of [960, 560]) {
  for (const segmented of [false, true]) {
    test.describe(`Paragraph fragmentation (${width}px, ${segmented ? "word segments" : "original text"})`, () => {
      test.use({ viewport: { width, height: 1100 }, colorScheme: "light" });

      test("the next paragraph fills page one and continues on page two", async ({
        page,
      }, info) => {
        await openRepro(page, segmented);
        const before = await capture(page, info, "page-one");
        const [first, next] = before.paragraphs;
        const remaining = before.pageHeight - first.height - first.marginBottom;
        expect(before.scrollLeft).toBe(0);
        expect(first.textColumns).toEqual([0]);
        expect(remaining).toBeGreaterThan(next.lineHeight * 2);
        expect(next.firstColumn).toBe(0);
        expect(next.firstLineTop).toBeGreaterThanOrEqual(
          first.height + first.marginBottom - 1,
        );
        expect(next.firstLineTop).toBeLessThan(
          first.height + first.marginBottom + next.lineHeight,
        );
        expect(next.textColumns).toEqual([0, 1]);

        if (!segmented) {
          expect(next.button).not.toBeNull();
          expect(next.button!.column).toBe(0);
          expect(Math.abs(next.button!.top - next.firstLineTop)).toBeLessThan(
            next.lineHeight / 2,
          );
          expect(next.button!.right).toBeLessThanOrEqual(next.textLeft);
        }

        await page.locator(".paragraphs-container").evaluate((el) => {
          el.scrollTo({ left: el.clientWidth, behavior: "instant" });
        });
        await expect
          .poll(async () => (await geometry(page)).scrollLeft)
          .toBeCloseTo(before.pageWidth, 0);
        const after = await capture(page, info, "page-two");
        expect(after.paragraphs[1].lastLineVisible).toBe(true);
        expect(after.paragraphs[1].textColumns).toEqual([0, 1]);
        await expect(page.locator('[data-paragraph-id="1"] p')).toHaveText(
          Array.from(
            { length: 30 },
            () =>
              "Synthetic paragraph 1 describes a quiet garden beside the river.",
          ).join(" "),
        );
      });
    });
  }
}
