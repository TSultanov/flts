/**
 * Mock implementation of @tauri-apps/api/core for Playwright tests.
 * Provides stateful mocks that maintain data between operations.
 */

// Types (matching the app's type definitions)
type UUID = string;

type Language = {
  id: string;
  name: string;
  localName?: string;
};

type TranslationProvider = 'google' | 'openai';

type ProviderMeta = {
  id: TranslationProvider;
  name: string;
  defaultModelId: number;
  apiKeyField: 'geminiApiKey' | 'openaiApiKey';
};

type Model = {
  id: number;
  name: string;
  provider?: TranslationProvider;
};

type Config = {
  targetLanguageId?: string;
  translationProvider: TranslationProvider;
  geminiApiKey?: string;
  openaiApiKey?: string;
  model: number;
  libraryPath?: string;
};

type MockBook = {
  id: UUID;
  title: string;
  chaptersCount: number;
  paragraphsCount: number;
  translationRatio: number;
  path: string[];
  chapters: MockChapter[];
  // Global paragraph storage keyed by global paragraph id. Matches the real
  // backend, where paragraph ids are unique across the whole book — not
  // per-chapter — so multi-chapter EPUB tests resolve to the right content.
  paragraphsById: Map<number, MockParagraph>;
};

type MockChapter = {
  title: string;
  // Global paragraph ids of the paragraphs in this chapter, in order.
  paragraphIds: number[];
};

type ParagraphSegment =
  | { kind: 'gap'; html: string }
  | {
      kind: 'word';
      text: string;
      sentence: number;
      word: number;
      flatIndex: number;
      translation: string | null;
    };

type MockParagraph = {
  html: string;
  segments?: ParagraphSegment[];
  visibleWords?: number[];
};

type ChapterMetaView = {
  id: number;
  title: string;
};

type ParagraphView = {
  id: number;
  original: string;
  segments?: ParagraphSegment[];
  visibleWords: number[];
};

type BookReadingState = {
  chapterId: number;
  paragraphId: number;
  pageOffset: number;
};

// ----- Translation simulation types --------------------------------------

type ParagraphTranslationActivity = {
  requestId: number;
  progressChars: number;
  expectedChars: number;
};

type ProgressStep = {
  progress: number;
  total: number;
  delayMs: number;
};

export type TranslateConfig =
  | { kind: 'immediate'; segments?: ParagraphSegment[]; visibleWords?: number[] }
  | {
      kind: 'progress';
      steps: ProgressStep[];
      segments: ParagraphSegment[];
      visibleWords?: number[];
    }
  | { kind: 'error'; errorMessage: string; delayMs: number };

type WordInfo = {
  original: string;
  note: string;
  isPunctuation: boolean;
  contextualTranslations: string[];
  fullSentenceTranslation: string;
  translationModel: number;
  sourceLanguage: string;
  grammar: {
    originalInitialForm: string;
    targetInitialForm: string;
    partOfSpeech: string;
    plurality?: string;
    person?: string;
    tense?: string;
    case?: string;
    other?: string;
  };
};

// ----- Lyrics mode types --------------------------------------------------

type PlayerState = 'playing' | 'paused' | 'stopped' | 'notrunning';

type NowPlaying = {
  state: PlayerState;
  trackId?: string;
  name?: string;
  artist?: string;
  album?: string;
  positionMs?: number;
  durationMs?: number;
};

type LyricsLine = { time_ms: number | null; text: string };
type Lyrics = { track_id: string; lines: LyricsLine[]; synced: boolean };
type Gloss = { fragment: string; gloss: string; note: string };
type LyricsLineTranslation = { translation: string; glosses: Gloss[] };
type LyricsTranslation = {
  track_id: string;
  target_lang: string;
  model: number;
  lines: LyricsLineTranslation[];
};

// Mock state
let mockLibrary: Map<UUID, MockBook> = new Map();
let mockConfig: Config = {
  model: 0,
  translationProvider: 'google',
  geminiApiKey: 'mock-api-key-for-testing',
  openaiApiKey: 'mock-openai-key-for-testing',
  libraryPath: '/mock/library/path',
  targetLanguageId: 'spa',
};
let mockReadingStates: Map<UUID, BookReadingState> = new Map();
let bookIdCounter = 0;
let requestIdCounter = 0;

