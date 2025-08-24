import { derived, readable, type Readable } from 'svelte/store';
import type { EpubBook } from "./epubLoader";
import type { UUID } from "./v2/db";
import { translationQueue } from "./queueDb";
import { sqlBooks, type IBookMeta } from "./sql/book";

export type LibraryFolder = {
    name?: string,
    folders: LibraryFolder[],
    books: IBookMeta[],
}

export class Library {
    getLibraryBooks(): Readable<LibraryFolder> {
        const booksStore = sqlBooks.listBooks();
        return derived([booksStore], (allBooks) => {
            const root: LibraryFolder = {
                folders: [],
                books: []
            };

            if (!allBooks || !allBooks[0]) {
                return root;
            }

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

            for (const book of allBooks[0]) {
                const targetFolder = getOrCreateFolder(book.path || []);
                targetFolder.books.push(book);
            }

            return root;
        })
    }

    async importEpub(book: EpubBook) {
        const uid = await sqlBooks.createFromEpub({
            epub: book
        });
        translationQueue.scheduleFullBookTranslation(uid)
    }

    async importText(title: string, text: string) {
        const uid = await sqlBooks.createFromText({
            title,
            text
        });
        translationQueue.scheduleFullBookTranslation(uid);
    }

    private async cleanupTranslationRequests(bookUid: UUID): Promise<void> {
        await translationQueue.cleanupTranslationRequests(bookUid);
    }

    async deleteBook(bookUid: UUID) {
        console.log(`starting book deletion ${bookUid}`)
        await this.cleanupTranslationRequests(bookUid);
        console.log(`cleaned up translation requests ${bookUid}`)
        await sqlBooks.deleteBook(bookUid);
        console.log(`deleted book ${bookUid}`)
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
}
