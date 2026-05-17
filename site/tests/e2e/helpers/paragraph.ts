import { expect, type Locator, type Page } from '@playwright/test';

export type SeedParagraph = {
  html: string;
  translation?: string;
  visibleWords?: number[];
};

export type TranslateConfig =
  | { kind: 'immediate'; translation?: string; visibleWords?: number[] }
  | {
      kind: 'progress';
      steps: Array<{ progress: number; total: number; delayMs: number }>;
      translation: string;
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
 * Wait until the paragraph shows translated content (the {:else} branch is
 * rendered — translate button has been removed and replaced by an empty div).
 */
export async function expectTranslated(paragraph: Locator): Promise<void> {
  await expect(paragraph.locator('button.translate')).toHaveCount(0);
}

/**
 * Build a word-span fragment matching what the real Rust translation_to_html
 * helper emits, with the attributes the frontend's word-click handler reads.
 */
export function wordSpanHtml(opts: {
  flatIndex: number;
  paragraph: number;
  sentence: number;
  word: number;
  text: string;
  translation: string;
}): string {
  return `<span class="word-span" data-flat-index="${opts.flatIndex}" data-paragraph="${opts.paragraph}" data-sentence="${opts.sentence}" data-word="${opts.word}" data-translation="${opts.translation}">${opts.text}</span>`;
}
