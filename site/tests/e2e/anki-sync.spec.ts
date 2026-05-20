import { expect, test, type Page } from '@playwright/test';

// Public-behavior tests for the Anki sync UI surface. Exercises both the
// nav button (visibility + click + status update) and the Config UI
// (endpoint + api key fields persisted through update_config).

type AnkiSyncStatusState = 'idle' | 'syncing' | 'ok' | 'err' | 'unreachable';

async function setAnkiStatus(
  page: Page,
  status: { state: AnkiSyncStatusState; lastError?: string | null },
): Promise<void> {
  await page.evaluate((s) => {
    (window as any).__test.setAnkiSyncStatus(s);
  }, status);
}

async function getSyncAnkiNowCallCount(page: Page): Promise<number> {
  return await page.evaluate(
    () => ((window as any).__test.getSyncAnkiNowCalls() as unknown[]).length,
  );
}

test.describe('Anki sync button', () => {
  test('hidden when AnkiConnect status is unreachable', async ({ page }) => {
    await page.addInitScript(() => {
      // Seed before app boot so the very first get_anki_sync_status returns
      // Unreachable. The Resource fetches once on construction.
      (window as any).__pendingAnkiStatus = { state: 'unreachable' };
    });
    await page.goto('/library');
    // Seed the status via the test surface once the mock is wired.
    await setAnkiStatus(page, { state: 'unreachable' });
    await expect(page.getByTestId('anki-sync-button')).toBeHidden();
  });

  test('visible when status is idle and clicking triggers sync_anki_now', async ({
    page,
  }) => {
    await page.goto('/library');
    await setAnkiStatus(page, { state: 'idle' });

    const button = page.getByTestId('anki-sync-button');
    await expect(button).toBeVisible();

    await button.click();
    await expect
      .poll(async () => await getSyncAnkiNowCallCount(page))
      .toBeGreaterThan(0);

    // Mock flips status syncing → ok with a short setTimeout; once the
    // status_changed event lands the Resource refetches and the button
    // reflects the new state.
    await expect
      .poll(async () => await page.evaluate(
        () => ((window as any).__test.getAnkiSyncStatus()).state,
      ))
      .toBe('ok');
  });

  test('hides itself if status transitions to unreachable mid-session', async ({
    page,
  }) => {
    await page.goto('/library');
    await setAnkiStatus(page, { state: 'idle' });
    await expect(page.getByTestId('anki-sync-button')).toBeVisible();

    await setAnkiStatus(page, {
      state: 'unreachable',
      lastError: 'connection refused',
    });
    await expect(page.getByTestId('anki-sync-button')).toBeHidden();
  });
});

test.describe('Anki config UI', () => {
  test('endpoint and api key fields are persisted via update_config', async ({
    page,
  }) => {
    await page.goto('/config');

    // Expand the Anki <details> section to reveal the inputs.
    const summary = page.getByText('Anki (optional)');
    await summary.click();

    const endpoint = page.getByTestId('anki-endpoint');
    const apiKey = page.getByTestId('anki-api-key');
    await endpoint.fill('http://anki.example.com:9999');
    await apiKey.fill('secret-token');
    await page.locator('#save').click();

    // Read back via the mock's __test surface to verify update_config
    // persisted the new values.
    const persisted = await page.evaluate(() =>
      (window as any).__test.getConfig() as {
        ankiEndpoint?: string;
        ankiApiKey?: string;
      },
    );
    expect(persisted.ankiEndpoint).toBe('http://anki.example.com:9999');
    expect(persisted.ankiApiKey).toBe('secret-token');
  });
});
