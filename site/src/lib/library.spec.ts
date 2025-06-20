import { describe, it, expect, beforeEach, vi } from 'vitest';
import 'fake-indexeddb/auto';
import * as fc from 'fast-check';
import type { Library as LibType, LibraryFolder, LibraryBook } from './library.svelte';
import type Dexie from 'dexie';

// Mock the config module before anything else
vi.mock('./config', () => ({
    getConfig: vi.fn().mockResolvedValue({ 
        geminiApiKey: 'test-key',
        targetLanguage: 'en',
        model: 'gemini-2.5-flash'
    }),
    setConfig: vi.fn(),
}));

// Mock the database before importing any library code that uses it.
vi.mock('./data/db', async (importOriginal) => {
    const DexiePackage = (await import('dexie')).default;
    const actual = await importOriginal() as typeof import('./data/db');
    
    const testDb = new DexiePackage('test-db');
    // This schema needs to match the one in the actual db.ts
    testDb.version(1).stores({
        books: 'uid, &title, *path',
        bookChapters: 'uid, bookUid, order',
        paragraphs: 'uid, chapterUid, order',
        languages: 'uid, &name',
        paragraphTranslations: 'uid, paragraphUid, languageUid',
        sentenceTranslations: 'uid, paragraphTranslationUid, order',
        words: 'uid, originalLanguageUid, &originalNormalized',
        wordTranslations: 'uid, languageUid, originalWordUid, &translationNormalized',
        sentenceWordTranslations: 'uid, sentenceUid, order, wordTranslationUid',
    });

    return {
        ...actual,
        db: testDb,
    };
});

// Mock the queue database
vi.mock('./data/queueDb', async (importOriginal) => {
    const DexiePackage = (await import('dexie')).default;
    const actual = await importOriginal() as typeof import('./data/queueDb');
    
    const testQueueDb = new DexiePackage('test-queue-db');
    testQueueDb.version(1).stores({
        directTranslationRequests: '++id, paragraphUid',
    });

    return {
        ...actual,
        queueDb: testQueueDb,
    };
});

// Dynamically import the modules after the mock is set up.
const { Library } = await import('./library.svelte');
const { db } = await import('./data/db');
const { queueDb } = await import('./data/queueDb');

