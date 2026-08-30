import { type Page } from "@playwright/test";
import { expect, test } from "./helpers/test";

// Anki sync UI surface: nav button and the Config endpoint/api-key fields.

type AnkiSyncStatusState = "idle" | "syncing" | "ok" | "err" | "unreachable";

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

test.describe("Anki sync button", () => {
  test("hidden when AnkiConnect status is unreachable", async ({ page }) => {
    await page.addInitScript(() => {
      // Must precede boot: the Resource fetches once on construction.
      (window as any).__pendingAnkiStatus = { state: "unreachable" };
    });
    await page.goto("/library");
    await setAnkiStatus(page, { state: "unreachable" });
    await expect(page.getByTestId("anki-sync-button")).toBeHidden();
  });

  test("visible when status is idle and clicking triggers sync_anki_now", async ({
    page,
  }) => {
    await page.goto("/library");
    await setAnkiStatus(page, { state: "idle" });

    const button = page.getByTestId("anki-sync-button");
    await expect(button).toBeVisible();

    await button.click();
    await expect
      .poll(async () => await getSyncAnkiNowCallCount(page))
      .toBeGreaterThan(0);

    // The mock flips syncing → ok on a timer; status_changed drives the refetch.
    await expect
      .poll(
        async () =>
          await page.evaluate(
            () => (window as any).__test.getAnkiSyncStatus().state,
          ),
      )
      .toBe("ok");
  });

  test("hides itself if status transitions to unreachable mid-session", async ({
    page,
  }) => {
    await page.goto("/library");
    await setAnkiStatus(page, { state: "idle" });
    await expect(page.getByTestId("anki-sync-button")).toBeVisible();

    await setAnkiStatus(page, {
      state: "unreachable",
      lastError: "connection refused",
    });
    await expect(page.getByTestId("anki-sync-button")).toBeHidden();
  });
});

test.describe("Anki config UI", () => {
  test("endpoint and api key fields are persisted via update_config", async ({
    page,
  }) => {
    await page.goto("/config");

    const summary = page.getByText("Anki (optional)");
    await summary.click();

    const endpoint = page.getByTestId("anki-endpoint");
    const apiKey = page.getByTestId("anki-api-key");
    await endpoint.fill("http://anki.example.com:9999");
    await apiKey.fill("secret-token");
    await page.locator("#save").click();

    // Read back through the mock to confirm update_config persisted.
    const persisted = await page.evaluate(
      () =>
        (window as any).__test.getConfig() as {
          ankiEndpoint?: string;
          ankiApiKey?: string;
        },
    );
    expect(persisted.ankiEndpoint).toBe("http://anki.example.com:9999");
    expect(persisted.ankiApiKey).toBe("secret-token");
  });

  test("sync is ticked when the config omits ankiSyncEnabled", async ({
    page,
  }) => {
    await page.goto("/config");
    await page.getByText("Anki (optional)").click();

    await expect(page.getByTestId("anki-sync-enabled")).toBeChecked();
  });

  test("unticking sync persists ankiSyncEnabled false", async ({ page }) => {
    await page.goto("/config");
    await page.getByText("Anki (optional)").click();

    await page.getByTestId("anki-sync-enabled").uncheck();
    await page.locator("#save").click();

    const persisted = await page.evaluate(
      () => (window as any).__test.getConfig() as { ankiSyncEnabled?: boolean },
    );
    expect(persisted.ankiSyncEnabled).toBe(false);
  });
});
