import { test, expect } from '@playwright/test';

test.describe('FLTS Application', () => {
  test('app loads successfully', async ({ page }) => {
    await page.goto('/');
    
    // Should redirect to /library and show the Library page
    await expect(page).toHaveURL('/library');
    
    // Should show the main navigation
    const nav = page.locator('nav');
    await expect(nav).toBeVisible();
    
    // Should have navigation links
    await expect(page.locator('nav a[href="/library"]')).toBeVisible();
    await expect(page.locator('nav a[href="/import"]')).toBeVisible();
    await expect(page.locator('nav a[href="/config"]')).toBeVisible();
    
    // Should show the Books header on library page
    await expect(page.locator('h1')).toContainText('Books');
    
    // Should show the main content area
    const main = page.locator('.main');
    await expect(main).toBeVisible();
  });

  test('navigation links work correctly', async ({ page }) => {
    await page.goto('/library');
    
    // Test Import navigation
    await page.click('nav a[href="/import"]');
    await expect(page).toHaveURL('/import');
    
    // Test Config navigation  
    await page.click('nav a[href="/config"]');
    await expect(page).toHaveURL('/config');
    
    // Test Library navigation
    await page.click('nav a[href="/library"]');
    await expect(page).toHaveURL('/library');
    await expect(page.locator('h1')).toContainText('Books');
  });

  test('library page shows book management interface', async ({ page }) => {
    await page.goto('/library');
    
    // Should show the Books heading
    await expect(page.locator('.books h1')).toContainText('Books');
    
    // Should show selection controls
    await expect(page.locator('.select-actions')).toBeVisible();
    await expect(page.locator('.select-actions button')).toContainText('Select All');
    
    // Should show folders container
    await expect(page.locator('.folders-container')).toBeVisible();
  });
});
