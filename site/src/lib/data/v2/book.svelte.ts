import Dexie, { type EntityTable } from "dexie"
import type { ModelId } from "../translators/translator"
import { generateUID, type Entity, type UUID } from "./db"
import type { EpubBook } from "../epubLoader"
import { readable, writable, type Readable } from "svelte/store";
import { debounce } from "./util";
import { translationQueue } from "../queueDb";

const bookCache = new Map<UUID, WeakRef<IBook>>();

const bookDb = new Dexie('books', {
    chromeTransactionDurability: "relaxed",
    cache: "immutable"
}) as BookDb;

type BookDb = Dexie & {
    books: EntityTable<BookEntity, 'uid'>,
    booksMeta: EntityTable<IBookMeta, 'uid'>,
}

bookDb.version(1).stores({
    books: "&uid, createdAt, updatedAt",
    booksMeta: "&uid",
});

type BookData = {
    path: string[];
    readonly title: string,
    readonly chapters: BookChapter[],
}

type BookEntity = Entity & BookData

type BookChapter = {
    readonly id: ChapterId,
    readonly title?: string,
    readonly paragraphs: Paragraph[],
}

type Paragraph = {
    readonly id: ParagraphId,
    readonly originalText: string,
    readonly originalHtml?: string,
    readonly translation?: BookParagraphTranslation,
}

export type BookParagraphTranslation = {
    readonly languageCode: string,
    readonly translatingModel: ModelId,
    readonly sentences: SentenceTranslation[],
}

export type SentenceTranslation = {
    readonly fullTranslation: string,
    readonly words: SentenceWordTranslation[],
}

export type SentenceWordTranslation = {
    readonly original: string,
    readonly isPunctuation: boolean,
    readonly isStandalonePunctuation?: boolean | null,
    readonly isOpeningParenthesis?: boolean | null,
    readonly isClosingParenthesis?: boolean | null,
    readonly wordTranslationUid?: UUID,
    readonly wordTranslationInContext?: string[],
    readonly grammarContext?: Grammar,
    readonly note?: string,
}

type Grammar = {
    originalInitialForm: string,
    targetInitialForm: string,
    partOfSpeech: string
    plurality?: string | null,
    person?: string | null,
    tense?: string | null,
    case?: string | null,
    other?: string | null,
}

export type ChapterId = {
    readonly __brand: "ChapterId",
    readonly chapter: number,
}

export type ParagraphId = {
    readonly __brand: "ParagraphId",
    readonly chapter: number,
    readonly paragraph: number,
}

export type TranslatedWordId = ParagraphId & {
    readonly sentence: number,
    readonly word: number,
}

export interface IParagraphView {
    get id(): ParagraphId,
    get originalPlain(): string,
    get original(): string,
    get translation(): BookParagraphTranslation | undefined,
    get translationStore(): Readable<BookParagraphTranslation | undefined>;
}

class ParagraphView implements IParagraphView {
    private refersher: (() => void) | undefined;

    constructor(
        private book: Book,
        private paragraphId: ParagraphId,
    ) { }

    refresh() {
        this.refersher?.();
    }

    private get paragraph() {
        return this.book.data.chapters[this.paragraphId.chapter].paragraphs[this.paragraphId.paragraph];
    }

    get id() {
        return this.paragraphId;
    }

    get originalPlain() {
        return this.paragraph.originalText;
    }

    get original() {
        return this.paragraph.originalHtml ?? this.paragraph.originalText;
    }

    get translation() {
        return this.paragraph.translation;
    }

    get translationStore() {
        return readable<BookParagraphTranslation | undefined>(undefined, (set) => {
            set(this.paragraph.translation);

            this.refersher = () => {
                set(this.paragraph.translation);
            }
        });
    }
}

interface IChapterView {
    get id(): ChapterId,
    get title(): string | undefined,
    get paragraphs(): IParagraphView[];
}

class ChapterView implements IChapterView {
    constructor(private book: Book, private chapterId: ChapterId) { }

    get id() {
        return this.chapterId;
    }

    get title() {
        return this.book.data.chapters[this.chapterId.chapter].title;
    }

    get paragraphs(): IParagraphView[] {
        return this.book.data.chapters[this.chapterId.chapter].paragraphs
            .map(p => this.book.getParagraphView(p.id)!);
    }
}

export interface IBookMeta {
    readonly uid: UUID,
    readonly chapterCount: number;
    readonly translationRatio: number;
    readonly title: string;
    path: string[];
}

export interface IBook extends IBookMeta {
    readonly chapters: IChapterView[];
    getChapterView(chapterId: ChapterId): IChapterView | undefined;
    getParagraphView(paragraphId: ParagraphId): IParagraphView | undefined;
    updateParagraphTranslation(paragraphId: ParagraphId, translation: BookParagraphTranslation): void;
}

class Book implements IBook {
    chapterViewCache = new Map<ChapterId, ChapterView>();
    paragraphViewCache = new Map<ParagraphId, ParagraphView>();

    private persist: () => void;

    paragraphsCount: number = 0;
    translatedParagraphsCount = $state(0);
    translationRatio = $derived(this.translatedParagraphsCount / this.paragraphsCount);

    private constructor(public data: BookEntity) {
        this.persist = debounce(async () => {
            this.persistImmediately();
        }, 1000);
        this.updateMetrics();
    }

