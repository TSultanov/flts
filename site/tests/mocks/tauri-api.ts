/**
 * Stateful stand-in for @tauri-apps/api/core in Playwright tests: state
 * persists across invokes within a test. Tests drive it through `window.__test`
 * / `window.__mock*` (see the control surface below) and reset via
 * `resetMockState()`.
 */

import { parseEpub } from "./parse-epub";

type UUID = string;

type Language = {
  id: string;
  name: string;
  localName?: string;
};

type TranslationProvider =
  | "google"
  | "openai"
  | "deepseek"
  | "zai"
  | "openrouter";

type ProviderMeta = {
  id: TranslationProvider;
  name: string;
  defaultModel: string;
  apiKeyField:
    | "geminiApiKey"
    | "openaiApiKey"
    | "deepseekApiKey"
    | "zaiApiKey"
    | "openrouterApiKey";
  modelSelection?: "flat" | "family";
};

type Model = {
  id: string;
  name: string;
  provider?: TranslationProvider;
};

type Config = {
  targetLanguageId?: string;
  translationProvider: TranslationProvider;
  geminiApiKey?: string;
  openaiApiKey?: string;
  deepseekApiKey?: string;
  zaiApiKey?: string;
  openrouterApiKey?: string;
  model: string;
  libraryPath?: string;
  ankiEndpoint?: string;
  ankiApiKey?: string;
  tapToRevealTranslations?: boolean;
};

type AnkiSyncStatusState = "idle" | "syncing" | "ok" | "err" | "unreachable";
type SyncReportDto = {
  totalCards: number;
  attempted: number;
  succeeded: number;
  failed: number;
  persistentFailures: string[];
};
type AnkiSyncStatus = {
  state: AnkiSyncStatusState;
  lastFinishedAtMs?: number | null;
  lastError?: string | null;
  lastReport?: SyncReportDto | null;
};

type MockBook = {
  id: UUID;
  title: string;
  chaptersCount: number;
  paragraphsCount: number;
  translationRatio: number;
  path: string[];
  chapters: MockChapter[];
  // Keyed by global paragraph id: ids are book-wide, not per-chapter.
  paragraphsById: Map<number, MockParagraph>;
};

type MockChapter = {
  title: string;
  // Global paragraph ids, in order.
  paragraphIds: number[];
};

type Mark = "emphasis" | "strong";

type ParagraphSegment =
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
      familiarity?: number;
    };

type MockParagraph = {
  html: string;
  segments?: ParagraphSegment[];
};

type ChapterMetaView = {
  id: number;
  title: string;
  translationRatio: number;
};

type ParagraphView = {
  id: number;
  original: string;
  segments?: ParagraphSegment[];
};

type BookReadingState = {
  chapterId: number;
  paragraphId: number;
  pageOffset: number;
};

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
  | { kind: "immediate"; segments?: ParagraphSegment[] }
  | {
      kind: "progress";
      steps: ProgressStep[];
      segments: ParagraphSegment[];
    }
  | { kind: "error"; errorMessage: string; delayMs: number };

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

type PlayerState = "playing" | "paused" | "stopped" | "notrunning";

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
  model: string;
  lines: LyricsLineTranslation[];
};

let mockLibrary: Map<UUID, MockBook> = new Map();
let mockConfig: Config = {
  model: "models/gemini-2.5-flash",
  translationProvider: "google",
  geminiApiKey: "mock-api-key-for-testing",
  openaiApiKey: "mock-openai-key-for-testing",
  libraryPath: "/mock/library/path",
  targetLanguageId: "spa",
};
let mockAnkiSyncStatus: AnkiSyncStatus = { state: "idle" };
const syncAnkiNowCalls: Array<{ at: number }> = [];
let mockReadingStates: Map<UUID, BookReadingState> = new Map();
let bookIdCounter = 0;
let requestIdCounter = 0;

const DEFAULT_TRANSLATE_CONFIG: TranslateConfig = { kind: "immediate" };

type SummaryStatusState = {
  totalChapters: number;
  generated: boolean[];
  activelyGenerating: number | null;
};

