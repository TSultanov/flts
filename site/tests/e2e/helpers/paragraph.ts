import { expect, type Locator, type Page } from "@playwright/test";
import { isRealMode, realModeUnsupported } from "./backend-mode";
import { realSeedAndOpen, realTranslateCalls } from "./real-seed";

export type Mark = "emphasis" | "strong";

export type ParagraphSegment =
  | { kind: "gap"; text: string; marks?: Mark[] }
  | { kind: "break"; marks?: Mark[] }
  | {
      kind: "word";
      text: string;
      marks?: Mark[];
      sentence: number;
      word: number;
      flatIndex: number;
      translation: string | null;
    };

export type SeedParagraph = {
  html: string;
  segments?: ParagraphSegment[];
};

export type TranslateConfig =
  | { kind: "immediate"; segments?: ParagraphSegment[] }
  | {
      kind: "progress";
      steps: Array<{ progress: number; total: number; delayMs: number }>;
      segments: ParagraphSegment[];
    }
  | { kind: "error"; errorMessage: string; delayMs: number };

export type WordInfoSeed = {
  original: string;
  note?: string;
  isPunctuation?: boolean;
  contextualTranslations?: string[];
  fullSentenceTranslation?: string;
  translationModel?: number;
  sourceLanguage?: string;
  canCreateCard?: boolean;
  hasCard?: boolean;
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
  readingState?: {
    chapterId: number;
    paragraphId: number;
    pageOffset?: number;
  };
  summaryStatus?: {
    generated: boolean[];
    activelyGenerating?: number | null;
  };
};

let bookIdSeq = 0;

function makeBookId(): string {
  return `test-book-${Date.now()}-${++bookIdSeq}`;
}

/**
 * Seeds the backend and opens chapter 0; identical signature in both tiers
 * (real mode routes through real-seed.ts).
 *
 * The seed is re-applied via an init script on every load, since page.goto
 * wipes the mock module's in-memory state.
 */
