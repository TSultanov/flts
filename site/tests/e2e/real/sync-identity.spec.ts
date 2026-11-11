import { test, expect } from "../../real/fixtures";
import { lemmaSets, seedBook } from "../../real/spec-helpers";

type ThisDevice = { deviceId: string } | null;
type SyncStatus = { state: string; storeIssues?: { sharedIdentity?: boolean } };

/**
 * The one spec that runs the embedded sync engine: an installation that
 * shares its identity with another takes one of its own, from the settings
 * page down to the engine coming back as a new device.
 */
test("a copy of an installation takes a new identity from the settings page", async ({
  harness,
  page,
}) => {
  test.setTimeout(120_000);
  const bookId = await seedBook(harness, lemmaSets(1), "identity-kept");
  // Loopback only, no discovery: nothing leaves the machine.
  await harness.restartApp({
    env: { FLTS_DISABLE_SYNC: "", FLTS_SYNC_HERMETIC: "1" },
  });
  try {
    await harness.invoke("sync_set_enabled", { enabled: true });
    const device = async () =>
      (await harness.invoke<ThisDevice>("sync_get_this_device"))?.deviceId;
    await expect.poll(device, { timeout: 30_000 }).toBeTruthy();
    const before = (await device())!;

    // Pairing a device with this one's own id: the two are copies.
    await expect(
      harness.invoke("sync_add_device", { deviceId: before, name: "copy" }),
    ).rejects.toThrow(/same sync identity/);
    await expect
      .poll(
        async () =>
          (await harness.invoke<SyncStatus>("get_sync_status")).storeIssues
            ?.sharedIdentity,
        { timeout: 30_000 },
      )
      .toBe(true);

    await page.goto("/config");
    await page.locator("details.sync-section > summary").click();
    await expect(
      page.locator('[data-testid="sync-shared-identity"]'),
    ).toBeVisible({ timeout: 15_000 });
    await page.locator('[data-testid="sync-reset-identity"]').click();
    await page.locator('[data-testid="confirm-dialog-confirm"]').click();

    await expect
      .poll(async () => (await device()) ?? before, { timeout: 60_000 })
      .not.toBe(before);
    await expect(
      page.locator('[data-testid="sync-shared-identity"]'),
    ).toHaveCount(0);
    await expect
      .poll(
        async () =>
          (await harness.invoke<SyncStatus>("get_sync_status")).storeIssues
            ?.sharedIdentity ?? false,
        { timeout: 30_000 },
      )
      .toBe(false);
    const books = await harness.invoke<{ id: string }[]>("list_books");
    expect(books.map((b) => b.id)).toContain(bookId);
  } finally {
    await harness
      .invoke("sync_set_enabled", { enabled: false })
      .catch(() => {});
    await harness.restartApp();
  }
});
