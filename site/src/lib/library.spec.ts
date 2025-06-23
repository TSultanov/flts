import { describe, it, expect, beforeEach, vi } from 'vitest';
import 'fake-indexeddb/auto';
import * as fc from 'fast-check';
import type { Library as LibType, LibraryFolder, LibraryBook } from './library.svelte';
import { type ParagraphTranslation, type WordTranslation, type SentenceTranslation } from './data/translators/translator';
import dbSql from "./data/dbSql";
import type Dexie from 'dexie';
import type { UUID } from './data/db';

// Mock the config module before anything else
vi.mock('./config', () => ({
    getConfig: vi.fn().mockResolvedValue({ 
        geminiApiKey: 'test-key',
        targetLanguage: 'en',
        model: 'gemini-2.5-flash'
    }),
    setConfig: vi.fn(),
}));

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
const { queueDb } = await import('./data/queueDb');

describe('Library', () => {
    let library: LibType;

    beforeEach(async () => {
        // Reset the database before each test
        vi.clearAllMocks();
        await (queueDb as Dexie).delete();
        await (queueDb as Dexie).open();
        
        // Reset the SQL database
        await dbSql.resetDatabase();
        
        library = dbSql.getLibrary()
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
            let libraryState = await new Promise<LibraryFolder>(resolve => {
                library.getLibraryBooks().subscribe(value => {
                    if (value && value.books.length > 0) {
                        resolve(value);
                    }
                });
            });
            let b1 = libraryState.books.find(b => b.title === 'Book 1')!;
            await library.moveBook(b1.uid, ['A', 'B']);

            await library.importText('Book 2', 'c2');
            libraryState = await new Promise<LibraryFolder>(resolve => {
                library.getLibraryBooks().subscribe(value => {
                    if (value && (value.books.length > 0 || value.folders.length > 0)) {
                        resolve(value);
                    }
                });
            });
            let b2 = libraryState.books.find(b => b.title === 'Book 2')!;
            await library.moveBook(b2.uid, ['A', 'C']);
            
            await library.importText('Book 3', 'c3');
            libraryState = await new Promise<LibraryFolder>(resolve => {
                library.getLibraryBooks().subscribe(value => {
                    if (value && (value.books.length > 0 || value.folders.length > 0)) {
                        resolve(value);
                    }
                });
            });
            let b3 = libraryState.books.find(b => b.title === 'Book 3')!;
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
                    // Reset the SQL database
                    await dbSql.resetDatabase();
                    const propLib = dbSql.getLibrary();

                    await propLib.importText(book.title, book.content);
                    
                    // Get the book UID through the library interface
                    let importedBookUid: string | undefined;
                    const initialState = await new Promise<LibraryFolder>(resolve => {
                        propLib.getLibraryBooks().subscribe(value => {
                            if (value && value.books.length > 0) {
                                resolve(value);
                            }
                        });
                    });
                    
                    const bookInRoot = initialState.books.find(b => b.title === book.title);
                    if (bookInRoot) {
                        importedBookUid = bookInRoot.uid;
                    }

                    if (book.path && importedBookUid) {
                        await propLib.moveBook(importedBookUid as UUID, book.path);
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
                    // Reset the SQL database
                    await dbSql.resetDatabase();
                    const propLib = dbSql.getLibrary();
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

    describe('Translation Integration Tests', () => {
        const createMockWordTranslation = (original: string, translations: string[], isPunctuation = false): WordTranslation => ({
            original,
            isPunctuation,
            isStandalonePunctuation: false,
            isOpeningParenthesis: false,
            isClosingParenthesis: false,
            translations,
            note: isPunctuation ? "" : "Test translation note",
            grammar: {
                originalInitialForm: original,
                targetInitialForm: translations[0],
                partOfSpeech: isPunctuation ? "punctuation" : "noun",
                plurality: "singular",
                person: "",
                tense: "",
                case: "",
                other: ""
            }
        });

        const createMockSentenceTranslation = (words: WordTranslation[], fullTranslation: string): SentenceTranslation => ({
            words,
            fullTranslation
        });

        const createMockParagraphTranslation = (sentences: SentenceTranslation[], sourceLanguage = "en", targetLanguage = "es"): ParagraphTranslation => ({
            sentences,
            sourceLanguage,
            targetLanguage
        });

        it('should add a simple paragraph translation and make it available through library interface', async () => {
            // First import a book with a paragraph
            await library.importText('Test Book for Translation', 'Hello world.');
            
            // Get the book and chapter 
            let rootFolder = await new Promise<LibraryFolder>(resolve => {
                library.getLibraryBooks().subscribe(value => {
                    if (value && value.books.length > 0) {
                        resolve(value);
                    }
                });
            });
            const book = rootFolder.books[0];
            
            const bookData = await new Promise(resolve => {
                library.getBook(book.uid).subscribe(value => {
                    if (value && value.chapters.length > 0) {
                        resolve(value);
                    }
                });
            });
            const chapter = (bookData as any).chapters[0];
            
            const chapterData = await new Promise(resolve => {
                library.getChapter(chapter.uid).subscribe(value => {
                    if (value && value.paragraphs.length > 0) {
                        resolve(value);
                    }
                });
            });
            const paragraph = (chapterData as any).paragraphs[0];

            // Create mock translation
            const words: WordTranslation[] = [
                createMockWordTranslation("Hello", ["Hola"]),
                createMockWordTranslation("world", ["mundo"]),
                createMockWordTranslation(".", ["."], true)
            ];
            const sentence = createMockSentenceTranslation(words, "Hola mundo.");
            const mockTranslation = createMockParagraphTranslation([sentence]);

            // Add translation using addTranslation function
            await dbSql.addTranslation(paragraph.uid, mockTranslation, 'gemini-2.5-flash');

            // Verify translation is accessible through library interface
            const translatedParagraph = await new Promise(resolve => {
                library.getParagraph(paragraph.uid).subscribe(value => {
                    if (value && value.translation) {
                        resolve(value);
                    }
                });
            });

            expect((translatedParagraph as any).translation).toBeDefined();
            expect((translatedParagraph as any).translation.sentences).toHaveLength(1);
        });

        it('should handle paragraph translation with multiple sentences', async () => {
            await library.importText('Multi-sentence Book', 'First sentence. Second sentence.');
            
            let rootFolder = await new Promise<LibraryFolder>(resolve => {
                library.getLibraryBooks().subscribe(value => {
                    if (value && value.books.length > 0) {
                        resolve(value);
                    }
                });
            });
            const book = rootFolder.books[0];
            
            const bookData = await new Promise(resolve => {
                library.getBook(book.uid).subscribe(value => {
                    if (value && value.chapters.length > 0) {
                        resolve(value);
                    }
                });
            });
            const chapter = (bookData as any).chapters[0];
            
            const chapterData = await new Promise(resolve => {
                library.getChapter(chapter.uid).subscribe(value => {
                    if (value && value.paragraphs.length > 0) {
                        resolve(value);
                    }
                });
            });
            const paragraph = (chapterData as any).paragraphs[0];

            // Create mock translation with multiple sentences
            const sentence1Words: WordTranslation[] = [
                createMockWordTranslation("First", ["Primera"]),
                createMockWordTranslation("sentence", ["oración"]),
                createMockWordTranslation(".", ["."], true)
            ];
            const sentence2Words: WordTranslation[] = [
                createMockWordTranslation("Second", ["Segunda"]),
                createMockWordTranslation("sentence", ["oración"]),
                createMockWordTranslation(".", ["."], true)
            ];
            
            const sentences = [
                createMockSentenceTranslation(sentence1Words, "Primera oración."),
                createMockSentenceTranslation(sentence2Words, "Segunda oración.")
            ];
            const mockTranslation = createMockParagraphTranslation(sentences);

            await dbSql.addTranslation(paragraph.uid, mockTranslation, 'gemini-2.5-flash');

            const translatedParagraph = await new Promise(resolve => {
                library.getParagraph(paragraph.uid).subscribe(value => {
                    if (value && value.translation && value.translation.sentences.length === 2) {
                        resolve(value);
                    }
                });
            });

            expect((translatedParagraph as any).translation.sentences).toHaveLength(2);
            expect((translatedParagraph as any).translation.sentences[0].fullTranslation).toBe("Primera oración.");
            expect((translatedParagraph as any).translation.sentences[1].fullTranslation).toBe("Segunda oración.");
        });

        it('should handle complex punctuation and formatting in translations', async () => {
            await library.importText('Punctuation Book', 'Hello, "world"!');
            
            let rootFolder = await new Promise<LibraryFolder>(resolve => {
                library.getLibraryBooks().subscribe(value => {
                    if (value && value.books.length > 0) {
                        resolve(value);
                    }
                });
            });
            const book = rootFolder.books[0];
            
            const bookData = await new Promise(resolve => {
                library.getBook(book.uid).subscribe(value => {
                    if (value && value.chapters.length > 0) {
                        resolve(value);
                    }
                });
            });
            const chapter = (bookData as any).chapters[0];
            
            const chapterData = await new Promise(resolve => {
                library.getChapter(chapter.uid).subscribe(value => {
                    if (value && value.paragraphs.length > 0) {
                        resolve(value);
                    }
                });
            });
            const paragraph = (chapterData as any).paragraphs[0];

            // Create translation with complex punctuation
            const words: WordTranslation[] = [
                createMockWordTranslation("Hello", ["Hola"]),
                createMockWordTranslation(",", [","], true),
                {
                    original: '"',
                    isPunctuation: true,
                    isStandalonePunctuation: false,
                    isOpeningParenthesis: true,
                    isClosingParenthesis: false,
                    translations: ['"'],
                    note: "",
                    grammar: {
                        originalInitialForm: '"',
                        targetInitialForm: '"',
                        partOfSpeech: "punctuation",
                        plurality: "",
                        person: "",
                        tense: "",
                        case: "",
                        other: ""
                    }
                },
                createMockWordTranslation("world", ["mundo"]),
                {
                    original: '"',
                    isPunctuation: true,
                    isStandalonePunctuation: false,
                    isOpeningParenthesis: false,
                    isClosingParenthesis: true,
                    translations: ['"'],
                    note: "",
                    grammar: {
                        originalInitialForm: '"',
                        targetInitialForm: '"',
                        partOfSpeech: "punctuation",
                        plurality: "",
                        person: "",
                        tense: "",
                        case: "",
                        other: ""
                    }
                },
                createMockWordTranslation("!", ["!"], true)
            ];
            const sentence = createMockSentenceTranslation(words, 'Hola, "mundo"!');
            const mockTranslation = createMockParagraphTranslation([sentence]);

            await dbSql.addTranslation(paragraph.uid, mockTranslation, 'gemini-2.5-flash');

            const translatedParagraph = await new Promise(resolve => {
                library.getParagraph(paragraph.uid).subscribe(value => {
                    if (value && value.translation) {
                        resolve(value);
                    }
                });
            });

            expect((translatedParagraph as any).translation.sentences[0].words).toHaveLength(6);
            expect((translatedParagraph as any).translation.sentences[0].words[2].isOpeningParenthesis).toBe(true);
            expect((translatedParagraph as any).translation.sentences[0].words[4].isClosingParenthesis).toBe(true);
        });

        it('should create and link language entries correctly', async () => {
            await library.importText('Language Test Book', 'Bonjour monde.');
            
            let rootFolder = await new Promise<LibraryFolder>(resolve => {
                library.getLibraryBooks().subscribe(value => {
                    if (value && value.books.length > 0) {
                        resolve(value);
                    }
                });
            });
            const book = rootFolder.books[0];
            
            const bookData = await new Promise(resolve => {
                library.getBook(book.uid).subscribe(value => {
                    if (value && value.chapters.length > 0) {
                        resolve(value);
                    }
                });
            });
            const chapter = (bookData as any).chapters[0];
            
            const chapterData = await new Promise(resolve => {
                library.getChapter(chapter.uid).subscribe(value => {
                    if (value && value.paragraphs.length > 0) {
                        resolve(value);
                    }
                });
            });
            const paragraph = (chapterData as any).paragraphs[0];

            // Create French to English translation
            const words: WordTranslation[] = [
                createMockWordTranslation("Bonjour", ["Hello"]),
                createMockWordTranslation("monde", ["world"]),
                createMockWordTranslation(".", ["."], true)
            ];
            const sentence = createMockSentenceTranslation(words, "Hello world.");
            const mockTranslation = createMockParagraphTranslation([sentence], "fr", "en");

            await dbSql.addTranslation(paragraph.uid, mockTranslation, 'gemini-2.5-flash');

            const translatedParagraph = await new Promise(resolve => {
                library.getParagraph(paragraph.uid).subscribe(value => {
                    if (value && value.translation) {
                        resolve(value);
                    }
                });
            });

            expect((translatedParagraph as any).translation).toBeDefined();
            expect((translatedParagraph as any).translation.translatingModel).toBe('gemini-2.5-flash');
        });

        it('should not duplicate translations for the same paragraph and language', async () => {
            await library.importText('Duplicate Test Book', 'Test text.');
            
            let rootFolder = await new Promise<LibraryFolder>(resolve => {
                library.getLibraryBooks().subscribe(value => {
                    if (value && value.books.length > 0) {
                        resolve(value);
                    }
                });
            });
            const book = rootFolder.books[0];
            
            const bookData = await new Promise(resolve => {
                library.getBook(book.uid).subscribe(value => {
                    if (value && value.chapters.length > 0) {
                        resolve(value);
                    }
                });
            });
            const chapter = (bookData as any).chapters[0];
            
            const chapterData = await new Promise(resolve => {
                library.getChapter(chapter.uid).subscribe(value => {
                    if (value && value.paragraphs.length > 0) {
                        resolve(value);
                    }
                });
            });
            const paragraph = (chapterData as any).paragraphs[0];

            const words: WordTranslation[] = [
                createMockWordTranslation("Test", ["Prueba"]),
                createMockWordTranslation("text", ["texto"]),
                createMockWordTranslation(".", ["."], true)
            ];
            const sentence = createMockSentenceTranslation(words, "Prueba texto.");
            const mockTranslation = createMockParagraphTranslation([sentence]);

            // Add same translation twice
            await dbSql.addTranslation(paragraph.uid, mockTranslation, 'gemini-2.5-flash');
            await dbSql.addTranslation(paragraph.uid, mockTranslation, 'gemini-2.5-flash');

            const translatedParagraph = await new Promise(resolve => {
                library.getParagraph(paragraph.uid).subscribe(value => {
                    if (value && value.translation) {
                        resolve(value);
                    }
                });
            });

            // Should still only have one translation
            expect((translatedParagraph as any).translation).toBeDefined();
            expect((translatedParagraph as any).translation.sentences).toHaveLength(1);
        });

        it('should handle words with multiple translation variants', async () => {
            await library.importText('Multi-variant Book', 'Run fast.');
            
            let rootFolder = await new Promise<LibraryFolder>(resolve => {
                library.getLibraryBooks().subscribe(value => {
                    if (value && value.books.length > 0) {
                        resolve(value);
                    }
                });
            });
            const book = rootFolder.books[0];
            
            const bookData = await new Promise(resolve => {
                library.getBook(book.uid).subscribe(value => {
                    if (value && value.chapters.length > 0) {
                        resolve(value);
                    }
                });
            });
            const chapter = (bookData as any).chapters[0];
            
            const chapterData = await new Promise(resolve => {
                library.getChapter(chapter.uid).subscribe(value => {
                    if (value && value.paragraphs.length > 0) {
                        resolve(value);
                    }
                });
            });
            const paragraph = (chapterData as any).paragraphs[0];

            // Create translation with multiple variants for "run"
            const words: WordTranslation[] = [
                {
                    original: "Run",
                    isPunctuation: false,
                    isStandalonePunctuation: false,
                    isOpeningParenthesis: false,
                    isClosingParenthesis: false,
                    translations: ["Correr", "Ejecutar", "Funcionar"],
                    note: "Multiple meanings depending on context",
                    grammar: {
                        originalInitialForm: "run",
                        targetInitialForm: "correr",
                        partOfSpeech: "verb",
                        plurality: "",
                        person: "",
                        tense: "imperative",
                        case: "",
                        other: ""
                    }
                },
                createMockWordTranslation("fast", ["rápido"]),
                createMockWordTranslation(".", ["."], true)
            ];
            const sentence = createMockSentenceTranslation(words, "Corre rápido.");
            const mockTranslation = createMockParagraphTranslation([sentence]);

            await dbSql.addTranslation(paragraph.uid, mockTranslation, 'gemini-2.5-flash');

            const translatedParagraph = await new Promise(resolve => {
                library.getParagraph(paragraph.uid).subscribe(value => {
                    if (value && value.translation) {
                        resolve(value);
                    }
                });
            });

            const runWord = (translatedParagraph as any).translation.sentences[0].words[0];
            expect(runWord.wordTranslationInContext).toEqual(["Correr", "Ejecutar", "Funcionar"]);
            expect(runWord.note).toBe("Multiple meanings depending on context");
        });

        it('should handle non-existent paragraph gracefully', async () => {
            const { generateUID } = await import('./data/db');
            const fakeUid = generateUID();
            const words: WordTranslation[] = [createMockWordTranslation("Test", ["Prueba"])];
            const sentence = createMockSentenceTranslation(words, "Prueba.");
            const mockTranslation = createMockParagraphTranslation([sentence]);

            // This should not throw an error and should return gracefully
            await expect(dbSql.addTranslation(fakeUid, mockTranslation, 'gemini-2.5-flash')).resolves.toBeUndefined();
        });
    });
});
