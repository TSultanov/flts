import { expect, test } from "./helpers/test";
import {
  expectTranslated,
  getTranslateCalls,
  paragraphLocator,
  seedAndOpen,
  setTranslateConfig,
  setWordInfo,
  translateButton,
  wordSegment,
  wordSpan,
} from "./helpers/paragraph";

// Chromium only: Svelte reactivity, DOM events and JS class toggles do not
// vary by engine, so extra engines cost CI time for no signal.
test.describe.configure({ mode: "parallel" });

test.describe("ParagraphView (chromium only)", () => {
  test.skip(({ browserName }) => browserName !== "chromium", "chromium-only");

  test("A1: untranslated paragraph renders original text with enabled translate button", async ({
    page,
  }) => {
    await seedAndOpen(page, {
      chapters: [{ paragraphs: [{ html: "Hello world!" }] }],
    });

    const p = paragraphLocator(page, 0);
    await expect(p).toBeVisible();
    await expect(p.locator(".original")).toHaveText("Hello world!");
    await expect(translateButton(p)).toBeEnabled();
    await expect(p.locator(".circular-progress")).toHaveCount(0);
  });

  test("A2: pre-translated paragraph renders translated HTML and no translate button", async ({
    page,
  }) => {
    const segments = [
      wordSegment({
        flatIndex: 0,
        sentence: 0,
        word: 0,
        text: "hola",
        translation: "hello",
      }),
    ];
    await seedAndOpen(page, {
      chapters: [{ paragraphs: [{ html: "hello", segments }] }],
    });

    const p = paragraphLocator(page, 0);
    await expect(p).toBeVisible();
    await expect(translateButton(p)).toHaveCount(0);
    await expect(p.locator(".word-span")).toHaveCount(1);
    await expect(p.locator(".word-span")).toHaveText("hola");
  });

  test("B1: click translate disables button and shows spinner; original still visible during the in-flight window", async ({
    page,
  }) => {
    const { bookId } = await seedAndOpen(page, {
      chapters: [{ paragraphs: [{ html: "Hello world!" }] }],
    });
    await setTranslateConfig(page, bookId, 0, {
      kind: "progress",
      steps: [
        { progress: 10, total: 100, delayMs: 80 },
        { progress: 50, total: 100, delayMs: 80 },
        { progress: 100, total: 100, delayMs: 80 },
      ],
      segments: [
        wordSegment({
          flatIndex: 0,
          sentence: 0,
          word: 0,
          text: "translated",
          translation: null,
        }),
      ],
    });

    const p = paragraphLocator(page, 0);
    const btn = translateButton(p);
    await btn.click();

    await expect(btn).toBeDisabled();
    await expect(p.locator(".circular-progress")).toBeVisible();
    await expect(p.locator(".original")).toHaveText("Hello world!");
  });

  test("B2: progress drives the spinner — non-zero progress observed during translation", async ({
    page,
  }) => {
    // Steps must outlast the 500ms poll interval, or transitions are missed.
    const { bookId } = await seedAndOpen(page, {
      chapters: [{ paragraphs: [{ html: "Hello!" }] }],
    });
    await setTranslateConfig(page, bookId, 0, {
      kind: "progress",
      steps: [
        { progress: 25, total: 100, delayMs: 600 },
        { progress: 75, total: 100, delayMs: 600 },
        { progress: 100, total: 100, delayMs: 600 },
      ],
      segments: [
        wordSegment({
          flatIndex: 0,
          sentence: 0,
          word: 0,
          text: "done",
          translation: null,
        }),
      ],
    });

    const p = paragraphLocator(page, 0);
    await translateButton(p).click();

    const circle = p.locator(".circular-progress svg circle").nth(1);
    await expect(circle).toBeVisible();

    // Circumference 2π·10 ≈ 62.83 is the progress=0 dashoffset; any progress
    // must come in under it.
    await expect
      .poll(
        async () => {
          const v = await circle.getAttribute("stroke-dashoffset");
          return v ? parseFloat(v) : Number.POSITIVE_INFINITY;
        },
        { timeout: 3000, intervals: [100, 100, 100] },
      )
      .toBeLessThan(60);
  });

  test("B3: translation completes, original is replaced, button removed", async ({
    page,
  }) => {
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
      chapters: [{ paragraphs: [{ html: "hello" }] }],
    });
    await setTranslateConfig(page, bookId, 0, {
      kind: "progress",
      steps: [
        { progress: 50, total: 100, delayMs: 60 },
        { progress: 100, total: 100, delayMs: 60 },
      ],
      segments,
    });

    const p = paragraphLocator(page, 0);
    await translateButton(p).click();
    await expectTranslated(p);
    await expect(p.locator(".word-span")).toHaveText("hola");
    await expect(p.locator(".circular-progress")).toHaveCount(0);
  });

  test("B4: error path clears spinner, re-enables button, logs console warning", async ({
    page,
  }) => {
    const warnings: string[] = [];
    page.on("console", (msg) => {
      if (msg.type() === "warning" || msg.type() === "warn")
        warnings.push(msg.text());
    });

    const { bookId } = await seedAndOpen(page, {
      chapters: [{ paragraphs: [{ html: "fails" }] }],
    });
    await setTranslateConfig(page, bookId, 0, {
      kind: "error",
      errorMessage: "rate limited",
      delayMs: 800,
    });

    const p = paragraphLocator(page, 0);
    const btn = translateButton(p);
    await btn.click();
    await expect(btn).toBeDisabled();

    await expect(p.locator(".circular-progress")).toHaveCount(0);
    await expect(btn).toBeEnabled();
    await expect(p.locator(".original")).toHaveText("fails");

    await expect
      .poll(() => warnings.some((w) => w.includes("rate limited")))
      .toBe(true);
  });

  test("C1: plain click sends useCache=true", async ({ page }) => {
    const { bookId } = await seedAndOpen(page, {
      chapters: [{ paragraphs: [{ html: "h" }] }],
    });
    await setTranslateConfig(page, bookId, 0, {
      kind: "immediate",
      segments: [
        wordSegment({
          flatIndex: 0,
          sentence: 0,
          word: 0,
          text: "x",
          translation: null,
        }),
      ],
    });

    const p = paragraphLocator(page, 0);
    await translateButton(p).click();

    await expect
      .poll(async () => (await getTranslateCalls(page)).length)
      .toBe(1);
    const calls = await getTranslateCalls(page);
    expect(calls[0].useCache).toBe(true);
  });

  test("C2: cmd-click sends useCache=false", async ({ page }) => {
    const { bookId } = await seedAndOpen(page, {
      chapters: [{ paragraphs: [{ html: "h" }] }],
    });
    await setTranslateConfig(page, bookId, 0, {
      kind: "immediate",
      segments: [
        wordSegment({
          flatIndex: 0,
          sentence: 0,
          word: 0,
          text: "x",
          translation: null,
        }),
      ],
    });

    const p = paragraphLocator(page, 0);
    await translateButton(p).click({ modifiers: ["Meta"] });
    await expect
      .poll(async () => (await getTranslateCalls(page)).length)
      .toBe(1);
    const calls = await getTranslateCalls(page);
    expect(calls[0].useCache).toBe(false);
  });

  // ctrl+click is untestable on macOS chromium (the browser turns it into a
  // contextmenu); C2's metaKey case covers the same handler expression.

  test("D1: pre-existing in-flight request shows spinner on mount without click", async ({
    page,
  }) => {
    await seedAndOpen(page, {
      chapters: [{ paragraphs: [{ html: "queued" }] }],
      inFlight: [
        {
          paragraphId: 0,
          requestId: 42,
          cfg: {
            kind: "progress",
            // The mock ticks from page-init, so these must outlive app boot.
            steps: [
              { progress: 30, total: 100, delayMs: 800 },
              { progress: 100, total: 100, delayMs: 800 },
            ],
            segments: [
              wordSegment({
                flatIndex: 0,
                sentence: 0,
                word: 0,
                text: "finally done",
                translation: null,
              }),
            ],
          },
        },
      ],
    });

    const p = paragraphLocator(page, 0);
    await expect(p.locator(".circular-progress")).toBeVisible();
    await expectTranslated(p);
    await expect(p.getByText("finally done")).toBeVisible();
  });

  test("E1: translated paragraph word-spans render without a translation overlay by default", async ({
    page,
  }) => {
    const segments = [0, 1, 2].flatMap((i) => [
      ...(i > 0 ? [{ kind: "gap" as const, html: " " }] : []),
      wordSegment({
        flatIndex: i,
        sentence: 0,
        word: i,
        text: `w${i}`,
        translation: `t${i}`,
      }),
    ]);
    await seedAndOpen(page, {
      chapters: [{ paragraphs: [{ html: "orig", segments }] }],
    });

    const p = paragraphLocator(page, 0);
    for (const i of [0, 1, 2]) {
      await expect(wordSpan(p, i)).toBeVisible();
      await expect(wordSpan(p, i).locator(".translation-overlay")).toHaveCount(
        0,
      );
    }
  });

  test("E2: clicking a word opens WordView with seeded info", async ({
    page,
  }) => {
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
      chapters: [{ paragraphs: [{ html: "hello", segments }] }],
    });
    await setWordInfo(page, bookId, 0, 0, 0, {
      original: "hello",
      contextualTranslations: ["hola"],
      fullSentenceTranslation: "hola",
    });

    const p = paragraphLocator(page, 0);
    await wordSpan(p, 0).click();
    await expect(wordSpan(p, 0)).toHaveClass(/\bselected\b/);
    // Selection opens WordView's peek: original word + comma-joined translations.
    const peek = page.locator('[data-testid="word-view-peek"]');
    await expect(peek.locator(".peek-word")).toHaveText("hello");
    await expect(peek.locator(".peek-translations")).toHaveText("hola");
  });

  test("F1: a familiarity-0 word renders the translation overlay automatically", async ({
    page,
  }) => {
    // Familiarity 0 auto-shows the overlay; familiarity 1 stays hidden.
    const segments = [
      wordSegment({
        flatIndex: 0,
        sentence: 0,
        word: 0,
        text: "w0",
        translation: "t0",
        familiarity: 0,
      }),
      { kind: "gap" as const, html: " " },
      wordSegment({
        flatIndex: 1,
        sentence: 0,
        word: 1,
        text: "w1",
        translation: "t1",
        familiarity: 1,
      }),
      { kind: "gap" as const, html: " " },
      wordSegment({
        flatIndex: 2,
        sentence: 0,
        word: 2,
        text: "w2",
        translation: "t2",
        familiarity: 0,
      }),
    ];
    await seedAndOpen(page, {
      chapters: [{ paragraphs: [{ html: "orig", segments }] }],
    });

    const p = paragraphLocator(page, 0);
    await expect(wordSpan(p, 0).locator(".translation-overlay")).toHaveCount(1);
    await expect(wordSpan(p, 2).locator(".translation-overlay")).toHaveCount(1);
    await expect(wordSpan(p, 1).locator(".translation-overlay")).toHaveCount(0);
  });

  test("F3: clicking a word paints its overlay and the overlay persists after deselect", async ({
    page,
  }) => {
    // No familiarity seeded, so only a click can reveal the overlay.
    const segments = [0, 1, 2].flatMap((i) => [
      ...(i > 0 ? [{ kind: "gap" as const, html: " " }] : []),
      wordSegment({
        flatIndex: i,
        sentence: 0,
        word: i,
        text: `w${i}`,
        translation: `t${i}`,
      }),
    ]);
    await seedAndOpen(page, {
      chapters: [{ paragraphs: [{ html: "orig", segments }] }],
    });

    const p = paragraphLocator(page, 0);
    await expect(p.locator(".translation-overlay")).toHaveCount(0);

    const isOverlayPainted = async (flatIndex: number) => {
      return page.evaluate((idx) => {
        const span = document.querySelector(
          `.word-span[data-flat-index="${idx}"]`,
        );
        if (!span) return false;
        const beforeStyle = getComputedStyle(span as Element, "::before");
        const beforeVisible =
          beforeStyle.content !== "none" &&
          beforeStyle.display !== "none" &&
          (parseFloat(beforeStyle.opacity) || 0) > 0;
        const overlay = span.querySelector(".translation-overlay");
        const overlayStyle = overlay ? getComputedStyle(overlay) : null;
        const overlayVisible =
          !!overlayStyle &&
          overlayStyle.display !== "none" &&
          (parseFloat(overlayStyle.opacity) || 0) > 0;
        return beforeVisible || overlayVisible;
      }, flatIndex);
    };

    await wordSpan(p, 1).click();
    await expect.poll(() => isOverlayPainted(1)).toBe(true);
    await expect.poll(() => isOverlayPainted(0)).toBe(false);
    await expect.poll(() => isOverlayPainted(2)).toBe(false);

    // Clearing the selection must not un-reveal: the click marks it for the session.
    await p.click({ position: { x: 1, y: 1 } });
    await expect(wordSpan(p, 1)).not.toHaveClass(/\bselected\b/);
    await expect.poll(() => isOverlayPainted(1)).toBe(true);
    await expect.poll(() => isOverlayPainted(0)).toBe(false);
    await expect.poll(() => isOverlayPainted(2)).toBe(false);
  });

  test("F2: auto-shown translation overlays are actually painted (opacity > 0)", async ({
    page,
  }) => {
    // Words 0 and 2 auto-show; word 1 stays hidden.
    const segments = [0, 1, 2].flatMap((i) => [
      ...(i > 0 ? [{ kind: "gap" as const, html: " " }] : []),
      wordSegment({
        flatIndex: i,
        sentence: 0,
        word: i,
        text: `w${i}`,
        translation: `t${i}`,
        familiarity: i === 1 ? 1 : 0,
      }),
    ]);
    await seedAndOpen(page, {
      chapters: [{ paragraphs: [{ html: "orig", segments }] }],
    });

    // The overlay is a ::before pseudo-element or a .translation-overlay child
    // depending on the render branch; either way require it painted.
    const visibility = await page.evaluate(() => {
      const probe = (flatIndex: number) => {
        const span = document.querySelector(
          `.word-span[data-flat-index="${flatIndex}"]`,
        );
        if (!span) return { flatIndex, visible: false, missing: true };

        const beforeStyle = getComputedStyle(span as Element, "::before");
        const beforeVisible =
          beforeStyle.content !== "none" &&
          beforeStyle.display !== "none" &&
          (parseFloat(beforeStyle.opacity) || 0) > 0;

        const overlay = span.querySelector(".translation-overlay");
        const overlayStyle = overlay ? getComputedStyle(overlay) : null;
        const overlayVisible =
          !!overlayStyle &&
          overlayStyle.display !== "none" &&
          (parseFloat(overlayStyle.opacity) || 0) > 0;

        return {
          flatIndex,
          visible: beforeVisible || overlayVisible,
          missing: false,
        };
      };
      return [0, 1, 2].map(probe);
    });

    expect(visibility[0].visible).toBe(true);
    expect(visibility[2].visible).toBe(true);
    expect(visibility[1].visible).toBe(false);
  });

  test("G1: word click on one paragraph does not blank peers (regression of 901e6a7)", async ({
    page,
  }) => {
    const s1 = [
      wordSegment({
        flatIndex: 0,
        sentence: 0,
        word: 0,
        text: "a1",
        translation: "A1",
      }),
    ];
    const s2 = [
      wordSegment({
        flatIndex: 0,
        sentence: 0,
        word: 0,
        text: "a2",
        translation: "A2",
      }),
    ];
    const s3 = [
      wordSegment({
        flatIndex: 0,
        sentence: 0,
        word: 0,
        text: "a3",
        translation: "A3",
      }),
    ];
    const { bookId } = await seedAndOpen(page, {
      chapters: [
        {
          paragraphs: [
            { html: "h1", segments: s1 },
            { html: "h2", segments: s2 },
            { html: "h3", segments: s3 },
          ],
        },
      ],
    });
    await setWordInfo(page, bookId, 0, 0, 0, { original: "a1" });

    // Flags any moment a peer's rendered translation text goes empty.
    await page.evaluate(() => {
      (window as any).__peerFlickered = false;
      const peers = [1, 2];
      for (const id of peers) {
        const wrapper = document.querySelector(
          `.paragraph-wrapper[data-paragraph-id="${id}"]`,
        );
        if (!wrapper) continue;
        const obs = new MutationObserver(() => {
          const span = wrapper.querySelector(".word-span");
          if (!span || !(span.textContent ?? "").trim()) {
            (window as any).__peerFlickered = true;
          }
        });
        obs.observe(wrapper, {
          childList: true,
          subtree: true,
          characterData: true,
        });
      }
    });

    const p0 = paragraphLocator(page, 0);
    await wordSpan(p0, 0).click();
    await page.waitForTimeout(250);
    const flickered = await page.evaluate(
      () => (window as any).__peerFlickered,
    );
    expect(flickered).toBe(false);
  });

  test("G3: clicking translate on multiple paragraphs in succession flips every clicked button into a spinner immediately (regression of 955b7d3)", async ({
    page,
  }) => {
    // The queue is serial, but "started" fires at enqueue, so every clicked
    // button must spin immediately — not only the active one.
    const { bookId } = await seedAndOpen(page, {
      chapters: [
        {
          paragraphs: [
            { html: "first paragraph" },
            { html: "second paragraph" },
            { html: "third paragraph" },
          ],
        },
      ],
    });

    // ~600ms per stage keeps paragraphs 1 and 2 queued while we sample.
    const slowCfg = (text: string) => ({
      kind: "progress" as const,
      steps: [
        { progress: 50, total: 100, delayMs: 300 },
        { progress: 100, total: 100, delayMs: 300 },
      ],
      segments: [
        wordSegment({
          flatIndex: 0,
          sentence: 0,
          word: 0,
          text,
          translation: null,
        }),
      ],
    });
    for (const pid of [0, 1, 2]) {
      await setTranslateConfig(page, bookId, pid, slowCfg(`done${pid}`));
    }

    const p0 = paragraphLocator(page, 0);
    const p1 = paragraphLocator(page, 1);
    const p2 = paragraphLocator(page, 2);

    await translateButton(p0).click();
    await translateButton(p1).click();
    await translateButton(p2).click();

    // Tight timeout: queued buttons must spin now, not on becoming active.
    await expect(translateButton(p0)).toBeDisabled({ timeout: 500 });
    await expect(translateButton(p1)).toBeDisabled({ timeout: 500 });
    await expect(translateButton(p2)).toBeDisabled({ timeout: 500 });
    await expect(p0.locator(".circular-progress")).toBeVisible({
      timeout: 500,
    });
    await expect(p1.locator(".circular-progress")).toBeVisible({
      timeout: 500,
    });
    await expect(p2.locator(".circular-progress")).toBeVisible({
      timeout: 500,
    });

    await expectTranslated(p0);
    await expectTranslated(p1);
    await expectTranslated(p2);
  });

  test("G2: translation completing on one paragraph does not blank peers (regression of 78d9b74)", async ({
    page,
  }) => {
    const s2 = [
      wordSegment({
        flatIndex: 0,
        sentence: 0,
        word: 0,
        text: "b",
        translation: "B",
      }),
    ];
    const s3 = [
      wordSegment({
        flatIndex: 0,
        sentence: 0,
        word: 0,
        text: "c",
        translation: "C",
      }),
    ];
    const { bookId } = await seedAndOpen(page, {
      chapters: [
        {
          paragraphs: [
            { html: "h1" },
            { html: "h2", segments: s2 },
            { html: "h3", segments: s3 },
          ],
        },
      ],
    });
    await setTranslateConfig(page, bookId, 0, {
      kind: "progress",
      steps: [
        { progress: 50, total: 100, delayMs: 80 },
        { progress: 100, total: 100, delayMs: 80 },
      ],
      segments: [
        wordSegment({
          flatIndex: 0,
          sentence: 0,
          word: 0,
          text: "done",
          translation: null,
        }),
      ],
    });

    await page.evaluate(() => {
      (window as any).__peerFlickered = false;
      const peers = [1, 2];
      for (const id of peers) {
        const wrapper = document.querySelector(
          `.paragraph-wrapper[data-paragraph-id="${id}"]`,
        );
        if (!wrapper) continue;
        const obs = new MutationObserver(() => {
          const span = wrapper.querySelector(".word-span");
          if (!span || !(span.textContent ?? "").trim()) {
            (window as any).__peerFlickered = true;
          }
        });
        obs.observe(wrapper, {
          childList: true,
          subtree: true,
          characterData: true,
        });
      }
    });

    const p0 = paragraphLocator(page, 0);
    await translateButton(p0).click();
    await expectTranslated(p0);
    const flickered = await page.evaluate(
      () => (window as any).__peerFlickered,
    );
    expect(flickered).toBe(false);
  });
});
