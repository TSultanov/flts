import { expect, test } from "./helpers/test";
import { isRealMode } from "./helpers/backend-mode";

// The first start after an upgrade: the book list waits for the import.

const STATUS = '[data-testid="import-status"]';

const emitProgress = ([done, total]: number[]) =>
  (window as any).__test.emitImportProgress(done, total);

test.describe("Library import progress", () => {
  test.skip(isRealMode(), "needs an old-format library; mock tier only");

  test("shows the import's progress until the list loads", async ({ page }) => {
    await page.addInitScript(() => {
      (window as any).__importPending = true;
    });
    await page.goto("/library");
    await expect(page.locator("h1")).toHaveText("Books");
    await expect(page.locator(STATUS)).toHaveCount(0);

    await page.evaluate(emitProgress, [3, 12]);

    await expect(page.locator(STATUS)).toContainText(
      "Updating your library… 3 of 12",
    );
    await expect(page.locator(`${STATUS} progress`)).toHaveJSProperty(
      "value",
      3,
    );
    await expect(page.locator(`${STATUS} progress`)).toHaveJSProperty(
      "max",
      12,
    );

    await page.evaluate(emitProgress, [12, 12]);

    await expect(page.locator(STATUS)).toHaveCount(0);
  });

  test("shows nothing once the list has loaded", async ({ page }) => {
    await page.goto("/library");
    await expect(page.locator("h1")).toHaveText("Books");

    await page.evaluate(emitProgress, [3, 12]);

    await expect(page.locator(STATUS)).toHaveCount(0);
  });
});