// ----- Translation simulation state --------------------------------------

const DEFAULT_TRANSLATE_CONFIG: TranslateConfig = { kind: 'immediate' };

// Keyed by `${bookId}:${paragraphId}`
const translateConfigs = new Map<string, TranslateConfig>();
const activeActivities = new Map<string, ParagraphTranslationActivity>();
const wordInfos = new Map<string, WordInfo>();

const translateCalls: Array<{
  bookId: UUID;
  paragraphId: number;
  useCache: boolean;
  model: unknown;
}> = [];
const markWordVisibleCalls: Array<{
  bookId: UUID;
  paragraphId: number;
  flatIndex: number;
}> = [];

function paragraphKey(bookId: UUID, paragraphId: number): string {
  return `${bookId}:${paragraphId}`;
}

function wordKey(
  bookId: UUID,
  paragraphId: number,
  sentenceId: number,
  wordId: number,
): string {
  return `${bookId}:${paragraphId}:${sentenceId}:${wordId}`;
}

function applyTranslationCompletion(
  bookId: UUID,
  paragraphId: number,
  segments: ParagraphSegment[],
  visibleWords?: number[],
): void {
  const book = mockLibrary.get(bookId);
  if (!book) return;
  const p = book.paragraphsById.get(paragraphId);
  if (!p) return;
  p.segments = segments;
  if (visibleWords) p.visibleWords = visibleWords;
  emit('paragraph_updated', { bookId, paragraphId });
}

function emitStarted(
  bookId: UUID,
  paragraphId: number,
  requestId: number,
  expectedChars: number,
): void {
  emit('paragraph_translation_started', {
    bookId,
    paragraphId,
    requestId,
    expectedChars,
  });
}

function emitProgress(
  bookId: UUID,
  paragraphId: number,
  requestId: number,
  progressChars: number,
  expectedChars: number,
): void {
  emit('paragraph_translation_progress', {
    bookId,
    paragraphId,
    requestId,
    progressChars,
    expectedChars,
  });
}

function emitFinished(
  bookId: UUID,
  paragraphId: number,
  requestId: number,
  error: string | null,
): void {
  emit('paragraph_translation_finished', {
    bookId,
    paragraphId,
    requestId,
    error,
  });
}

// Mirror the real backend's single-worker queue. Translation requests get an
// activity record + a "started" event synchronously at translate-time so the
// UI can flip the button into spinner state for every clicked paragraph, even
// the ones that won't actually begin running until the worker drains earlier
// items. The actual progress/finished events fire serially as the worker
// pulls each item off this queue.
const translationWorkQueue: Array<() => Promise<void>> = [];
let translationWorkerBusy = false;

async function drainTranslationWorkQueue(): Promise<void> {
  if (translationWorkerBusy) return;
  translationWorkerBusy = true;
  try {
    while (translationWorkQueue.length > 0) {
      const work = translationWorkQueue.shift();
      if (work) {
        try {
          await work();
        } catch {
          // Swallow — mock-side bookkeeping errors shouldn't stall the queue.
        }
      }
    }
  } finally {
    translationWorkerBusy = false;
  }
}

function enqueueTranslationWork(work: () => Promise<void>): void {
  translationWorkQueue.push(work);
  void drainTranslationWorkQueue();
}

function sleep(ms: number): Promise<void> {
  return new Promise((resolve) => setTimeout(resolve, ms));
}

function runTranslateRequest(
  requestId: number,
  bookId: UUID,
  paragraphId: number,
  cfg: TranslateConfig,
): void {
  const key = paragraphKey(bookId, paragraphId);

  // The fix for the multi-click bug: announce activity at enqueue, not when
  // the worker picks up the request. The first progress event later carries
  // the real expectedChars.
  activeActivities.set(key, {
    requestId,
    progressChars: 0,
    expectedChars: 0,
  });
  emitStarted(bookId, paragraphId, requestId, 0);

  enqueueTranslationWork(() =>
    runTranslationWork(requestId, bookId, paragraphId, key, cfg),
  );
}

