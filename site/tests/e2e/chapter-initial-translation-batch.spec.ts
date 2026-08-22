import { expect, test } from "./helpers/test";
import {
  getTranslationsBatchCalls,
  seedAndOpen,
  wordSegment,
} from "./helpers/paragraph";

// Chromium only: Svelte reactivity / DOM measurement, not layout behaviour.
test.describe.configure({ mode: "parallel" });

test.describe("Chapter initial translation batch (chromium only)", () => {
  test.skip(({ browserName }) => browserName !== "chromium", "chromium-only");

  test("opening a chapter does not enqueue translations for the whole chapter on initial mount", async ({
    page,
  }) => {
    // Guards against #recomputeMountWindow running on empty wrappers and
    // classifying the whole chapter as mounted, which floods the translations
    // queue. Each paragraph needs real text, or every wrapper is one line tall
    // and the mount window catches everything either way.
    const N = 80;
    const bodyText = (
      "Lorem ipsum dolor sit amet, consectetur adipiscing elit. " +
      "Sed do eiusmod tempor incididunt ut labore et dolore magna " +
      "aliqua. Ut enim ad minim veniam, quis nostrud exercitation " +
      "ullamco laboris nisi ut aliquip ex ea commodo consequat. " +
      "Duis aute irure dolor in reprehenderit in voluptate velit " +
      "esse cillum dolore eu fugiat nulla pariatur."
    ).repeat(2);
    const paragraphs = Array.from({ length: N }, (_, i) => ({
      html: `<p>${bodyText} (paragraph ${i})</p>`,
      segments: [
        wordSegment({
          flatIndex: 0,
          sentence: 0,
          word: 0,
          text: `w${i}`,
          translation: `t${i}`,
        }),
      ],
    }));

    await seedAndOpen(page, { chapters: [{ paragraphs }] });

    await expect(page.locator(".paragraphs-container.is-ready")).toBeVisible();

    // The triggering mount-window computation runs in a rAF after originals.
    await expect
      .poll(async () => (await getTranslationsBatchCalls(page)).length, {
        timeout: 5000,
      })
      .toBeGreaterThan(0);

    const calls = await getTranslationsBatchCalls(page);
    const totalQueued = new Set(calls.flatMap((c) => c.paragraphIds)).size;
    // Healthy is 15-25 at the default viewport; broken enqueues all ~80.
    expect(totalQueued).toBeLessThanOrEqual(40);
  });
});
