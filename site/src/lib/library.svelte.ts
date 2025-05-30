import { db, type Book, type BookChapter, type Language, type Paragraph, type ParagraphTranslation, type SentenceTranslation, type SentenceWordTranslation, type Word, type WordTranslation } from "./data/db";
import type { ImportWorkerController } from "./data/importWorkerController";

export type LibraryBook = Book & {
    chapters: BookChapter[],
}

export type LibrarySentenceTranslation = SentenceTranslation & {
    words: SentenceWordTranslation[];
}

export type LibraryParagraphTranslation = ParagraphTranslation & {
    sentences: LibrarySentenceTranslation[]
}

export type LibraryBookParagraph = Paragraph & {
    translation?: LibraryParagraphTranslation,
}

export type LibraryBookChapter = BookChapter & {
    paragraphs: Paragraph[],
}

export type LibraryWord = Word & {
    originalLanguage: Language,
}

export type LibraryWordTranslation = WordTranslation & {
    language: Language,
    originalWord: LibraryWord,
}

export type LibrarySentenceWordTranslation = SentenceWordTranslation & {
    wordTranslation?: LibraryWordTranslation,
}

export class Library {
    libraryBooks: LibraryBook[] = $state([]);
    workerController: ImportWorkerController;

    constructor(workerController: ImportWorkerController) {
        this.workerController = workerController;
        this.workerController.addOnParagraphTranslatedHandler(() => this.refresh());
    }

    async getWordTranslation(sentenceWordId: number): Promise<LibrarySentenceWordTranslation | null> {
        const sentenceWordTranslation = await db.sentenceWordTranslations.get(sentenceWordId);
        if (!sentenceWordTranslation) {
            return null;
        }

        const wordTranslation = sentenceWordTranslation.wordTranslationId != null ? await db.wordTranslations.get(sentenceWordTranslation.wordTranslationId) : null;
        if (!wordTranslation) {
            return sentenceWordTranslation;
        }

        const targetLanguage = await db.languages.get(wordTranslation.languageId);
        if (!targetLanguage) {
            console.log(`Can't find targetLanguage id ${wordTranslation.languageId}`);
            return sentenceWordTranslation;
        }

        const originalWord = await db.words.get(wordTranslation.originalWordId);
        if (!originalWord) {
            console.log(`Can't find original word for wordTranslation id ${wordTranslation.id}`);
            return sentenceWordTranslation;
        }

        const originalLanguage = await db.languages.get(originalWord.originalLanguageId);
        if (!originalLanguage) {
            console.log(`Can't find originalLanguage id ${originalWord.originalLanguageId}`);
            return sentenceWordTranslation;
        }

        return {
            ...sentenceWordTranslation,
            wordTranslation: {
                ...wordTranslation,
                language: targetLanguage,
                originalWord: {
                    ...originalWord,
                    originalLanguage: originalLanguage
                }
            }
        };
    }

    async getBook(bookId: number): Promise<LibraryBook | null> {
        const book = await db.books.get(bookId);
        if (!book) {
            return null;
        }
        const chapters = await db.bookChapters.where("bookId").equals(book.id).sortBy("order");
        return {
            ...book,
            chapters,
        }
    }

    async getParagraph(paragraphId: number): Promise<LibraryBookParagraph | null> {
        const paragraph = await db.paragraphs.get(paragraphId);
        if (!paragraph) {
            return null;
        }

        const translations = await db.paragraphTranslations.where('paragraphId').equals(paragraphId).toArray();
        if (translations.length === 0) {
            return {
                ...paragraph,
                translation: undefined,
            };
        }
        const librarySentences: LibrarySentenceTranslation[] = [];
        for (const translation of translations) {
            const sentences = await db.sentenceTranslations.where('paragraphTranslationId').equals(translation.id).sortBy('order');
            for (const sentence of sentences) {
                const words = await db.sentenceWordTranslations.where('sentenceId').equals(sentence.id).toArray();
                librarySentences.push({
                    ...sentence,
                    words,
                });
            }
        }
        return {
            ...paragraph,
            translation: {
                ...translations[0],
                sentences: librarySentences,
            },
        };
    }

    async getChapter(chapterId: number): Promise<LibraryBookChapter | null> {
        const chapter = await db.bookChapters.get(chapterId);
        if (!chapter) {
            return null;
        }
        const paragraphs = await db.paragraphs.where('chapterId').equals(chapter.id).sortBy('order');
        return {
            ...chapter,
            paragraphs: paragraphs,
        };
    }

    async refresh() {
        await db.transaction(
            'r',
            [
                db.books,
                db.bookChapters,
            ],
            async () => {
                const books = await db.books.toArray();
                const lBooks: LibraryBook[] = await Promise.all(books.map(async b => {
                    const chapters = await db.bookChapters.where("bookId").equals(b.id).sortBy("order");
                    return {
                        ...b,
                        chapters,
                    }
                }));
                this.libraryBooks = lBooks;
            }
        );
    }

    async importText(title: string, text: string) {
        await db.transaction(
            'rw',
            [
                db.books,
                db.bookChapters,
                db.paragraphs,
            ],
            async () => {
                const bookId = await db.books.add({
                    title
                });

                const chapterId = await db.bookChapters.add({
                    bookId,
                    order: 0,
                });

                const paragraphs = this.splitParagraphs(text);

                const paragraphIds = [];
                let order = 0;
                for (const paragraph of paragraphs) {
                    const paragraphId = await db.paragraphs.add({
                        chapterId,
                        order,
                        originalText: paragraph,
                    });
                    paragraphIds.push(paragraphId);
                    order += 1;
                }
            }
        );
        await this.refresh();
    }

    async deleteBook(bookId: number) {
        await db.transaction('rw', [
            db.books,
            db.bookChapters,
            db.paragraphs,
            db.paragraphTranslations,
            db.sentenceTranslations,
            db.sentenceWordTranslations,
        ],
            async () => {
                await db.books.delete(bookId);
                const chapterIds = await db.bookChapters.where("bookId").equals(bookId).primaryKeys();
                for (const chapterId of chapterIds) {
                    const paragraphIds = await db.paragraphs.where("chapterId").equals(chapterId).primaryKeys();
                    for (const paragraphId of paragraphIds) {
                        const paragraphTranslationIds = await db.paragraphTranslations.where("paragraphId").equals(paragraphId).primaryKeys();
                        for (const paragraphTranslationId of paragraphTranslationIds) {
                            const sentenceTranslationIds = await db.sentenceTranslations.where("paragraphTranslationId").equals(paragraphTranslationId).primaryKeys();
                            for (const sentenceTranslationId of sentenceTranslationIds) {
                                await db.sentenceWordTranslations.where("sentenceId").equals(sentenceTranslationId).delete();
                                await db.sentenceTranslations.delete(sentenceTranslationId);
                            }
                            await db.paragraphTranslations.delete(paragraphTranslationId);
                        }
                        await db.paragraphs.delete(paragraphId);
                    }
                    await db.bookChapters.delete(chapterId);
                }
            }
        );
        await this.refresh();
    }

    private splitParagraphs(text: string): string[] {
        return text
            .split(/\n+/)
            .map(p => p.trim())
            .filter(p => p.length > 0);
    }
}