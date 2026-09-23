import { expect, test } from "./helpers/test";

test("config page scrolls on a small screen with every section open", async ({
  page,
}) => {
  await page.setViewportSize({ width: 375, height: 480 });
  await page.goto("/config");

  const form = page.locator(".config-form");
  const scroller = form.locator("..");
  await page.locator("#translationConcurrency").fill("3");
  for (const summary of await form.locator("details > summary").all()) {
    await summary.click();
  }

  const overflows = await scroller.evaluate((el) => {
    el.scrollTop = 0;
    return el.scrollHeight > el.clientHeight;
  });
  expect(overflows).toBe(true);

  // Centered flex overflow used to push the top of the form out of reach.
  const scrollerTop = (await scroller.boundingBox())!.y;
  const firstControlTop = (await page.locator("#targetlanguage").boundingBox())!
    .y;
  expect(firstControlTop).toBeGreaterThanOrEqual(scrollerTop);

  await page.locator("#save").click();
  const persisted = await page.evaluate(
    () =>
      (window as any).__test.getConfig() as {
        translationConcurrency?: number;
      },
  );
  expect(persisted.translationConcurrency).toBe(3);

  const pageWidth = await page.evaluate(
    () => document.documentElement.scrollWidth,
  );
  expect(pageWidth).toBeLessThanOrEqual(375);
});
