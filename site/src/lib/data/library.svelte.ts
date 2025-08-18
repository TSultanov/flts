import { readable, type Readable } from 'svelte/store';
import type { EpubBook } from "../data/epubLoader";
import type { UUID } from "../data/v2/db";
import { translationQueue } from "../data/queueDb";
import { sqlBooks, type IBookMeta } from "./sql/book";

export type LibraryFolder = {
    name?: string,
    folders: LibraryFolder[],
    books: IBookMeta[],
}

export class Library {
    getLibraryBooks(): Readable<LibraryFolder> {
        return this.useQuery(async () => {
            const allBooks = await sqlBooks.listBooks();

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
        return sqlBooks.createFromEpub({
            epub: book
        });
    }

    async importText(title: string, text: string) {
        return sqlBooks.createFromText({
            title,
            text
        });
    }

    private async cleanupTranslationRequests(bookUid: UUID): Promise<void> {
        await translationQueue.cleanupTranslationRequests(bookUid);
    }

    async deleteBook(bookUid: UUID) {
        await this.cleanupTranslationRequests(bookUid);
        await sqlBooks.deleteBook(bookUid);
    }

    async moveBook(bookUid: UUID, newPath: string[]) {
        await sqlBooks.updateBookPath({
            bookUid,
            path: newPath
        })
    }

    async deleteBooksInBatch(bookUids: UUID[]) {
        await Promise.all(bookUids.map(u => this.deleteBook(u)));
    }

    async moveBooksInBatch(bookUids: UUID[], newPath: string[]) {
        await Promise.all(bookUids.map(u => this.moveBook(u, newPath)));
    }

    private useQuery<T>(querier: () => Promise<T>): Readable<T> {
        return readable<T>(undefined, (set) => {
            querier().then(res => set(res));
            // TODO: broadcast DB changes and update readable stores
        })
    }
}