describe('Library', () => {
    let library: LibType;

    beforeEach(async () => {
        // Reset the database before each test
        vi.clearAllMocks();
        await (db as Dexie).delete();
        await (db as Dexie).open();
        await (queueDb as Dexie).delete();
        await (queueDb as Dexie).open();
        library = new Library();
    });

    it('should start with an empty library', async () => {
        const rootFolder = await new Promise<LibraryFolder>(resolve => {
            library.getLibraryBooks().subscribe(value => {
                if (value) {
                    resolve(value);
                }
            });
        });
        expect(rootFolder).toEqual({
            name: undefined,
            folders: [],
            books: [],
        });
    });

    describe('Book Management', () => {
        it('should import a text book and place it in the root', async () => {
            await library.importText('Test Book', 'This is the content.');
            
            const rootFolder = await new Promise<LibraryFolder>(resolve => {
                library.getLibraryBooks().subscribe(value => {
                    if (value && value.books.length > 0) {
                        resolve(value);
                    }
                });
            });

            expect(rootFolder.books).toHaveLength(1);
            expect(rootFolder.books[0].title).toBe('Test Book');
            expect(rootFolder.books[0].path).toBeUndefined();
        });

        it('should delete a book', async () => {
            await library.importText('Book to Delete', 'Content.');
            
            let rootFolder = await new Promise<LibraryFolder>(resolve => {
                library.getLibraryBooks().subscribe(value => {
                    if (value && value.books.length > 0) {
                        resolve(value);
                    }
                });
            });
            const bookToDelete = rootFolder.books[0];

            await library.deleteBook(bookToDelete.uid);

            rootFolder = await new Promise<LibraryFolder>(resolve => {
                library.getLibraryBooks().subscribe(value => {
                    if (value && value.books.length === 0 && value.folders.length === 0) {
                        resolve(value);
                    }
                });
            });

            expect(rootFolder.books).toHaveLength(0);
        });

        it('should move a book to a new folder path', async () => {
            await library.importText('Movable Book', 'Content.');
             let rootFolder = await new Promise<LibraryFolder>(resolve => {
                library.getLibraryBooks().subscribe(value => {
                    if (value && value.books.length > 0) {
                        resolve(value);
                    }
                });
            });
            const bookToMove = rootFolder.books[0];
            const newPath = ['Fiction', 'Sci-Fi'];

            await library.moveBook(bookToMove.uid, newPath);

            rootFolder = await new Promise<LibraryFolder>(resolve => {
                library.getLibraryBooks().subscribe(value => {
                    if (value && value.folders.length > 0 && value.folders[0]?.folders.length > 0) {
                        resolve(value);
                    }
                });
            });
            
            expect(rootFolder.books).toHaveLength(0);
            expect(rootFolder.folders).toHaveLength(1);
            expect(rootFolder.folders[0].name).toBe('Fiction');
            expect(rootFolder.folders[0].folders).toHaveLength(1);
            expect(rootFolder.folders[0].folders[0].name).toBe('Sci-Fi');
            expect(rootFolder.folders[0].folders[0].books).toHaveLength(1);
            expect(rootFolder.folders[0].folders[0].books[0].title).toBe('Movable Book');
            expect(rootFolder.folders[0].folders[0].books[0].path).toEqual(newPath);
        });

        it('should move a book from a folder back to the root', async () => {
            await library.importText('Root-Bound Book', 'Content.');
            let rootFolder = await new Promise<LibraryFolder>(resolve => library.getLibraryBooks().subscribe(v => { if(v && v.books.length) resolve(v) }));
            const bookToMove = rootFolder.books[0];
            await library.moveBook(bookToMove.uid, ['Temporary']);
            
            rootFolder = await new Promise<LibraryFolder>(resolve => library.getLibraryBooks().subscribe(v => { if(v && v.folders.length > 0 && v.folders[0].books.length > 0) resolve(v) }));
            const bookInFolder = rootFolder.folders[0].books[0];

            await library.moveBook(bookInFolder.uid, null);

            rootFolder = await new Promise<LibraryFolder>(resolve => library.getLibraryBooks().subscribe(v => {
                if (v && v.books.length > 0) {
                    resolve(v);
                }
            }));

            expect(rootFolder.books).toHaveLength(1);
            expect(rootFolder.books[0].title).toBe('Root-Bound Book');
            expect(rootFolder.books[0].path).toBeUndefined();
        });
    });

    describe('Folder Structure', () => {
        it('should create nested folders correctly when importing books with paths', async () => {
            await library.importText('Book 1', 'c1');
            let b1 = (await (db as any).books.where('title').equals('Book 1').first());
            await library.moveBook(b1.uid, ['A', 'B']);

            await library.importText('Book 2', 'c2');
            let b2 = (await (db as any).books.where('title').equals('Book 2').first());
            await library.moveBook(b2.uid, ['A', 'C']);
            
            await library.importText('Book 3', 'c3');
            let b3 = (await (db as any).books.where('title').equals('Book 3').first());
            await library.moveBook(b3.uid, ['A']);

            await library.importText('Book 4', 'c4');

            const rootFolder = await new Promise<LibraryFolder>(resolve => {
                library.getLibraryBooks().subscribe(value => {
                    const count = (f: LibraryFolder): number => f.books.length + f.folders.reduce((acc: number, sf: LibraryFolder) => acc + count(sf), 0);
                    if (value && count(value) === 4) {
                        resolve(value);
                    }
                });
            });

            expect(rootFolder.books).toHaveLength(1);
            expect(rootFolder.books[0].title).toBe('Book 4');

            const folderA = rootFolder.folders.find(f => f.name === 'A');
            expect(folderA).toBeDefined();
            expect(folderA!.books).toHaveLength(1);
            expect(folderA!.books[0].title).toBe('Book 3');

            const folderB = folderA!.folders.find(f => f.name === 'B');
            expect(folderB).toBeDefined();
            expect(folderB!.books).toHaveLength(1);
            expect(folderB!.books[0].title).toBe('Book 1');

            const folderC = folderA!.folders.find(f => f.name === 'C');
            expect(folderC).toBeDefined();
            expect(folderC!.books).toHaveLength(1);
            expect(folderC!.books[0].title).toBe('Book 2');
        });
    });

    describe('Property-Based Tests', () => {
        const nonEmptyString = fc.string({ minLength: 1 }).filter(s => s.trim().length > 0);
        const pathSegment = nonEmptyString.filter(s => !s.includes('/')); // No slashes in folder names
        const bookArbitrary = fc.record({
            title: nonEmptyString,
            content: fc.string(),
            path: fc.option(fc.array(pathSegment, { minLength: 1, maxLength: 5 }), { nil: undefined })
        });

        it('should correctly store and retrieve any imported book', async () => {
            await fc.assert(
                fc.asyncProperty(bookArbitrary, async (book) => {
                    await (db as Dexie).delete();
                    await (db as Dexie).open();
                    const propLib = new Library();

                    await propLib.importText(book.title, book.content);
                    const savedBook = await (db as any).books.where('title').equals(book.title).first();
                    expect(savedBook).toBeDefined();

                    if (book.path) {
                        await propLib.moveBook(savedBook!.uid, book.path);
                    }

                    const rootFolder = await new Promise<LibraryFolder>(resolve => {
                        propLib.getLibraryBooks().subscribe(value => {
                            const findBook = (folder: LibraryFolder): LibraryBook | null => {
                                const found = folder.books.find((b) => b.title === book.title);
                                if (found) return found;
                                for (const subfolder of folder.folders) {
                                    const deepFound = findBook(subfolder);
                                    if (deepFound) return deepFound;
                                }
                                return null;
                            };
                            if(value && findBook(value)) {
                                resolve(value);
                            }
                        });
                    });

                    const findAndVerifyBook = (folder: LibraryFolder) => {
                        if (!book.path) {
                            const found = folder.books.find((b) => b.title === book.title);
                            expect(found).toBeDefined();
                            expect(found!.path).toBeUndefined();
                        } else {
                            let currentFolder: LibraryFolder | undefined = folder;
                            for (const segment of book.path) {
                                currentFolder = currentFolder!.folders.find((f) => f.name === segment);
                                expect(currentFolder).toBeDefined();
                            }
                            const found = currentFolder!.books.find((b) => b.title === book.title);
                            expect(found).toBeDefined();
                            expect(found!.path).toEqual(book.path);
                        }
                    };

                    findAndVerifyBook(rootFolder);
                })
            , { numRuns: 30 });
        });

        it('deleting a book should remove it from the library', async () => {
            await fc.assert(
                fc.asyncProperty(nonEmptyString, async (title) => {
                    await (db as Dexie).delete();
                    await (db as Dexie).open();
                    const propLib = new Library();
                    await propLib.importText(title, "content");

                    let root = await new Promise<LibraryFolder>(r => propLib.getLibraryBooks().subscribe(v => { if(v && v.books.length) r(v) }));
                    const bookUid = root.books[0].uid;

                    await propLib.deleteBook(bookUid);

                    root = await new Promise<LibraryFolder>(r => propLib.getLibraryBooks().subscribe(v => {
                        const findBookRecursive = (folder: LibraryFolder): LibraryBook | undefined => {
                            const found = folder.books.find(b => b.uid === bookUid);
                            if (found) return found;
                            for (const sub of folder.folders) {
                                const subFound = findBookRecursive(sub);
                                if (subFound) return subFound;
                            }
                        };
                        if (v && findBookRecursive(v) === undefined) {
                            r(v);
                        }
                    }));
                     const findBook = (folder: LibraryFolder): LibraryBook | null => {
                        const found = folder.books.find((b) => b.uid === bookUid);
                        if (found) return found;
                        for (const subfolder of folder.folders) {
                            const deepFound = findBook(subfolder);
                            if (deepFound) return deepFound;
                        }
                        return null;
                    };

                    expect(findBook(root)).toBeNull();
                })
            , { numRuns: 30 });
        });
    });
});
