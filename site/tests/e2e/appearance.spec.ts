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
});

test.describe("appearance (dark)", () => {
  test.use({ colorScheme: "dark" });

  test("document is a dark page with light text", async ({ page }) => {
    await page.goto("/library");
    expect(rgbLuminance(await bg(page, "body"))).toBeLessThan(0.15);
    expect(rgbLuminance(await fg(page, "body"))).toBeGreaterThan(0.7);
  });
});
