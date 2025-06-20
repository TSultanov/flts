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
    uid: UUID,
    createdAt: number,
}

interface Book extends Entity {
    title: string,
    path?: string[],
}

interface BookChapter extends Entity {
    bookUid: UUID,
    order: number,
    title?: string,
}

interface Paragraph extends Entity {
    chapterUid: UUID,
    order: number,
    originalText: string,
    originalHtml?: string,
}

interface Language extends Entity {
    name: string,
}

interface ParagraphTranslation extends Entity {
    paragraphUid: UUID,
    languageUid: UUID,
    translatingModel: ModelId,
}

interface SentenceTranslation extends Entity {
    paragraphTranslationUid: UUID,
    order: number,
    fullTranslation: string,
}

interface Word extends Entity {
    originalLanguageUid: UUID,
    original: string,
    originalNormalized: string,
}

interface WordTranslation extends Entity {
    languageUid: UUID,
    originalWordUid: UUID,
    translation: string,
    translationNormalized: string,
}

interface SentenceWordTranslation extends Entity {
    sentenceUid: UUID,
    order: number,
    original: string,
    isPunctuation: boolean,
    isStandalonePunctuation: boolean,
    isOpeningParenthesis: boolean,
    isClosingParenthesis: boolean,
    wordTranslationUid?: UUID,
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

export type DB = Dexie & {
    books: EntityTable<Book, 'uid'>,
    bookChapters: EntityTable<BookChapter, 'uid'>,
    paragraphs: EntityTable<Paragraph, 'uid'>,
    languages: EntityTable<Language, 'uid'>,
    paragraphTranslations: EntityTable<ParagraphTranslation, 'uid'>,
    sentenceTranslations: EntityTable<SentenceTranslation, 'uid'>,
    words: EntityTable<Word, 'uid'>,
    wordTranslations: EntityTable<WordTranslation, 'uid'>,
    sentenceWordTranslations: EntityTable<SentenceWordTranslation, 'uid'>,
};

export const db = new Dexie('library', {
    chromeTransactionDurability: "relaxed",
    cache: "immutable",
}) as DB;

db.version(1).stores({
    books: '&uid, title, createdAt',
    bookChapters: '&uid, bookUid, order, createdAt',
    paragraphs: '&uid, chapterUid, order, createdAt',
    languages: '&uid, name, createdAt',
    paragraphTranslations: '&uid, paragraphUid, languageUid, createdAt',
    sentenceTranslations: '&uid, paragraphTranslationUid, order, createdAt',
    words: '&uid, originalLanguageUid, original, originalNormalized, createdAt',
    wordTranslations: '&uid, languageUid, originalWordUid, translation, translationNormalized, createdAt',
    sentenceWordTranslations: '&uid, sentenceUid, order, original, wordTranslationUid, createdAt',
    queryCache: '&hash',
});

db.version(2).stores({
    books: '&uid, title, createdAt',
    bookChapters: '&uid, bookUid, order, createdAt',
    paragraphs: '&uid, chapterUid, order, createdAt',
    languages: '&uid, name, createdAt',
    paragraphTranslations: '&uid, paragraphUid, languageUid, createdAt',
    sentenceTranslations: '&uid, paragraphTranslationUid, order, createdAt',
    words: '&uid, originalLanguageUid, original, originalNormalized, createdAt',
    wordTranslations: '&uid, languageUid, originalWordUid, translation, translationNormalized, createdAt',
    sentenceWordTranslations: '&uid, sentenceUid, order, original, wordTranslationUid, createdAt',
    queryCache: null,
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
