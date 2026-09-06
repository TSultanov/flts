import { expect, test } from "./helpers/test";

function rgbLuminance(rgb: string): number {
  const m = rgb.match(/rgba?\((\d+),\s*(\d+),\s*(\d+)/);
  if (!m) throw new Error(`not rgb: ${rgb}`);
  const r = Number(m[1]) / 255;
  const g = Number(m[2]) / 255;
  const b = Number(m[3]) / 255;
  const lin = (c: number) =>
    c <= 0.04045 ? c / 12.92 : ((c + 0.055) / 1.055) ** 2.4;
  return 0.2126 * lin(r) + 0.7152 * lin(g) + 0.0722 * lin(b);
}

async function bg(page: import("@playwright/test").Page, selector: string) {
  return page
    .locator(selector)
    .evaluate((el) => getComputedStyle(el).backgroundColor);
}

async function fg(page: import("@playwright/test").Page, selector: string) {
  return page.locator(selector).evaluate((el) => getComputedStyle(el).color);
}

async function seedAndOpenChapter(page: import("@playwright/test").Page) {
  await page.goto("/library");
  await page.evaluate(() => {
    (window as any).__test.seedBook({
      title: "Theme Book",
      chapters: [{ paragraphs: [{ html: "<p>hello</p>" }] }],
    });
  });
  await page.locator('a[href^="/book/"]').first().click();
  await expect(page.locator(".chapter")).toBeVisible();
}

test.describe("appearance (light)", () => {
  test.use({ colorScheme: "light" });

  test("document is a light page with dark text", async ({ page }) => {
    await page.goto("/library");
    expect(rgbLuminance(await bg(page, "body"))).toBeGreaterThan(0.7);
    expect(rgbLuminance(await fg(page, "body"))).toBeLessThan(0.3);
    await expect(page.locator("html")).toHaveCSS(
      "color-scheme",
      /light\s+dark|dark\s+light/,
    );
  });

  test("chapter paper is light", async ({ page }) => {
    await seedAndOpenChapter(page);
    expect(rgbLuminance(await bg(page, ".chapter"))).toBeGreaterThan(0.7);
  });

  test("confirm dialog is a light surface", async ({ page }) => {
    await page.goto("/library");
    await page.evaluate(() => {
      (window as any).__test.seedBook({
        title: "Theme Book",
        chapters: [{ paragraphs: [{ html: "<p>hello</p>" }] }],
      });
    });
    await page.getByTestId("select-all-button").click();
    await page.getByTestId("delete-selected-button").click();
    const dialog = page.getByTestId("confirm-dialog");
    await expect(dialog).toBeVisible();
    expect(
      rgbLuminance(await bg(page, "[data-testid=confirm-dialog]")),
    ).toBeGreaterThan(0.7);
  });
});

test.describe("appearance (dark)", () => {
  test.use({ colorScheme: "dark" });

  test("document is a dark page with light text", async ({ page }) => {
    await page.goto("/library");
    expect(rgbLuminance(await bg(page, "body"))).toBeLessThan(0.15);
    expect(rgbLuminance(await fg(page, "body"))).toBeGreaterThan(0.7);
  });

  test("chapter paper is dark", async ({ page }) => {
    await seedAndOpenChapter(page);
    expect(rgbLuminance(await bg(page, ".chapter"))).toBeLessThan(0.2);
    expect(rgbLuminance(await fg(page, ".chapter"))).toBeGreaterThan(0.7);
  });

  test("confirm dialog is a dark surface", async ({ page }) => {
    await page.goto("/library");
    await page.evaluate(() => {
      (window as any).__test.seedBook({
        title: "Theme Book",
        chapters: [{ paragraphs: [{ html: "<p>hello</p>" }] }],
      });
    });
    await page.getByTestId("select-all-button").click();
    await page.getByTestId("delete-selected-button").click();
    const dialog = page.getByTestId("confirm-dialog");
    await expect(dialog).toBeVisible();
    expect(
      rgbLuminance(await bg(page, "[data-testid=confirm-dialog]")),
    ).toBeLessThan(0.2);
    expect(
      rgbLuminance(await fg(page, "[data-testid=confirm-dialog] h3")),
    ).toBeGreaterThan(0.7);
  });
});