    static async load(bookUid: UUID): Promise<IBook | null> {
        const book = await bookDb.books.get(bookUid);
        if (!book) {
            return null;
        }
        return new Book(book);
    }

    static createFromText(title: string, text: string): IBook {
        const paragraphs = Book.splitParagraphs(text);

        const bookData: BookData = {
            path: [],
            title: title,
            chapters: [
                {
                    id: {
                        chapter: 0,
                    } as ChapterId,
                    paragraphs: paragraphs.map((p, i) => {
                        return {
                            id: {
                                chapter: 0,
                                paragraph: i
                            } as ParagraphId,
                            originalText: p
                        }
                    })
                }
            ],
        };

        const now = Date.now();
        const bookEntity: BookEntity = {
            uid: generateUID(),
            createdAt: now,
            updatedAt: now,
            ...bookData
        };

        const book = new Book(bookEntity);
        book.persistImmediately();
        translationQueue.scheduleFullBookTranslation(book.uid);
        return book;
    }

    static createFromEpub(epubBook: EpubBook): IBook {
        const bookData: BookData = {
            path: [],
            title: epubBook.title,
            chapters: epubBook.chapters.map((ec, ci) => {
                return {
                    id: {
                        chapter: ci
                    } as ChapterId,
                    title: ec.title,
                    paragraphs: ec.paragraphs.map((ep, pi) => {
                        return {
                            id: {
                                chapter: ci,
                                paragraph: pi,
                            } as ParagraphId,
                            originalText: ep.text,
                            originalHtml: ep.html,
                        }
                    })
                }
            })
        };

        const now = Date.now();
        const bookEntity: BookEntity = {
            uid: generateUID(),
            createdAt: now,
            updatedAt: now,
            ...bookData
        };

        const book = new Book(bookEntity);
        book.persistImmediately();
        translationQueue.scheduleFullBookTranslation(book.uid);
        return book;
    }

    get uid() {
        return this.data.uid;
    }

    get title() {
        return this.data.title;
    }

    get chapterCount() {
        return this.data.chapters.length;
    }

    get chapters(): IChapterView[] {
        return this.data.chapters.map(c => {
            return this.getChapterView(c.id)!;
        });
    }

    get path() {
        return this.data.path;
    }

    set path(value) {
        this.data.path = value;
        this.persistImmediately();
    }

    updateParagraphTranslation(paragraphId: ParagraphId, translation: BookParagraphTranslation) {
        const paragraph = this.data.chapters[paragraphId.chapter].paragraphs[paragraphId.paragraph];
        const translatedParagraph = {
            ...paragraph,
            translation,
        };
        this.data.chapters[paragraphId.chapter].paragraphs[paragraphId.paragraph] = translatedParagraph;
        this.persist();
    }

    getChapterView(chapterId: ChapterId): IChapterView | undefined {
        if (chapterId.chapter >= this.data.chapters.length) {
            return;
        }
        const cached = this.chapterViewCache.get(chapterId);
        if (cached) {
            return cached;
        }

        const view = new ChapterView(this, chapterId);
        this.chapterViewCache.set(chapterId, view);
        return view;
    }

    getParagraphView(paragraphId: ParagraphId): IParagraphView | undefined {
        if (paragraphId.chapter >= this.data.chapters.length) {
            return;
        }

        if (paragraphId.paragraph >= this.data.chapters[paragraphId.chapter].paragraphs.length) {
            return;
        }

        const cached = this.paragraphViewCache.get(paragraphId);
        if (cached) {
            return cached;
        }

        const view = new ParagraphView(this, paragraphId);
        return view;
    }

    private async persistImmediately() {
        this.updateMetrics();
        for (const [_, obj] of this.paragraphViewCache) {
            obj.refresh();
        }

        const meta: IBookMeta = {
            uid: this.uid,
            title: this.title,
            chapterCount: this.chapterCount,
            translationRatio: this.translationRatio,
            path: this.path
        }

        this.data.updatedAt = Date.now();
        await bookDb.books.put(this.data);
        await bookDb.booksMeta.put(meta);
    }

    private static splitParagraphs(text: string): string[] {
        return text
            .split(/\n+/)
            .map(p => p.trim())
            .filter(p => p.length > 0);
    }

    private updateMetrics() {
        let pCount = 0;
        let tpCount = 0;
        for (const c of this.data.chapters) {
            for (const p of c.paragraphs) {
                pCount++;
                if (p.translation) {
                    tpCount++;
                }
            }
        }
        this.paragraphsCount = pCount;
        this.translatedParagraphsCount = tpCount;
    }
}

export const books = {
    listBooks: async (): Promise<IBookMeta[]> => {
        return await bookDb.booksMeta.toArray();
    },

    getBook: async (uid: UUID): Promise<IBook | null> => {
        const cachedRef = bookCache.get(uid);
        if (cachedRef) {
            const instance = cachedRef.deref();
            if (instance) {
                return instance;
            }
        }

        const loaded = await Book.load(uid);
        if (loaded) {
            bookCache.set(uid, new WeakRef<IBook>(loaded));
        }
        return loaded;
    },

    importEpub: (epub: EpubBook): UUID => {
        return Book.createFromEpub(epub).uid;
    },

    importText: (title: string, text: string): UUID => {
        return Book.createFromText(title, text).uid;
    },

    deleteBook: async (uid: UUID) => {
        await bookDb.books.delete(uid);
        await bookDb.booksMeta.delete(uid);
        bookCache.delete(uid);
    },

};