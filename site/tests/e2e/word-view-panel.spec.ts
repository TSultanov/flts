import { expect, test } from "./helpers/test";
import { isRealMode } from "./helpers/backend-mode";
import {
  paragraphLocator,
  seedAndOpen,
  wordSegment,
  wordSpan,
} from "./helpers/paragraph";

// WordView bottom overlay: peek/expand/collapse (button and `w`), overlaying
// rather than resizing the book viewport, with a drag size that survives reload.

test.describe.configure({ mode: "parallel" });

const PANEL = '[data-testid="word-view"]';
const PEEK = '[data-testid="word-view-peek"]';
const EXPAND = '[data-testid="word-view-expand"]';
const COLLAPSE = '[data-testid="word-view-collapse"]';
const EXPANDED = '[data-testid="word-view-expanded"]';

async function seedClickableBook(page: import("@playwright/test").Page) {
  const segments = [
    wordSegment({
      flatIndex: 0,
      sentence: 0,
      word: 0,
      text: "hola",
      translation: "hello",
    }),
  ];
  // In the seed, so the init script re-applies it on every navigation.
  const { bookId } = await seedAndOpen(page, {
    chapters: [{ paragraphs: [{ html: "hola", segments }] }],
    wordInfos: [
      {
        paragraphId: 0,
        sentenceId: 0,
        wordId: 0,
        info: {
          original: "hello",
          contextualTranslations: ["hola", "hi"],
          fullSentenceTranslation: "hola",
          note: "a greeting",
        },
      },
    ],
  });
  return { bookId };
}

async function seedCardBook(
  page: import("@playwright/test").Page,
  opts: { canCreateCard?: boolean; hasCard?: boolean } = {},
) {
  const segments = [
    wordSegment({
      flatIndex: 0,
      sentence: 0,
      word: 0,
      text: "hola",
      translation: "hello",
    }),
  ];
  const { bookId } = await seedAndOpen(page, {
    chapters: [{ paragraphs: [{ html: "hola", segments }] }],
    wordInfos: [
      {
        paragraphId: 0,
        sentenceId: 0,
        wordId: 0,
        info: {
          original: "hello",
          contextualTranslations: ["hola"],
          fullSentenceTranslation: "hola",
          note: "",
          canCreateCard: opts.canCreateCard ?? true,
          hasCard: opts.hasCard ?? false,
          grammar: {
            originalInitialForm: "hablar",
            targetInitialForm: "говорить",
            partOfSpeech: "verb",
          },
        },
      },
    ],
  });
  return { bookId };
}

test.describe("WordView panel — hint state", () => {
  test.skip(
    ({ browserName }) => browserName === "firefox",
    "chromium + webkit only",
  );

  test("peek shows hint text when no word is selected", async ({ page }) => {
    await seedClickableBook(page);
    await page.waitForSelector(".paragraphs-container.is-ready");

    await expect(page.locator(PEEK)).toBeVisible();
    await expect(page.locator(PEEK)).toContainText("Select a word");
    await expect(page.locator(EXPAND)).toHaveCount(0);
  });
});

test.describe("WordView panel — selection populates peek", () => {
  test.skip(
    ({ browserName }) => browserName === "firefox",
    "chromium + webkit only",
  );

  test("clicking a word fills the peek bar", async ({ page }) => {
    await seedClickableBook(page);
    const p = paragraphLocator(page, 0);
    await wordSpan(p, 0).click();

    const peek = page.locator(PEEK);
    await expect(peek.locator(".peek-word")).toHaveText("hello");
    await expect(peek.locator(".peek-translations")).toHaveText("hola, hi");
    await expect(page.locator(EXPAND)).toBeVisible();
    await expect(page.locator(EXPANDED)).toHaveCount(0);
  });
});

