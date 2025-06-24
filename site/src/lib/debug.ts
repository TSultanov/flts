import { cacheDb } from './data/cache';
import JSZip from 'jszip';
import { encode, decode } from '@msgpack/msgpack';
import dbSql from './data/dbSql';

export const debug = {
    /**
     * Downloads the current state of the Cache DB table as a JSON file
     */
    async downloadCache(): Promise<void> {
        try {
            const cacheEntries = await cacheDb.queryCache.toArray();
            const jsonData = JSON.stringify(cacheEntries, null, 2);
            const blob = new Blob([jsonData], { type: 'application/json' });
            const url = URL.createObjectURL(blob);
            const link = document.createElement('a');
            link.href = url;
            link.download = `flts-cache-${new Date().toISOString().replace(/[:.]/g, '-')}.json`;
            document.body.appendChild(link);
            link.click();
            document.body.removeChild(link);
            URL.revokeObjectURL(url);
            console.log(`Downloaded cache with ${cacheEntries.length} entries`);
        } catch (error) {
            console.error('Failed to download cache:', error);
            throw error;
        }
    },

    /**
     * Imports cache data from a JSON file selected by the user
     */
    async importCache(): Promise<void> {
        return new Promise((resolve, reject) => {
            const fileInput = document.createElement('input');
            fileInput.type = 'file';
            fileInput.accept = '.json';
            fileInput.onchange = async (event) => {
                try {
                    const file = (event.target as HTMLInputElement).files?.[0];
                    if (!file) {
                        reject(new Error('No file selected'));
                        return;
                    }
                    const fileText = await file.text();
                    let cacheData;
                    try {
                        cacheData = JSON.parse(fileText);
                    } catch (parseError) {
                        reject(new Error('Invalid JSON file'));
                        return;
                    }
                    if (!Array.isArray(cacheData)) {
                        reject(new Error('Cache data must be an array'));
                        return;
                    }
                    const currentTime = Date.now();
                    for (const entry of cacheData) {
                        if (!entry.hash || typeof entry.hash !== 'string') {
                            reject(new Error('Invalid cache entry: missing or invalid hash'));
                            return;
                        }
                        if (entry.value === undefined) {
                            reject(new Error('Invalid cache entry: missing value'));
                            return;
                        }
                        if (!entry.createdAt || typeof entry.createdAt !== 'number') {
                            entry.createdAt = currentTime;
                        }
                    }
                    await cacheDb.transaction('rw', cacheDb.queryCache, async () => {
                        await cacheDb.queryCache.clear();
                        await cacheDb.queryCache.bulkAdd(cacheData);
                    });
                    console.log(`Imported ${cacheData.length} cache entries`);
                    resolve();
                } catch (error) {
                    console.error('Failed to import cache:', error);
                    reject(error);
                }
            };
            fileInput.oncancel = () => {
                reject(new Error('File selection cancelled'));
            };
            fileInput.click();
        });
    },

    /**
     * Compresses data using the Compression Streams API with gzip compression
     */
    async compressData(data: Uint8Array): Promise<Uint8Array> {
        const compressionStream = new CompressionStream('gzip');
        const writer = compressionStream.writable.getWriter();
        const reader = compressionStream.readable.getReader();
        
        // Start writing the data
        const writePromise = writer.write(data).then(() => writer.close());
        
        // Read the compressed chunks
        const chunks: Uint8Array[] = [];
        const readPromise = (async () => {
            let done = false;
            while (!done) {
                const { value, done: readerDone } = await reader.read();
                done = readerDone;
                if (value) {
                    chunks.push(value);
                }
            }
        })();
        
        // Wait for both operations to complete
        await Promise.all([writePromise, readPromise]);
        
        // Combine all chunks into a single Uint8Array
        const totalLength = chunks.reduce((sum, chunk) => sum + chunk.length, 0);
        const result = new Uint8Array(totalLength);
        let offset = 0;
        for (const chunk of chunks) {
            result.set(chunk, offset);
            offset += chunk.length;
        }
        
        return result;
    },

    /**
     * Decompresses data using the Decompression Streams API with gzip decompression
     */
    async decompressData(compressedData: Uint8Array): Promise<Uint8Array> {
        const decompressionStream = new DecompressionStream('gzip');
        const writer = decompressionStream.writable.getWriter();
        const reader = decompressionStream.readable.getReader();
        
        // Start writing the compressed data
        const writePromise = writer.write(compressedData).then(() => writer.close());
        
        // Read the decompressed chunks
        const chunks: Uint8Array[] = [];
        const readPromise = (async () => {
            let done = false;
            while (!done) {
                const { value, done: readerDone } = await reader.read();
                done = readerDone;
                if (value) {
                    chunks.push(value);
                }
            }
        })();
        
        // Wait for both operations to complete
        await Promise.all([writePromise, readPromise]);
        
        // Combine all chunks into a single Uint8Array
        const totalLength = chunks.reduce((sum, chunk) => sum + chunk.length, 0);
        const result = new Uint8Array(totalLength);
        let offset = 0;
        for (const chunk of chunks) {
            result.set(chunk, offset);
            offset += chunk.length;
        }
        
        return result;
    },

    /**
     * Exports the entire library as a zip archive with MessagePack files.
     * - dictionary.pack: contains words and wordTranslations tables (compressed with gzip)
     * - books/book_<book uid>.pack: contains all chapters, paragraphs, sentences, and translations for each book (compressed with gzip)
     */
    async exportLibrary(): Promise<void> {
        console.time('exportLibrary-total');
        
        const pg = dbSql.getPgInstance();
        
        // Fetch dictionary tables sequentially with timing logs
        console.time('sql-words');
        const wordsResult = await pg.query('SELECT * FROM words ORDER BY created_at');
        console.timeEnd('sql-words');
        
        console.time('sql-wordTranslations');
        const wordTranslationsResult = await pg.query('SELECT * FROM word_translations ORDER BY created_at');
        console.timeEnd('sql-wordTranslations');

        // Prepare dictionary MessagePack and compress it
        console.time('dictionary-encode');
        const dictionaryPack = encode({ 
            words: wordsResult.rows, 
            wordTranslations: wordTranslationsResult.rows 
        });
        console.timeEnd('dictionary-encode');
        
        console.time('dictionary-compress');
        const compressedDictionaryPack = await this.compressData(dictionaryPack);
        console.timeEnd('dictionary-compress');

        // Fetch all books
        console.time('sql-books');
        const booksResult = await pg.query('SELECT * FROM books ORDER BY created_at');
        console.timeEnd('sql-books');
        const books = booksResult.rows;
        console.log(`Processing ${books.length} books for export`);

        // Prepare zip
        const zip = new JSZip();
        zip.file('dictionary.pack', compressedDictionaryPack);
        const booksFolder = zip.folder('books');

        // For each book, gather all related data and pack
        for (const book of books as any[]) {
            console.time(`book-${book.uid}-data-fetch`);

            // Fetch related data sequentially with timing logs
            console.time(`sql-book-${book.uid}-chapters`);
            const chaptersResult = await pg.query('SELECT * FROM book_chapters WHERE book_uid = $1 ORDER BY "order"', [book.uid]);
            console.timeEnd(`sql-book-${book.uid}-chapters`);

            console.time(`sql-book-${book.uid}-paragraphs`);
            const paragraphsResult = await pg.query(`
                SELECT p.* FROM paragraphs p
                JOIN book_chapters bc ON p.chapter_uid = bc.uid
                WHERE bc.book_uid = $1
                ORDER BY bc."order", p."order"
            `, [book.uid]);
            console.timeEnd(`sql-book-${book.uid}-paragraphs`);

            console.time(`sql-book-${book.uid}-paragraphTranslations`);
            const paragraphTranslationsResult = await pg.query(`
                SELECT pt.* FROM paragraph_translations pt
                JOIN paragraphs p ON pt.paragraph_uid = p.uid
                JOIN book_chapters bc ON p.chapter_uid = bc.uid
                WHERE bc.book_uid = $1
                ORDER BY bc."order", p."order", pt.created_at
            `, [book.uid]);
            console.timeEnd(`sql-book-${book.uid}-paragraphTranslations`);

            console.time(`sql-book-${book.uid}-sentenceTranslations`);
            const sentenceTranslationsResult = await pg.query(`
                SELECT st.* FROM sentence_translations st
                JOIN paragraph_translations pt ON st.paragraph_translation_uid = pt.uid
                JOIN paragraphs p ON pt.paragraph_uid = p.uid
                JOIN book_chapters bc ON p.chapter_uid = bc.uid
                WHERE bc.book_uid = $1
                ORDER BY bc."order", p."order", st."order"
            `, [book.uid]);
            console.timeEnd(`sql-book-${book.uid}-sentenceTranslations`);

            console.time(`sql-book-${book.uid}-sentenceWordTranslations`);
            const sentenceWordTranslationsResult = await pg.query(`
                SELECT swt.* FROM sentence_word_translations swt
                JOIN sentence_translations st ON swt.sentence_uid = st.uid
                JOIN paragraph_translations pt ON st.paragraph_translation_uid = pt.uid
                JOIN paragraphs p ON pt.paragraph_uid = p.uid
                JOIN book_chapters bc ON p.chapter_uid = bc.uid
                WHERE bc.book_uid = $1
                ORDER BY bc."order", p."order", st."order", swt."order"
            `, [book.uid]);
            console.timeEnd(`sql-book-${book.uid}-sentenceWordTranslations`);
            
            console.timeEnd(`book-${book.uid}-data-fetch`);
            
            console.time(`book-${book.uid}-encode`);
            const bookPack = encode({
                book,
                chapters: chaptersResult.rows,
                paragraphs: paragraphsResult.rows,
                paragraphTranslations: paragraphTranslationsResult.rows,
                sentenceTranslations: sentenceTranslationsResult.rows,
                sentenceWordTranslations: sentenceWordTranslationsResult.rows
            });
            console.timeEnd(`book-${book.uid}-encode`);
            
            console.time(`book-${book.uid}-compress`);
            const compressedBookPack = await this.compressData(bookPack);
            console.timeEnd(`book-${book.uid}-compress`);
            
            booksFolder?.file(`book_${book.uid}.pack`, compressedBookPack);
        }

        // Generate zip and trigger download
        console.time('zip-generate');
        const blob = await zip.generateAsync({ type: 'blob', compression: "DEFLATE" });
        console.timeEnd('zip-generate');
        
        const url = URL.createObjectURL(blob);
        const link = document.createElement('a');
        link.href = url;
        link.download = `flts-library-${new Date().toISOString().replace(/[:.]/g, '-')}.zip`;
        document.body.appendChild(link);
        link.click();
        document.body.removeChild(link);
        URL.revokeObjectURL(url);
        
        console.timeEnd('exportLibrary-total');
        console.log(`Export completed successfully. File size: ${(blob.size / 1024 / 1024).toFixed(2)} MB`);
    },

    /**
     * Imports a library from a zip archive with MessagePack files created by exportLibrary().
     * Skips objects with existing UIDs to avoid duplicates.
     */
    async importLibrary(): Promise<void> {
        return new Promise((resolve, reject) => {
            const fileInput = document.createElement('input');
            fileInput.type = 'file';
            fileInput.accept = '.zip';
            fileInput.onchange = async (event) => {
                try {
                    const file = (event.target as HTMLInputElement).files?.[0];
                    if (!file) {
                        reject(new Error('No file selected'));
                        return;
                    }

                    console.time('importLibrary-total');
                    console.log(`Importing library from file: ${file.name} (${(file.size / 1024 / 1024).toFixed(2)} MB)`);

                    const pg = dbSql.getPgInstance();

                    // Load and extract the zip file
                    const zip = await JSZip.loadAsync(file);

                    // Import dictionary data first
                    const dictionaryFile = zip.file('dictionary.pack');
                    if (dictionaryFile) {
                        console.time('dictionary-import');
                        
                        const compressedDictionaryData = await dictionaryFile.async('uint8array');
                        const dictionaryData = await this.decompressData(compressedDictionaryData);
                        const dictionaryPack = decode(dictionaryData) as {
                            words: any[],
                            wordTranslations: any[]
                        };

                        await pg.transaction(async (tx) => {
                            // Import words (skip existing UIDs)
                            const existingWordsResult = await tx.query('SELECT uid FROM words');
                            const existingWordUids = new Set(existingWordsResult.rows.map((row: any) => row.uid));
                            const newWords = dictionaryPack.words.filter(word => !existingWordUids.has(word.uid));
                            
                            for (const word of newWords) {
                                await tx.query(`
                                    INSERT INTO words (uid, original_language_uid, original, original_normalized, created_at)
                                    VALUES ($1, $2, $3, $4, $5)
                                    ON CONFLICT (uid) DO NOTHING
                                `, [word.uid, word.original_language_uid, word.original, word.original_normalized, word.created_at]);
                            }
                            
                            if (newWords.length > 0) {
                                console.log(`Imported ${newWords.length} new words (skipped ${dictionaryPack.words.length - newWords.length} existing)`);
                            }

                            // Import word translations (skip existing UIDs)
                            const existingWordTranslationsResult = await tx.query('SELECT uid FROM word_translations');
                            const existingWordTranslationUids = new Set(existingWordTranslationsResult.rows.map((row: any) => row.uid));
                            const newWordTranslations = dictionaryPack.wordTranslations.filter(wt => !existingWordTranslationUids.has(wt.uid));
                            
                            for (const wt of newWordTranslations) {
                                await tx.query(`
                                    INSERT INTO word_translations (uid, language_uid, original_word_uid, translation, translation_normalized, created_at)
                                    VALUES ($1, $2, $3, $4, $5, $6)
                                    ON CONFLICT (uid) DO NOTHING
                                `, [wt.uid, wt.language_uid, wt.original_word_uid, wt.translation, wt.translation_normalized, wt.created_at]);
                            }
                            
                            if (newWordTranslations.length > 0) {
                                console.log(`Imported ${newWordTranslations.length} new word translations (skipped ${dictionaryPack.wordTranslations.length - newWordTranslations.length} existing)`);
                            }
                        });

                        console.timeEnd('dictionary-import');
                    }

                    // Import book data
                    const booksFolder = zip.folder('books');
                    if (booksFolder) {
                        const bookFiles = Object.keys(booksFolder.files).filter(filename => 
                            filename.startsWith('books/') && filename.endsWith('.pack')
                        );

                        console.log(`Found ${bookFiles.length} book files to import`);

                        for (const bookFileName of bookFiles) {
                            const bookFile = zip.file(bookFileName);
                            if (!bookFile) continue;

                            console.time(`book-import-${bookFileName}`);

                            const compressedBookData = await bookFile.async('uint8array');
                            const bookData = await this.decompressData(compressedBookData);
                            const bookPack = decode(bookData) as {
                                book: any,
                                chapters: any[],
                                paragraphs: any[],
                                paragraphTranslations: any[],
                                sentenceTranslations: any[],
                                sentenceWordTranslations: any[]
                            };

                            // Check if book already exists
                            const existingBookResult = await pg.query('SELECT uid FROM books WHERE uid = $1', [bookPack.book.uid]);
                            if (existingBookResult.rows.length > 0) {
                                console.log(`Skipping book "${bookPack.book.title}" - already exists`);
                                console.timeEnd(`book-import-${bookFileName}`);
                                continue;
                            }

                            // Import book and related data in a transaction
                            await pg.transaction(async (tx) => {
                                // Import book
                                await tx.query(`
                                    INSERT INTO books (uid, title, path, created_at)
                                    VALUES ($1, $2, $3, $4)
                                    ON CONFLICT (uid) DO NOTHING
                                `, [bookPack.book.uid, bookPack.book.title, bookPack.book.path, bookPack.book.created_at]);

                                // Import chapters (skip existing UIDs)
                                for (const chapter of bookPack.chapters) {
                                    await tx.query(`
                                        INSERT INTO book_chapters (uid, book_uid, "order", title, created_at)
                                        VALUES ($1, $2, $3, $4, $5)
                                        ON CONFLICT (uid) DO NOTHING
                                    `, [chapter.uid, chapter.book_uid, chapter.order, chapter.title, chapter.created_at]);
                                }

                                // Import paragraphs (skip existing UIDs)
                                for (const paragraph of bookPack.paragraphs) {
                                    await tx.query(`
                                        INSERT INTO paragraphs (uid, chapter_uid, "order", original_text, original_html, created_at)
                                        VALUES ($1, $2, $3, $4, $5, $6)
                                        ON CONFLICT (uid) DO NOTHING
                                    `, [paragraph.uid, paragraph.chapter_uid, paragraph.order, paragraph.original_text, paragraph.original_html, paragraph.created_at]);
                                }

                                // Import paragraph translations (skip existing UIDs)
                                for (const pt of bookPack.paragraphTranslations) {
                                    await tx.query(`
                                        INSERT INTO paragraph_translations (uid, paragraph_uid, language_uid, translating_model, created_at)
                                        VALUES ($1, $2, $3, $4, $5)
                                        ON CONFLICT (uid) DO NOTHING
                                    `, [pt.uid, pt.paragraph_uid, pt.language_uid, pt.translating_model, pt.created_at]);
                                }

                                // Import sentence translations (skip existing UIDs)
                                for (const st of bookPack.sentenceTranslations) {
                                    await tx.query(`
                                        INSERT INTO sentence_translations (uid, paragraph_translation_uid, "order", full_translation, created_at)
                                        VALUES ($1, $2, $3, $4, $5)
                                        ON CONFLICT (uid) DO NOTHING
                                    `, [st.uid, st.paragraph_translation_uid, st.order, st.full_translation, st.created_at]);
                                }

                                // Import sentence word translations (skip existing UIDs)
                                for (const swt of bookPack.sentenceWordTranslations) {
                                    await tx.query(`
                                        INSERT INTO sentence_word_translations (
                                            uid, sentence_uid, "order", original, is_punctuation,
                                            is_standalone_punctuation, is_opening_parenthesis, is_closing_parenthesis,
                                            word_translation_uid, word_translation_in_context, grammar_context, note, created_at
                                        )
                                        VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13)
                                        ON CONFLICT (uid) DO NOTHING
                                    `, [
                                        swt.uid, swt.sentence_uid, swt.order, swt.original, swt.is_punctuation,
                                        swt.is_standalone_punctuation, swt.is_opening_parenthesis, swt.is_closing_parenthesis,
                                        swt.word_translation_uid, swt.word_translation_in_context, swt.grammar_context, swt.note, swt.created_at
                                    ]);
                                }
                            });

                            console.log(`Imported book "${bookPack.book.title}" with ${bookPack.chapters.length} chapters`);
                            console.timeEnd(`book-import-${bookFileName}`);
                        }
                    }

                    console.timeEnd('importLibrary-total');
                    console.log('Library import completed successfully');
                    resolve();
                } catch (error) {
                    console.error('Failed to import library:', error);
                    reject(error);
                }
            };
            fileInput.oncancel = () => {
                reject(new Error('File selection cancelled'));
            };
            fileInput.click();
        });
    },

    /**
     * Executes an arbitrary SQL statement and returns the result
     * @param query - The SQL query to execute
     * @param params - Optional parameters for the query
     * @returns Promise<any> - The query result
     */
    async sql(query: string, params?: any[]): Promise<any> {
        try {
            const pg = dbSql.getPgInstance();
            const result = await pg.query(query, params);
            console.log('SQL Query executed successfully');
            console.log('Query:', query);
            if (params && params.length > 0) {
                console.log('Parameters:', params);
            }
            console.log('Result:', result);
            console.log('Rows returned:', result.rows?.length || 0);
            return result;
        } catch (error) {
            console.error('SQL Query failed:', error);
            console.error('Query:', query);
            if (params && params.length > 0) {
                console.error('Parameters:', params);
            }
            throw error;
        }
    }

};
