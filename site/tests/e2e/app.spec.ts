import { test, expect } from './helpers/test';

test.describe('FLTS Application', () => {
  test('app loads successfully', async ({ page }) => {
    await page.goto('/');
    await expect(page).toHaveURL('/library');
    
    const nav = page.locator('nav');
    await expect(nav).toBeVisible();
    await expect(page.locator('nav a[href="/library"]')).toBeVisible();
    await expect(page.locator('nav a[href="/import"]')).toBeVisible();
    await expect(page.locator('nav a[href="/config"]')).toBeVisible();
    await expect(page.locator('h1')).toContainText('Books');
    
    const main = page.locator('.main');
    await expect(main).toBeVisible();
  });

  test('navigation links work correctly', async ({ page }) => {
    await page.goto('/library');
    await page.click('nav a[href="/import"]');
    await expect(page).toHaveURL('/import');
    await page.click('nav a[href="/config"]');
    await expect(page).toHaveURL('/config');
    await page.click('nav a[href="/library"]');
    await expect(page).toHaveURL('/library');
    await expect(page.locator('h1')).toContainText('Books');
  });

  test('library page shows book management interface', async ({ page }) => {
    await page.goto('/library');
    await expect(page.locator('.books h1')).toContainText('Books');
    await expect(page.locator('.select-actions')).toBeVisible();
    await expect(page.locator('.select-actions button')).toContainText('Select All');
    await expect(page.locator('.folders-container')).toBeVisible();
  });
});