async function runTranslationWork(
  requestId: number,
  bookId: UUID,
  paragraphId: number,
  key: string,
  cfg: TranslateConfig,
): Promise<void> {
  if (cfg.kind === 'immediate') {
    await sleep(100);
    if (cfg.segments !== undefined) {
      applyTranslationCompletion(
        bookId,
        paragraphId,
        cfg.segments,
        cfg.visibleWords,
      );
    } else {
      emit('paragraph_updated', { bookId, paragraphId });
    }
    activeActivities.delete(key);
    emitFinished(bookId, paragraphId, requestId, null);
    return;
  }

  if (cfg.kind === 'error') {
    activeActivities.set(key, {
      requestId,
      progressChars: 0,
      expectedChars: 100,
    });
    emitProgress(bookId, paragraphId, requestId, 0, 100);
    await sleep(cfg.delayMs);
    activeActivities.delete(key);
    emitFinished(bookId, paragraphId, requestId, cfg.errorMessage);
    return;
  }

  // progress: emit each step in order, with the step's delay following the
  // emit. The final step's delay precedes completion + finished.
  for (const step of cfg.steps) {
    activeActivities.set(key, {
      requestId,
      progressChars: step.progress,
      expectedChars: step.total,
    });
    emitProgress(bookId, paragraphId, requestId, step.progress, step.total);
    await sleep(step.delayMs);
  }

  applyTranslationCompletion(
    bookId,
    paragraphId,
    cfg.segments,
    cfg.visibleWords,
  );
  activeActivities.delete(key);
  emitFinished(bookId, paragraphId, requestId, null);
}

// ----- Lyrics mode state --------------------------------------------------

let mockNowPlaying: NowPlaying | null = null;
let mockLyricsByTrack: Map<string, Lyrics | null> = new Map();
let mockTranslationCache: Map<string, LyricsTranslation> = new Map();

function translationKey(trackId: string, target: string, model: number): string {
  return `${trackId}|${target}|${model}`;
}

/**
 * Build a MockBook whose paragraphs are stored under globally unique ids, so
 * `get_paragraph_view(paragraph_id)` and `get_book_chapter_paragraph_ids` use
 * the same id space as the real backend.
 */
function buildBookFromChapters(
  id: UUID,
  title: string,
  chapters: Array<{
    title: string;
    paragraphs: Array<{
      html: string;
      segments?: ParagraphSegment[];
      visibleWords?: number[];
    }>;
  }>,
): MockBook {
  const paragraphsById = new Map<number, MockParagraph>();
  let nextParagraphId = 0;
  const mockChapters: MockChapter[] = chapters.map((c) => {
    const paragraphIds: number[] = [];
    for (const p of c.paragraphs) {
      const pid = nextParagraphId++;
      paragraphsById.set(pid, {
        html: p.html,
        segments: p.segments,
        visibleWords: p.visibleWords,
      });
      paragraphIds.push(pid);
    }
    return { title: c.title, paragraphIds };
  });
  return {
    id,
    title,
    chaptersCount: mockChapters.length,
    paragraphsCount: paragraphsById.size,
    translationRatio: 0,
    path: [],
    chapters: mockChapters,
    paragraphsById,
  };
}

// Dispatch a mock event through the shared `tauri-event.ts` bus so that the
// app's `listen(...)` subscribers actually receive it. Without this, emits
// from mock command handlers would land in a private map that the app never
// touches — the test infra was previously two disconnected event buses.
function emit(event: string, payload: unknown) {
  const dispatch = (window as any).__tauriEmit as
    | ((e: string, p?: unknown) => void)
    | undefined;
  dispatch?.(event, payload);
}

// Reset state between tests
export function resetMockState() {
  mockLibrary.clear();
  mockConfig = {
    model: 0,
    translationProvider: 'google',
    geminiApiKey: 'mock-api-key-for-testing',
    openaiApiKey: 'mock-openai-key-for-testing',
    libraryPath: '/mock/library/path',
    targetLanguageId: 'spa',
  };
  mockReadingStates.clear();
  bookIdCounter = 0;
  requestIdCounter = 0;
  translateConfigs.clear();
  activeActivities.clear();
  wordInfos.clear();
  translateCalls.length = 0;
  markWordVisibleCalls.length = 0;
  translationWorkQueue.length = 0;
  translationWorkerBusy = false;
  mockNowPlaying = null;
  mockLyricsByTrack.clear();
  mockTranslationCache.clear();
}

