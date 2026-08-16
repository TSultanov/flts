import { test, expect } from '../../real/fixtures';
import type { Locator, Page } from '@playwright/test';
import {
  expectTranslated,
  paragraphLocator,
  seedAndOpen,
  translateButton,
  wordSegment,
  type ParagraphSegment,
} from '../helpers/paragraph';
import { getHarness } from '../../real/harness-registry';

/**
 * Translation results are cached on disk by source text, and that cache dir
 * outlives the per-test config dir. Any spec that asserts LLM traffic or
 * re-translation needs textually unique paragraphs.
 */
function nonceText(): string {
  const n = `${Date.now().toString(36)}${Math.random().toString(36).slice(2, 8)}`;
  return `Der ${n} Vogel singt heute Morgen`;
}

function segmentsOf(text: string): ParagraphSegment[] {
  return text.split(' ').map((token, i) =>
    wordSegment({
      flatIndex: i,
      sentence: 0,
      word: i,
      text: token,
      translation: `t${i}`,
    }),
  );
}

/** Streaming translation traffic only; summaries are unary `:generateContent`. */
const STREAM_GLOB = '*streamGenerateContent*';

async function streamCallsFor(text: string): Promise<number> {
  const reqs = await getHarness().llm.requests();
  return reqs.filter(
    (r) => r.path.endsWith(':streamGenerateContent') && r.body.includes(text),
  ).length;
}

/** Seed one chapter-0 paragraph with a scripted translation, ready to click. */
async function seedOneParagraph(page: Page, text: string) {
  const seeded = await seedAndOpen(page, {
    chapters: [{ paragraphs: [{ html: text }] }],
    translateConfigs: [
      { paragraphId: 0, cfg: { kind: 'immediate', segments: segmentsOf(text) } },
    ],
  });
  const p = paragraphLocator(page, 0);
  const btn = translateButton(p);
  // Gated on chapter summaries being ready.
  await expect(btn).toBeEnabled({ timeout: 30_000 });
  return { ...seeded, p, btn };
}

function collectWarnings(page: Page): string[] {
  const warnings: string[] = [];
  page.on('console', (msg) => {
    if (msg.type() === 'warning' || msg.type() === 'warn') warnings.push(msg.text());
  });
  return warnings;
}

/** The failure affordance ParagraphView actually has: spinner gone, button back. */
async function expectFailedBack(p: Locator, text: string) {
  const btn = translateButton(p);
  await expect(btn).toBeEnabled({ timeout: 60_000 });
  await expect(p.locator('.circular-progress')).toHaveCount(0);
  await expect(p.locator('.word-span')).toHaveCount(0);
  await expect(p.locator('.original')).toHaveText(text);
}

