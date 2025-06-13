import { liveQuery } from "dexie";
import { db, type Book, type BookChapter, type Language, type Paragraph, type ParagraphTranslation, type SentenceTranslation, type SentenceWordTranslation, type Word, type WordTranslation } from "./data/db";
import { readable, type Readable } from 'svelte/store';
import type { EpubBook } from "./data/epubLoader";
import type { ModelId } from "./data/translators/translator";

export type LibraryFolder = {
    name?: string,
    folders: LibraryFolder[],
    books: LibraryBook[],
}

export type LibraryBook = Book & {
    chapters: BookChapter[],
    paragraphsCount: number,
    translatedParagraphsCount: number,
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
    fullSentenceTranslation: string,
    model: ModelId,
    paragraphId: number,
    wordTranslation?: LibraryWordTranslation,
}

export class Library {
    getWordTranslation(sentenceWordId: number): Readable<LibrarySentenceWordTranslation | null> {
        return this.useQuery(() =>
            db.transaction(
                'r',
                [
                    db.paragraphTranslations,
                    db.sentenceWordTranslations,
                    db.sentenceTranslations,
                    db.wordTranslations,
                    db.languages,
                    db.words,
                    db.languages,
                ],
                async () => {
                    const sentenceWordTranslation = await db.sentenceWordTranslations.get(sentenceWordId);
                    if (!sentenceWordTranslation) {
                        return null;
                    }

                    const sentenceTranslation = await db.sentenceTranslations.get(sentenceWordTranslation.sentenceId);
                    if (!sentenceTranslation) {
                        return null;
                    }

                    const paragraphTranslation = await db.paragraphTranslations.get(sentenceTranslation.paragraphTranslationId);
                    if (!paragraphTranslation) {
                        return null;
                    }

                    const sentenceWordFullTranslation = {
                        ...sentenceWordTranslation,
                        fullSentenceTranslation: sentenceTranslation.fullTranslation,
                        model: paragraphTranslation.translatingModel,
                        paragraphId: paragraphTranslation.paragraphId,
                    };

                    const wordTranslation = sentenceWordTranslation.wordTranslationId != null ? await db.wordTranslations.get(sentenceWordTranslation.wordTranslationId) : null;
                    if (!wordTranslation) {
                        return sentenceWordFullTranslation;
                    }

                    const targetLanguage = await db.languages.get(wordTranslation.languageId);
                    if (!targetLanguage) {
                        console.log(`Can't find targetLanguage id ${wordTranslation.languageId}`);
                        return sentenceWordFullTranslation;
                    }

                    const originalWord = await db.words.get(wordTranslation.originalWordId);
                    if (!originalWord) {
                        console.log(`Can't find original word for wordTranslation id ${wordTranslation.id}`);
                        return sentenceWordFullTranslation;
                    }

                    const originalLanguage = await db.languages.get(originalWord.originalLanguageId);
                    if (!originalLanguage) {
                        console.log(`Can't find originalLanguage id ${originalWord.originalLanguageId}`);
                        return sentenceWordFullTranslation;
                    }

                    let ret: LibrarySentenceWordTranslation = {
                        ...sentenceWordTranslation,
                        fullSentenceTranslation: sentenceTranslation.fullTranslation,
                        model: paragraphTranslation.translatingModel,
                        paragraphId: paragraphTranslation.paragraphId,
                        wordTranslation: {
                            ...wordTranslation,
                            language: targetLanguage,
                            originalWord: {
                                ...originalWord,
                                originalLanguage: originalLanguage
                            }
                        },
                    };

                    return ret;
                }
            ));
    }

    getBook(bookId: number): Readable<LibraryBook | null> {
        return this.useQuery(() => db.transaction(
            'r',
            [
                db.books,
                db.bookChapters,
                db.paragraphs,
                db.paragraphTranslations
            ],
            async () => {
                const book = await db.books.get(bookId);
                if (!book) {
                    return null;
                }
                const chapters = await db.bookChapters.where("bookId").equals(book.id).sortBy("order");

                const paragraphIds = (await db.paragraphs.where("chapterId").anyOf(chapters.map(c => c.id)).toArray()).map(p => p.id);
                const translatedParagraphsCount = await db.paragraphTranslations.where("paragraphId").anyOf(paragraphIds).count()

                return {
                    ...book,
                    chapters,
                    paragraphsCount: paragraphIds.length,
                    translatedParagraphsCount
                }
            }
        ));
    }