type PendingSeed = {
  bookId: string;
  title?: string;
  chapters: Array<{
    title?: string;
    paragraphs: Array<{
      html: string;
      segments?: ParagraphSegment[];
      visibleWords?: number[];
    }>;
  }>;
  translateConfigs?: Array<{ paragraphId: number; cfg: TranslateConfig }>;
  inFlight?: Array<{ paragraphId: number; requestId: number; cfg: TranslateConfig }>;
  wordInfos?: Array<{
    paragraphId: number;
    sentenceId: number;
    wordId: number;
    info: WordInfo;
  }>;
  readingState?: { chapterId: number; paragraphId: number; pageOffset?: number };
};

function applyPendingSeed(seed: PendingSeed): void {
  resetMockState();
  const book = buildBookFromChapters(
    seed.bookId,
    seed.title ?? 'Test Book',
    seed.chapters.map((c, idx) => ({
      title: c.title ?? `Chapter ${idx + 1}`,
      paragraphs: c.paragraphs,
    })),
  );
  mockLibrary.set(seed.bookId, book);
  for (const tc of seed.translateConfigs ?? []) {
    translateConfigs.set(paragraphKey(seed.bookId, tc.paragraphId), tc.cfg);
  }
  for (const inf of seed.inFlight ?? []) {
    runTranslateRequest(inf.requestId, seed.bookId, inf.paragraphId, inf.cfg);
  }
  for (const w of seed.wordInfos ?? []) {
    wordInfos.set(
      wordKey(seed.bookId, w.paragraphId, w.sentenceId, w.wordId),
      w.info,
    );
  }
  if (seed.readingState) {
    mockReadingStates.set(seed.bookId, {
      chapterId: seed.readingState.chapterId,
      paragraphId: seed.readingState.paragraphId,
      pageOffset: seed.readingState.pageOffset ?? 0,
    });
  }
}

// Expose reset function globally for tests
if (typeof window !== 'undefined') {
  (window as any).__resetTauriMock = resetMockState;

  // Apply any seed that Playwright stashed via addInitScript before the app
  // booted. This runs synchronously during mock module init, before any
  // invoke() call resolves, so Library.* Resources see populated data on
  // their very first fetch.
  const pending = (window as any).__pendingSeed as PendingSeed | undefined;
  if (pending) {
    applyPendingSeed(pending);
    (window as any).__pendingSeed = undefined;
  }

  // ----- ParagraphView test control surface ----------------------------
  // Mounted as `window.__test` for use from Playwright via page.evaluate.
  (window as any).__test = {
    seedBook(opts: {
      id?: UUID;
      title?: string;
      chapters: Array<{
        title?: string;
        paragraphs: Array<{
          html: string;
          segments?: ParagraphSegment[];
          visibleWords?: number[];
        }>;
      }>;
    }): UUID {
      const id = opts.id ?? `mock-book-${++bookIdCounter}`;
      const newBook = buildBookFromChapters(
        id,
        opts.title ?? 'Test Book',
        opts.chapters.map((c, idx) => ({
          title: c.title ?? `Chapter ${idx + 1}`,
          paragraphs: c.paragraphs,
        })),
      );
      mockLibrary.set(id, newBook);
      emit('library_updated', Array.from(mockLibrary.values()));
      return id;
    },
    setTranslateConfig(bookId: UUID, paragraphId: number, cfg: TranslateConfig) {
      translateConfigs.set(paragraphKey(bookId, paragraphId), cfg);
    },
    setWordInfo(
      bookId: UUID,
      paragraphId: number,
      sentenceId: number,
      wordId: number,
      info: WordInfo,
    ) {
      wordInfos.set(wordKey(bookId, paragraphId, sentenceId, wordId), info);
    },
    seedRequest(requestId: number, bookId: UUID, paragraphId: number, cfg: TranslateConfig) {
      requestIdCounter = Math.max(requestIdCounter, requestId);
      runTranslateRequest(requestId, bookId, paragraphId, cfg);
    },
    emitParagraphUpdated(bookId: UUID, paragraphId: number) {
      emit('paragraph_updated', { bookId, paragraphId });
    },
    setParagraphTranslation(
      bookId: UUID,
      paragraphId: number,
      segments: ParagraphSegment[] | undefined,
      visibleWords?: number[],
    ) {
      const book = mockLibrary.get(bookId);
      if (!book) return;
      const p = book.paragraphsById.get(paragraphId);
      if (!p) return;
      p.segments = segments;
      if (visibleWords !== undefined) p.visibleWords = visibleWords;
      emit('paragraph_updated', { bookId, paragraphId });
    },
    getTranslateCalls() {
      return translateCalls.slice();
    },
    getMarkWordVisibleCalls() {
      return markWordVisibleCalls.slice();
    },
    reset() {
      resetMockState();
    },
  };

  // ----- Lyrics mode test helpers --------------------------------------
  // Tests call these from `page.evaluate(...)` to set up backend state.
  (window as any).__mockSpotifyState = (np: NowPlaying | null) => {
    mockNowPlaying = np;
    const dispatch = (window as any).__tauriEmit as
      | ((e: string, p?: unknown) => void)
      | undefined;
    dispatch?.('spotify_state', np);
  };
  (window as any).__mockLyrics = (trackId: string, lyrics: Lyrics | null) => {
    mockLyricsByTrack.set(trackId, lyrics);
  };
  (window as any).__mockTranslationCache = (t: LyricsTranslation) => {
    mockTranslationCache.set(translationKey(t.track_id, t.target_lang, t.model), t);
  };
}

