import { liveQuery } from "dexie";
import { db, type Book, type BookChapter, type Language, type Paragraph, type ParagraphTranslation, type SentenceTranslation, type SentenceWordTranslation, type Word, type WordTranslation, generateUID, type UUID } from "./data/db";
import { readable, type Readable } from 'svelte/store';
import type { EpubBook } from "./data/epubLoader";
import type { ModelId } from "./data/translators/translator";
import { getConfig } from "./config";

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
    paragraphUid: UUID,
    wordTranslation?: LibraryWordTranslation,
}

export class Library {
    getWordTranslation(sentenceWordUid: UUID): Readable<LibrarySentenceWordTranslation | null> {
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
                    const sentenceWordTranslation = await db.sentenceWordTranslations.where('uid').equals(sentenceWordUid).first();
                    if (!sentenceWordTranslation) {
                        return null;
                    }

                    const sentenceTranslation = await db.sentenceTranslations.where('uid').equals(sentenceWordTranslation.sentenceUid).first();
                    if (!sentenceTranslation) {
                        return null;
                    }

                    const paragraphTranslation = await db.paragraphTranslations.where('uid').equals(sentenceTranslation.paragraphTranslationUid).first();
                    if (!paragraphTranslation) {
                        return null;
                    }

                    const sentenceWordFullTranslation = {
                        ...sentenceWordTranslation,
                        fullSentenceTranslation: sentenceTranslation.fullTranslation,
                        model: paragraphTranslation.translatingModel,
                        paragraphUid: paragraphTranslation.paragraphUid,
                    };

                    const wordTranslation = sentenceWordTranslation.wordTranslationUid != null ? await db.wordTranslations.where('uid').equals(sentenceWordTranslation.wordTranslationUid).first() : null;
                    if (!wordTranslation) {
                        return sentenceWordFullTranslation;
                    }

                    const targetLanguage = await db.languages.where('uid').equals(wordTranslation.languageUid).first();
                    if (!targetLanguage) {
                        console.log(`Can't find targetLanguage uid ${wordTranslation.languageUid}`);
                        return sentenceWordFullTranslation;
                    }

                    const originalWord = await db.words.where('uid').equals(wordTranslation.originalWordUid).first();
                    if (!originalWord) {
                        console.log(`Can't find original word for wordTranslation uid ${wordTranslation.uid}`);
                        return sentenceWordFullTranslation;
                    }

                    const originalLanguage = await db.languages.where('uid').equals(originalWord.originalLanguageUid).first();
                    if (!originalLanguage) {
                        console.log(`Can't find originalLanguage uid ${originalWord.originalLanguageUid}`);
                        return sentenceWordFullTranslation;
                    }

                    let ret: LibrarySentenceWordTranslation = {
                        ...sentenceWordTranslation,
                        fullSentenceTranslation: sentenceTranslation.fullTranslation,
                        model: paragraphTranslation.translatingModel,
                        paragraphUid: paragraphTranslation.paragraphUid,
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

    getBook(bookUid: UUID): Readable<LibraryBook | null> {
        return this.useQuery(() => db.transaction(
            'r',
            [
                db.books,
                db.bookChapters,
                db.paragraphs,
                db.paragraphTranslations
            ],
            async () => {
                const book = await db.books.where('uid').equals(bookUid).first();
                if (!book) {
                    return null;
                }
                const chapters = await db.bookChapters.where("bookUid").equals(book.uid).sortBy("order");

                const paragraphs = await db.paragraphs.where("chapterUid").anyOf(chapters.map(c => c.uid)).toArray();
                const paragraphUids = paragraphs.map(p => p.uid);
                const translatedParagraphs = await db.paragraphTranslations.where("paragraphUid").anyOf(paragraphUids).toArray();
                const translatedParagraphsCount = (new Set(translatedParagraphs.map(p => p.paragraphUid))).size;

                return {
                    ...book,
                    chapters,
                    paragraphsCount: paragraphs.length,
                    translatedParagraphsCount
                }
            }
        ));
    }

    getParagraph(paragraphUid: UUID): Readable<LibraryBookParagraph | null> {
        return this.useQuery(() => db.transaction(
            'r',
            [
                db.paragraphs,
                db.paragraphTranslations,
                db.sentenceTranslations,
                db.sentenceWordTranslations,
            ],
            async () => {
                const paragraph = await db.paragraphs.where('uid').equals(paragraphUid).first();
                if (!paragraph) {
                    return null;
                }

                const translations = await db.paragraphTranslations.where('paragraphUid').equals(paragraphUid).toArray();
                if (translations.length === 0) {
                    return {
                        ...paragraph,
                        translation: undefined,
                    };
                }
                const librarySentences: LibrarySentenceTranslation[] = [];
                for (const translation of translations) {
                    const sentences = await db.sentenceTranslations.where('paragraphTranslationUid').equals(translation.uid).sortBy('order');
                    for (const sentence of sentences) {
                        const words = await db.sentenceWordTranslations.where('sentenceUid').equals(sentence.uid).sortBy("order");
                        librarySentences.push({
                            ...sentence,
                            words,
                        });
                    }
                }
                // translations.sort((a, b) => b.id - a.id);
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

    getChapter(chapterUid: UUID): Readable<LibraryBookChapter | null> {
        return this.useQuery(() => db.transaction(
            'r',
            [
                db.bookChapters,
                db.paragraphs,
            ],
            async () => {
                const chapter = await db.bookChapters.where('uid').equals(chapterUid).first();
                if (!chapter) {
                    return null;
                }
                const paragraphs = await db.paragraphs.where('chapterUid').equals(chapter.uid).sortBy('order');
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
                    const chapters = await db.bookChapters.where("bookUid").equals(b.uid).sortBy("order");

                    const paragraphs = await db.paragraphs.where("chapterUid").anyOf(chapters.map(c => c.uid)).toArray();
                    const paragraphUids = paragraphs.map(p => p.uid);
                    const translatedParagraphsCount = await db.paragraphTranslations.where("paragraphUid").anyOf(paragraphUids).count()

                    return {
                        ...b,
                        chapters,
                        paragraphsCount: paragraphs.length,
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
        const config = await getConfig();
        const model = config.model;

        await db.transaction(
            'rw',
            [
                db.books,
                db.bookChapters,
                db.paragraphs,
                db.directTranslationRequests,
            ],
            async () => {
                const bookUid = generateUID();
                await db.books.add({
                    title: book.title,
                    uid: bookUid,
                    createdAt: Date.now(),
                });

                const paragraphUids: UUID[] = [];

                let chapterOrder = 0;
                for (const c of book.chapters) {
                    const chapterUid = generateUID();
                    await db.bookChapters.add({
                        bookUid,
                        order: chapterOrder,
                        title: c.title,
                        uid: chapterUid,
                        createdAt: Date.now(),
                    });

                    let paragraphOrder = 0;
                    for (const paragraph of c.paragraphs) {
                        const paragraphUid = generateUID();
                        await db.paragraphs.add({
                            chapterUid,
                            order: paragraphOrder,
                            originalText: paragraph.text,
                            originalHtml: paragraph.html,
                            uid: paragraphUid,
                            createdAt: Date.now(),
                        });

                        paragraphUids.push(paragraphUid);

                        paragraphOrder += 1;
                    }
                    chapterOrder += 1;
                }

                await Promise.all(paragraphUids.map(puid => this.scheduleTranslationInternal(puid, model)));
            }
        );
    }

    async importText(title: string, text: string) {
        const config = await getConfig();
        const model = config.model;

        await db.transaction(
            'rw',
            [
                db.books,
                db.bookChapters,
                db.paragraphs,
                db.directTranslationRequests,
            ],
            async () => {
                const bookUid = generateUID();
                await db.books.add({
                    title,
                    uid: bookUid,
                    createdAt: Date.now(),
                });

                const chapterUid = generateUID();
                await db.bookChapters.add({
                    bookUid,
                    order: 0,
                    uid: chapterUid,
                    createdAt: Date.now(),
                });

                const paragraphs = this.splitParagraphs(text);

                const paragraphUids: UUID[] = [];
                let order = 0;
                for (const paragraph of paragraphs) {
                    const paragraphUid = generateUID();
                    await db.paragraphs.add({
                        chapterUid,
                        order,
                        originalText: paragraph,
                        uid: paragraphUid,
                        createdAt: Date.now(),
                    });
                    paragraphUids.push(paragraphUid);
                    order += 1;
                }

                await Promise.all(paragraphUids.map(puid => this.scheduleTranslationInternal(puid, model)));
            }
        );
    }

    public async scheduleTranslation(paragraphUid: UUID) {
        const config = await getConfig();
        const model = config.model;

        await db.transaction(
            'rw',
            [
                db.paragraphs,
                db.directTranslationRequests,
            ],
            async () => {
                await this.scheduleTranslationInternal(paragraphUid, model);
            }
        )
    }

    private async scheduleTranslationInternal(paragraphUid: UUID, model: ModelId) {
        const requestExists = await db.directTranslationRequests.where("paragraphUid").equals(paragraphUid).count() > 0;
        if (!requestExists) {
            await db.directTranslationRequests.add({
                paragraphUid: paragraphUid,
                model,
            });
        }
    }

    async deleteBook(bookUid: UUID) {
        await db.transaction('rw', [
            db.books,
            db.bookChapters,
            db.paragraphs,
            db.paragraphTranslations,
            db.sentenceTranslations,
            db.sentenceWordTranslations,
            db.directTranslationRequests,
        ],
            async () => {
                await this.deleteBookInternal(bookUid);
            }
        );
    }

    async moveBook(bookUid: UUID, newPath: string[] | null) {
        await db.transaction('rw', [db.books], async () => {
            await this.moveBookInternal(bookUid, newPath);
        });
    }

    private async deleteBookInternal(bookUid: UUID) {
        const chapterUids = (await db.bookChapters.where("bookUid").equals(bookUid).toArray()).map(c => c.uid);
        const paragraphUids = (await db.paragraphs.where("chapterUid").anyOf(chapterUids).toArray()).map(p => p.uid);
        const paragraphTranslationUids = (await db.paragraphTranslations.where("paragraphUid").anyOf(paragraphUids).toArray()).map(pt => pt.uid);
        const sentenceTranslationUids = (await db.sentenceTranslations.where("paragraphTranslationUid").anyOf(paragraphTranslationUids).toArray()).map(st => st.uid);

        await db.sentenceWordTranslations.where("sentenceUid").anyOf(sentenceTranslationUids).delete();
        await db.sentenceTranslations.where("paragraphTranslationUid").anyOf(paragraphTranslationUids).delete();
        await db.paragraphTranslations.where("paragraphUid").anyOf(paragraphUids).delete();
        await db.directTranslationRequests.where("paragraphUid").anyOf(paragraphUids).delete();
        await db.paragraphs.where("chapterUid").anyOf(chapterUids).delete();
        await db.bookChapters.where("bookUid").equals(bookUid).delete();
        await db.books.where("uid").equals(bookUid).delete();
    }

    private async moveBookInternal(bookUid: UUID, newPath: string[] | null) {
        await db.books.where("uid").equals(bookUid).modify({ path: newPath || undefined });
    }

    async deleteBooksInBatch(bookUids: UUID[]) {
        await db.transaction('rw', [
            db.books,
            db.bookChapters,
            db.paragraphs,
            db.paragraphTranslations,
            db.sentenceTranslations,
            db.sentenceWordTranslations,
            db.directTranslationRequests,
        ],
            async () => {
                for (const bookUid of bookUids) {
                    await this.deleteBookInternal(bookUid);
                }
            }
        );
    }

    async moveBooksInBatch(bookUids: UUID[], newPath: string[] | null) {
        await db.transaction('rw', [db.books], async () => {
            for (const bookUid of bookUids) {
                await this.moveBookInternal(bookUid, newPath);
            }
        });
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
