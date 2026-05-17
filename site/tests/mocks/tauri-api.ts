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

type MockParagraph = {
  html: string;
};

type ChapterMetaView = {
  id: number;
  title: string;
};

type ParagraphView = {
  id: number;
  original: string;
  translation?: string;
};

type BookReadingState = {
  chapterId: number;
  paragraphId: number;
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
  chapters: Array<{ title: string; paragraphs: Array<{ html: string }> }>,
): MockBook {
  const paragraphsById = new Map<number, MockParagraph>();
  let nextParagraphId = 0;
  const mockChapters: MockChapter[] = chapters.map((c) => {
    const paragraphIds: number[] = [];
    for (const p of c.paragraphs) {
      const pid = nextParagraphId++;
      paragraphsById.set(pid, { html: p.html });
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
  mockNowPlaying = null;
  mockLyricsByTrack.clear();
  mockTranslationCache.clear();
}

// Expose reset function globally for tests
if (typeof window !== 'undefined') {
  (window as any).__resetTauriMock = resetMockState;

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
        translation: undefined,
      };
      return Promise.resolve(view as T);
    }

    case 'get_word_info': {
      // Return undefined - word info not available in mock
      return Promise.resolve(undefined as T);
    }

    case 'translate_paragraph': {
      const bookId = args?.bookId as UUID;
      const paragraphId = args?.paragraphId as number;

      // Simulate translation by returning a request ID
      const requestId = ++requestIdCounter;

      // Simulate async translation completion. Mirror real backend ordering:
      // paragraph_updated fires from save_and_emit before book_updated
      // (the latter only fires via the file watcher for whole-book changes).
      setTimeout(() => {
        emit('paragraph_updated', { bookId, paragraphId });
      }, 100);

      return Promise.resolve(requestId as T);
    }

    case 'get_paragraph_translation_request_id': {
      // No translation in flight in the mock world — returning null keeps
      // ParagraphViewModel out of the get_translation_status polling loop.
      return Promise.resolve(null as T);
    }

    case 'get_translation_status': {
      // Defensive: if anything kicks off a poll, mark it complete immediately
      // so the polling effect tears itself down.
      const requestId = (args?.requestId as number) ?? 0;
      return Promise.resolve({
        request_id: requestId,
        progress_chars: 0,
        expected_chars: 0,
        is_complete: true,
      } as T);
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

      mockReadingStates.set(bookId, { chapterId, paragraphId });
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