// Mock languages (subset for testing)
const mockLanguages: Language[] = [
  { id: 'eng', name: 'English' },
  { id: 'spa', name: 'Spanish', localName: 'Español' },
  { id: 'fra', name: 'French', localName: 'Français' },
  { id: 'deu', name: 'German', localName: 'Deutsch' },
  { id: 'ita', name: 'Italian', localName: 'Italiano' },
  { id: 'por', name: 'Portuguese', localName: 'Português' },
  { id: 'rus', name: 'Russian', localName: 'Русский' },
  { id: 'jpn', name: 'Japanese', localName: '日本語' },
  { id: 'zho', name: 'Chinese', localName: '中文' },
  { id: 'kor', name: 'Korean', localName: '한국어' },
];

// Mock models
const mockModels: Model[] = [
  { id: 0, name: 'Not set' },
  { id: 1, name: 'Gemini 2.5 Flash', provider: 'google' },
  { id: 2, name: 'Gemini 2.5 Pro', provider: 'google' },
  { id: 3, name: 'Gemini 2.5 Flash Light', provider: 'google' },
  { id: 4, name: 'OpenAI GPT-5 mini', provider: 'openai' },
  { id: 5, name: 'OpenAI GPT-5.2', provider: 'openai' },
  { id: 6, name: 'OpenAI GPT-5.2 Pro', provider: 'openai' },
  { id: 7, name: 'OpenAI GPT-5 nano', provider: 'openai' },
];

const mockProviders: ProviderMeta[] = [
  { id: 'google', name: 'Google', defaultModelId: 1, apiKeyField: 'geminiApiKey' },
  { id: 'openai', name: 'OpenAI', defaultModelId: 4, apiKeyField: 'openaiApiKey' },
];

// InvokeArgs type for compatibility
export type InvokeArgs = Record<string, unknown>;

/**
 * Mock implementation of Tauri's invoke function.
 * Handles all commands used by the application.
 */
