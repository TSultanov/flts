import { expect, type Locator, type Page } from '@playwright/test';

export type ParagraphSegment =
  | { kind: 'gap'; html: string }
  | {
      kind: 'word';
      text: string;
      sentence: number;
      word: number;
      flatIndex: number;
      translation: string | null;
    };

export type SeedParagraph = {
  html: string;
  segments?: ParagraphSegment[];
  visibleWords?: number[];
};

export type TranslateConfig =
  | { kind: 'immediate'; segments?: ParagraphSegment[]; visibleWords?: number[] }
  | {
      kind: 'progress';
      steps: Array<{ progress: number; total: number; delayMs: number }>;
      segments: ParagraphSegment[];
      visibleWords?: number[];
    }
  | { kind: 'error'; errorMessage: string; delayMs: number };

export type WordInfoSeed = {
  original: string;
  note?: string;
  isPunctuation?: boolean;
  contextualTranslations?: string[];
  fullSentenceTranslation?: string;
  translationModel?: number;
  sourceLanguage?: string;
  grammar?: {
    originalInitialForm?: string;
    targetInitialForm?: string;
    partOfSpeech?: string;
  };
};

export type SeedSpec = {
  bookId?: string;
  title?: string;
  chapters: Array<{
    title?: string;
    paragraphs: SeedParagraph[];
  }>;
  translateConfigs?: Array<{
    paragraphId: number;
    cfg: TranslateConfig;
  }>;
  inFlight?: Array<{
    paragraphId: number;
    requestId: number;
    cfg: TranslateConfig;
  }>;
  wordInfos?: Array<{
    paragraphId: number;
    sentenceId: number;
    wordId: number;
    info: WordInfoSeed;
  }>;
};

let bookIdSeq = 0;

function makeBookId(): string {
  return `test-book-${Date.now()}-${++bookIdSeq}`;
}

/**
 * Seed the mock backend and open the chapter at index 0.
 *
 * page.goto triggers a hard reload that wipes the mock module's in-memory
 * state. We install an init script that re-applies the seed on every page
 * load so the backend is populated by the time the chapter view mounts.
 */
export async function seedAndOpen(
  page: Page,
  spec: SeedSpec,
): Promise<{ bookId: string }> {
  page.on('pageerror', (err) => console.log('PAGE ERROR:', err.message));
  const bookId = spec.bookId ?? makeBookId();
  const fullSpec = { ...spec, bookId };

  // Stash the seed for the mock module to pick up synchronously on init.
  // The mock applies this before any invoke resolves, so Library Resources
  // see populated data on their first fetch.
  await page.addInitScript((s) => {
    const wordInfoDefaults = (info: any) => ({
      original: info.original,
      note: info.note ?? '',
      isPunctuation: info.isPunctuation ?? false,
      contextualTranslations: info.contextualTranslations ?? [],
      fullSentenceTranslation: info.fullSentenceTranslation ?? '',
      translationModel: info.translationModel ?? 1,
      sourceLanguage: info.sourceLanguage ?? 'eng',
      grammar: {
        originalInitialForm: info.grammar?.originalInitialForm ?? info.original,
        targetInitialForm: info.grammar?.targetInitialForm ?? '',
        partOfSpeech: info.grammar?.partOfSpeech ?? 'noun',
      },
    });
    (window as any).__pendingSeed = {
      bookId: s.bookId,
      title: s.title,
      chapters: s.chapters,
      translateConfigs: s.translateConfigs ?? [],
      inFlight: s.inFlight ?? [],
      wordInfos: (s.wordInfos ?? []).map((w: any) => ({
        paragraphId: w.paragraphId,
        sentenceId: w.sentenceId,
        wordId: w.wordId,
        info: wordInfoDefaults(w.info),
      })),
    };
  }, fullSpec);

  await page.goto(`/book/${bookId}/0`);
  return { bookId };
}

/**
 * Set a translate-config dynamically (post-navigation). Useful when the
 * test wants to configure the translation behavior after mount.
 */
export async function setTranslateConfig(
  page: Page,
  bookId: string,
  paragraphId: number,
  cfg: TranslateConfig,
): Promise<void> {
  await page.evaluate(
    ({ bookId, paragraphId, cfg }) => {
      (window as any).__test.setTranslateConfig(bookId, paragraphId, cfg);
    },
    { bookId, paragraphId, cfg },
  );
}

export async function setWordInfo(
  page: Page,
  bookId: string,
  paragraphId: number,
  sentenceId: number,
  wordId: number,
  info: WordInfoSeed,
): Promise<void> {
  const full = {
    original: info.original,
    note: info.note ?? '',
    isPunctuation: info.isPunctuation ?? false,
    contextualTranslations: info.contextualTranslations ?? [],
    fullSentenceTranslation: info.fullSentenceTranslation ?? '',
    translationModel: info.translationModel ?? 1,
    sourceLanguage: info.sourceLanguage ?? 'eng',
    grammar: {
      originalInitialForm: info.grammar?.originalInitialForm ?? info.original,
      targetInitialForm: info.grammar?.targetInitialForm ?? '',
      partOfSpeech: info.grammar?.partOfSpeech ?? 'noun',
    },
  };
  await page.evaluate(
    ({ bookId, paragraphId, sentenceId, wordId, info }) => {
      (window as any).__test.setWordInfo(bookId, paragraphId, sentenceId, wordId, info);
    },
    { bookId, paragraphId, sentenceId, wordId, info: full },
  );
}