    getParagraph(paragraphId: number): Readable<LibraryBookParagraph | null> {
        return this.useQuery(() => db.transaction(
            'r',
            [
                db.paragraphs,
                db.paragraphTranslations,
                db.sentenceTranslations,
                db.sentenceWordTranslations,
            ],
            async () => {
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
                translations.sort((a, b) => b.id - a.id);
                return {
                    ...paragraph,
                    translation: {
                        ...translations[0],
                        sentences: librarySentences,
                    },
                };
            }
        ));
    }

    getChapter(chapterId: number): Readable<LibraryBookChapter | null> {
        return this.useQuery(() => db.transaction(
            'r',
            [
                db.bookChapters,
                db.paragraphs,
            ],
            async () => {
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
        ));
    }

    getLibraryBooks(): Readable<LibraryFolder> {
        return this.useQuery(() => db.transaction(
            'r',
            [
                db.books,
                db.bookChapters,
                db.paragraphs,
                db.paragraphTranslations
            ],
            async () => {
                const books = await db.books.toArray();
                const libraryBooks = await Promise.all(books.map(async b => {
                    const chapters = await db.bookChapters.where("bookId").equals(b.id).sortBy("order");

                    const paragraphIds = (await db.paragraphs.where("chapterId").anyOf(chapters.map(c => c.id)).toArray()).map(p => p.id);
                    const translatedParagraphsCount = await db.paragraphTranslations.where("paragraphId").anyOf(paragraphIds).count()

                    return {
                        ...b,
                        chapters,
                        paragraphsCount: paragraphIds.length,
                        translatedParagraphsCount
                    };
                }));

                const rootFolder: LibraryFolder = {
                    name: undefined,
                    folders: [],
                    books: []
                };

                const findOrCreateFolder = (folder: LibraryFolder, pathSegments: string[]): LibraryFolder => {
                    if (pathSegments.length === 0) {
                        return folder;
                    }

                    const [currentSegment, ...remainingSegments] = pathSegments;
                    let targetFolder = folder.folders.find(f => f.name === currentSegment);

                    if (!targetFolder) {
                        targetFolder = {
                            name: currentSegment,
                            folders: [],
                            books: []
                        };
                        folder.folders.push(targetFolder);
                    }

                    return findOrCreateFolder(targetFolder, remainingSegments);
                };

                for (const book of libraryBooks) {
                    if (!book.path || book.path.length === 0) {
                        rootFolder.books.push(book);
                    } else {
                        // Book goes in nested folder
                        const targetFolder = findOrCreateFolder(rootFolder, book.path);
                        targetFolder.books.push(book);
                    }
                }

                return rootFolder;
            }
        ));
    }

    async importEpub(book: EpubBook) {
        await db.transaction(
            'rw',
            [
                db.books,
                db.bookChapters,
                db.paragraphs,
            ],
            async () => {
                const bookId = await db.books.add({
                    title: book.title,
                });

                let chapterOrder = 0;
                for (const c of book.chapters) {
                    const chapterId = await db.bookChapters.add({
                        bookId,
                        order: chapterOrder,
                        title: c.title,
                    });

                    let paragraphOrder = 0;
                    for (const paragraph of c.paragraphs) {
                        await db.paragraphs.add({
                            chapterId,
                            order: paragraphOrder,
                            originalText: paragraph.text,
                            originalHtml: paragraph.html
                        });
                        paragraphOrder += 1;
                    }
                    chapterOrder += 1;
                }
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
                            }
                            await db.sentenceTranslations.where("paragraphTranslationId").equals(paragraphTranslationId).delete();
                        }
                        await await db.paragraphTranslations.where("paragraphId").equals(paragraphId).delete();
                    }
                    await await db.paragraphs.where("chapterId").equals(chapterId).delete();
                }
                await db.bookChapters.where("bookId").equals(bookId).delete();
            }
        );
    }

    private splitParagraphs(text: string): string[] {
        return text
            .split(/\n+/)
            .map(p => p.trim())
            .filter(p => p.length > 0);
    }

    private useQuery<T>(querier: () => T | Promise<T>): Readable<T> {
        return readable<T>(undefined, (set) => {
            return liveQuery(querier).subscribe(set).unsubscribe;
        })
    }
}
