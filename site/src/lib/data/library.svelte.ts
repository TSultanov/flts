import { liveQuery } from "dexie";
import { readable, type Readable } from 'svelte/store';
import type { EpubBook } from "../data/epubLoader";
import type { UUID } from "../data/v2/db";
import { books, type IBookMeta } from "../data/v2/book.svelte";
import { translationQueue } from "../data/queueDb";

export type LibraryFolder = {
    name?: string,
    folders: LibraryFolder[],
    books: IBookMeta[],
}

export class Library {
    getLibraryBooks(): Readable<LibraryFolder> {
        return this.useQuery(async () => {
            const allBooks = await books.listBooks();

            const root: LibraryFolder = {
                folders: [],
                books: []
            };

            const getOrCreateFolder = (path: string[]): LibraryFolder => {
                if (path.length === 0) {
                    return root;
                }

                let current = root;
                for (const folderName of path) {
                    let folder = current.folders.find(f => f.name === folderName);
                    if (!folder) {
                        folder = {
                            name: folderName,
                            folders: [],
                            books: []
                        };
                        current.folders.push(folder);
                    }
                    current = folder;
                }
                return current;
            };

            for (const book of allBooks) {
                const targetFolder = getOrCreateFolder(book.path || []);
                targetFolder.books.push(book);
            }

            return root;
        })
    }

    importEpub(book: EpubBook) {
        return books.importEpub(book);
    }

    async importText(title: string, text: string) {
        return books.importText(title, text);
    }

    private async cleanupTranslationRequests(bookUid: UUID): Promise<void> {
        await translationQueue.cleanupTranslationRequests(bookUid);
    }

    async deleteBook(bookUid: UUID) {
        await this.cleanupTranslationRequests(bookUid);
        await books.deleteBook(bookUid);
    }

    async moveBook(bookUid: UUID, newPath: string[]) {
        const book = await books.getBook(bookUid);
        if (book) {
            book.path = newPath;
        }
    }

    async deleteBooksInBatch(bookUids: UUID[]) {
        await Promise.all(bookUids.map(u => this.deleteBook(u)));
    }

    async moveBooksInBatch(bookUids: UUID[], newPath: string[]) {
        await Promise.all(bookUids.map(u => this.moveBook(u, newPath)));
    }

    private useQuery<T>(querier: () => T | Promise<T>): Readable<T> {
        return readable<T>(undefined, (set) => {
            let timeoutId: NodeJS.Timeout | null = null;
            let lastValue: T;
            let hasValue = false;
            let lastUpdateTime = 0;

            return liveQuery(querier).subscribe((x) => {
                lastValue = x;
                const now = Date.now();

                if (!hasValue) {
                    hasValue = true;
                    lastUpdateTime = now;
                    set(x);
                    return;
                }

                if (now - lastUpdateTime >= 1000) {
                    lastUpdateTime = now;
                    set(x);
                    // Clear any pending timeout since we just updated
                    if (timeoutId) {
                        clearTimeout(timeoutId);
                        timeoutId = null;
                    }
                    return;
                }

                if (timeoutId) {
                    clearTimeout(timeoutId);
                }

                timeoutId = setTimeout(() => {
                    lastUpdateTime = Date.now();
                    set(lastValue);
                    timeoutId = null;
                }, 1000);
            }).unsubscribe;
        })
    }
}
