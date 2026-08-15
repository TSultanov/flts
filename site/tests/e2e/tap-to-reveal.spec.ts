import { expect, test } from '@playwright/test';

test.describe('Tap to reveal translations — config', () => {
  test('checkbox defaults off and persists via update_config', async ({ page }) => {
    await page.goto('/config');

    const checkbox = page.getByTestId('tap-to-reveal');
    await expect(checkbox).toBeVisible();
    await expect(checkbox).not.toBeChecked();

    await checkbox.check();
    await page.locator('#save').click();

    const persisted = await page.evaluate(
      () =>
        (window as any).__test.getConfig() as {
          tapToRevealTranslations?: boolean;
        },
    );
    expect(persisted.tapToRevealTranslations).toBe(true);
  });
});
