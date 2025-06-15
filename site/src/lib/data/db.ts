import Dexie, { type EntityTable } from "dexie";
import { v4 as uuidv4 } from "uuid";
import type { ModelId } from "./translators/translator";

export type UUID = string & { readonly __brand: "UUID" };

function isValidUUID(value: string): value is UUID {
    const uuidRegex = /^[0-9a-f]{8}-[0-9a-f]{4}-[1-5][0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/i;
    return uuidRegex.test(value);
}

export function createUUID(value: string): UUID {
    if (!isValidUUID(value)) {
        throw new Error(`Invalid UUID format: ${value}`);
    }
    return value as UUID;
}

export function generateUID(): UUID {
    return createUUID(uuidv4());
}

interface Entity {
    id: number,
    uid: UUID,
    createdAt: number,
}

interface Book extends Entity {
    title: string,
    path?: string[],
}

interface BookChapter extends Entity {
    bookId: number,
    order: number,
    title?: string,
}

interface Paragraph extends Entity {
    chapterId: number,
    order: number,
    originalText: string,
    originalHtml?: string,
}

interface Language extends Entity {
    name: string,
}

interface ParagraphTranslation extends Entity {
    paragraphId: number,
    languageId: number,
    translatingModel: ModelId,
}

interface SentenceTranslation extends Entity {
    paragraphTranslationId: number,
    order: number,
    fullTranslation: string,
}

interface Word extends Entity {
    originalLanguageId: number,
    original: string,
    originalNormalized: string,
}

interface WordTranslation extends Entity {
    languageId: number,
    originalWordId: number,
    translation: string,
    translationNormalized: string,
}

interface SentenceWordTranslation extends Entity {
    sentenceId: number,
    order: number,
    original: string,
    isPunctuation: boolean,
    isStandalonePunctuation: boolean,
    isOpeningParenthesis: boolean,
    isClosingParenthesis: boolean,
    wordTranslationId?: number,
    wordTranslationInContext?: string[],
    grammarContext?: Grammar,
    note?: string,
}

interface Grammar {
    partOfSpeech: string
    plurality?: string,
    person?: string,
    tense?: string,
    case?: string,
    other?: string
}

interface Cache {
    hash: string,
    value: any,
}

export interface TranslationRequest {
    id: number,
    paragraphId: number,
    model: ModelId,
}

export type DB = Dexie & {
    books: EntityTable<Book, 'id'>,
    bookChapters: EntityTable<BookChapter, 'id'>,
    paragraphs: EntityTable<Paragraph, 'id'>,
    languages: EntityTable<Language, 'id'>,
    paragraphTranslations: EntityTable<ParagraphTranslation, 'id'>,
    sentenceTranslations: EntityTable<SentenceTranslation, 'id'>,
    words: EntityTable<Word, 'id'>,
    wordTranslations: EntityTable<WordTranslation, 'id'>,
    sentenceWordTranslations: EntityTable<SentenceWordTranslation, 'id'>,
    queryCache: EntityTable<Cache, 'hash'>,
    directTranslationRequests: EntityTable<TranslationRequest, 'id'>,
};

export const db = new Dexie('library', {
    chromeTransactionDurability: "relaxed",
    cache: "immutable",
}) as DB;

db.version(4).stores({
    books: '++id, title',
    bookChapters: '++id, bookId, order',
    paragraphs: '++id, chapterId, order',
    languages: '++id, name',
    paragraphTranslations: '++id, paragraphId, languageId',
    sentenceTranslations: '++id, paragraphTranslationId, order',
    words: '++id, originalLanguageId, original',
    wordTranslations: '++id, languageId, originalWordId, translation',
    sentenceWordTranslations: '++id, sentenceId, order, original, wordTranslationId',
    queryCache: '&hash',
    directTranslationRequests: '++id,paragraphId',
});

db.version(5).stores({
    books: '++id, title',
    bookChapters: '++id, bookId, order',
    paragraphs: '++id, chapterId, order',
    languages: '++id, name',
    paragraphTranslations: '++id, paragraphId, languageId',
    sentenceTranslations: '++id, paragraphTranslationId, order',
    words: '++id, originalLanguageId, original, originalNormalized',
    wordTranslations: '++id, languageId, originalWordId, translation, translationNormalized',
    sentenceWordTranslations: '++id, sentenceId, order, original, wordTranslationId',
    queryCache: '&hash',
    directTranslationRequests: '++id,paragraphId',
}).upgrade(t => {
    return t.table("words").toCollection().modify((w: Word) => {
        w.originalNormalized = w.original.toLowerCase();
    });
}).upgrade(t => {
    return t.table("wordTranslations").toCollection().modify((wt: WordTranslation) => {
        wt.translationNormalized = wt.translation.toLowerCase();
    });
});

db.version(6).stores({
    books: '++id, title, uid',
    bookChapters: '++id, bookId, order, uid',
    paragraphs: '++id, chapterId, order, uid',
    languages: '++id, name, uid',
    paragraphTranslations: '++id, paragraphId, languageId, uid',
    sentenceTranslations: '++id, paragraphTranslationId, order, uid',
    words: '++id, originalLanguageId, original, originalNormalized, uid',
    wordTranslations: '++id, languageId, originalWordId, translation, translationNormalized, uid',
    sentenceWordTranslations: '++id, sentenceId, order, original, wordTranslationId, uid',
    queryCache: '&hash',
    directTranslationRequests: '++id, paragraphId',
}).upgrade(t => {
    const migrationTime = Date.now();
    
    return Promise.all([
        t.table("books").toCollection().modify((book: Book) => {
            book.uid = generateUID();
            book.createdAt = migrationTime;
        }),
        t.table("bookChapters").toCollection().modify((chapter: BookChapter) => {
            chapter.uid = generateUID();
            chapter.createdAt = migrationTime;
        }),
        t.table("paragraphs").toCollection().modify((paragraph: Paragraph) => {
            paragraph.uid = generateUID();
            paragraph.createdAt = migrationTime;
        }),
        t.table("languages").toCollection().modify((language: Language) => {
            language.uid = generateUID();
            language.createdAt = migrationTime;
        }),
        t.table("paragraphTranslations").toCollection().modify((pt: ParagraphTranslation) => {
            pt.uid = generateUID();
            pt.createdAt = migrationTime;
        }),
        t.table("sentenceTranslations").toCollection().modify((st: SentenceTranslation) => {
            st.uid = generateUID();
            st.createdAt = migrationTime;
        }),
        t.table("words").toCollection().modify((word: Word) => {
            word.uid = generateUID();
            word.createdAt = migrationTime;
        }),
        t.table("wordTranslations").toCollection().modify((wt: WordTranslation) => {
            wt.uid = generateUID();
            wt.createdAt = migrationTime;
        }),
        t.table("sentenceWordTranslations").toCollection().modify((swt: SentenceWordTranslation) => {
            swt.uid = generateUID();
            swt.createdAt = migrationTime;
        })
    ]);
});

db.version(7).stores({
    books: '++id, title, createdAt, uid',
    bookChapters: '++id, bookId, order, createdAt, uid',
    paragraphs: '++id, chapterId, order, createdAt, uid',
    languages: '++id, name, createdAt, uid',
    paragraphTranslations: '++id, paragraphId, languageId, createdAt, uid',
    sentenceTranslations: '++id, paragraphTranslationId, order, createdAt, uid',
    words: '++id, originalLanguageId, original, originalNormalized, createdAt, uid',
    wordTranslations: '++id, languageId, originalWordId, translation, translationNormalized, createdAt, uid',
    sentenceWordTranslations: '++id, sentenceId, order, original, wordTranslationId, createdAt, uid',
    queryCache: '&hash',
    directTranslationRequests: '++id, paragraphId',
});

export type {
    Book,
    BookChapter,
    Paragraph,
    Language,
    ParagraphTranslation,
    SentenceTranslation,
    Word,
    WordTranslation,
    SentenceWordTranslation
}
