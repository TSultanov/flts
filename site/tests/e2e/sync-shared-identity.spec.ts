import { expect, test } from "./helpers/test";
import { isRealMode } from "./helpers/backend-mode";

// An installation restored from another device's backup.

const NOTICE = '[data-testid="sync-shared-identity"]';

const setStatus = ([state, sharedIdentity]: [string, boolean]) =>
  (window as any).__test.setSyncStatus({
    state,
    deviceCount: 0,
    connectedCount: 0,
    storeIssues: {
      newerVersion: 0,
      unreadable: 0,
      strangers: 0,
      sharedIdentity,
    },
  });

test.describe("Shared sync identity", () => {
  test.skip(isRealMode(), "needs a restored backup; mock tier only");

  test("keeping the identity hides the notice", async ({ page }) => {
    await page.goto("/config");
    await page.evaluate(setStatus, ["online", true]);
    await page.locator("details.sync-section > summary").click();

    const notice = page.locator(NOTICE);
    await expect(notice).toContainText("shares its sync identity");
    await expect(
      page.locator('[data-testid="sync-reset-identity"]'),
    ).toBeVisible();
    await expect(page.locator('[data-testid="sync-store-issue"]')).toHaveCount(
      0,
    );

    await page.locator('[data-testid="sync-keep-identity"]').click();
    await expect(notice).toHaveCount(0);

    // A status that still carries the flag must not bring it back.
    const label = page.locator(".sync .status-line").first();
    await page.evaluate(setStatus, ["syncing", true]);
    await expect(label).toContainText("Syncing");
    await expect(notice).toHaveCount(0);

    await page.evaluate(setStatus, ["starting", false]);
    await expect(label).toContainText("Starting");
    await page.evaluate(setStatus, ["online", true]);
    await expect(notice).toBeVisible();
  });

  const open = async (page: import("@playwright/test").Page) => {
    await page.goto("/config");
    await page.evaluate(setStatus, ["online", true]);
    await page.locator("details.sync-section > summary").click();
    await expect(page.locator(NOTICE)).toBeVisible();
  };
  const reset = '[data-testid="sync-reset-identity"]';
  const keep = '[data-testid="sync-keep-identity"]';
  const resets = (page: import("@playwright/test").Page) =>
    page.evaluate(() => (window as any).__test.identityResets());

  test("a new identity is asked for twice before it is taken", async ({
    page,
  }) => {
    await open(page);

    await page.locator(reset).click();
    const dialog = page.locator('[data-testid="confirm-dialog"]');
    await expect(dialog).toContainText("New sync identity");
    await expect(
      page.locator('[data-testid="confirm-dialog-message"]'),
    ).toContainText("paired with your other devices again");
    await page.locator('[data-testid="confirm-dialog-cancel"]').click();
    await expect(dialog).toBeHidden();
    expect(await resets(page)).toBe(0);
    await expect(page.locator(NOTICE)).toBeVisible();

    // Held open: the buttons say so and take no second press meanwhile.
    await page.evaluate(() => (window as any).__test.holdIdentityReset());
    await page.locator(reset).click();
    await page.locator('[data-testid="confirm-dialog-confirm"]').click();
    await expect(page.locator(reset)).toContainText(
      "Giving this device a new identity…",
    );
    await expect(page.locator(reset)).toBeDisabled();
    await expect(page.locator(keep)).toBeDisabled();
    await page.evaluate(() => (window as any).__test.releaseIdentityReset());
    await expect(page.locator(NOTICE)).toHaveCount(0);
    expect(await resets(page)).toBe(1);
  });

  test("a reset that fails says why and leaves the notice", async ({
    page,
  }) => {
    await open(page);
    await page.evaluate(() =>
      (window as any).__test.holdIdentityReset("the sync engine did not stop"),
    );
    await page.locator(reset).click();
    await page.locator('[data-testid="confirm-dialog-confirm"]').click();
    await page.evaluate(() => (window as any).__test.releaseIdentityReset());

    await expect(page.locator(".sync p.err").last()).toContainText(
      "the sync engine did not stop",
    );
    // The view polls every four seconds; what the user was told stays.
    await page.waitForTimeout(5000);
    await expect(page.locator(".sync p.err").last()).toContainText(
      "the sync engine did not stop",
    );
    await expect(page.locator(NOTICE)).toBeVisible();
    await expect(page.locator(reset)).toBeEnabled();
    await expect(page.locator(reset)).toContainText(
      "Give this device a new identity",
    );
  });
});
