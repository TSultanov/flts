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
    bookUid: UUID,
    order: number,
    title?: string,
}

interface Paragraph extends Entity {
    chapterId: number,
    chapterUid: UUID,
    order: number,
    originalText: string,
    originalHtml?: string,
}

interface Language extends Entity {
    name: string,
}

interface ParagraphTranslation extends Entity {
    paragraphId: number,
    paragraphUid: UUID,
    languageId: number,
    languageUid: UUID,
    translatingModel: ModelId,
}

interface SentenceTranslation extends Entity {
    paragraphTranslationId: number,
    paragraphTranslationUid: UUID,
    order: number,
    fullTranslation: string,
}

interface Word extends Entity {
    originalLanguageId: number,
    originalLanguageUid: UUID,
    original: string,
    originalNormalized: string,
}

interface WordTranslation extends Entity {
    languageId: number,
    languageUid: UUID,
    originalWordId: number,
    originalWordUid: UUID,
    translation: string,
    translationNormalized: string,
}

interface SentenceWordTranslation extends Entity {
    sentenceId: number,
    sentenceUid: UUID,
    order: number,
    original: string,
    isPunctuation: boolean,
    isStandalonePunctuation: boolean,
    isOpeningParenthesis: boolean,
    isClosingParenthesis: boolean,
    wordTranslationId?: number,
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

interface Cache {
    hash: string,
    value: any,
}

export interface TranslationRequest {
    id: number,
    paragraphId: number,
    paragraphUid: UUID,
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

db.version(8).stores({
    books: '++id, title, createdAt, uid',
    bookChapters: '++id, bookId, bookUid, order, createdAt, uid',
    paragraphs: '++id, chapterId, chapterUid, order, createdAt, uid',
    languages: '++id, name, createdAt, uid',
    paragraphTranslations: '++id, paragraphId, paragraphUid, languageId, languageUid, createdAt, uid',
    sentenceTranslations: '++id, paragraphTranslationId, paragraphTranslationUid, order, createdAt, uid',
    words: '++id, originalLanguageId, originalLanguageUid, original, originalNormalized, createdAt, uid',
    wordTranslations: '++id, languageId, languageUid, originalWordId, originalWordUid, translation, translationNormalized, createdAt, uid',
    sentenceWordTranslations: '++id, sentenceId, sentenceUid, order, original, wordTranslationId, wordTranslationUid, createdAt, uid',
    queryCache: '&hash',
    directTranslationRequests: '++id, paragraphId, paragraphUid',
}).upgrade(async t => {
    console.log('Migrating to version 8: Adding UID-based foreign keys');
    
    // Populate BookChapter.bookUid
    const books = await t.table("books").toArray();
    const bookIdToUid = new Map(books.map(b => [b.id, b.uid]));
    
    await t.table("bookChapters").toCollection().modify((chapter: BookChapter) => {
        const bookUid = bookIdToUid.get(chapter.bookId);
        if (bookUid) {
            chapter.bookUid = bookUid;
        } else {
            console.warn(`Could not find book UID for bookId ${chapter.bookId}`);
        }
    });
    
    // Populate Paragraph.chapterUid
    const chapters = await t.table("bookChapters").toArray();
    const chapterIdToUid = new Map(chapters.map(c => [c.id, c.uid]));
    
    await t.table("paragraphs").toCollection().modify((paragraph: Paragraph) => {
        const chapterUid = chapterIdToUid.get(paragraph.chapterId);
        if (chapterUid) {
            paragraph.chapterUid = chapterUid;
        } else {
            console.warn(`Could not find chapter UID for chapterId ${paragraph.chapterId}`);
        }
    });
    
    // Populate ParagraphTranslation.paragraphUid and languageUid
    const paragraphs = await t.table("paragraphs").toArray();
    const paragraphIdToUid = new Map(paragraphs.map(p => [p.id, p.uid]));
    const languages = await t.table("languages").toArray();
    const languageIdToUid = new Map(languages.map(l => [l.id, l.uid]));
    
    await t.table("paragraphTranslations").toCollection().modify((pt: ParagraphTranslation) => {
        const paragraphUid = paragraphIdToUid.get(pt.paragraphId);
        const languageUid = languageIdToUid.get(pt.languageId);
        if (paragraphUid) {
            pt.paragraphUid = paragraphUid;
        } else {
            console.warn(`Could not find paragraph UID for paragraphId ${pt.paragraphId}`);
        }
        if (languageUid) {
            pt.languageUid = languageUid;
        } else {
            console.warn(`Could not find language UID for languageId ${pt.languageId}`);
        }
    });
    
    // Populate SentenceTranslation.paragraphTranslationUid
    const paragraphTranslations = await t.table("paragraphTranslations").toArray();
    const ptIdToUid = new Map(paragraphTranslations.map(pt => [pt.id, pt.uid]));
    
    await t.table("sentenceTranslations").toCollection().modify((st: SentenceTranslation) => {
        const ptUid = ptIdToUid.get(st.paragraphTranslationId);
        if (ptUid) {
            st.paragraphTranslationUid = ptUid;
        } else {
            console.warn(`Could not find paragraph translation UID for paragraphTranslationId ${st.paragraphTranslationId}`);
        }
    });
    
    // Populate Word.originalLanguageUid
    await t.table("words").toCollection().modify((word: Word) => {
        const languageUid = languageIdToUid.get(word.originalLanguageId);
        if (languageUid) {
            word.originalLanguageUid = languageUid;
        } else {
            console.warn(`Could not find language UID for originalLanguageId ${word.originalLanguageId}`);
        }
    });
    
    // Populate WordTranslation.languageUid and originalWordUid
    const words = await t.table("words").toArray();
    const wordIdToUid = new Map(words.map(w => [w.id, w.uid]));
    
    await t.table("wordTranslations").toCollection().modify((wt: WordTranslation) => {
        const languageUid = languageIdToUid.get(wt.languageId);
        const wordUid = wordIdToUid.get(wt.originalWordId);
        if (languageUid) {
            wt.languageUid = languageUid;
        } else {
            console.warn(`Could not find language UID for languageId ${wt.languageId}`);
        }
        if (wordUid) {
            wt.originalWordUid = wordUid;
        } else {
            console.warn(`Could not find word UID for originalWordId ${wt.originalWordId}`);
        }
    });
    
    // Populate SentenceWordTranslation.sentenceUid and wordTranslationUid
    const sentenceTranslations = await t.table("sentenceTranslations").toArray();
    const stIdToUid = new Map(sentenceTranslations.map(st => [st.id, st.uid]));
    const wordTranslations = await t.table("wordTranslations").toArray();
    const wtIdToUid = new Map(wordTranslations.map(wt => [wt.id, wt.uid]));
    
    await t.table("sentenceWordTranslations").toCollection().modify((swt: SentenceWordTranslation) => {
        const sentenceUid = stIdToUid.get(swt.sentenceId);
        if (sentenceUid) {
            swt.sentenceUid = sentenceUid;
        } else {
            console.warn(`Could not find sentence UID for sentenceId ${swt.sentenceId}`);
        }
        
        if (swt.wordTranslationId != null) {
            const wtUid = wtIdToUid.get(swt.wordTranslationId);
            if (wtUid) {
                swt.wordTranslationUid = wtUid;
            } else {
                console.warn(`Could not find word translation UID for wordTranslationId ${swt.wordTranslationId}`);
            }
        }
    });
    
    // Populate TranslationRequest.paragraphUid
    await t.table("directTranslationRequests").toCollection().modify((tr: TranslationRequest) => {
        const paragraphUid = paragraphIdToUid.get(tr.paragraphId);
        if (paragraphUid) {
            tr.paragraphUid = paragraphUid;
        } else {
            console.warn(`Could not find paragraph UID for paragraphId ${tr.paragraphId}`);
        }
    });
    
    console.log('Version 8 migration completed: UID-based foreign keys populated');
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