test.describe("WordView panel — expand and collapse", () => {
  test.skip(
    ({ browserName }) => browserName === "firefox",
    "chromium + webkit only",
  );

  test("expand button opens the full form", async ({ page }) => {
    await seedClickableBook(page);
    const p = paragraphLocator(page, 0);
    await wordSpan(p, 0).click();

    await page.locator(EXPAND).click();
    await expect(page.locator(EXPANDED)).toBeVisible();
    await expect(page.locator(EXPANDED).locator(".word-original")).toHaveText(
      "hello",
    );
    await expect(page.locator(EXPANDED)).toContainText("a greeting");
    await expect(page.locator(PEEK)).toHaveCount(0);

    await page.locator(COLLAPSE).click();
    await expect(page.locator(EXPANDED)).toHaveCount(0);
    await expect(page.locator(PEEK)).toBeVisible();
  });

  test("deselecting the word collapses the expanded panel", async ({
    page,
  }) => {
    await seedClickableBook(page);
    await page.waitForSelector(".paragraphs-container.is-ready");
    const p = paragraphLocator(page, 0);
    await wordSpan(p, 0).click();

    await page.locator(EXPAND).click();
    await expect(page.locator(EXPANDED)).toBeVisible();

    await page.locator(".chapter").click({ position: { x: 5, y: 5 } });

    await expect(page.locator(EXPANDED)).toHaveCount(0);
    await expect(page.locator(PEEK)).toBeVisible();
    await expect(page.locator(PEEK)).toContainText("Select a word");
  });

  test("`w` shortcut toggles expand", async ({ page }) => {
    await seedClickableBook(page);
    const p = paragraphLocator(page, 0);
    await wordSpan(p, 0).click();

    await page.keyboard.press("w");
    await expect(page.locator(EXPANDED)).toBeVisible();
    await page.keyboard.press("w");
    await expect(page.locator(EXPANDED)).toHaveCount(0);
  });
});

test.describe("WordView panel — overlay does not resize book viewport", () => {
  test.skip(
    ({ browserName }) => browserName === "firefox",
    "chromium + webkit only",
  );

  test("expanding the panel leaves the chapter container width unchanged", async ({
    page,
  }) => {
    await seedClickableBook(page);
    await page.waitForSelector(".paragraphs-container.is-ready");

    const beforeWidth = await page.evaluate(() => {
      const c = document.querySelector(
        ".paragraphs-container",
      ) as HTMLElement | null;
      return c ? c.clientWidth : -1;
    });
    expect(beforeWidth).toBeGreaterThan(0);

    const p = paragraphLocator(page, 0);
    await wordSpan(p, 0).click();
    await page.locator(EXPAND).click();
    await expect(page.locator(EXPANDED)).toBeVisible();
    await page.waitForTimeout(250);

    const afterWidth = await page.evaluate(() => {
      const c = document.querySelector(
        ".paragraphs-container",
      ) as HTMLElement | null;
      return c ? c.clientWidth : -1;
    });
    expect(afterWidth).toBe(beforeWidth);
  });
});

const ADD_CARD = '[data-testid="word-view-add-card"]';
const ADD_CARD_EXPANDED = '[data-testid="word-view-add-card-expanded"]';
const ADD_CARD_ADDED = '[data-testid="word-view-add-card-added"]';
const ADD_CARD_EXPANDED_ADDED =
  '[data-testid="word-view-add-card-expanded-added"]';

