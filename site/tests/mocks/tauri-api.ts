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

type Model = {
  id: number;
  name: string;
};

type Config = {
  targetLanguageId?: string;
  geminiApiKey?: string;
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
};

type MockChapter = {
  title: string;
  paragraphs: MockParagraph[];
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

// Mock state
let mockLibrary: Map<UUID, MockBook> = new Map();
let mockConfig: Config = {
  model: 0,
  geminiApiKey: 'mock-api-key-for-testing',
  libraryPath: '/mock/library/path',
  targetLanguageId: 'spa',
};
let mockReadingStates: Map<UUID, BookReadingState> = new Map();
let bookIdCounter = 0;
let requestIdCounter = 0;

// Event system for Tauri events
const eventHandlers = new Map<string, Set<(event: { payload: unknown }) => void>>();

function emit(event: string, payload: unknown) {
  const handlers = eventHandlers.get(event);
  if (handlers) {
    handlers.forEach(handler => handler({ payload }));
  }
}

// Reset state between tests
export function resetMockState() {
  mockLibrary.clear();
  mockConfig = {
    model: 0,
    geminiApiKey: 'mock-api-key-for-testing',
    libraryPath: '/mock/library/path',
    targetLanguageId: 'spa',
  };
  mockReadingStates.clear();
  bookIdCounter = 0;
  requestIdCounter = 0;
  eventHandlers.clear();
}

// Expose reset function globally for tests
if (typeof window !== 'undefined') {
  (window as any).__resetTauriMock = resetMockState;
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
  { id: 0, name: 'gemini-1.5-flash' },
  { id: 1, name: 'gemini-1.5-pro' },
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
      const bookData = args?.book as { title: string; chapters: MockChapter[] };

      if (!bookData) {
        return Promise.reject(new Error('No book data provided'));
      }

      const paragraphsCount = bookData.chapters.reduce(
        (sum, c) => sum + c.paragraphs.length,
        0
      );

      const newBook: MockBook = {
        id,
        title: bookData.title,
        chaptersCount: bookData.chapters.length,
        paragraphsCount,
        translationRatio: 0,
        path: [],
        chapters: bookData.chapters,
      };

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

      const newBook: MockBook = {
        id,
        title,
        chaptersCount: 1,
        paragraphsCount: paragraphs.length,
        translationRatio: 0,
        path: [],
        chapters: [{
          title: title,
          paragraphs: paragraphs.map(p => ({ html: p })),
        }],
      };

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

    case 'get_book_chapter_paragraphs': {
      const bookId = args?.bookId as UUID;
      const chapterId = args?.chapterId as number;
      const book = mockLibrary.get(bookId);

      if (!book || !book.chapters[chapterId]) {
        return Promise.resolve([] as T);
      }

      const paragraphs: ParagraphView[] = book.chapters[chapterId].paragraphs.map(
        (p, idx) => ({
          id: idx,
          original: p.html,
          translation: undefined,
        })
      );

      return Promise.resolve(paragraphs as T);
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

      // Simulate async translation completion
      setTimeout(() => {
        emit('book_updated', bookId);
      }, 100);

      return Promise.resolve(requestId as T);
    }

    case 'get_paragraph_translation_request_id': {
      return Promise.resolve(0 as T);
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

    default:
      console.warn(`[Tauri Mock] Unhandled command: ${cmd}`);
      return Promise.resolve(undefined as T);
  }
}

// Re-export for compatibility with @tauri-apps/api/core
export { invoke as default };