const summaryStatusByBook = new Map<UUID, SummaryStatusState>();

function defaultSummaryStatus(chapterCount: number): SummaryStatusState {
  return {
    totalChapters: chapterCount,
    generated: Array.from({ length: chapterCount }, () => true),
    activelyGenerating: null,
  };
}

function getOrInitSummaryStatus(bookId: UUID): SummaryStatusState | null {
  const existing = summaryStatusByBook.get(bookId);
  if (existing) return existing;
  const book = mockLibrary.get(bookId);
  if (!book) return null;
  const state = defaultSummaryStatus(book.chapters.length);
  summaryStatusByBook.set(bookId, state);
  return state;
}

function emitSummaryProgress(
  bookId: UUID,
  status: "in_progress" | "done" | "failed",
  current: number,
  total: number,
  error?: string,
): void {
  emit("summary_generation_progress", {
    bookId,
    current,
    total,
    status,
    ...(error !== undefined ? { error } : {}),
  });
}

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
const translateChapterCalls: Array<{
  bookId: UUID;
  chapterId: number;
  useCache: boolean;
  model: unknown;
  enqueuedCount: number;
}> = [];
const translationsBatchCalls: Array<{
  bookId: UUID;
  paragraphIds: number[];
  at: number;
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
): void {
  const book = mockLibrary.get(bookId);
  if (!book) return;
  const p = book.paragraphsById.get(paragraphId);
  if (!p) return;
  p.segments = segments;
  emit("paragraph_updated", { bookId, paragraphId });
  // The chapter list Resource only refreshes on book_updated, as in production.
  emit("book_updated", bookId);
}