export async function seedAndOpen(
  page: Page,
  spec: SeedSpec,
  opts: { path?: string } = {},
): Promise<{ bookId: string }> {
  page.on("pageerror", (err) => console.log("PAGE ERROR:", err.message));
  if (isRealMode()) return realSeedAndOpen(page, spec, opts);
  const bookId = spec.bookId ?? makeBookId();
  const fullSpec = { ...spec, bookId };

  await page.addInitScript((s) => {
    const wordInfoDefaults = (info: any) => ({
      original: info.original,
      note: info.note ?? "",
      isPunctuation: info.isPunctuation ?? false,
      contextualTranslations: info.contextualTranslations ?? [],
      fullSentenceTranslation: info.fullSentenceTranslation ?? "",
      translationModel: info.translationModel ?? 1,
      sourceLanguage: info.sourceLanguage ?? "eng",
      canCreateCard: info.canCreateCard ?? false,
      hasCard: info.hasCard ?? false,
      grammar: {
        originalInitialForm: info.grammar?.originalInitialForm ?? info.original,
        targetInitialForm: info.grammar?.targetInitialForm ?? "",
        partOfSpeech: info.grammar?.partOfSpeech ?? "noun",
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
      readingState: s.readingState,
      summaryStatus: s.summaryStatus,
    };
  }, fullSpec);

  await page.goto(opts.path ?? `/book/${bookId}/0`);
  return { bookId };
}

/** Sets a translate-config after mount. */
export async function setTranslateConfig(
  page: Page,
  bookId: string,
  paragraphId: number,
  cfg: TranslateConfig,
): Promise<void> {
  if (isRealMode()) realModeUnsupported("setTranslateConfig");
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
  if (isRealMode()) realModeUnsupported("setWordInfo");
  const full = {
    original: info.original,
    note: info.note ?? "",
    isPunctuation: info.isPunctuation ?? false,
    contextualTranslations: info.contextualTranslations ?? [],
    fullSentenceTranslation: info.fullSentenceTranslation ?? "",
    translationModel: info.translationModel ?? 1,
    sourceLanguage: info.sourceLanguage ?? "eng",
    grammar: {
      originalInitialForm: info.grammar?.originalInitialForm ?? info.original,
      targetInitialForm: info.grammar?.targetInitialForm ?? "",
      partOfSpeech: info.grammar?.partOfSpeech ?? "noun",
    },
  };
  await page.evaluate(
    ({ bookId, paragraphId, sentenceId, wordId, info }) => {
      (window as any).__test.setWordInfo(
        bookId,
        paragraphId,
        sentenceId,
        wordId,
        info,
      );
    },
    { bookId, paragraphId, sentenceId, wordId, info: full },
  );
}

export async function getTranslateCalls(page: Page): Promise<
  Array<{
    bookId: string;
    paragraphId: number;
    useCache: boolean;
    model: unknown;
  }>
> {
  if (isRealMode()) return realTranslateCalls();
  return page.evaluate(() => (window as any).__test.getTranslateCalls());
}

export async function getTranslationsBatchCalls(
  page: Page,
): Promise<Array<{ bookId: string; paragraphIds: number[]; at: number }>> {
  if (isRealMode()) realModeUnsupported("getTranslationsBatchCalls");
  return page.evaluate(() =>
    (window as any).__test.getTranslationsBatchCalls(),
  );
}

export function paragraphLocator(page: Page, paragraphId: number): Locator {
  return page.locator(`.paragraph-wrapper[data-paragraph-id="${paragraphId}"]`);
}

export function translateButton(paragraph: Locator): Locator {
  return paragraph.locator("button.translate");
}

export function wordSpan(paragraph: Locator, flatIndex: number): Locator {
  return paragraph.locator(`.word-span[data-flat-index="${flatIndex}"]`);
}

/**
 * Mirrors ChapterView.scrollParagraphIntoView (inline 'center', 'auto') so the
 * snap settles synchronously; the trailing wait lets the IO callback fire.
 */
export async function scrollToParagraph(
  page: Page,
  paragraphId: number,
): Promise<void> {
  // A chapter's rest is drawn over the frames after it shows; before
  // that a paragraph further on is not yet where it will be.
  await page.waitForSelector(".paragraphs-container.is-whole");
  await page.evaluate((id) => {
    const el = document.querySelector(
      `.paragraph-wrapper[data-paragraph-id="${id}"]`,
    );
    el?.scrollIntoView({
      behavior: "auto",
      block: "nearest",
      inline: "center",
    });
  }, paragraphId);
  await page.waitForTimeout(50);
}

/**
 * Sends `paragraphId` far enough from the reader to become a block, then
 * brings it back: what comes back is drawn afresh from the backend's HTML.
 */
export async function leaveAndReturn(
  page: Page,
  paragraphId: number,
): Promise<void> {
  await page.waitForSelector(".paragraphs-container.is-whole");
  const container = page.locator(".paragraphs-container");
  const target = paragraphLocator(page, paragraphId);
  const far = await container.evaluate((c, id) => {
    const frame = c.getBoundingClientRect();
    const rect = c
      .querySelector(`.paragraph-wrapper[data-paragraph-id="${id}"]`)!
      .getClientRects()[0];
    const column = Math.round(
      (rect.left - frame.left + c.scrollLeft) / frame.width,
    );
    const pages = Math.round(c.scrollWidth / frame.width);
    return column > pages / 2 ? 0 : pages - 1;
  }, paragraphId);
  await container.evaluate((c, column) => {
    c.scrollLeft = column * c.clientWidth;
  }, far);
  await expect(target).toHaveClass(/far/);
  await scrollToParagraph(page, paragraphId);
  await expect(target).not.toHaveClass(/far/);
}

/** Per-idx-stable sentence shape, so wrapped widths stay deterministic. */
export function htmlOfSize(idx: number, sentences: number): string {
  const sentence = `Paragraph ${idx} sentence about subject ${idx} doing thing ${idx} in place ${idx}.`;
  return Array.from({ length: sentences }, () => sentence).join(" ");
}

/**
 * ~15 sentences per paragraph, enough that the columnar layout yields real
 * horizontal scroll distance (>50 pages at 80 paragraphs).
 */
export function fillerHtml(idx: number): string {
  return htmlOfSize(idx, 15);
}

/** SeedSpec with N filler paragraphs; `overrides` merge in per paragraph. */
export function multipageSpec(
  count: number,
  overrides: Partial<Record<number, Partial<SeedParagraph>>> = {},
  extras: Omit<SeedSpec, "chapters"> = {},
): SeedSpec {
  const paragraphs: SeedParagraph[] = Array.from({ length: count }, (_, i) => ({
    html: fillerHtml(i),
    ...overrides[i],
  }));
  return { chapters: [{ paragraphs }], ...extras };
}

/** Waits for the translated branch (translate button replaced by an empty div). */
export async function expectTranslated(paragraph: Locator): Promise<void> {
  await expect(paragraph.locator("button.translate")).toHaveCount(0);
}

/** Asserts the paragraph is inside the lazy-mount window. */
export async function expectWordSpansMounted(
  page: Page,
  paragraphId: number,
): Promise<void> {
  await expect(
    paragraphLocator(page, paragraphId).locator(".word-span").first(),
  ).toBeAttached();
}

/** Asserts the unmounted fallback: wrapper in DOM, no WordSpans inside. */
export async function expectWordSpansUnmounted(
  page: Page,
  paragraphId: number,
): Promise<void> {
  await expect(
    paragraphLocator(page, paragraphId).locator(".word-span"),
  ).toHaveCount(0);
}

/** One word segment shaped as the Rust `paragraph_to_segments` emits. */
export function wordSegment(opts: {
  flatIndex: number;
  sentence: number;
  word: number;
  text: string;
  translation: string | null;
}): ParagraphSegment {
  return {
    kind: "word",
    text: opts.text,
    sentence: opts.sentence,
    word: opts.word,
    flatIndex: opts.flatIndex,
    translation: opts.translation,
  };
}

export async function setParagraphTranslation(
  page: Page,
  bookId: string,
  paragraphId: number,
  segments: ParagraphSegment[],
): Promise<void> {
  if (isRealMode()) realModeUnsupported("setParagraphTranslation");
  await page.evaluate(
    ({ bookId, paragraphId, segments }) => {
      (window as any).__test.setParagraphTranslation(
        bookId,
        paragraphId,
        segments,
      );
    },
    { bookId, paragraphId, segments },
  );
}

/**
 * Tokenizes `html` into segments the way the backend does, one per token.
 * The text is the same either way, so a paragraph's mounted and unmounted
 * renderings are comparable.
 */
export function segmentsOfHtml(
  html: string,
  translationOf: (token: string) => string | null = () => null,
): ParagraphSegment[] {
  const segments: ParagraphSegment[] = [];
  let flatIdx = 0;
  let sentenceIdx = 0;
  let wordIdx = 0;
  for (const token of html.split(/(\s+)/)) {
    if (token === "") continue;
    if (/^\s+$/.test(token)) {
      segments.push({ kind: "gap", text: token });
      continue;
    }
    segments.push(
      wordSegment({
        flatIndex: flatIdx++,
        sentence: sentenceIdx,
        word: wordIdx++,
        text: token,
        translation: translationOf(token),
      }),
    );
    if (/[.!?]$/.test(token)) {
      sentenceIdx++;
      wordIdx = 0;
    }
  }
  return segments;
}

/**
 * Tiles all of `fillerHtml(idx)`, one segment per token, as the backend does.
 * Keeps translated and untranslated widths comparable, which the lazy-mount
 * scroll-stability tests need.
 */
export function fillerSegments(idx: number): ParagraphSegment[] {
  return segmentsOfHtml(fillerHtml(idx));
}