test.describe('LLM failure injection', () => {
  test('5xx twice then success: the queue retries and the paragraph translates', async ({
    page,
    harness,
  }) => {
    const text = nonceText();
    const { p, btn } = await seedOneParagraph(page, text);

    await harness.llm.addRule({
      matcher: { pathGlob: STREAM_GLOB },
      action: { type: 'status', code: 503, body: { error: 'sim overloaded' } },
      times: 2,
    });

    await btn.click();
    await expectTranslated(p);
    await expect(p.locator('.word-span').first()).toBeAttached();

    // Two rejected attempts plus the one that stuck.
    expect(await streamCallsFor(text)).toBeGreaterThanOrEqual(3);
  });

  test('malformed JSON surfaces a failure and leaves the app usable', async ({
    page,
    harness,
  }) => {
    const warnings = collectWarnings(page);
    const text = nonceText();
    const { bookId, p, btn } = await seedOneParagraph(page, text);

    await harness.llm.addRule({
      matcher: { pathGlob: STREAM_GLOB },
      action: { type: 'corrupt', mode: 'malformed_json' },
    });

    await btn.click();
    await expectFailedBack(p, text);
    await expect
      .poll(() => warnings.some((w) => w.includes('Translation failed for paragraph 0')), {
        timeout: 30_000,
      })
      .toBe(true);

    // Still responsive: away and back re-renders the chapter.
    await page.goto('/');
    await page.goto(`/book/${bookId}/0`);
    const p2 = paragraphLocator(page, 0);
    await expect(p2.locator('.original')).toHaveText(text);
    await expect(translateButton(p2)).toBeEnabled({ timeout: 30_000 });
  });

  test('a stalled stream holds the in-progress state and recovers after reset', async ({
    page,
    harness,
  }) => {
    const text = nonceText();
    const { p, btn } = await seedOneParagraph(page, text);

    await harness.llm.addRule({
      matcher: { pathGlob: STREAM_GLOB },
      action: { type: 'stall' },
    });

    await btn.click();
    await expect(btn).toBeDisabled();
    await expect(p.locator('.circular-progress')).toHaveCount(1);

    // Backend request/idle timeouts are 1200s/180s, so nothing can resolve
    // this on its own — 5s is purely "the UI does not fall over".
    await page.waitForTimeout(5000);
    await expect(btn).toBeDisabled();
    await expect(p.locator('.circular-progress')).toHaveCount(1);
    expect(await page.evaluate(() => document.title)).toBeTruthy();

    // reset is the only stall release; it also wipes scripts, so re-seed.
    await harness.llm.reset();
    await harness.llm.seed({
      scripts: [{ matchSubstring: text, translation: translationJson(text) }],
    });

    // The released request retries on its own; if it lost the race with the
    // re-seed it terminates and re-enables the button, so click again.
    await expect
      .poll(
        async () => {
          if ((await translateButton(p).count()) === 0) return true;
          if (await translateButton(p).isEnabled()) {
            await translateButton(p).click().catch(() => {});
          }
          return false;
        },
        { timeout: 60_000, intervals: [250] },
      )
      .toBe(true);
    await expectTranslated(p);
    await expect(p.locator('.word-span').first()).toBeAttached();
  });

  test('a truncated stream fails without rendering partial words', async ({
    page,
    harness,
  }) => {
    const warnings = collectWarnings(page);
    const text = nonceText();
    const { p, btn } = await seedOneParagraph(page, text);

    await harness.llm.addRule({
      matcher: { pathGlob: STREAM_GLOB },
      action: { type: 'truncate', fraction: 0.5 },
    });

    await btn.click();
    await expectFailedBack(p, text);
    await expect
      .poll(() => warnings.some((w) => w.includes('Translation failed for paragraph 0')), {
        timeout: 30_000,
      })
      .toBe(true);
  });

  test('a failing re-translate never drops the stored translation', async ({
    page,
    harness,
  }) => {
    const text = nonceText();
    const { bookId, p, btn } = await seedOneParagraph(page, text);

    await btn.click();
    await expectTranslated(p);
    const wordCount = await p.locator('.word-span').count();
    expect(wordCount).toBeGreaterThan(0);

    await harness.llm.addRule({
      matcher: { pathGlob: STREAM_GLOB },
      action: { type: 'status', code: 500, body: { error: 'sim down' } },
    });

    // No UI affordance re-translates an already-translated paragraph (the
    // button is gone once segments exist), so drive the command directly.
    const before = await streamCallsFor(text);
    await harness.invoke('translate_paragraph', {
      bookId,
      paragraphId: 0,
      model: 1,
      useCache: false,
    });
    await expect
      .poll(() => streamCallsFor(text), { timeout: 60_000 })
      .toBeGreaterThan(before);

    // The old translation survives the failed pass, in the live view...
    await expect(p.locator('.word-span')).toHaveCount(wordCount);
    // ...and on disk.
    await page.reload();
    const p2 = paragraphLocator(page, 0);
    await expectTranslated(p2);
    await expect(p2.locator('.word-span')).toHaveCount(wordCount);
  });
});

/** Gemini's compact translation schema (library/src/book/translation_import.rs). */
function translationJson(text: string): unknown {
  return {
    s: [
      {
        ft: 'full-0',
        wl: text.split(' ').map((token, i) => ({
          o: token,
          t: [`t${i}`],
          n: null,
          p: false,
          g: {
            lf: token,
            lt: `t${i}`,
            pos: 'common_noun',
            pl: null,
            pe: null,
            te: null,
            ca: null,
            ot: null,
          },
        })),
      },
    ],
  };
}