test.describe("WordView panel — add to Anki", () => {
  test.skip(
    ({ browserName }) => browserName === "firefox",
    "chromium + webkit only",
  );
  test.skip(isRealMode(), "mock-only card creation control surface");

  test("adds the selected word from the peek bar", async ({ page }) => {
    const { bookId } = await seedCardBook(page);
    await page.waitForSelector(".paragraphs-container.is-ready");
    await wordSpan(paragraphLocator(page, 0), 0).click();

    const add = page.locator(ADD_CARD);
    await expect(add).toBeVisible();
    await add.click();

    // The action disappears once the card exists; a check confirms the add.
    await expect(page.locator(ADD_CARD_ADDED)).toBeVisible();
    await expect(page.locator(ADD_CARD)).toHaveCount(0);

    const calls = await page.evaluate(() =>
      (window as any).__test.getCreateCardCalls(),
    );
    expect(calls).toEqual([
      { bookId, paragraphId: 0, sentenceId: 0, wordId: 0 },
    ]);
  });

  test("adds the selected word from the expanded header", async ({ page }) => {
    await seedCardBook(page);
    await page.waitForSelector(".paragraphs-container.is-ready");
    await wordSpan(paragraphLocator(page, 0), 0).click();
    await page.locator(EXPAND).click();

    const add = page.locator(ADD_CARD_EXPANDED);
    await expect(add).toBeVisible();
    await add.click();

    await expect(page.locator(ADD_CARD_EXPANDED_ADDED)).toBeVisible();
    await expect(page.locator(ADD_CARD_EXPANDED)).toHaveCount(0);
  });

  test("hides the action when a card already exists", async ({ page }) => {
    await seedCardBook(page, { hasCard: true });
    await page.waitForSelector(".paragraphs-container.is-ready");
    await wordSpan(paragraphLocator(page, 0), 0).click();

    await expect(page.locator(ADD_CARD)).toHaveCount(0);
    await page.locator(EXPAND).click();
    await expect(page.locator(ADD_CARD_EXPANDED)).toHaveCount(0);
  });

  test("hides the action when the word cannot become a card", async ({
    page,
  }) => {
    await seedCardBook(page, { canCreateCard: false });
    await page.waitForSelector(".paragraphs-container.is-ready");
    await wordSpan(paragraphLocator(page, 0), 0).click();

    await expect(page.locator(ADD_CARD)).toHaveCount(0);
  });

  test("shows a failed add and retries", async ({ page }) => {
    await seedCardBook(page);
    await page.waitForSelector(".paragraphs-container.is-ready");
    await wordSpan(paragraphLocator(page, 0), 0).click();

    await page.evaluate(() =>
      (window as any).__test.setCardCreateError("Anki is offline"),
    );
    const add = page.locator(ADD_CARD);
    await add.click();
    await expect(add).toHaveAttribute("title", "Anki is offline");

    await page.locator(EXPAND).click();
    await expect(page.locator(EXPANDED)).toContainText("Anki is offline");

    await page.evaluate(() => (window as any).__test.setCardCreateError(null));
    await page.locator(ADD_CARD_EXPANDED).click();
    await expect(page.locator(ADD_CARD_EXPANDED_ADDED)).toBeVisible();
  });
});

test.describe("WordView panel — drag resize", () => {
  test.skip(
    ({ browserName }) => browserName !== "chromium",
    "chromium-only — mouse drag",
  );

  test("dragging the top grip changes the height within the session", async ({
    page,
  }) => {
    await seedClickableBook(page);
    const p = paragraphLocator(page, 0);
    await wordSpan(p, 0).click();
    await page.locator(EXPAND).click();
    await expect(page.locator(EXPANDED)).toBeVisible();
    await page.waitForTimeout(250);

    const grip = page.locator('[data-testid="word-view-resize"]');
    const startBox = await grip.boundingBox();
    if (!startBox) throw new Error("resize grip not visible");

    const heightBefore = await page.evaluate(
      () =>
        (
          document.querySelector(
            '[data-testid="word-view"]',
          ) as HTMLElement | null
        )?.clientHeight ?? -1,
    );

    const startX = startBox.x + startBox.width / 2;
    const startY = startBox.y + startBox.height / 2;
    await page.mouse.move(startX, startY);
    await page.mouse.down();
    await page.mouse.move(startX, startY - 20, { steps: 5 });
    await page.mouse.move(startX, startY - 100, { steps: 10 });
    await page.mouse.up();

    const heightAfter = await page.evaluate(
      () =>
        (
          document.querySelector(
            '[data-testid="word-view"]',
          ) as HTMLElement | null
        )?.clientHeight ?? -1,
    );
    expect(heightAfter).toBeGreaterThan(heightBefore + 80);
  });
});