export async function getTranslateCalls(
  page: Page,
): Promise<Array<{ bookId: string; paragraphId: number; useCache: boolean; model: unknown }>> {
  return page.evaluate(() => (window as any).__test.getTranslateCalls());
}

export async function getMarkWordVisibleCalls(
  page: Page,
): Promise<Array<{ bookId: string; paragraphId: number; flatIndex: number }>> {
  return page.evaluate(() => (window as any).__test.getMarkWordVisibleCalls());
}

export function paragraphLocator(page: Page, paragraphId: number): Locator {
  return page.locator(`.paragraph-wrapper[data-paragraph-id="${paragraphId}"]`);
}

export function translateButton(paragraph: Locator): Locator {
  return paragraph.locator('button.translate');
}

export function wordSpan(paragraph: Locator, flatIndex: number): Locator {
  return paragraph.locator(`.word-span[data-flat-index="${flatIndex}"]`);
}

/**
 * Scroll the chapter's horizontal page-flip container to the given paragraph.
 * Mirrors ChapterView.scrollParagraphIntoView (inline 'center', behavior 'auto')
 * so the snap settles synchronously in chromium. The trailing wait lets the
 * IntersectionObserver callback fire on the new viewport.
 */
export async function scrollToParagraph(page: Page, paragraphId: number): Promise<void> {
  await page.evaluate((id) => {
    const el = document.querySelector(
      `.paragraph-wrapper[data-paragraph-id="${id}"]`,
    );
    el?.scrollIntoView({ behavior: 'auto', block: 'nearest', inline: 'center' });
  }, paragraphId);
  await page.waitForTimeout(50);
}

/**
 * Deterministic ~15-sentence filler so each paragraph contributes meaningful
 * height and the columnar layout produces real horizontal scroll distance.
 * At 80 paragraphs this puts the chapter's scrollWidth / clientWidth well
 * over 50, matching a realistic long chapter.
 */
export function fillerHtml(idx: number): string {
  const sentence =
    `Paragraph ${idx} sentence about subject ${idx} doing thing ${idx} in place ${idx}.`;
  return Array.from({ length: 15 }, () => sentence).join(' ');
}

/**
 * Build a SeedSpec with N filler paragraphs. Per-paragraph overrides
 * (translation, visibleWords) merge in via `overrides`.
 */
export function multipageSpec(
  count: number,
  overrides: Partial<Record<number, Partial<SeedParagraph>>> = {},
  extras: Omit<SeedSpec, 'chapters'> = {},
): SeedSpec {
  const paragraphs: SeedParagraph[] = Array.from({ length: count }, (_, i) => ({
    html: fillerHtml(i),
    ...overrides[i],
  }));
  return { chapters: [{ paragraphs }], ...extras };
}

/**
 * Wait until the paragraph shows translated content (the {:else} branch is
 * rendered — translate button has been removed and replaced by an empty div).
 */
export async function expectTranslated(paragraph: Locator): Promise<void> {
  await expect(paragraph.locator('button.translate')).toHaveCount(0);
}

/**
 * Assert the paragraph currently renders WordSpans (i.e. lazy-mount window
 * includes it).
 */
export async function expectWordSpansMounted(
  page: Page,
  paragraphId: number,
): Promise<void> {
  await expect(
    paragraphLocator(page, paragraphId).locator('.word-span').first(),
  ).toBeAttached();
}

/**
 * Assert the paragraph is in the unmounted fallback (no WordSpan descendants).
 * The wrapper itself is still in DOM — only the segment-based inner render is
 * gone.
 */
export async function expectWordSpansUnmounted(
  page: Page,
  paragraphId: number,
): Promise<void> {
  await expect(
    paragraphLocator(page, paragraphId).locator('.word-span'),
  ).toHaveCount(0);
}

/**
 * Build a single word segment matching what the real Rust paragraph_to_segments
 * helper emits, for use inside a SeedParagraph.segments array.
 */
export function wordSegment(opts: {
  flatIndex: number;
  sentence: number;
  word: number;
  text: string;
  translation: string | null;
}): ParagraphSegment {
  return {
    kind: 'word',
    text: opts.text,
    sentence: opts.sentence,
    word: opts.word,
    flatIndex: opts.flatIndex,
    translation: opts.translation,
  };
}

/**
 * Build a segments array that tiles the entire fillerHtml(idx) source text,
 * one word-segment per whitespace-delimited token. This mirrors production,
 * where the backend's paragraph_to_segments emits a segment for every word
 * in the original, so the rendered widths of the translated and untranslated
 * branches stay roughly the same — a precondition for lazy-mount tests that
 * exercise scroll stability across mount/unmount transitions.
 */
export function fillerSegments(idx: number): ParagraphSegment[] {
  const html = fillerHtml(idx);
  const segments: ParagraphSegment[] = [];
  let flatIdx = 0;
  let sentenceIdx = 0;
  let wordIdx = 0;
  const tokens = html.split(/(\s+)/);
  for (const token of tokens) {
    if (token === '') continue;
    if (/^\s+$/.test(token)) {
      segments.push({ kind: 'gap', html: token });
    } else {
      segments.push(
        wordSegment({
          flatIndex: flatIdx++,
          sentence: sentenceIdx,
          word: wordIdx++,
          text: token,
          translation: null,
        }),
      );
      if (/[.!?]$/.test(token)) {
        sentenceIdx++;
        wordIdx = 0;
      }
    }
  }
  return segments;
}
