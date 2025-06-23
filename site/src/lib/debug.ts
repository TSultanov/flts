import { cacheDb } from './data/cache';
import JSZip from 'jszip';
import { encode, decode } from '@msgpack/msgpack';

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

    // /**
    //  * Exports the entire library as a zip archive with MessagePack files.
    //  * - dictionary.pack: contains words and wordTranslations tables (compressed with gzip)
    //  * - books/book_<book uid>.pack: contains all chapters, paragraphs, sentences, and translations for each book (compressed with gzip)
    //  */
    // async exportLibrary(): Promise<void> {
    //     console.time('exportLibrary-total');
        
    //     // Fetch dictionary tables
    //     const [words, wordTranslations] = await Promise.all([
    //         db.words.toArray(),
    //         db.wordTranslations.toArray()
    //     ]);

    //     // Prepare dictionary MessagePack and compress it
    //     console.time('dictionary-encode');
    //     const dictionaryPack = encode({ words, wordTranslations });
    //     console.timeEnd('dictionary-encode');
        
    //     console.time('dictionary-compress');
    //     const compressedDictionaryPack = await this.compressData(dictionaryPack);
    //     console.timeEnd('dictionary-compress');

    //     // Fetch all books
    //     const books = await db.books.toArray();
    //     console.log(`Processing ${books.length} books for export`);

    //     // Prepare zip
    //     const zip = new JSZip();
    //     zip.file('dictionary.pack', compressedDictionaryPack);
    //     const booksFolder = zip.folder('books');

    //     // For each book, gather all related data and pack
    //     for (const book of books) {
    //         console.time(`book-${book.uid}-data-fetch`);

    //         const chapterUids = await db.bookChapters.where('bookUid').equals(book.uid).primaryKeys();
    //         const paragraphUids = await db.paragraphs.where('chapterUid').anyOf(chapterUids).primaryKeys();
    //         const paragraphTranslationUids = await db.paragraphTranslations.where('paragraphUid').anyOf(paragraphUids).primaryKeys();
    //         const sentenceTranslationUids = await db.sentenceTranslations.where('paragraphTranslationUid').anyOf(paragraphTranslationUids).primaryKeys();

    //         const [
    //             chapters,
    //             paragraphs,
    //             paragraphTranslations,
    //             sentenceTranslations,
    //             sentenceWordTranslations
    //         ] = await Promise.all([
    //             db.bookChapters.where('bookUid').equals(book.uid).toArray(),
    //             db.paragraphs.where('chapterUid').anyOf(chapterUids).toArray(),
    //             db.paragraphTranslations.where('paragraphUid').anyOf(paragraphUids).toArray(),
    //             db.sentenceTranslations.where('paragraphTranslationUid').anyOf(paragraphTranslationUids).toArray(),
    //             db.sentenceWordTranslations.where('sentenceUid').anyOf(sentenceTranslationUids).toArray()
    //         ]);
    //         console.timeEnd(`book-${book.uid}-data-fetch`);
            
    //         console.time(`book-${book.uid}-encode`);
    //         const bookPack = encode({ book, chapters, paragraphs, paragraphTranslations, sentenceTranslations, sentenceWordTranslations });
    //         console.timeEnd(`book-${book.uid}-encode`);
            
    //         console.time(`book-${book.uid}-compress`);
    //         const compressedBookPack = await this.compressData(bookPack);
    //         console.timeEnd(`book-${book.uid}-compress`);
            
    //         booksFolder?.file(`book_${book.uid}.pack`, compressedBookPack);
    //     }

    //     // Generate zip and trigger download
    //     console.time('zip-generate');
    //     const blob = await zip.generateAsync({ type: 'blob', compression: "DEFLATE" });
    //     console.timeEnd('zip-generate');
        
    //     const url = URL.createObjectURL(blob);
    //     const link = document.createElement('a');
    //     link.href = url;
    //     link.download = `flts-library-${new Date().toISOString().replace(/[:.]/g, '-')}.zip`;
    //     document.body.appendChild(link);
    //     link.click();
    //     document.body.removeChild(link);
    //     URL.revokeObjectURL(url);
        
    //     console.timeEnd('exportLibrary-total');
    //     console.log(`Export completed successfully. File size: ${(blob.size / 1024 / 1024).toFixed(2)} MB`);
    // },

    // /**
    //  * Imports a library from a zip archive with MessagePack files created by exportLibrary().
    //  * Skips objects with existing UIDs to avoid duplicates.
    //  */
    // async importLibrary(): Promise<void> {
    //     return new Promise((resolve, reject) => {
    //         const fileInput = document.createElement('input');
    //         fileInput.type = 'file';
    //         fileInput.accept = '.zip';
    //         fileInput.onchange = async (event) => {
    //             try {
    //                 const file = (event.target as HTMLInputElement).files?.[0];
    //                 if (!file) {
    //                     reject(new Error('No file selected'));
    //                     return;
    //                 }

    //                 console.time('importLibrary-total');
    //                 console.log(`Importing library from file: ${file.name} (${(file.size / 1024 / 1024).toFixed(2)} MB)`);

    //                 // Load and extract the zip file
    //                 const zip = await JSZip.loadAsync(file);

    //                 // Import dictionary data first
    //                 const dictionaryFile = zip.file('dictionary.pack');
    //                 if (dictionaryFile) {
    //                     console.time('dictionary-import');
                        
    //                     const compressedDictionaryData = await dictionaryFile.async('uint8array');
    //                     const dictionaryData = await this.decompressData(compressedDictionaryData);
    //                     const dictionaryPack = decode(dictionaryData) as {
    //                         words: any[],
    //                         wordTranslations: any[]
    //                     };

    //                     // Import words (skip existing UIDs)
    //                     const existingWordUids = new Set((await db.words.toCollection().primaryKeys()) as string[]);
    //                     const newWords = dictionaryPack.words.filter(word => !existingWordUids.has(word.uid));
    //                     if (newWords.length > 0) {
    //                         await db.words.bulkAdd(newWords);
    //                         console.log(`Imported ${newWords.length} new words (skipped ${dictionaryPack.words.length - newWords.length} existing)`);
    //                     }

    //                     // Import word translations (skip existing UIDs)
    //                     const existingWordTranslationUids = new Set((await db.wordTranslations.toCollection().primaryKeys()) as string[]);
    //                     const newWordTranslations = dictionaryPack.wordTranslations.filter(wt => !existingWordTranslationUids.has(wt.uid));
    //                     if (newWordTranslations.length > 0) {
    //                         await db.wordTranslations.bulkAdd(newWordTranslations);
    //                         console.log(`Imported ${newWordTranslations.length} new word translations (skipped ${dictionaryPack.wordTranslations.length - newWordTranslations.length} existing)`);
    //                     }

    //                     console.timeEnd('dictionary-import');
    //                 }

    //                 // Import book data
    //                 const booksFolder = zip.folder('books');
    //                 if (booksFolder) {
    //                     const bookFiles = Object.keys(booksFolder.files).filter(filename => 
    //                         filename.startsWith('books/') && filename.endsWith('.pack')
    //                     );

    //                     console.log(`Found ${bookFiles.length} book files to import`);

    //                     for (const bookFileName of bookFiles) {
    //                         const bookFile = zip.file(bookFileName);
    //                         if (!bookFile) continue;

    //                         console.time(`book-import-${bookFileName}`);

    //                         const compressedBookData = await bookFile.async('uint8array');
    //                         const bookData = await this.decompressData(compressedBookData);
    //                         const bookPack = decode(bookData) as {
    //                             book: any,
    //                             chapters: any[],
    //                             paragraphs: any[],
    //                             paragraphTranslations: any[],
    //                             sentenceTranslations: any[],
    //                             sentenceWordTranslations: any[]
    //                         };

    //                         // Check if book already exists
    //                         const existingBook = await db.books.where('uid').equals(bookPack.book.uid).first();
    //                         if (existingBook) {
    //                             console.log(`Skipping book "${bookPack.book.title}" - already exists`);
    //                             console.timeEnd(`book-import-${bookFileName}`);
    //                             continue;
    //                         }

    //                         // Import book and related data in a transaction
    //                         await db.transaction('rw', [
    //                             db.books,
    //                             db.bookChapters,
    //                             db.paragraphs,
    //                             db.paragraphTranslations,
    //                             db.sentenceTranslations,
    //                             db.sentenceWordTranslations
    //                         ], async () => {
    //                             // Import book
    //                             await db.books.add(bookPack.book);

    //                             // Import chapters (skip existing UIDs)
    //                             const existingChapterUids = new Set((await db.bookChapters.toCollection().primaryKeys()) as string[]);
    //                             const newChapters = bookPack.chapters.filter(chapter => !existingChapterUids.has(chapter.uid));
    //                             if (newChapters.length > 0) {
    //                                 await db.bookChapters.bulkAdd(newChapters);
    //                             }

    //                             // Import paragraphs (skip existing UIDs)
    //                             const existingParagraphUids = new Set((await db.paragraphs.toCollection().primaryKeys()) as string[]);
    //                             const newParagraphs = bookPack.paragraphs.filter(paragraph => !existingParagraphUids.has(paragraph.uid));
    //                             if (newParagraphs.length > 0) {
    //                                 await db.paragraphs.bulkAdd(newParagraphs);
    //                             }

    //                             // Import paragraph translations (skip existing UIDs)
    //                             const existingParagraphTranslationUids = new Set((await db.paragraphTranslations.toCollection().primaryKeys()) as string[]);
    //                             const newParagraphTranslations = bookPack.paragraphTranslations.filter(pt => !existingParagraphTranslationUids.has(pt.uid));
    //                             if (newParagraphTranslations.length > 0) {
    //                                 await db.paragraphTranslations.bulkAdd(newParagraphTranslations);
    //                             }

    //                             // Import sentence translations (skip existing UIDs)
    //                             const existingSentenceTranslationUids = new Set((await db.sentenceTranslations.toCollection().primaryKeys()) as string[]);
    //                             const newSentenceTranslations = bookPack.sentenceTranslations.filter(st => !existingSentenceTranslationUids.has(st.uid));
    //                             if (newSentenceTranslations.length > 0) {
    //                                 await db.sentenceTranslations.bulkAdd(newSentenceTranslations);
    //                             }

    //                             // Import sentence word translations (skip existing UIDs)
    //                             const existingSentenceWordTranslationUids = new Set((await db.sentenceWordTranslations.toCollection().primaryKeys()) as string[]);
    //                             const newSentenceWordTranslations = bookPack.sentenceWordTranslations.filter(swt => !existingSentenceWordTranslationUids.has(swt.uid));
    //                             if (newSentenceWordTranslations.length > 0) {
    //                                 await db.sentenceWordTranslations.bulkAdd(newSentenceWordTranslations);
    //                             }
    //                         });

    //                         console.log(`Imported book "${bookPack.book.title}" with ${bookPack.chapters.length} chapters`);
    //                         console.timeEnd(`book-import-${bookFileName}`);
    //                     }
    //                 }

    //                 console.timeEnd('importLibrary-total');
    //                 console.log('Library import completed successfully');
    //                 resolve();
    //             } catch (error) {
    //                 console.error('Failed to import library:', error);
    //                 reject(error);
    //             }
    //         };
    //         fileInput.oncancel = () => {
    //             reject(new Error('File selection cancelled'));
    //         };
    //         fileInput.click();
    //     });
    // }

};
