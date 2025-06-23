import { type Book, type BookChapter, type Language, type Paragraph, type ParagraphTranslation, type SentenceTranslation, type SentenceWordTranslation, type Word, type WordTranslation, generateUID, type UUID } from "./data/db";
import { queueDb } from "./data/queueDb";
import { derived, readable, type Readable } from 'svelte/store';
import type { EpubBook } from "./data/epubLoader";
import type { ModelId } from "./data/translators/translator";
import { getConfig } from "./config";
import type { PGliteInterface } from "@electric-sql/pglite";
import type { LiveQueryResults, PGliteWithLive } from "@electric-sql/pglite/live";

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
    constructor(private pg: PGliteWithLive) { }

    getWordTranslation(sentenceWordUid: UUID): Readable<LibrarySentenceWordTranslation | null> {
        const query = `
            SELECT 
                swt.uid,
                swt.sentence_uid,
                swt."order",
                swt.original,
                swt.is_punctuation,
                swt.is_standalone_punctuation,
                swt.is_opening_parenthesis,
                swt.is_closing_parenthesis,
                swt.word_translation_uid,
                swt.word_translation_in_context,
                swt.grammar_context,
                swt.note,
                swt.created_at,
                st.full_translation as sentence_full_translation,
                pt.translating_model,
                pt.paragraph_uid,
                wt.uid as word_translation_uid_full,
                wt.language_uid as word_translation_language_uid,
                wt.original_word_uid,
                wt.translation,
                wt.translation_normalized,
                wt.created_at as word_translation_created_at,
                tl.uid as target_language_uid,
                tl.name as target_language_name,
                tl.created_at as target_language_created_at,
                w.uid as original_word_uid_full,
                w.original_language_uid,
                w.original as original_word_original,
                w.original_normalized as original_word_normalized,
                w.created_at as original_word_created_at,
                ol.uid as original_language_uid_full,
                ol.name as original_language_name,
                ol.created_at as original_language_created_at
            FROM sentence_word_translations swt
            JOIN sentence_translations st ON swt.sentence_uid = st.uid
            JOIN paragraph_translations pt ON st.paragraph_translation_uid = pt.uid
            LEFT JOIN word_translations wt ON swt.word_translation_uid = wt.uid
            LEFT JOIN languages tl ON wt.language_uid = tl.uid
            LEFT JOIN words w ON wt.original_word_uid = w.uid
            LEFT JOIN languages ol ON w.original_language_uid = ol.uid
            WHERE swt.uid = $1
        `;

        const result = this.useQuery<{
            uid: UUID,
            sentence_uid: UUID,
            order: number,
            original: string,
            is_punctuation: boolean,
            is_standalone_punctuation: boolean,
            is_opening_parenthesis: boolean,
            is_closing_parenthesis: boolean,
            word_translation_uid?: UUID,
            word_translation_in_context?: string[],
            grammar_context?: any,
            note?: string,
            created_at: number,
            sentence_full_translation: string,
            translating_model: string,
            paragraph_uid: UUID,
            word_translation_uid_full?: UUID,
            word_translation_language_uid?: UUID,
            original_word_uid?: UUID,
            translation?: string,
            translation_normalized?: string,
            word_translation_created_at?: number,
            target_language_uid?: UUID,
            target_language_name?: string,
            target_language_created_at?: number,
            original_word_uid_full?: UUID,
            original_language_uid?: UUID,
            original_word_original?: string,
            original_word_normalized?: string,
            original_word_created_at?: number,
            original_language_uid_full?: UUID,
            original_language_name?: string,
            original_language_created_at?: number
        }>(query, [sentenceWordUid]);

        const ret = derived(result, x => {
            if (x.length === 0) {
                return null;
            }

            const row = x[0];
            
            const sentenceWordFullTranslation = {
                uid: row.uid,
                sentenceUid: row.sentence_uid,
                order: row.order,
                original: row.original,
                isPunctuation: row.is_punctuation,
                isStandalonePunctuation: row.is_standalone_punctuation,
                isOpeningParenthesis: row.is_opening_parenthesis,
                isClosingParenthesis: row.is_closing_parenthesis,
                wordTranslationUid: row.word_translation_uid,
                wordTranslationInContext: row.word_translation_in_context,
                grammarContext: row.grammar_context,
                note: row.note,
                createdAt: row.created_at,
                fullSentenceTranslation: row.sentence_full_translation,
                model: row.translating_model as ModelId,
                paragraphUid: row.paragraph_uid,
            };

            // If no word translation exists, return basic translation
            if (!row.word_translation_uid_full) {
                return sentenceWordFullTranslation;
            }

            // If no target language, log error and return basic
            if (!row.target_language_uid) {
                console.log(`Can't find targetLanguage uid ${row.word_translation_language_uid}`);
                return sentenceWordFullTranslation;
            }

            // If no original word, log error and return basic
            if (!row.original_word_uid_full) {
                console.log(`Can't find original word for wordTranslation uid ${row.word_translation_uid_full}`);
                return sentenceWordFullTranslation;
            }

            // If no original language, log error and return basic
            if (!row.original_language_uid_full) {
                console.log(`Can't find originalLanguage uid ${row.original_language_uid}`);
                return sentenceWordFullTranslation;
            }

            return {
                ...sentenceWordFullTranslation,
                wordTranslation: {
                    uid: row.word_translation_uid_full,
                    languageUid: row.word_translation_language_uid!,
                    originalWordUid: row.original_word_uid!,
                    translation: row.translation!,
                    translationNormalized: row.translation_normalized!,
                    createdAt: row.word_translation_created_at!,
                    language: {
                        uid: row.target_language_uid,
                        name: row.target_language_name!,
                        createdAt: row.target_language_created_at!
                    },
                    originalWord: {
                        uid: row.original_word_uid_full,
                        originalLanguageUid: row.original_language_uid!,
                        original: row.original_word_original!,
                        originalNormalized: row.original_word_normalized!,
                        createdAt: row.original_word_created_at!,
                        originalLanguage: {
                            uid: row.original_language_uid_full,
                            name: row.original_language_name!,
                            createdAt: row.original_language_created_at!
                        }
                    }
                }
            };
        });
        
        return ret;
    }

    getBook(bookUid: UUID): Readable<LibraryBook | null> {
        const query = `
            SELECT 
                b.uid,
                b.title,
                b.path,
                b.created_at,
                bc.uid as chapter_uid,
                bc.title as chapter_title,
                bc.book_uid as chapter_book_uid,
                bc.order as chapter_order,
                bc.created_at as chapter_created_at,
                COUNT(DISTINCT p.uid) as paragraphsCount,
                COUNT(DISTINCT pt.paragraph_uid) as translatedParagraphsCount
            FROM books b
            LEFT JOIN book_chapters bc ON b.uid = bc.book_uid
            LEFT JOIN paragraphs p ON bc.uid = p.chapter_uid
            LEFT JOIN paragraph_translations pt ON p.uid = pt.paragraph_uid
            WHERE b.uid = $1
            GROUP BY b.uid, bc.uid
            ORDER BY bc.order
        `;

        const result = this.useQuery<{
            uid: UUID,
            title: string,
            path?: string[],
            created_at: number,
            chapter_uid: UUID,
            chapter_title: string,
            chapter_book_uid: UUID,
            chapter_order: number,
            chapter_created_at: number,
            paragraphsCount: number,
            translatedParagraphsCount: number
        }>(query, [bookUid]);
        
        const ret = derived(result, x => {
            if (x.length === 0) {
                return null;
            }

            const bookData = x[0];
            const chapters: BookChapter[] = x.map(row => ({
                uid: row.chapter_uid,
                createdAt: row.chapter_created_at,
                bookUid: row.chapter_book_uid,
                order: row.chapter_order,
                title: row.chapter_title,
            }));

            return {
                uid: bookData.uid,
                createdAt: bookData.created_at,
                title: bookData.title,
                path: bookData.path,
                chapters: chapters,
                paragraphsCount: bookData.paragraphsCount,
                translatedParagraphsCount: bookData.translatedParagraphsCount,
            };
        })
        return ret;
    }

    getParagraph(paragraphUid: UUID): Readable<LibraryBookParagraph | null> {
        const query = `
            SELECT 
                p.uid,
                p.chapter_uid,
                p."order",
                p.original_text,
                p.original_html,
                p.created_at,
                pt.uid as translation_uid,
                pt.language_uid as translation_language_uid,
                pt.translating_model as translation_model,  
                pt.created_at as translation_created_at,
                st.uid as sentence_uid,
                st."order" as sentence_order,
                st.full_translation as sentence_translation,
                st.created_at as sentence_created_at,
                swt.uid as word_uid,
                swt."order" as word_order,
                swt.original as word_original,
                swt.is_punctuation as word_is_punctuation,
                swt.is_standalone_punctuation as word_is_standalone_punctuation,
                swt.is_opening_parenthesis as word_is_opening_parenthesis,
                swt.is_closing_parenthesis as word_is_closing_parenthesis,
                swt.word_translation_uid as word_translation_uid,
                swt.word_translation_in_context as word_translation_in_context,
                swt.grammar_context as word_grammar_context,
                swt.note as word_note,
                swt.created_at as word_created_at
            FROM paragraphs p
            LEFT JOIN paragraph_translations pt ON p.uid = pt.paragraph_uid
            LEFT JOIN sentence_translations st ON pt.uid = st.paragraph_translation_uid
            LEFT JOIN sentence_word_translations swt ON st.uid = swt.sentence_uid
            WHERE p.uid = $1
            ORDER BY pt.created_at DESC, st."order", swt."order"
        `;

        const result = this.useQuery<{
            uid: UUID,
            chapter_uid: UUID,
            order: number,
            original_text: string,
            original_html?: string,
            created_at: number,
            translation_uid?: UUID,
            translation_language_uid?: UUID,
            translation_model?: string,
            translation_created_at?: number,
            sentence_uid?: UUID,
            sentence_order?: number,
            sentence_translation?: string,
            sentence_created_at?: number,
            word_uid?: UUID,
            word_order?: number,
            word_original?: string,
            word_is_punctuation?: boolean,
            word_is_standalone_punctuation?: boolean,
            word_is_opening_parenthesis?: boolean,
            word_is_closing_parenthesis?: boolean,
            word_translation_uid?: UUID,
            word_translation_in_context?: string[],
            word_grammar_context?: any,
            word_note?: string,
            word_created_at?: number
        }>(query, [paragraphUid]);
        
        const ret = derived(result, x => {
            if (x.length === 0) {
                return null;
            }

            const paragraphData = x[0];
            
            // If there's no translation data, return paragraph without translation
            if (!paragraphData.translation_uid) {
                return {
                    uid: paragraphData.uid,
                    chapterUid: paragraphData.chapter_uid,
                    order: paragraphData.order,
                    originalText: paragraphData.original_text,
                    originalHtml: paragraphData.original_html,
                    createdAt: paragraphData.created_at,
                    translation: undefined,
                };
            }

            // Group sentences and words
            const sentenceMap = new Map<UUID, {
                uid: UUID,
                paragraphTranslationUid: UUID,
                order: number,
                fullTranslation: string,
                createdAt: number,
                words: SentenceWordTranslation[]
            }>();

            for (const row of x) {
                if (row.sentence_uid) {
                    if (!sentenceMap.has(row.sentence_uid)) {
                        sentenceMap.set(row.sentence_uid, {
                            uid: row.sentence_uid,
                            paragraphTranslationUid: row.translation_uid!,
                            order: row.sentence_order!,
                            fullTranslation: row.sentence_translation!,
                            createdAt: row.sentence_created_at!,
                            words: []
                        });
                    }

                    if (row.word_uid) {
                        sentenceMap.get(row.sentence_uid)!.words.push({
                            uid: row.word_uid,
                            sentenceUid: row.sentence_uid,
                            order: row.word_order!,
                            original: row.word_original!,
                            isPunctuation: row.word_is_punctuation!,
                            isStandalonePunctuation: row.word_is_standalone_punctuation!,
                            isOpeningParenthesis: row.word_is_opening_parenthesis!,
                            isClosingParenthesis: row.word_is_closing_parenthesis!,
                            wordTranslationUid: row.word_translation_uid,
                            wordTranslationInContext: row.word_translation_in_context,
                            grammarContext: row.word_grammar_context,
                            note: row.word_note,
                            createdAt: row.word_created_at!,
                        });
                    }
                }
            }

            // Convert to array and sort
            const sentences = Array.from(sentenceMap.values())
                .sort((a, b) => a.order - b.order)
                .map(sentence => ({
                    ...sentence,
                    words: sentence.words.sort((a, b) => a.order - b.order)
                }));

            return {
                uid: paragraphData.uid,
                chapterUid: paragraphData.chapter_uid,
                order: paragraphData.order,
                originalText: paragraphData.original_text,
                originalHtml: paragraphData.original_html,
                createdAt: paragraphData.created_at,
                translation: {
                    uid: paragraphData.translation_uid,
                    paragraphUid: paragraphData.uid,
                    languageUid: paragraphData.translation_language_uid!,
                    translatingModel: paragraphData.translation_model! as ModelId,
                    createdAt: paragraphData.translation_created_at!,
                    sentences: sentences,
                },
            };
        });
        
        return ret;
    }

    getChapter(chapterUid: UUID): Readable<LibraryBookChapter | null> {
        const query = `
            SELECT 
                bc.uid,
                bc.book_uid,
                bc."order",
                bc.title,
                bc.created_at,
                p.uid as paragraph_uid,
                p.chapter_uid as paragraph_chapter_uid,
                p."order" as paragraph_order,
                p.original_text,
                p.original_html,
                p.created_at as paragraph_created_at
            FROM book_chapters bc
            LEFT JOIN paragraphs p ON bc.uid = p.chapter_uid
            WHERE bc.uid = $1
            ORDER BY p."order"
        `;

        const result = this.useQuery<{
            uid: UUID,
            book_uid: UUID,
            order: number,
            title?: string,
            created_at: number,
            paragraph_uid?: UUID,
            paragraph_chapter_uid?: UUID,
            paragraph_order?: number,
            original_text?: string,
            original_html?: string,
            paragraph_created_at?: number
        }>(query, [chapterUid]);

        const ret = derived(result, x => {
            if (x.length === 0) {
                return null;
            }

            const chapterData = x[0];
            const paragraphs: Paragraph[] = x
                .filter(row => row.paragraph_uid)
                .map(row => ({
                    uid: row.paragraph_uid!,
                    chapterUid: row.paragraph_chapter_uid!,
                    order: row.paragraph_order!,
                    originalText: row.original_text!,
                    originalHtml: row.original_html,
                    createdAt: row.paragraph_created_at!,
                }));

            return {
                uid: chapterData.uid,
                bookUid: chapterData.book_uid,
                order: chapterData.order,
                title: chapterData.title,
                createdAt: chapterData.created_at,
                paragraphs: paragraphs,
            };
        });
        
        return ret;
    }

    getLibraryBooks(): Readable<LibraryFolder> {
        const query = `
            SELECT 
                b.uid,
                b.title,
                b.path,
                b.created_at,
                bc.uid as chapter_uid,
                bc.book_uid as chapter_book_uid,
                bc."order" as chapter_order,
                bc.title as chapter_title,
                bc.created_at as chapter_created_at,
                COUNT(DISTINCT p.uid) as paragraphs_count,
                COUNT(DISTINCT pt.paragraph_uid) as translated_paragraphs_count
            FROM books b
            LEFT JOIN book_chapters bc ON b.uid = bc.book_uid
            LEFT JOIN paragraphs p ON bc.uid = p.chapter_uid
            LEFT JOIN paragraph_translations pt ON p.uid = pt.paragraph_uid
            GROUP BY b.uid, bc.uid
            ORDER BY b.created_at, bc."order"
        `;

        const result = this.useQuery<{
            uid: UUID,
            title: string,
            path?: string[],
            created_at: number,
            chapter_uid?: UUID,
            chapter_book_uid?: UUID,
            chapter_order?: number,
            chapter_title?: string,
            chapter_created_at?: number,
            paragraphs_count: number,
            translated_paragraphs_count: number
        }>(query, []);

        const ret = derived(result, x => {
            // Handle undefined case when query hasn't loaded yet
            if (!x) {
                return {
                    name: undefined,
                    folders: [],
                    books: [],
                };
            }

            // Group rows by book
            const bookMap = new Map<UUID, {
                book: Book,
                chapters: BookChapter[],
                paragraphsCount: number,
                translatedParagraphsCount: number
            }>();

            for (const row of x) {
                if (!bookMap.has(row.uid)) {
                    bookMap.set(row.uid, {
                        book: {
                            uid: row.uid,
                            title: row.title,
                            path: row.path || undefined,
                            createdAt: row.created_at
                        },
                        chapters: [],
                        paragraphsCount: 0,
                        translatedParagraphsCount: 0
                    });
                }

                const bookData = bookMap.get(row.uid)!;
                
                // Accumulate paragraph counts (avoid double-counting from multiple chapters)
                bookData.paragraphsCount = Math.max(bookData.paragraphsCount, row.paragraphs_count);
                bookData.translatedParagraphsCount = Math.max(bookData.translatedParagraphsCount, row.translated_paragraphs_count);

                // Add chapter if it exists and hasn't been added yet
                if (row.chapter_uid && !bookData.chapters.find(c => c.uid === row.chapter_uid)) {
                    bookData.chapters.push({
                        uid: row.chapter_uid,
                        bookUid: row.chapter_book_uid!,
                        order: row.chapter_order!,
                        title: row.chapter_title,
                        createdAt: row.chapter_created_at!
                    });
                }
            }

            // Convert to library books
            const libraryBooks: LibraryBook[] = Array.from(bookMap.values()).map(({ book, chapters, paragraphsCount, translatedParagraphsCount }) => ({
                ...book,
                chapters: chapters.sort((a, b) => a.order - b.order),
                paragraphsCount,
                translatedParagraphsCount
            }));

            // Build folder structure
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
        });
        
        return ret;
    }

    async importEpub(book: EpubBook) {
        const config = await getConfig();
        const model = config.model;

        const paragraphUids: UUID[] = [];

        // Use SQL transaction for importing
        await this.pg.query('BEGIN');
        
        try {
            const bookUid = generateUID();
            await this.pg.query(
                'INSERT INTO books (uid, title, created_at) VALUES ($1, $2, $3)',
                [bookUid, book.title, Date.now()]
            );

            let chapterOrder = 0;
            for (const c of book.chapters) {
                const chapterUid = generateUID();
                await this.pg.query(
                    'INSERT INTO book_chapters (uid, book_uid, "order", title, created_at) VALUES ($1, $2, $3, $4, $5)',
                    [chapterUid, bookUid, chapterOrder, c.title, Date.now()]
                );

                let paragraphOrder = 0;
                for (const paragraph of c.paragraphs) {
                    const paragraphUid = generateUID();
                    await this.pg.query(
                        'INSERT INTO paragraphs (uid, chapter_uid, "order", original_text, original_html, created_at) VALUES ($1, $2, $3, $4, $5, $6)',
                        [paragraphUid, chapterUid, paragraphOrder, paragraph.text, paragraph.html, Date.now()]
                    );

                    paragraphUids.push(paragraphUid);
                    paragraphOrder += 1;
                }
                chapterOrder += 1;
            }

            await this.pg.query('COMMIT');
        } catch (error) {
            await this.pg.query('ROLLBACK');
            throw error;
        }

        // Schedule translations after the main transaction is complete
        await Promise.all(paragraphUids.map(puid => this.scheduleTranslationInternal(puid, model)));
    }

    async importText(title: string, text: string) {
        const config = await getConfig();
        const model = config.model;

        const paragraphUids: UUID[] = [];

        // Use SQL transaction for importing
        await this.pg.query('BEGIN');
        
        try {
            const bookUid = generateUID();
            await this.pg.query(
                'INSERT INTO books (uid, title, created_at) VALUES ($1, $2, $3)',
                [bookUid, title, Date.now()]
            );

            const chapterUid = generateUID();
            await this.pg.query(
                'INSERT INTO book_chapters (uid, book_uid, "order", created_at) VALUES ($1, $2, $3, $4)',
                [chapterUid, bookUid, 0, Date.now()]
            );

            const paragraphs = this.splitParagraphs(text);

            let order = 0;
            for (const paragraph of paragraphs) {
                const paragraphUid = generateUID();
                await this.pg.query(
                    'INSERT INTO paragraphs (uid, chapter_uid, "order", original_text, created_at) VALUES ($1, $2, $3, $4, $5)',
                    [paragraphUid, chapterUid, order, paragraph, Date.now()]
                );
                paragraphUids.push(paragraphUid);
                order += 1;
            }

            await this.pg.query('COMMIT');
        } catch (error) {
            await this.pg.query('ROLLBACK');
            throw error;
        }

        // Schedule translations after the main transaction is complete
        await Promise.all(paragraphUids.map(puid => this.scheduleTranslationInternal(puid, model)));
    }

    public async scheduleTranslation(paragraphUid: UUID) {
        const config = await getConfig();
        const model = config.model;

        await this.scheduleTranslationInternal(paragraphUid, model);
    }

    private async scheduleTranslationInternal(paragraphUid: UUID, model: ModelId) {
        const requestExists = await queueDb.directTranslationRequests.where("paragraphUid").equals(paragraphUid).count() > 0;
        if (!requestExists) {
            await queueDb.directTranslationRequests.add({
                paragraphUid: paragraphUid,
                model,
            });
        }
    }

    private async cleanupTranslationRequests(bookUids: UUID[]): Promise<void> {
        if (bookUids.length === 0) return;

        // Use SQL query to get all paragraph UIDs for the books
        const query = `
            SELECT DISTINCT p.uid as paragraph_uid
            FROM books b
            JOIN book_chapters bc ON b.uid = bc.book_uid
            JOIN paragraphs p ON bc.uid = p.chapter_uid
            WHERE b.uid = ANY($1)
        `;

        const result = await this.pg.query(query, [bookUids]);
        const allParagraphUids = result.rows.map(row => (row as { paragraph_uid: UUID }).paragraph_uid);

        // Delete translation requests from queue database
        if (allParagraphUids.length > 0) {
            await queueDb.directTranslationRequests.where("paragraphUid").anyOf(allParagraphUids).delete();
        }
    }

    async deleteBook(bookUid: UUID) {
        // First handle the queueDb operations separately
        await this.cleanupTranslationRequests([bookUid]);

        // Then handle main database operations using SQL transaction
        await this.pg.query('BEGIN');
        try {
            await this.deleteBookInternal(bookUid);
            await this.pg.query('COMMIT');
        } catch (error) {
            await this.pg.query('ROLLBACK');
            throw error;
        }
    }

    async moveBook(bookUid: UUID, newPath: string[] | null) {
        await this.pg.query('BEGIN');
        try {
            await this.moveBookInternal(bookUid, newPath);
            await this.pg.query('COMMIT');
        } catch (error) {
            await this.pg.query('ROLLBACK');
            throw error;
        }
    }

    private async deleteBookInternal(bookUid: UUID) {
        // Use SQL to delete all related data in proper order (foreign key constraints)
        const queries = [
            // Delete sentence word translations
            `DELETE FROM sentence_word_translations 
             WHERE sentence_uid IN (
                 SELECT st.uid FROM sentence_translations st
                 JOIN paragraph_translations pt ON st.paragraph_translation_uid = pt.uid
                 JOIN paragraphs p ON pt.paragraph_uid = p.uid
                 JOIN book_chapters bc ON p.chapter_uid = bc.uid
                 WHERE bc.book_uid = $1
             )`,
            // Delete sentence translations
            `DELETE FROM sentence_translations 
             WHERE paragraph_translation_uid IN (
                 SELECT pt.uid FROM paragraph_translations pt
                 JOIN paragraphs p ON pt.paragraph_uid = p.uid
                 JOIN book_chapters bc ON p.chapter_uid = bc.uid
                 WHERE bc.book_uid = $1
             )`,
            // Delete paragraph translations
            `DELETE FROM paragraph_translations 
             WHERE paragraph_uid IN (
                 SELECT p.uid FROM paragraphs p
                 JOIN book_chapters bc ON p.chapter_uid = bc.uid
                 WHERE bc.book_uid = $1
             )`,
            // Delete paragraphs
            `DELETE FROM paragraphs 
             WHERE chapter_uid IN (
                 SELECT bc.uid FROM book_chapters bc
                 WHERE bc.book_uid = $1
             )`,
            // Delete book chapters
            `DELETE FROM book_chapters WHERE book_uid = $1`,
            // Delete book
            `DELETE FROM books WHERE uid = $1`
        ];

        for (const query of queries) {
            await this.pg.query(query, [bookUid]);
        }
    }

    private async moveBookInternal(bookUid: UUID, newPath: string[] | null) {
        const query = `UPDATE books SET path = $1 WHERE uid = $2`;
        await this.pg.query(query, [newPath || null, bookUid]);
    }

    async deleteBooksInBatch(bookUids: UUID[]) {
        // First handle the queueDb operations for all books
        await this.cleanupTranslationRequests(bookUids);

        // Then handle main database operations using SQL transaction
        await this.pg.query('BEGIN');
        try {
            for (const bookUid of bookUids) {
                await this.deleteBookInternal(bookUid);
            }
            await this.pg.query('COMMIT');
        } catch (error) {
            await this.pg.query('ROLLBACK');
            throw error;
        }
    }

    async moveBooksInBatch(bookUids: UUID[], newPath: string[] | null) {
        await this.pg.query('BEGIN');
        try {
            for (const bookUid of bookUids) {
                await this.moveBookInternal(bookUid, newPath);
            }
            await this.pg.query('COMMIT');
        } catch (error) {
            await this.pg.query('ROLLBACK');
            throw error;
        }
    }

    private splitParagraphs(text: string): string[] {
        return text
            .split(/\n+/)
            .map(p => p.trim())
            .filter(p => p.length > 0);
    }

    private useQuery<T>(query: string, args: any[]): Readable<T[]> {
        let unsubscribe: ((callback?: ((results: LiveQueryResults<{ [key: string]: any; }>) => void) | undefined) => Promise<void>) | (() => void) | null = null;

        return readable<T[]>([], (set) => {
            this.pg.live.query(query, args, (res) => {
                set(res.rows.map(x => x as T));
            }).then(x => {
                unsubscribe = x.unsubscribe;
            });

            const callOrScheduleUnsubsribe = () => {
                if (unsubscribe) {
                    unsubscribe();
                } else {
                    setTimeout(callOrScheduleUnsubsribe, 100);
                }
            }

            return () => callOrScheduleUnsubsribe();
        });
    }
}
