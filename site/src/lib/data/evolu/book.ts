import { createIdFromString, FiniteNumber, getOrThrow, type Evolu } from "@evolu/common";
import type { BookChapterId, BookChapterParagraphId, BookId, BookParagraphTranslationId, BookParagraphTranslationSentenceId, BookParagraphTranslationSentenceWordId, DatabaseSchema, LanguageId, WordTranslationSpellingVariantId } from "./schema";
import type { EpubBook } from "../epubLoader";
import type { ModelId } from "../translators/translator";
import { sql } from 'kysely';

export class Books {
    constructor(private evolu: Evolu<DatabaseSchema>) {
    }

    private splitParagraphs(text: string): { text: string; html?: string }[] {
        return text
            .split(/\n+/)
            .map(p => p.trim())
            .filter(p => p.length > 0)
            .map(p => ({ text: p }));
    }

    get allBooks() {
        return this.evolu.createQuery(db =>
            db.selectFrom("book as b")
                .where("b.isDeleted", "is not", 1)
                .select(({ selectFrom }) => [
                    "b.id",
                    "b.title",
                    "b.path",
                    "b.isDeleted",
                    selectFrom("bookChapter as bc")
                        .whereRef("bc.bookId", "=", "b.id")
                        .select([sql<number>`count(bc.id)`.as("chapterCount")])
                        .as("chapterCount"),
                    selectFrom("bookChapterParagraph as bcp")
                        .innerJoin("bookChapter as bc", "bcp.chapterId", "bc.id")
                        .whereRef("bc.bookId", "=", "b.id")
                        .select([sql<number>`count(bcp.id)`.as("paragraphsCount")])
                        .as("paragraphsCount"),
                    selectFrom("bookParagraphTranslation as bpt")
                        .innerJoin("bookChapterParagraph as bcp", "bpt.chapterParagraphId", "bcp.id")
                        .innerJoin("bookChapter as bc", "bcp.chapterId", "bc.id")
                        .whereRef("bc.bookId", "=", "b.id")
                        .select([sql<number>`count(bpt.id)`.as("translatedParagraphsCount")])
                        .as("translatedParagraphsCount")
                ])
        );
    }

    createBookFromText(title: string, text: string, path?: string[]) {
        const paragraphs = this.splitParagraphs(text);

        const bookResult = this.evolu.insert("book", {
            path: path ?? [],
            title: title,
        });

        const bookId = getOrThrow(bookResult).id;

        const chapterResult = this.evolu.insert("bookChapter", {
            bookId: bookId,
            chapterIndex: 0
        });

        const chapterId = getOrThrow(chapterResult).id;

        paragraphs.forEach((p, idx) => {
            this.evolu.insert("bookChapterParagraph", {
                chapterId: chapterId,
                paragraphIndex: idx,
                originalText: p.text,
                originalHtml: p.html ?? null,
            });
        });

        return bookId;
    }

    createBookFromEpub(epub: EpubBook, path?: string[]) {
        const chapterCount = epub.chapters.length;
        const paragraphCount = epub.chapters.reduce((acc, c) => acc + c.paragraphs.length, 0);

        const bookResult = this.evolu.insert("book", {
            path: path ?? [],
            title: epub.title,
        });

        const bookId = getOrThrow(bookResult).id;

        epub.chapters.forEach((chapter, chapterIndex) => {
            const chapterResult = this.evolu.insert("bookChapter", {
                bookId: bookId,
                chapterIndex: chapterIndex,
                title: chapter.title,
            });

            const chapterId = getOrThrow(chapterResult).id;

            chapter.paragraphs.forEach((para, paragraphIndex) => {
                this.evolu.insert("bookChapterParagraph", {
                    chapterId: chapterId,
                    paragraphIndex: paragraphIndex,
                    originalText: para.text,
                    originalHtml: para.html,
                });
            });
        });

        return bookId;
    }

