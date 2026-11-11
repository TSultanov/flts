import { expect, test } from "./helpers/test";
import { isRealMode } from "./helpers/backend-mode";
import { fillerHtml, seedAndOpen } from "./helpers/paragraph";

// A deletion synced in from another device while the book is open.

const NOTICE = '[data-testid="book-deleted-notice"]';

const spec = (title: string) => ({
  title,
  chapters: [{ paragraphs: [{ html: fillerHtml(0) }] }],
});

const deleteRemotely = (bookId: string) =>
  (window as any).__test.deleteBookRemotely(bookId);

test.describe("Book deleted on another device", () => {
  test.skip(isRealMode(), "needs a second device; mock tier only");

  test("the open book closes to the library with a notice", async ({
    page,
  }) => {
    const { bookId } = await seedAndOpen(page, spec("Doomed"));
    await page.waitForSelector(".paragraphs-container");

    await page.evaluate(deleteRemotely, bookId);

    await expect(page).toHaveURL(/\/library$/);
    await expect(page.locator(NOTICE)).toBeVisible();
    await expect(page.locator(`a[href="/book/${bookId}"]`)).toHaveCount(0);

    await page.locator(`${NOTICE} button`).click();
    await expect(page.locator(NOTICE)).toHaveCount(0);
  });

  test("another book's deletion leaves the open book alone", async ({
    page,
  }) => {
    await seedAndOpen(page, spec("Kept"));
    await page.waitForSelector(".paragraphs-container");
    const other = await page.evaluate(() =>
      (window as any).__test.seedBook({
        title: "Other",
        chapters: [{ paragraphs: [{ html: "<p>x</p>" }] }],
      }),
    );

    await page.evaluate(deleteRemotely, other);

    await expect(page).toHaveURL(/\/book\//);
    await expect(page.locator(".paragraphs-container")).toBeVisible();
  });
});
