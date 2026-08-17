// Real-tier implementation of the seed helpers: instead of stuffing a mock
// store, import the book through the real backend and script the LLM sim so
// the app's own translation pipeline produces the expected segments.
import type { Page } from '@playwright/test';
import { realModeUnsupported } from './backend-mode';
import { getHarness } from '../../real/harness-registry';
import type { ParagraphSegment, SeedSpec, TranslateConfig } from './paragraph';

/** Gemini's compact translation schema (library/src/book/translation_import.rs). */
type SimWord = {
  o: string;
  t: string[];
  n: string | null;
  p: boolean;
  g: {
    lf: string;
    lt: string;
    pos: string;
    pl: null;
    pe: null;
    te: null;
    ca: null;
    ot: null;
  };
};

/** Paragraph text per id, so `getTranslateCalls` can attribute sim requests. */
type SeedIndex = {
  bookId: string;
  /** Longest text first: "Paragraph 1" must not swallow "Paragraph 10". */
  texts: Array<[number, string]>;
  /** Requests the seed itself made; the mock's call log excludes them. */
  seedRequests: number;
};

let lastSeed: SeedIndex | undefined;

/** Paragraphs are one-per-line for the importer, so newlines cannot survive. */
function toPlainText(html: string): string {
  return html
    .replace(/<[^>]*>/g, '')
    .replace(/&nbsp;/g, ' ')
    .replace(/&amp;/g, '&')
    .replace(/&lt;/g, '<')
    .replace(/&gt;/g, '>')
    .replace(/\s+/g, ' ')
    .trim();
}

function wordsOf(segments: ParagraphSegment[]): SimWord[] {
  return segments
    .filter((s): s is Extract<ParagraphSegment, { kind: 'word' }> => s.kind === 'word')
    .map((s) => ({
      o: s.text,
      t: s.translation ? [s.translation] : [],
      n: null,
      p: false,
      g: {
        lf: s.text,
        lt: s.translation ?? '',
        pos: 'common_noun',
        pl: null,
        pe: null,
        te: null,
        ca: null,
        ot: null,
      },
    }));
}

/** One word per whitespace token when the spec left segments implicit. */
function defaultSegments(text: string): ParagraphSegment[] {
  let flatIndex = 0;
  return text
    .split(/\s+/)
    .filter(Boolean)
    .map((token) => ({
      kind: 'word' as const,
      text: token,
      sentence: 0,
      word: flatIndex,
      flatIndex: flatIndex++,
      translation: `t-${token}`,
    }));
}

function translationJson(segments: ParagraphSegment[]): unknown {
  const bySentence = new Map<number, ParagraphSegment[]>();
  for (const s of segments) {
    if (s.kind !== 'word') continue;
    const bucket = bySentence.get(s.sentence);
    if (bucket) bucket.push(s);
    else bySentence.set(s.sentence, [s]);
  }
  const sentences = [...bySentence.keys()].sort((a, b) => a - b);
  return {
    s: sentences.map((idx) => ({
      ft: `full-${idx}`,
      wl: wordsOf(bySentence.get(idx)!),
    })),
  };
}

/**
 * The sim matches `matchSubstring` against the raw serialized request body, so
 * quotes/backslashes/newlines in the needle can never match. Paragraph text is
 * plain by construction; this is the guard against a spec that isn't.
 */