    updateParagraphTranslation(
        paragraphId: BookChapterParagraphId,
        translation: ParagraphTranslation,
    ) {
        const clearWordsForSentence = async (sentenceId: BookParagraphTranslationSentenceId, startingWordIdx: FiniteNumber) => {
            const queryWordIds = this.evolu.createQuery(db =>
                db.selectFrom("bookParagraphTranslationSentenceWord as w")
                    .where("w.isDeleted", "is not", 1)
                    .where("w.sentenceId", "=", sentenceId)
                    .where("w.wordIndex", ">=", startingWordIdx)
                    .select(["w.id"])
            );

            const idsToRemove = await this.evolu.loadQuery(queryWordIds);

            idsToRemove.forEach(row => {
                this.evolu.update("bookParagraphTranslationSentenceWord", {
                    id: row.id,
                    isDeleted: true
                });
            });
        };

        const clearSentences = async (translationId: BookParagraphTranslationId, startingSentenceIdx: FiniteNumber) => {
            const querySentences = this.evolu.createQuery(db =>
                db.selectFrom("bookParagraphTranslationSentence as s")
                    .where("s.isDeleted", "is not", 1)
                    .where("s.paragraphTranslationId", "=", translationId)
                    .where("s.sentenceIndex", ">=", startingSentenceIdx)
                    .select(["s.id"])
            );

            const idsToRemove = await this.evolu.loadQuery(querySentences);

            idsToRemove.forEach(row => {
                this.evolu.update("bookParagraphTranslationSentence", {
                    id: row.id,
                    isDeleted: true
                });

                clearWordsForSentence(row.id, getOrThrow(FiniteNumber.from(0)));
            });
        };

        const preparedTranslationId = createIdFromString(`translation-${paragraphId}`);
        const translationId = getOrThrow(this.evolu.upsert("bookParagraphTranslation", {
            id: preparedTranslationId,
            chapterParagraphId: paragraphId,
            languageId: translation.languageId,
            translatingModel: translation.translatingModel,
        })).id;

        translation.sentences.forEach((s, idx) => {
            const preparedSentenceId = createIdFromString(`sentence-${translationId}-${idx}`);
            const sentenceId = getOrThrow(this.evolu.upsert("bookParagraphTranslationSentence", {
                id: preparedSentenceId,
                paragraphTranslationId: translationId,
                sentenceIndex: idx,
                fullTranslation: s.fullTranslation
            })).id;

            s.words.forEach((w, idx) => {
                const preparedWordId = createIdFromString(`word-${preparedSentenceId}-${idx}`);
                getOrThrow(this.evolu.upsert("bookParagraphTranslationSentenceWord", {
                    id: preparedWordId,
                    sentenceId,
                    wordIndex: idx,
                    original: w.original,
                    isPunctuation: w.isPunctuation,
                    isStandalonePunctuation: w.isStandalonePunctuation ?? null,
                    isOpeningParenthesis: w.isOpeningParenthesis ?? null,
                    isClosingParenthesis: w.isClosingParenthesis ?? null,
                    wordTranslationId: w.wordTranslationId,
                    wordTranslationInContext: w.wordTranslationInContext,
                    grammarContext: {
                        originalInitialForm: w.grammarContext?.originalInitialForm,
                        targetInitialForm: w.grammarContext?.targetInitialForm,
                        partOfSpeech: w.grammarContext?.partOfSpeech,
                        plurality: w.grammarContext?.plurality ?? null,
                        person: w.grammarContext?.person ?? null,
                        tense: w.grammarContext?.tense ?? null,
                        case: w.grammarContext?.case ?? null,
                        other: w.grammarContext?.other ?? null,
                    },
                    note: w.note ?? null,
                }));
            });

            clearWordsForSentence(sentenceId, getOrThrow(FiniteNumber.from(s.words.length)));
        });

        clearSentences(translationId, getOrThrow(FiniteNumber.from(translation.sentences.length)))
    }

    updateBookPath(bookId: BookId, path: string[]) {
        getOrThrow(this.evolu.update("book", {
            id: bookId,
            path,
        }));
    }

    deleteBook(bookId: BookId) {
        this.evolu.update("book", {
            id: bookId,
            isDeleted: true
        });
    }

    bookChapters(bookId: BookId) {
        return this.evolu.createQuery(db =>
            db.selectFrom("bookChapter")
                .where("bookId", "=", bookId).orderBy("chapterIndex", "asc")
                // .where("isDeleted", "is not", 1)
                .selectAll()
        );
    }

    paragraphs(chapterId: BookChapterId) {
        return this.evolu.createQuery(db =>
            db.selectFrom("bookChapterParagraph")
                .where("chapterId", "=", chapterId).orderBy("paragraphIndex", "asc")
                .where("isDeleted", "is not", 1)
                .selectAll()
        );
    }

