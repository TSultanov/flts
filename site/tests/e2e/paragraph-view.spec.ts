import { expect, test } from '@playwright/test';
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
} from './helpers/paragraph';

// All paragraph-view tests stay on chromium. The behaviors we're testing
// (Svelte reactivity, DOM events, CSS class toggles via JS) don't vary by
// browser engine; running them on three engines triples CI time for the
// same signal. Existing import specs continue to cross-browser.
test.describe.configure({ mode: 'parallel' });

test.describe('ParagraphView (chromium only)', () => {
  test.skip(({ browserName }) => browserName !== 'chromium', 'chromium-only');

  // ----- Group A: render ---------------------------------------------------

  test('A1: untranslated paragraph renders original text with enabled translate button', async ({
    page,
  }) => {
    await seedAndOpen(page, {
      chapters: [{ paragraphs: [{ html: 'Hello world!' }] }],
    });

    const p = paragraphLocator(page, 0);
    await expect(p).toBeVisible();
    await expect(p.locator('.original')).toHaveText('Hello world!');
    await expect(translateButton(p)).toBeEnabled();
    // No spinner yet
    await expect(p.locator('.circular-progress')).toHaveCount(0);
  });

  test('A2: pre-translated paragraph renders translated HTML and no translate button', async ({
    page,
  }) => {
    const segments = [
      wordSegment({
        flatIndex: 0,
        sentence: 0,
        word: 0,
        text: 'hola',
        translation: 'hello',
      }),
    ];
    await seedAndOpen(page, {
      chapters: [{ paragraphs: [{ html: 'hello', segments }] }],
    });

    const p = paragraphLocator(page, 0);
    await expect(p).toBeVisible();
    await expect(translateButton(p)).toHaveCount(0);
    await expect(p.locator('.word-span')).toHaveCount(1);
    await expect(p.locator('.word-span')).toHaveText('hola');
  });

  // ----- Group B: translation flow ----------------------------------------

  test('B1: click translate disables button and shows spinner; original still visible during the in-flight window', async ({
    page,
  }) => {
    const { bookId } = await seedAndOpen(page, {
      chapters: [{ paragraphs: [{ html: 'Hello world!' }] }],
    });
    await setTranslateConfig(page, bookId, 0, {
      kind: 'progress',
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
          text: 'translated',
          translation: null,
        }),
      ],
    });

    const p = paragraphLocator(page, 0);
    const btn = translateButton(p);
    await btn.click();

    await expect(btn).toBeDisabled();
    await expect(p.locator('.circular-progress')).toBeVisible();
    // Original text is still rendered during the translating window
    await expect(p.locator('.original')).toHaveText('Hello world!');
  });

  test('B2: progress drives the spinner — non-zero progress observed during translation', async ({
    page,
  }) => {
    // Polling interval is 500ms; per-step delays must be long enough that
    // polls can land between transitions. Three steps of 600ms each ≈ 1.8s.
    const { bookId } = await seedAndOpen(page, {
      chapters: [{ paragraphs: [{ html: 'Hello!' }] }],
    });
    await setTranslateConfig(page, bookId, 0, {
      kind: 'progress',
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
          text: 'done',
          translation: null,
        }),
      ],
    });

    const p = paragraphLocator(page, 0);
    await translateButton(p).click();

    const circle = p.locator('.circular-progress svg circle').nth(1);
    await expect(circle).toBeVisible();

    // Wait until stroke-dashoffset reflects a non-zero progress (poll @500ms
    // will pick up the 25/100 snapshot once the in-progress index is in place).
    // Circumference = 2π·10 ≈ 62.83. At progress=25, dashoffset ≈ 47.12;
    // at progress=0, dashoffset == 62.83. We just want "less than max".
    await expect
      .poll(async () => {
        const v = await circle.getAttribute('stroke-dashoffset');
        return v ? parseFloat(v) : Number.POSITIVE_INFINITY;
      }, { timeout: 3000, intervals: [100, 100, 100] })
      .toBeLessThan(60);
  });

  test('B3: translation completes, original is replaced, button removed', async ({
    page,
  }) => {
    const segments = [
      wordSegment({
        flatIndex: 0,
        sentence: 0,
        word: 0,
        text: 'hola',
        translation: 'hello',
      }),
    ];
    const { bookId } = await seedAndOpen(page, {
      chapters: [{ paragraphs: [{ html: 'hello' }] }],
    });
    await setTranslateConfig(page, bookId, 0, {
      kind: 'progress',
      steps: [
        { progress: 50, total: 100, delayMs: 60 },
        { progress: 100, total: 100, delayMs: 60 },
      ],
      segments,
    });

    const p = paragraphLocator(page, 0);
    await translateButton(p).click();
    await expectTranslated(p);
    await expect(p.locator('.word-span')).toHaveText('hola');
    // Translation-side <p> rendered; left column is the empty placeholder <div>.
    await expect(p.locator('.circular-progress')).toHaveCount(0);
  });

  test('B4: error path clears spinner, re-enables button, logs console warning', async ({
    page,
  }) => {
    const warnings: string[] = [];
    page.on('console', (msg) => {
      if (msg.type() === 'warning' || msg.type() === 'warn') warnings.push(msg.text());
    });

    const { bookId } = await seedAndOpen(page, {
      chapters: [{ paragraphs: [{ html: 'fails' }] }],
    });
    await setTranslateConfig(page, bookId, 0, {
      kind: 'error',
      errorMessage: 'rate limited',
      delayMs: 800,
    });

    const p = paragraphLocator(page, 0);
    const btn = translateButton(p);
    await btn.click();
    await expect(btn).toBeDisabled();

    // After error completes, spinner clears and button is enabled again.
    await expect(p.locator('.circular-progress')).toHaveCount(0);
    await expect(btn).toBeEnabled();
    // Original text still rendered (no translation HTML).
    await expect(p.locator('.original')).toHaveText('fails');

    await expect.poll(() => warnings.some((w) => w.includes('rate limited'))).toBe(true);
  });

  // ----- Group C: cache bypass --------------------------------------------

  test('C1: plain click sends useCache=true', async ({ page }) => {
    const { bookId } = await seedAndOpen(page, {
      chapters: [{ paragraphs: [{ html: 'h' }] }],
    });
    await setTranslateConfig(page, bookId, 0, {
      kind: 'immediate',
      segments: [
        wordSegment({ flatIndex: 0, sentence: 0, word: 0, text: 'x', translation: null }),
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

  test('C2: cmd-click sends useCache=false', async ({ page }) => {
    const { bookId } = await seedAndOpen(page, {
      chapters: [{ paragraphs: [{ html: 'h' }] }],
    });
    await setTranslateConfig(page, bookId, 0, {
      kind: 'immediate',
      segments: [
        wordSegment({ flatIndex: 0, sentence: 0, word: 0, text: 'x', translation: null }),
      ],
    });

    const p = paragraphLocator(page, 0);
    await translateButton(p).click({ modifiers: ['Meta'] });
    await expect
      .poll(async () => (await getTranslateCalls(page)).length)
      .toBe(1);
    const calls = await getTranslateCalls(page);
    expect(calls[0].useCache).toBe(false);
  });

  // Note: ctrl+click on macOS chromium is intercepted as a contextmenu event
  // by the browser layer (Playwright's `modifiers: ['Control']` doesn't bypass
  // this), so we don't separately test ctrl+click. The handler is
  // `!(e.metaKey || e.ctrlKey)` — C2 covers metaKey; the ctrlKey branch is the
  // same expression.

  // ----- Group D: in-flight reconciliation --------------------------------

  test('D1: pre-existing in-flight request shows spinner on mount without click', async ({
    page,
  }) => {
    await seedAndOpen(page, {
      chapters: [{ paragraphs: [{ html: 'queued' }] }],
      inFlight: [
        {
          paragraphId: 0,
          requestId: 42,
          cfg: {
            kind: 'progress',
            steps: [
              { progress: 30, total: 100, delayMs: 200 },
              { progress: 100, total: 100, delayMs: 200 },
            ],
            segments: [
              wordSegment({
                flatIndex: 0,
                sentence: 0,
                word: 0,
                text: 'finally done',
                translation: null,
              }),
            ],
          },
        },
      ],
    });

    const p = paragraphLocator(page, 0);
    // Spinner appears without any user click — VM picked up the in-flight id.
    await expect(p.locator('.circular-progress')).toBeVisible();
    // After translation finishes, the translated HTML lands.
    await expectTranslated(p);
    await expect(p.getByText('finally done')).toBeVisible();
  });

  // ----- Group E: word translations ---------------------------------------

  test('E1: translated paragraph word-spans render without a translation overlay by default', async ({
    page,
  }) => {
    const segments = [0, 1, 2].flatMap((i) => [
      ...(i > 0 ? [{ kind: 'gap' as const, html: ' ' }] : []),
      wordSegment({
        flatIndex: i,
        sentence: 0,
        word: i,
        text: `w${i}`,
        translation: `t${i}`,
      }),
    ]);
    await seedAndOpen(page, {
      chapters: [{ paragraphs: [{ html: 'orig', segments }] }],
    });

    const p = paragraphLocator(page, 0);
    for (const i of [0, 1, 2]) {
      await expect(wordSpan(p, i)).toBeVisible();
      await expect(wordSpan(p, i).locator('.translation-overlay')).toHaveCount(0);
    }
  });

  test('E2: clicking a word opens WordView with seeded info', async ({ page }) => {
    const segments = [
      wordSegment({
        flatIndex: 0,
        sentence: 0,
        word: 0,
        text: 'hola',
        translation: 'hello',
      }),
    ];
    const { bookId } = await seedAndOpen(page, {
      chapters: [{ paragraphs: [{ html: 'hello', segments }] }],
    });
    await setWordInfo(page, bookId, 0, 0, 0, {
      original: 'hello',
      contextualTranslations: ['hola'],
      fullSentenceTranslation: 'hola',
    });

    const p = paragraphLocator(page, 0);
    await wordSpan(p, 0).click();
    await expect(wordSpan(p, 0)).toHaveClass(/\bselected\b/);
    // WordView now lives in a bottom overlay panel that opens collapsed
    // (peek state) on selection. The peek shows the original word and the
    // comma-joined contextual translations.
    const peek = page.locator('[data-testid="word-view-peek"]');
    await expect(peek.locator('.peek-word')).toHaveText('hello');
    await expect(peek.locator('.peek-translations')).toHaveText('hola');
  });

  // ----- Group F: reveal-on-click annotation ------------------------------

  test('F1: a familiarity-0 word renders the translation overlay automatically', async ({
    page,
  }) => {
    // Words at familiarity 0 (e.g. never-synced cards) auto-show their
    // overlay without any user click. Words at familiarity 1 stay hidden.
    const segments = [
      wordSegment({
        flatIndex: 0,
        sentence: 0,
        word: 0,
        text: 'w0',
        translation: 't0',
        familiarity: 0,
      }),
      { kind: 'gap' as const, html: ' ' },
      wordSegment({
        flatIndex: 1,
        sentence: 0,
        word: 1,
        text: 'w1',
        translation: 't1',
        familiarity: 1,
      }),
      { kind: 'gap' as const, html: ' ' },
      wordSegment({
        flatIndex: 2,
        sentence: 0,
        word: 2,
        text: 'w2',
        translation: 't2',
        familiarity: 0,
      }),
    ];
    await seedAndOpen(page, {
      chapters: [{ paragraphs: [{ html: 'orig', segments }] }],
    });

    const p = paragraphLocator(page, 0);
    await expect(wordSpan(p, 0).locator('.translation-overlay')).toHaveCount(1);
    await expect(wordSpan(p, 2).locator('.translation-overlay')).toHaveCount(1);
    // Familiar word (familiarity 1) stays hidden until the user clicks it.
    await expect(wordSpan(p, 1).locator('.translation-overlay')).toHaveCount(0);
  });

  test('F3: clicking a word paints its overlay and the overlay persists after deselect', async ({
    page,
  }) => {
    // No familiarity seeded — no auto-show; the only path to the overlay
    // is the user clicking the word.
    const segments = [0, 1, 2].flatMap((i) => [
      ...(i > 0 ? [{ kind: 'gap' as const, html: ' ' }] : []),
      wordSegment({
        flatIndex: i,
        sentence: 0,
        word: i,
        text: `w${i}`,
        translation: `t${i}`,
      }),
    ]);
    await seedAndOpen(page, {
      chapters: [{ paragraphs: [{ html: 'orig', segments }] }],
    });

    const p = paragraphLocator(page, 0);
    // Sanity: no overlay anywhere before the click.
    await expect(p.locator('.translation-overlay')).toHaveCount(0);

    const isOverlayPainted = async (flatIndex: number) => {
      return page.evaluate((idx) => {
        const span = document.querySelector(
          `.word-span[data-flat-index="${idx}"]`,
        );
        if (!span) return false;
        const beforeStyle = getComputedStyle(span as Element, '::before');
        const beforeVisible =
          beforeStyle.content !== 'none' &&
          beforeStyle.display !== 'none' &&
          (parseFloat(beforeStyle.opacity) || 0) > 0;
        const overlay = span.querySelector('.translation-overlay');
        const overlayStyle = overlay ? getComputedStyle(overlay) : null;
        const overlayVisible =
          !!overlayStyle &&
          overlayStyle.display !== 'none' &&
          (parseFloat(overlayStyle.opacity) || 0) > 0;
        return beforeVisible || overlayVisible;
      }, flatIndex);
    };

    // Click word 1: overlay paints, peers stay hidden.
    await wordSpan(p, 1).click();
    await expect.poll(() => isOverlayPainted(1)).toBe(true);
    await expect.poll(() => isOverlayPainted(0)).toBe(false);
    await expect.poll(() => isOverlayPainted(2)).toBe(false);

    // Click outside any word to clear the selection. Word 1's overlay must
    // STILL be painted — the click marked it visible for the session.
    await p.click({ position: { x: 1, y: 1 } });
    await expect(wordSpan(p, 1)).not.toHaveClass(/\bselected\b/);
    await expect.poll(() => isOverlayPainted(1)).toBe(true);
    // And peers still don't paint.
    await expect.poll(() => isOverlayPainted(0)).toBe(false);
    await expect.poll(() => isOverlayPainted(2)).toBe(false);
  });

  test('F2: auto-shown translation overlays are actually painted (opacity > 0)', async ({
    page,
  }) => {
    // Seed words 0 and 2 at familiarity 0 (auto-show); word 1 at familiarity
    // 1 (stays hidden).
    const segments = [0, 1, 2].flatMap((i) => [
      ...(i > 0 ? [{ kind: 'gap' as const, html: ' ' }] : []),
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
      chapters: [{ paragraphs: [{ html: 'orig', segments }] }],
    });

    // Implementation-agnostic visibility probe. The "translation overlay" can
    // be either a ::before pseudo-element (HTML-blob implementation) or a real
    // .translation-overlay child (structured-segments implementation). In
    // both cases we require the painted result: display != none, opacity > 0,
    // and (for ::before) content actually set.
    const visibility = await page.evaluate(() => {
      const probe = (flatIndex: number) => {
        const span = document.querySelector(
          `.word-span[data-flat-index="${flatIndex}"]`,
        );
        if (!span) return { flatIndex, visible: false, missing: true };

        const beforeStyle = getComputedStyle(span as Element, '::before');
        const beforeVisible =
          beforeStyle.content !== 'none' &&
          beforeStyle.display !== 'none' &&
          (parseFloat(beforeStyle.opacity) || 0) > 0;

        const overlay = span.querySelector('.translation-overlay');
        const overlayStyle = overlay ? getComputedStyle(overlay) : null;
        const overlayVisible =
          !!overlayStyle &&
          overlayStyle.display !== 'none' &&
          (parseFloat(overlayStyle.opacity) || 0) > 0;

        return { flatIndex, visible: beforeVisible || overlayVisible, missing: false };
      };
      return [0, 1, 2].map(probe);
    });

    expect(visibility[0].visible).toBe(true);
    expect(visibility[2].visible).toBe(true);
    expect(visibility[1].visible).toBe(false);
  });

  // ----- Group G: no-flicker regressions ----------------------------------

  test('G1: word click on one paragraph does not blank peers (regression of 901e6a7)', async ({
    page,
  }) => {
    const s1 = [
      wordSegment({ flatIndex: 0, sentence: 0, word: 0, text: 'a1', translation: 'A1' }),
    ];
    const s2 = [
      wordSegment({ flatIndex: 0, sentence: 0, word: 0, text: 'a2', translation: 'A2' }),
    ];
    const s3 = [
      wordSegment({ flatIndex: 0, sentence: 0, word: 0, text: 'a3', translation: 'A3' }),
    ];
    const { bookId } = await seedAndOpen(page, {
      chapters: [
        {
          paragraphs: [
            { html: 'h1', segments: s1 },
            { html: 'h2', segments: s2 },
            { html: 'h3', segments: s3 },
          ],
        },
      ],
    });
    await setWordInfo(page, bookId, 0, 0, 0, { original: 'a1' });

    // Install a MutationObserver on peer paragraphs that flags any moment
    // where the rendered translation text becomes empty.
    await page.evaluate(() => {
      (window as any).__peerFlickered = false;
      const peers = [1, 2];
      for (const id of peers) {
        const wrapper = document.querySelector(
          `.paragraph-wrapper[data-paragraph-id="${id}"]`,
        );
        if (!wrapper) continue;
        const obs = new MutationObserver(() => {
          const span = wrapper.querySelector('.word-span');
          if (!span || !(span.textContent ?? '').trim()) {
            (window as any).__peerFlickered = true;
          }
        });
        obs.observe(wrapper, { childList: true, subtree: true, characterData: true });
      }
    });

    const p0 = paragraphLocator(page, 0);
    await wordSpan(p0, 0).click();
    await page.waitForTimeout(250);
    const flickered = await page.evaluate(() => (window as any).__peerFlickered);
    expect(flickered).toBe(false);
  });

  test('G3: clicking translate on multiple paragraphs in succession flips every clicked button into a spinner immediately (regression of 955b7d3)', async ({
    page,
  }) => {
    // The translation queue runs serially: only one paragraph is in
    // active progress at a time. The bug: the "started" event was emitted
    // when the worker picked the request up, so paragraphs sitting in the
    // queue showed no UI feedback. The fix is to emit "started" at enqueue
    // time so every clicked button immediately flips into a spinner. This
    // test enforces that contract end-to-end.
    const { bookId } = await seedAndOpen(page, {
      chapters: [
        {
          paragraphs: [
            { html: 'first paragraph' },
            { html: 'second paragraph' },
            { html: 'third paragraph' },
          ],
        },
      ],
    });

    // Each paragraph's worker stage takes ~600ms — long enough that
    // paragraphs 1 and 2 are still queued (not yet picked up by the
    // single worker) when we sample the buttons below.
    const slowCfg = (text: string) => ({
      kind: 'progress' as const,
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

    // Click all three in rapid succession.
    await translateButton(p0).click();
    await translateButton(p1).click();
    await translateButton(p2).click();

    // Every clicked button must flip to spinner state straight away, even
    // though the queue is single-threaded and only one paragraph is
    // actively translating at a time. Use a tight timeout: if the bug
    // recurs, the queued buttons won't ever show a spinner until they
    // become the active item.
    await expect(translateButton(p0)).toBeDisabled({ timeout: 500 });
    await expect(translateButton(p1)).toBeDisabled({ timeout: 500 });
    await expect(translateButton(p2)).toBeDisabled({ timeout: 500 });
    await expect(p0.locator('.circular-progress')).toBeVisible({ timeout: 500 });
    await expect(p1.locator('.circular-progress')).toBeVisible({ timeout: 500 });
    await expect(p2.locator('.circular-progress')).toBeVisible({ timeout: 500 });

    // Sanity: as the queue drains, each paragraph eventually completes.
    await expectTranslated(p0);
    await expectTranslated(p1);
    await expectTranslated(p2);
  });

  test('G2: translation completing on one paragraph does not blank peers (regression of 78d9b74)', async ({
    page,
  }) => {
    const s2 = [
      wordSegment({ flatIndex: 0, sentence: 0, word: 0, text: 'b', translation: 'B' }),
    ];
    const s3 = [
      wordSegment({ flatIndex: 0, sentence: 0, word: 0, text: 'c', translation: 'C' }),
    ];
    const { bookId } = await seedAndOpen(page, {
      chapters: [
        {
          paragraphs: [
            { html: 'h1' },
            { html: 'h2', segments: s2 },
            { html: 'h3', segments: s3 },
          ],
        },
      ],
    });
    await setTranslateConfig(page, bookId, 0, {
      kind: 'progress',
      steps: [
        { progress: 50, total: 100, delayMs: 80 },
        { progress: 100, total: 100, delayMs: 80 },
      ],
      segments: [
        wordSegment({
          flatIndex: 0,
          sentence: 0,
          word: 0,
          text: 'done',
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
          const span = wrapper.querySelector('.word-span');
          if (!span || !(span.textContent ?? '').trim()) {
            (window as any).__peerFlickered = true;
          }
        });
        obs.observe(wrapper, { childList: true, subtree: true, characterData: true });
      }
    });

    const p0 = paragraphLocator(page, 0);
    await translateButton(p0).click();
    await expectTranslated(p0);
    const flickered = await page.evaluate(() => (window as any).__peerFlickered);
    expect(flickered).toBe(false);
  });
});
