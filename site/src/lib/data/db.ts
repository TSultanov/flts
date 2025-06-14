import Dexie, { type EntityTable } from "dexie";
import type { ModelId } from "./translators/translator";

interface Book {
    id: number,
    title: string,
}

interface BookChapter {
    id: number,
    bookId: number,
    order: number,
    title?: string,
}

interface Paragraph {
    id: number,
    chapterId: number,
    order: number,
    originalText: string,
    originalHtml?: string,
}

interface Language {
    id: number,
    name: string,
}

interface ParagraphTranslation {
    id: number,
    paragraphId: number,
    languageId: number,
    translatingModel: ModelId,
}

interface SentenceTranslation {
    id: number,
    paragraphTranslationId: number,
    order: number,
    fullTranslation: string,
}

interface Word {
    id: number,
    originalLanguageId: number,
    original: string,
}

interface WordTranslation {
    id: number,
    languageId: number,
    originalWordId: number,
    translation: string,
}

interface SentenceWordTranslation {
    id: number,
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

export const db = new Dexie('library') as DB;

db.version(1).stores({
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
    directTranslationRequests: '++id',
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