    paragraph(paragraphId: BookChapterParagraphId) {
        return this.evolu.createQuery(db => db.selectFrom("bookChapterParagraph")
            .where("id", "=", paragraphId)
            .where("isDeleted", "is not", 1)
            .selectAll()
            .limit(1)
        );
    }

    paragraphTranslation(paragraphId: BookChapterParagraphId) {
        return this.evolu.createQuery(db =>
            db.selectFrom("bookParagraphTranslation as p")
                .where("p.chapterParagraphId", "=", paragraphId)
                .innerJoin("bookParagraphTranslationSentence as s", "s.paragraphTranslationId", "p.id")
                .innerJoin("bookParagraphTranslationSentenceWord as w", "w.sentenceId", "s.id")
                .orderBy("s.sentenceIndex", "asc")
                .orderBy("w.wordIndex", "asc")
                .where("p.isDeleted", "is not", 1)
                .where("s.isDeleted", "is not", 1)
                .where("w.isDeleted", "is not", 1)
                .select([
                    "p.id as paragraphId",
                    "s.id as sentenceId",
                    "w.id as wordId",
                    "w.original",
                    "w.isPunctuation"
                ])
        );
    }

    wordTranslation(sentnceWordId: BookParagraphTranslationSentenceWordId) {
        return this.evolu.createQuery(db =>
            db.selectFrom("bookParagraphTranslationSentenceWord as w")
                .innerJoin("bookParagraphTranslationSentence as s", "s.id", "w.sentenceId")
                .innerJoin("bookParagraphTranslation as p", "p.id", "s.paragraphTranslationId")
                .where("w.id", "=", sentnceWordId)
                .where("w.isDeleted", "is not", 1)
                .where("s.isDeleted", "is not", 1)
                .where("p.isDeleted", "is not", 1)
                .select([
                    "w.id as wordId",
                    "w.original",
                    "w.wordTranslationInContext",
                    "w.note",
                    "w.grammarContext",
                    "s.id as sentenceId",
                    "s.fullTranslation",
                    "p.translatingModel"
                ])
                .limit(1)
        );
    }

    nonTranslatedParagraphsIds(bookId?: BookId) {
        return bookId ?
            this.evolu.createQuery(db =>
                db.selectFrom("bookChapterParagraph as p")
                    .innerJoin("bookChapter as c", "p.chapterId", "c.id")
                    .innerJoin("book as b", "c.bookId", "b.id")
                    .leftJoin("bookParagraphTranslation as t", "t.chapterParagraphId", "p.id")
                    .where("t.id", "is", null)
                    .where("b.id", "=", bookId)
                    .where("p.isDeleted", "is not", 1)
                    .where("c.isDeleted", "is not", 1)
                    .where("b.isDeleted", "is not", 1)
                    .where("t.isDeleted", "is not", 1)
                    .select(["p.id as paragraphId", "b.id as bookId"])
                    .distinct()
            ) :
            this.evolu.createQuery(db =>
                db.selectFrom("bookChapterParagraph as p")
                    .innerJoin("bookChapter as c", "p.chapterId", "c.id")
                    .innerJoin("book as b", "c.bookId", "b.id")
                    .leftJoin("bookParagraphTranslation as t", "t.chapterParagraphId", "p.id")
                    .where("t.id", "is", null)
                    .where("p.isDeleted", "is not", 1)
                    .where("t.isDeleted", "is not", 1)
                    .select(["p.id as paragraphId", "b.id as bookId"])
                    .distinct()
            );
    }
}

export type ParagraphTranslationSentenceWordGrammar = {
    originalInitialForm: string;
    targetInitialForm: string;
    partOfSpeech: string;
    plurality: string | null;
    person: string | null;
    tense: string | null;
    case: string | null;
    other: string | null;
}

export type ParagraphTranslationSentenceWord = {
    original: string;
    isPunctuation: boolean;
    isStandalonePunctuation: boolean | null;
    isOpeningParenthesis: boolean | null;
    isClosingParenthesis: boolean | null;
    wordTranslationId: WordTranslationSpellingVariantId;
    wordTranslationInContext: string[];
    grammarContext: ParagraphTranslationSentenceWordGrammar;
    note: string | null;
}

export type ParagraphTranslationSentence = {
    fullTranslation: string,
    words: ParagraphTranslationSentenceWord[],
}

export type ParagraphTranslation = {
    languageId: LanguageId,
    translatingModel: ModelId,
    sentences: ParagraphTranslationSentence[],
}