function assertMatchable(text: string): string {
  if (/["\\\n\r]/.test(text)) {
    throw new Error(`real mode: paragraph text is not sim-matchable: ${text}`);
  }
  return text;
}

function rejectUnsupported(spec: SeedSpec): void {
  const fields = [
    'inFlight',
    'wordInfos',
    'readingState',
    'summaryStatus',
    'config',
  ] as const;
  for (const field of fields) {
    if (spec[field] !== undefined) realModeUnsupported(field);
  }
}

async function importBook(
  spec: SeedSpec,
  chapters: Array<{ title?: string; paragraphs: string[] }>,
): Promise<string> {
  const harness = getHarness();
  const title = spec.title ?? 'E2E Book';
  // create_book_plain makes exactly one untitled chapter; anything richer has
  // to go through the epub importer.
  const plain =
    chapters.length === 1 && chapters[0].title === undefined;
  if (plain) {
    return harness.invoke<string>('import_plain_text', {
      title,
      text: chapters[0].paragraphs.join('\n'),
      sourceLanguageId: 'deu',
    });
  }
  return harness.invoke<string>('import_epub', {
    book: {
      title,
      chapters: chapters.map((c, i) => ({
        title: c.title ?? `Chapter ${i + 1}`,
        paragraphs: c.paragraphs.map((p) => ({ text: p, html: p })),
      })),
    },
    sourceLanguageId: 'deu',
  });
}

async function waitForTranslations(
  bookId: string,
  paragraphIds: number[],
): Promise<void> {
  const harness = getHarness();
  const deadline = Date.now() + 30_000;
  for (;;) {
    const rows = await harness.invoke<Array<{ id: number; segments?: unknown }>>(
      'get_paragraph_translations_batch',
      { bookId, paragraphIds },
    );
    const done = new Set(rows.filter((r) => r.segments).map((r) => r.id));
    if (paragraphIds.every((id) => done.has(id))) return;
    if (Date.now() > deadline) {
      throw new Error(
        `real seed: translations never landed for ${paragraphIds.filter(
          (id) => !done.has(id),
        )}`,
      );
    }
    await new Promise((r) => setTimeout(r, 50));
  }
}

export async function realSeedAndOpen(
  page: Page,
  spec: SeedSpec,
  opts: { path?: string } = {},
): Promise<{ bookId: string }> {
  rejectUnsupported(spec);
  const harness = getHarness();

  const chapters = spec.chapters.map((c) => ({
    title: c.title,
    paragraphs: c.paragraphs.map((p) => assertMatchable(toPlainText(p.html))),
  }));

  const texts = new Map<number, string>();
  let pid = 0;
  const flatSegments: Array<ParagraphSegment[] | undefined> = [];
  for (const [ci, chapter] of chapters.entries()) {
    for (const [pi, text] of chapter.paragraphs.entries()) {
      texts.set(pid++, text);
      flatSegments.push(spec.chapters[ci].paragraphs[pi].segments);
    }
  }

  const bookId = await importBook(spec, chapters);


  // Pre-translated paragraphs: run them through the real translate pipeline
  // before the page opens, so the view loads with segments already stored.
  const preTranslated: number[] = [];
  const scripts: Array<{ matchSubstring: string; translation: unknown }> = [];
  for (const [id, segments] of flatSegments.entries()) {
    if (!segments) continue;
    const text = texts.get(id)!;
    scripts.push({ matchSubstring: text, translation: translationJson(segments) });
    preTranslated.push(id);
  }

  const errorRules: Array<{ text: string }> = [];
  for (const { paragraphId, cfg } of spec.translateConfigs ?? []) {
    const text = texts.get(paragraphId);
    if (text === undefined) continue;
    applyConfig(scripts, errorRules, text, cfg);
  }

  if (scripts.length) await harness.llm.seed({ scripts });
  // translate_paragraph only enqueues; the page must not open until the
  // segments are actually stored, or the view races the seed.
  for (const paragraphId of preTranslated) {
    await harness.invoke('translate_paragraph', {
      bookId,
      paragraphId,
      model: 1,
      useCache: false,
    });
  }
  if (preTranslated.length) await waitForTranslations(bookId, preTranslated);
  for (const { text } of errorRules) {
    await harness.llm.addRule({
      matcher: { bodyContains: text },
      action: { type: 'status', code: 500, body: { error: 'sim failure' } },
    });
  }

  lastSeed = {
    bookId,
    texts: [...texts].sort((a, b) => b[1].length - a[1].length),
    seedRequests: (await harness.llm.requests()).length,
  };

  await page.goto(opts.path ?? `/book/${bookId}/0`);
  return { bookId };
}

function applyConfig(
  scripts: Array<{ matchSubstring: string; translation: unknown }>,
  errorRules: Array<{ text: string }>,
  text: string,
  cfg: TranslateConfig,
): void {
  if (cfg.kind === 'error') {
    errorRules.push({ text });
    return;
  }
  const segments = cfg.segments ?? defaultSegments(text);
  scripts.push({ matchSubstring: text, translation: translationJson(segments) });
}

/**
 * Reconstructed from the LLM sim's request log. Only paragraph translation
 * streams (`:streamGenerateContent`); chapter summaries quote paragraph text
 * too, so the path is what separates them. Seed requests are sliced off to
 * match the mock's post-seed-only log. `model` is not on the wire.
 */
export async function realTranslateCalls(): Promise<
  Array<{ bookId: string; paragraphId: number; useCache: boolean; model: unknown }>
> {
  const harness = getHarness();
  const seed = lastSeed;
  const reqs = (await harness.llm.requests()).slice(seed?.seedRequests ?? 0);
  return reqs
    .filter((r) => r.path.endsWith(':streamGenerateContent'))
    .map((r) => ({
      bookId: seed?.bookId ?? '',
      paragraphId:
        seed?.texts.find(([, text]) => r.body.includes(text))?.[0] ?? -1,
      useCache: r.body.includes('cachedContent'),
      model: null,
    }));
}