function emitStarted(
  bookId: UUID,
  paragraphId: number,
  requestId: number,
  expectedChars: number,
): void {
  emit("paragraph_translation_started", {
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
  emit("paragraph_translation_progress", {
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
  emit("paragraph_translation_finished", {
    bookId,
    paragraphId,
    requestId,
    error,
  });
}

// Mirrors the backend's single worker: requests announce themselves at enqueue
// but their progress/finished events fire serially off this queue.
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
          // Bookkeeping errors must not stall the queue.
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

  // Announce at enqueue, not at pickup, so multi-click shows every spinner;
  // the first progress event carries the real expectedChars.
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

// Seeded in-flight requests, keyed by paragraph. Timers start on the app's
// first activity query, not at seed time — a countdown racing app boot could
// finish before the test ever sees the spinner.
const pendingInFlightWork = new Map<string, () => void>();

function seedInFlightRequest(
  requestId: number,
  bookId: UUID,
  paragraphId: number,
  cfg: TranslateConfig,
): void {
  const key = paragraphKey(bookId, paragraphId);
  activeActivities.set(key, {
    requestId,
    progressChars: 0,
    expectedChars: 0,
  });
  pendingInFlightWork.set(key, () =>
    enqueueTranslationWork(() =>
      runTranslationWork(requestId, bookId, paragraphId, key, cfg),
    ),
  );
}

/** Starts a seeded in-flight request's timers on first observation. */
function startPendingInFlightWork(key: string): void {
  const start = pendingInFlightWork.get(key);
  if (start) {
    pendingInFlightWork.delete(key);
    start();
  }
}

async function runTranslationWork(
  requestId: number,
  bookId: UUID,
  paragraphId: number,
  key: string,
  cfg: TranslateConfig,
): Promise<void> {
  if (cfg.kind === "immediate") {
    await sleep(100);
    if (cfg.segments !== undefined) {
      applyTranslationCompletion(bookId, paragraphId, cfg.segments);
    } else {
      emit("paragraph_updated", { bookId, paragraphId });
    }
    activeActivities.delete(key);
    emitFinished(bookId, paragraphId, requestId, null);
    return;
  }

  if (cfg.kind === "error") {
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

  // Each step's delay follows its emit, so the last one precedes completion.
  for (const step of cfg.steps) {
    activeActivities.set(key, {
      requestId,
      progressChars: step.progress,
      expectedChars: step.total,
    });
    emitProgress(bookId, paragraphId, requestId, step.progress, step.total);
    await sleep(step.delayMs);
  }

  applyTranslationCompletion(bookId, paragraphId, cfg.segments);
  activeActivities.delete(key);
  emitFinished(bookId, paragraphId, requestId, null);
}

let mockNowPlaying: NowPlaying | null = null;
let mockLyricsByTrack: Map<string, Lyrics | null> = new Map();
let mockTranslationCache: Map<string, LyricsTranslation> = new Map();

function translationKey(
  trackId: string,
  target: string,
  model: string,
): string {
  return `${trackId}|${target}|${model}`;
}

/** Assigns book-wide paragraph ids, matching the real backend's id space. */
function buildBookFromChapters(
  id: UUID,
  title: string,
  chapters: Array<{
    title: string;
    paragraphs: Array<{
      html: string;
      segments?: ParagraphSegment[];
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

// Must go through the shared `tauri-event.ts` bus, or the app's `listen(...)`
// subscribers never see it.
function emit(event: string, payload: unknown) {
  const dispatch = (window as any).__tauriEmit as
    | ((e: string, p?: unknown) => void)
    | undefined;
  dispatch?.(event, payload);
}

export function resetMockState() {
  mockLibrary.clear();
  mockConfig = {
    model: "models/gemini-2.5-flash",
    translationProvider: "google",
    geminiApiKey: "mock-api-key-for-testing",
    openaiApiKey: "mock-openai-key-for-testing",
    libraryPath: "/mock/library/path",
    targetLanguageId: "spa",
  };
  mockReadingStates.clear();
  bookIdCounter = 0;
  requestIdCounter = 0;
  translateConfigs.clear();
  activeActivities.clear();
  pendingInFlightWork.clear();
  wordInfos.clear();
  translateCalls.length = 0;
  translateChapterCalls.length = 0;
  translationsBatchCalls.length = 0;
  translationWorkQueue.length = 0;
  translationWorkerBusy = false;
  mockNowPlaying = null;
  mockLyricsByTrack.clear();
  mockTranslationCache.clear();
  mockAnkiSyncStatus = { state: "idle" };
  syncAnkiNowCalls.length = 0;
  summaryStatusByBook.clear();
}

type PendingSeed = {
  bookId: string;
  title?: string;
  chapters: Array<{
    title?: string;
    paragraphs: Array<{
      html: string;
      segments?: ParagraphSegment[];
    }>;
  }>;
  translateConfigs?: Array<{ paragraphId: number; cfg: TranslateConfig }>;
  inFlight?: Array<{
    paragraphId: number;
    requestId: number;
    cfg: TranslateConfig;
  }>;
  wordInfos?: Array<{
    paragraphId: number;
    sentenceId: number;
    wordId: number;
    info: WordInfo;
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
  config?: { tapToRevealTranslations?: boolean };
};

function applyPendingSeed(seed: PendingSeed): void {
  resetMockState();
  const book = buildBookFromChapters(
    seed.bookId,
    seed.title ?? "Test Book",
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
    seedInFlightRequest(inf.requestId, seed.bookId, inf.paragraphId, inf.cfg);
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
  if (seed.summaryStatus) {
    summaryStatusByBook.set(seed.bookId, {
      totalChapters: seed.summaryStatus.generated.length,
      generated: seed.summaryStatus.generated.slice(),
      activelyGenerating:
        seed.summaryStatus.activelyGenerating === undefined
          ? null
          : seed.summaryStatus.activelyGenerating,
    });
  }
  if (seed.config) {
    mockConfig = { ...mockConfig, ...seed.config };
  }
}

if (typeof window !== "undefined") {
  (window as any).__resetTauriMock = resetMockState;

  // Must run during module init, before any invoke() resolves, so Library.*
  // Resources see the addInitScript seed on their first fetch.
  const pending = (window as any).__pendingSeed as PendingSeed | undefined;
  if (pending) {
    applyPendingSeed(pending);
    (window as any).__pendingSeed = undefined;
  }

  // Test control surface, driven from Playwright via page.evaluate.
  (window as any).__test = {
    seedBook(opts: {
      id?: UUID;
      title?: string;
      chapters: Array<{
        title?: string;
        paragraphs: Array<{
          html: string;
          segments?: ParagraphSegment[];
        }>;
      }>;
    }): UUID {
      const id = opts.id ?? `mock-book-${++bookIdCounter}`;
      const newBook = buildBookFromChapters(
        id,
        opts.title ?? "Test Book",
        opts.chapters.map((c, idx) => ({
          title: c.title ?? `Chapter ${idx + 1}`,
          paragraphs: c.paragraphs,
        })),
      );
      mockLibrary.set(id, newBook);
      emit("library_updated", Array.from(mockLibrary.values()));
      return id;
    },
    setTranslateConfig(
      bookId: UUID,
      paragraphId: number,
      cfg: TranslateConfig,
    ) {
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
    seedRequest(
      requestId: number,
      bookId: UUID,
      paragraphId: number,
      cfg: TranslateConfig,
    ) {
      requestIdCounter = Math.max(requestIdCounter, requestId);
      runTranslateRequest(requestId, bookId, paragraphId, cfg);
    },
    emitParagraphUpdated(bookId: UUID, paragraphId: number) {
      emit("paragraph_updated", { bookId, paragraphId });
    },
    setParagraphTranslation(
      bookId: UUID,
      paragraphId: number,
      segments: ParagraphSegment[] | undefined,
    ) {
      const book = mockLibrary.get(bookId);
      if (!book) return;
      const p = book.paragraphsById.get(paragraphId);
      if (!p) return;
      p.segments = segments;
      emit("paragraph_updated", { bookId, paragraphId });
      emit("book_updated", bookId);
    },
    // No paragraph_updated emit: stages state so a later cards_updated
    // exercises the soft-refetch path instead of invalidation.
    setParagraphTranslationSilent(
      bookId: UUID,
      paragraphId: number,
      segments: ParagraphSegment[] | undefined,
    ) {
      const book = mockLibrary.get(bookId);
      if (!book) return;
      const p = book.paragraphsById.get(paragraphId);
      if (!p) return;
      p.segments = segments;
    },
    emitCardsUpdated() {
      emit("cards_updated", null);
    },
    getTranslateCalls() {
      return translateCalls.slice();
    },
    getTranslateChapterCalls() {
      return translateChapterCalls.slice();
    },
    getTranslationsBatchCalls() {
      return translationsBatchCalls.slice();
    },
    getConfig(): Config {
      return { ...mockConfig };
    },
    setAnkiSyncStatus(status: AnkiSyncStatus) {
      mockAnkiSyncStatus = { ...status };
      emit("anki_sync_status_changed", undefined);
    },
    getAnkiSyncStatus(): AnkiSyncStatus {
      return { ...mockAnkiSyncStatus };
    },
    getSyncAnkiNowCalls() {
      return syncAnkiNowCalls.slice();
    },
    setSummaryStatus(
      bookId: UUID,
      opts: {
        generated: boolean[];
        activelyGenerating?: number | null;
      },
    ) {
      const status: SummaryStatusState = {
        totalChapters: opts.generated.length,
        generated: opts.generated.slice(),
        activelyGenerating:
          opts.activelyGenerating === undefined
            ? null
            : opts.activelyGenerating,
      };
      summaryStatusByBook.set(bookId, status);
      const generatedCount = status.generated.filter((g) => g).length;
      if (status.activelyGenerating !== null) {
        emitSummaryProgress(
          bookId,
          "in_progress",
          status.activelyGenerating,
          status.totalChapters,
        );
      } else if (generatedCount === status.totalChapters) {
        emitSummaryProgress(
          bookId,
          "done",
          status.totalChapters,
          status.totalChapters,
        );
      } else {
        emitSummaryProgress(
          bookId,
          "failed",
          generatedCount,
          status.totalChapters,
          "simulated failure",
        );
      }
    },
    advanceSummaryGeneration(bookId: UUID) {
      const status = getOrInitSummaryStatus(bookId);
      if (!status) return;
      const nextIdx = status.generated.findIndex((g) => !g);
      if (nextIdx === -1) {
        emitSummaryProgress(
          bookId,
          "done",
          status.totalChapters,
          status.totalChapters,
        );
        return;
      }
      status.generated[nextIdx] = true;
      const afterIdx = status.generated.findIndex((g) => !g);
      if (afterIdx === -1) {
        status.activelyGenerating = null;
        emitSummaryProgress(
          bookId,
          "done",
          status.totalChapters,
          status.totalChapters,
        );
      } else {
        status.activelyGenerating = afterIdx;
        // Backend emits `current = idx + 1`, i.e. the next pending index.
        emitSummaryProgress(
          bookId,
          "in_progress",
          afterIdx,
          status.totalChapters,
        );
      }
    },
    reset() {
      resetMockState();
    },
  };

  (window as any).__mockSpotifyState = (np: NowPlaying | null) => {
    mockNowPlaying = np;
    const dispatch = (window as any).__tauriEmit as
      | ((e: string, p?: unknown) => void)
      | undefined;
    dispatch?.("spotify_state", np);
  };
  (window as any).__mockLyrics = (trackId: string, lyrics: Lyrics | null) => {
    mockLyricsByTrack.set(trackId, lyrics);
  };
  (window as any).__mockTranslationCache = (t: LyricsTranslation) => {
    mockTranslationCache.set(
      translationKey(t.track_id, t.target_lang, t.model),
      t,
    );
  };
}

const mockLanguages: Language[] = [
  { id: "eng", name: "English" },
  { id: "spa", name: "Spanish", localName: "Español" },
  { id: "fra", name: "French", localName: "Français" },
  { id: "deu", name: "German", localName: "Deutsch" },
  { id: "ita", name: "Italian", localName: "Italiano" },
  { id: "por", name: "Portuguese", localName: "Português" },
  { id: "rus", name: "Russian", localName: "Русский" },
  { id: "jpn", name: "Japanese", localName: "日本語" },
  { id: "zho", name: "Chinese", localName: "中文" },
  { id: "kor", name: "Korean", localName: "한국어" },
];

const mockModels: Model[] = [
  {
    id: "models/gemini-2.5-flash",
    name: "Gemini 2.5 Flash",
    provider: "google",
  },
  { id: "models/gemini-2.5-pro", name: "Gemini 2.5 Pro", provider: "google" },
  { id: "gpt-5-mini", name: "OpenAI GPT-5 mini", provider: "openai" },
  {
    id: "~deepseek/deepseek-v4-flash-latest",
    name: "DeepSeek V4 Flash Latest",
    provider: "openrouter",
  },
];
const mockProviders: ProviderMeta[] = [
  {
    id: "google",
    name: "Google",
    defaultModel: "models/gemini-2.5-flash",
    apiKeyField: "geminiApiKey",
  },
  {
    id: "openai",
    name: "OpenAI",
    defaultModel: "gpt-5-mini",
    apiKeyField: "openaiApiKey",
  },
  {
    id: "deepseek",
    name: "DeepSeek",
    defaultModel: "deepseek-v4-flash",
    apiKeyField: "deepseekApiKey",
  },
  {
    id: "zai",
    name: "z.AI",
    defaultModel: "glm-5.2",
    apiKeyField: "zaiApiKey",
  },
  {
    id: "openrouter",
    name: "OpenRouter",
    defaultModel: "~deepseek/deepseek-v4-flash-latest",
    apiKeyField: "openrouterApiKey",
    modelSelection: "family",
  },
];

export type InvokeArgs = Record<string, unknown>;

/** Playwright stand-in for Rust `parse_language_id`. Isolang is not imported. */
function mockParseLanguageId(code: unknown): string | null {
  if (typeof code !== "string") return null;
  const raw = code.trim();
  if (!raw) return null;
  const primary = raw.split(/[-_]/)[0].toLowerCase();
  const map: Record<string, string> = {
    en: "eng",
    eng: "eng",
    es: "spa",
    spa: "spa",
    de: "deu",
    deu: "deu",
    ru: "rus",
    rus: "rus",
    zh: "zho",
    zho: "zho",
    ka: "kat",
    kat: "kat",
    fr: "fra",
    fra: "fra",
    nl: "nld",
    nld: "nld",
    und: "und",
  };
  return map[primary] ?? null;
}

export function invoke<T>(cmd: string, args?: InvokeArgs): Promise<T> {
  console.log(`[Tauri Mock] invoke: ${cmd}`, args);

  switch (cmd) {
    case "get_languages":
      return Promise.resolve(mockLanguages as T);

    case "parse_language_id":
      return Promise.resolve(mockParseLanguageId(args?.code) as T);

    case "get_models":
      return Promise.resolve(mockModels as T);

    case "get_translation_providers":
      return Promise.resolve(mockProviders as T);

    case "get_config":
      return Promise.resolve(mockConfig as T);

    case "update_config": {
      const newConfig = args?.config as Config;
      if (newConfig) {
        mockConfig = { ...mockConfig, ...newConfig };
        emit("config_updated", mockConfig);
      }
      return Promise.resolve(undefined as T);
    }

    case "list_books": {
      const books = Array.from(mockLibrary.values()).map((book) => ({
        id: book.id,
        title: book.title,
        chaptersCount: book.chaptersCount,
        paragraphsCount: book.paragraphsCount,
        translationRatio: book.translationRatio,
        path: book.path,
      }));
      return Promise.resolve(books as T);
    }

    case "parse_epub": {
      const epubBase64 = args?.epubBase64 as string | undefined;
      if (!epubBase64) {
        return Promise.reject(new Error("No EPUB data provided"));
      }
      const binary = atob(epubBase64);
      const bytes = new Uint8Array(binary.length);
      for (let i = 0; i < binary.length; i++) bytes[i] = binary.charCodeAt(i);
      const file = new File([bytes], "book.epub", {
        type: "application/epub+zip",
      });
      return parseEpub(file);
    }

    case "import_epub": {
      const id = `mock-book-${++bookIdCounter}`;
      const bookData = args?.book as {
        title: string;
        chapters: Array<{ title: string; paragraphs: Array<{ html: string }> }>;
      };

      if (!bookData) {
        return Promise.reject(new Error("No book data provided"));
      }

      const newBook = buildBookFromChapters(
        id,
        bookData.title,
        bookData.chapters,
      );

      mockLibrary.set(id, newBook);
      emit("library_updated", Array.from(mockLibrary.values()));
      return Promise.resolve(id as T);
    }

    case "import_plain_text": {
      const id = `mock-book-${++bookIdCounter}`;
      const title = args?.title as string;
      const text = args?.text as string;

      if (!title || !text) {
        return Promise.reject(new Error("Title and text are required"));
      }

      const paragraphs = text.split(/\n\n+/).filter((p) => p.trim());

      const newBook = buildBookFromChapters(id, title, [
        { title, paragraphs: paragraphs.map((p) => ({ html: p })) },
      ]);

      mockLibrary.set(id, newBook);
      emit("library_updated", Array.from(mockLibrary.values()));
      return Promise.resolve(id as T);
    }

    case "get_book_summary_status": {
      const bookId = args?.bookId as UUID;
      const status = getOrInitSummaryStatus(bookId);
      if (!status) {
        return Promise.resolve({
          totalChapters: 0,
          generated: [] as boolean[],
        } as T);
      }
      const payload: {
        totalChapters: number;
        generated: boolean[];
        activelyGenerating?: number;
      } = {
        totalChapters: status.totalChapters,
        generated: status.generated.slice(),
      };
      if (status.activelyGenerating !== null) {
        payload.activelyGenerating = status.activelyGenerating;
      }
      return Promise.resolve(payload as T);
    }

    case "list_book_chapters": {
      const bookId = args?.bookId as UUID;
      const book = mockLibrary.get(bookId);

      if (!book) {
        return Promise.resolve([] as T);
      }

      const chapters: ChapterMetaView[] = book.chapters.map((chapter, idx) => {
        const total = chapter.paragraphIds.length;
        const translated = chapter.paragraphIds.reduce(
          (count, pid) =>
            count + (book.paragraphsById.get(pid)?.segments ? 1 : 0),
          0,
        );
        return {
          id: idx,
          title: chapter.title || `Chapter ${idx + 1}`,
          translationRatio: total === 0 ? 0 : translated / total,
        };
      });

      return Promise.resolve(chapters as T);
    }

    case "get_book_chapter_paragraph_ids": {
      const bookId = args?.bookId as UUID;
      const chapterId = args?.chapterId as number;
      const book = mockLibrary.get(bookId);

      if (!book || !book.chapters[chapterId]) {
        return Promise.resolve([] as T);
      }

      return Promise.resolve(
        book.chapters[chapterId].paragraphIds.slice() as T,
      );
    }

    case "get_paragraph_view": {
      const bookId = args?.bookId as UUID;
      const paragraphId = args?.paragraphId as number;
      const book = mockLibrary.get(bookId);
      if (!book) return Promise.reject(new Error("book not found"));
      const p = book.paragraphsById.get(paragraphId);
      if (!p) return Promise.reject(new Error("paragraph not found"));
      const view: ParagraphView = {
        id: paragraphId,
        original: p.html,
        segments: p.segments,
      };
      return Promise.resolve(view as T);
    }

    case "get_paragraph_originals_batch": {
      const bookId = args?.bookId as UUID;
      const paragraphIds = (args?.paragraphIds ?? []) as number[];
      const book = mockLibrary.get(bookId);
      if (!book) return Promise.reject(new Error("book not found"));
      const rows = paragraphIds.flatMap((id) => {
        const p = book.paragraphsById.get(id);
        return p ? [{ id, original: p.html }] : [];
      });
      return Promise.resolve(rows as T);
    }

    case "get_paragraph_translations_batch": {
      const bookId = args?.bookId as UUID;
      const paragraphIds = (args?.paragraphIds ?? []) as number[];
      translationsBatchCalls.push({
        bookId,
        paragraphIds: paragraphIds.slice(),
        at: Date.now(),
      });
      const book = mockLibrary.get(bookId);
      if (!book) return Promise.reject(new Error("book not found"));
      const rows = paragraphIds.flatMap((id) => {
        const p = book.paragraphsById.get(id);
        return p ? [{ id, segments: p.segments }] : [];
      });
      return Promise.resolve(rows as T);
    }

    case "get_word_info": {
      const bookId = args?.bookId as UUID;
      const paragraphId = args?.paragraphId as number;
      const sentenceId = args?.sentenceId as number;
      const wordId = args?.wordId as number;
      const info = wordInfos.get(
        wordKey(bookId, paragraphId, sentenceId, wordId),
      );
      return Promise.resolve((info ?? undefined) as T);
    }

    case "translate_paragraph": {
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

    case "translate_chapter": {
      const bookId = args?.bookId as UUID;
      const chapterId = args?.chapterId as number;
      const useCache = args?.useCache as boolean;
      const model = args?.model;

      const book = mockLibrary.get(bookId);
      const chapter = book?.chapters[chapterId];
      let enqueuedCount = 0;
      if (book && chapter) {
        for (const paragraphId of chapter.paragraphIds) {
          const p = book.paragraphsById.get(paragraphId);
          // AppState::translate_chapter skips already-translated paragraphs.
          if (!p || p.segments) continue;
          const requestId = ++requestIdCounter;
          const cfg =
            translateConfigs.get(paragraphKey(bookId, paragraphId)) ??
            DEFAULT_TRANSLATE_CONFIG;
          runTranslateRequest(requestId, bookId, paragraphId, cfg);
          enqueuedCount++;
        }
      }
      translateChapterCalls.push({
        bookId,
        chapterId,
        useCache,
        model,
        enqueuedCount,
      });
      return Promise.resolve(enqueuedCount as T);
    }

    case "get_paragraph_translation_activity": {
      const bookId = args?.bookId as UUID;
      const paragraphId = args?.paragraphId as number;
      const key = paragraphKey(bookId, paragraphId);
      const activity = activeActivities.get(key) ?? null;
      // First observation starts the seeded request's timers.
      startPendingInFlightWork(key);
      return Promise.resolve(activity as T);
    }

    case "list_paragraph_translation_activity": {
      const rows = [...activeActivities.entries()].map(([key, activity]) => {
        // paragraphKey is `${bookId}:${paragraphId}`; bookIds contain no ':'.
        const sep = key.lastIndexOf(":");
        return {
          bookId: key.slice(0, sep) as UUID,
          paragraphId: Number(key.slice(sep + 1)),
          ...activity,
        };
      });
      for (const key of [...pendingInFlightWork.keys()]) {
        startPendingInFlightWork(key);
      }
      return Promise.resolve(rows as T);
    }

    case "get_book_reading_state": {
      const bookId = args?.bookId as UUID;
      const state = mockReadingStates.get(bookId);
      return Promise.resolve((state || null) as T);
    }

    case "save_book_reading_state": {
      const bookId = args?.bookId as UUID;
      const chapterId = args?.chapterId as number;
      const paragraphId = args?.paragraphId as number;
      const pageOffset = (args?.pageOffset as number) ?? 0;

      mockReadingStates.set(bookId, { chapterId, paragraphId, pageOffset });
      return Promise.resolve(undefined as T);
    }

    case "delete_book": {
      const bookId = args?.bookId as UUID;
      mockLibrary.delete(bookId);
      mockReadingStates.delete(bookId);
      emit("library_updated", Array.from(mockLibrary.values()));
      return Promise.resolve(undefined as T);
    }

    case "move_book": {
      const bookId = args?.bookId as UUID;
      const newPath = args?.path as string[];
      const book = mockLibrary.get(bookId);

      if (book) {
        book.path = newPath;
        emit("library_updated", Array.from(mockLibrary.values()));
      }

      return Promise.resolve(undefined as T);
    }

    // Lyrics mode
    case "start_spotify_watcher":
    case "stop_spotify_watcher":
      return Promise.resolve(undefined as T);

    case "get_now_playing":
      return Promise.resolve((mockNowPlaying ?? null) as T);

    case "get_track_lyrics_state": {
      // Read-only bootstrap snapshot; tests prime it via __mockLyrics and
      // __mockTranslationCache.
      const trackId = args?.trackId as string;
      const target = args?.targetLang as string;
      const model = args?.model as string;
      const lyrics = mockLyricsByTrack.has(trackId)
        ? mockLyricsByTrack.get(trackId)!
        : null;
      const translation =
        mockTranslationCache.get(translationKey(trackId, target, model)) ??
        null;
      // A mocked "no lyrics" track needs lyrics_resolved *after* the bootstrap
      // promise settles, or the view never leaves `fetching`.
      if (mockLyricsByTrack.has(trackId) && lyrics === null) {
        setTimeout(() => {
          emit("lyrics_resolved", { trackId, lyrics: null });
        }, 0);
      }
      return Promise.resolve({ lyrics, translation } as T);
    }

    case "get_anki_sync_status":
      return Promise.resolve({ ...mockAnkiSyncStatus } as T);

    case "sync_anki_now": {
      syncAnkiNowCalls.push({ at: Date.now() });
      mockAnkiSyncStatus = { state: "syncing" };
      emit("anki_sync_status_changed", undefined);
      const report: SyncReportDto = {
        totalCards: 1,
        attempted: 1,
        succeeded: 1,
        failed: 0,
        persistentFailures: [],
      };
      // Delayed so the syncing → ok transition is observable.
      setTimeout(() => {
        mockAnkiSyncStatus = {
          state: "ok",
          lastFinishedAtMs: Date.now(),
          lastError: null,
          lastReport: report,
        };
        emit("anki_sync_status_changed", undefined);
      }, 10);
      return Promise.resolve(report as T);
    }

    case "spotify_web_status":
      return Promise.resolve({
        connected: false,
        premiumRequired: false,
        lastError: null,
      } as T);

    case "spotify_web_get_queue":
      return Promise.resolve(null as T);

    case "spotify_web_connect":
    case "spotify_web_disconnect":
      return Promise.resolve(undefined as T);

    default:
      console.warn(`[Tauri Mock] Unhandled command: ${cmd}`);
      return Promise.resolve(undefined as T);
  }
}

export { invoke as default };