export function invoke<T>(cmd: string, args?: InvokeArgs): Promise<T> {
  console.log(`[Tauri Mock] invoke: ${cmd}`, args);

  switch (cmd) {
    // Config commands
    case 'get_languages':
      return Promise.resolve(mockLanguages as T);

    case 'get_models':
      return Promise.resolve(mockModels as T);

    case 'get_translation_providers':
      return Promise.resolve(mockProviders as T);

    case 'get_config':
      return Promise.resolve(mockConfig as T);

    case 'update_config': {
      const newConfig = args?.config as Config;
      if (newConfig) {
        mockConfig = { ...mockConfig, ...newConfig };
        emit('config_updated', mockConfig);
      }
      return Promise.resolve(undefined as T);
    }

    // Library commands
    case 'list_books': {
      const books = Array.from(mockLibrary.values()).map(book => ({
        id: book.id,
        title: book.title,
        chaptersCount: book.chaptersCount,
        paragraphsCount: book.paragraphsCount,
        translationRatio: book.translationRatio,
        path: book.path,
      }));
      return Promise.resolve(books as T);
    }

    case 'import_epub': {
      const id = `mock-book-${++bookIdCounter}`;
      // Frontend ships `{ title, chapters: [{ title, paragraphs: [{ html }] }] }`.
      // We re-key paragraphs into a global-id map to match the real backend.
      const bookData = args?.book as {
        title: string;
        chapters: Array<{ title: string; paragraphs: Array<{ html: string }> }>;
      };

      if (!bookData) {
        return Promise.reject(new Error('No book data provided'));
      }

      const newBook = buildBookFromChapters(
        id,
        bookData.title,
        bookData.chapters,
      );

      mockLibrary.set(id, newBook);
      emit('library_updated', Array.from(mockLibrary.values()));
      return Promise.resolve(id as T);
    }

    case 'import_plain_text': {
      const id = `mock-book-${++bookIdCounter}`;
      const title = args?.title as string;
      const text = args?.text as string;

      if (!title || !text) {
        return Promise.reject(new Error('Title and text are required'));
      }

      // Split text into paragraphs
      const paragraphs = text.split(/\n\n+/).filter(p => p.trim());

      const newBook = buildBookFromChapters(id, title, [
        { title, paragraphs: paragraphs.map((p) => ({ html: p })) },
      ]);

      mockLibrary.set(id, newBook);
      emit('library_updated', Array.from(mockLibrary.values()));
      return Promise.resolve(id as T);
    }

    case 'list_book_chapters': {
      const bookId = args?.bookId as UUID;
      const book = mockLibrary.get(bookId);

      if (!book) {
        return Promise.resolve([] as T);
      }

      const chapters: ChapterMetaView[] = book.chapters.map((chapter, idx) => ({
        id: idx,
        title: chapter.title || `Chapter ${idx + 1}`,
      }));

      return Promise.resolve(chapters as T);
    }

    case 'get_book_chapter_paragraph_ids': {
      const bookId = args?.bookId as UUID;
      const chapterId = args?.chapterId as number;
      const book = mockLibrary.get(bookId);

      if (!book || !book.chapters[chapterId]) {
        return Promise.resolve([] as T);
      }

      return Promise.resolve(book.chapters[chapterId].paragraphIds.slice() as T);
    }

    case 'get_paragraph_view': {
      const bookId = args?.bookId as UUID;
      const paragraphId = args?.paragraphId as number;
      const book = mockLibrary.get(bookId);
      if (!book) return Promise.reject(new Error('book not found'));
      const p = book.paragraphsById.get(paragraphId);
      if (!p) return Promise.reject(new Error('paragraph not found'));
      const view: ParagraphView = {
        id: paragraphId,
        original: p.html,
        segments: p.segments,
        visibleWords: p.visibleWords ?? [],
      };
      return Promise.resolve(view as T);
    }

    case 'get_word_info': {
      const bookId = args?.bookId as UUID;
      const paragraphId = args?.paragraphId as number;
      const sentenceId = args?.sentenceId as number;
      const wordId = args?.wordId as number;
      const info = wordInfos.get(wordKey(bookId, paragraphId, sentenceId, wordId));
      return Promise.resolve((info ?? undefined) as T);
    }

    case 'translate_paragraph': {
      const bookId = args?.bookId as UUID;
      const paragraphId = args?.paragraphId as number;
      const useCache = args?.useCache as boolean;
      const model = args?.model;

      translateCalls.push({ bookId, paragraphId, useCache, model });

      const requestId = ++requestIdCounter;
      const cfg =
        translateConfigs.get(paragraphKey(bookId, paragraphId)) ??
        DEFAULT_TRANSLATE_CONFIG;
      runTranslateRequest(requestId, bookId, paragraphId, cfg);

      return Promise.resolve(requestId as T);
    }

    case 'get_paragraph_translation_activity': {
      const bookId = args?.bookId as UUID;
      const paragraphId = args?.paragraphId as number;
      const activity =
        activeActivities.get(paragraphKey(bookId, paragraphId)) ?? null;
      return Promise.resolve(activity as T);
    }

    case 'mark_word_visible': {
      const bookId = args?.bookId as UUID;
      const paragraphId = args?.paragraphId as number;
      const flatIndex = args?.flatIndex as number;
      markWordVisibleCalls.push({ bookId, paragraphId, flatIndex });
      // Mirror the real backend: persist the visible word into the mock book
      // state and emit paragraph_updated so the frontend Resource re-fetches.
      const book = mockLibrary.get(bookId);
      const p = book?.paragraphsById.get(paragraphId);
      if (p) {
        const existing = new Set(p.visibleWords ?? []);
        if (!existing.has(flatIndex)) {
          existing.add(flatIndex);
          p.visibleWords = Array.from(existing);
          emit('paragraph_updated', { bookId, paragraphId });
        }
      }
      return Promise.resolve(true as T);
    }

    case 'get_book_reading_state': {
      const bookId = args?.bookId as UUID;
      const state = mockReadingStates.get(bookId);
      return Promise.resolve((state || null) as T);
    }

    case 'save_book_reading_state': {
      const bookId = args?.bookId as UUID;
      const chapterId = args?.chapterId as number;
      const paragraphId = args?.paragraphId as number;
      const pageOffset = (args?.pageOffset as number) ?? 0;

      mockReadingStates.set(bookId, { chapterId, paragraphId, pageOffset });
      return Promise.resolve(undefined as T);
    }

    case 'delete_book': {
      const bookId = args?.bookId as UUID;
      mockLibrary.delete(bookId);
      mockReadingStates.delete(bookId);
      emit('library_updated', Array.from(mockLibrary.values()));
      return Promise.resolve(undefined as T);
    }

    case 'move_book': {
      const bookId = args?.bookId as UUID;
      const newPath = args?.path as string[];
      const book = mockLibrary.get(bookId);

      if (book) {
        book.path = newPath;
        emit('library_updated', Array.from(mockLibrary.values()));
      }

      return Promise.resolve(undefined as T);
    }

    // ----- Lyrics mode commands ----------------------------------------

    case 'start_spotify_watcher':
    case 'stop_spotify_watcher':
      return Promise.resolve(undefined as T);

    case 'get_now_playing':
      return Promise.resolve((mockNowPlaying ?? null) as T);

    case 'get_track_lyrics_state': {
      // Read-only bootstrap snapshot — mirrors the real backend, which moved
      // all orchestration server-side. Tests prime state via __mockLyrics
      // (sets the per-track lyrics) and __mockTranslationCache (sets the
      // cached translation for a target lang/model).
      const trackId = args?.trackId as string;
      const target = args?.targetLang as string;
      const model = args?.model as number;
      const lyrics = mockLyricsByTrack.has(trackId)
        ? mockLyricsByTrack.get(trackId)!
        : null;
      const translation =
        mockTranslationCache.get(translationKey(trackId, target, model)) ?? null;
      // If the track has been explicitly mocked as "no lyrics", fire a
      // lyrics_resolved event after the bootstrap promise resolves so the
      // frontend transitions from `fetching` to `unsupported-track`. We
      // schedule it on the microtask queue + setTimeout(0) so the resolved
      // promise lands first.
      if (mockLyricsByTrack.has(trackId) && lyrics === null) {
        setTimeout(() => {
          emit('lyrics_resolved', { trackId, lyrics: null });
        }, 0);
      }
      return Promise.resolve({ lyrics, translation } as T);
    }

    // ----- Spotify Web (queue/preload) commands ------------------------
    // The lyrics view's queue store hits these on mount; without handlers the
    // mock logs `Unhandled command` warnings and the store sits in its
    // disconnected default forever (which is fine, just noisy).

    case 'spotify_web_status':
      return Promise.resolve({
        connected: false,
        premiumRequired: false,
        lastError: null,
      } as T);

    case 'spotify_web_get_queue':
      return Promise.resolve(null as T);

    case 'spotify_web_connect':
    case 'spotify_web_disconnect':
      return Promise.resolve(undefined as T);

    default:
      console.warn(`[Tauri Mock] Unhandled command: ${cmd}`);
      return Promise.resolve(undefined as T);
  }
}

// Re-export for compatibility with @tauri-apps/api/core
export { invoke as default };
