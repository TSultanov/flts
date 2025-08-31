import type { Books } from './evolu/book';
import type { Evolu } from '@evolu/common';
import type { BookId, DatabaseSchema } from './evolu/schema';
import { queryState } from "@evolu/svelte";
import type { TranslationQueue } from './queueDb';

export interface IBookMeta {
    readonly id: BookId,
    readonly chapterCount: number;
    readonly translationRatio: number;
    readonly title: string;
    path: string[];
}

export type LibraryFolder = {
    name?: string,
    folders: LibraryFolder[],
    books: IBookMeta[],
}

export class Library {
    libraryBooks: LibraryFolder;

    constructor(
        private evolu: Evolu<DatabaseSchema>,
        private books: Books,
        private translationQueue: TranslationQueue,
    ) {
        const allBooks = queryState(evolu, () => books.allBooks);

        this.libraryBooks = $derived.by(() => {
            const root: LibraryFolder = {
                folders: [],
                books: []
            };

            if (!allBooks.rows || !allBooks.rows[0]) {
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

            for (const book of allBooks.rows) {
                const path: string[] = JSON.parse(book?.path ?? "[]")
                const targetFolder = getOrCreateFolder(path);
                targetFolder.books.push({
                    id: book.id,
                    title: book.title ?? "<unknown>",
                    path,
                    chapterCount: book.chapterCount,
                    translationRatio: (book.paragraphsCount ?? 0) / (book.translatedParagraphsCount ?? 0),
                });
            }

            return root;
        })
    }

    private async cleanupTranslationRequests(bookId: BookId): Promise<void> {
        await this.translationQueue.cleanupTranslationRequests(bookId);
    }

    async deleteBook(bookId: BookId) {
        console.log(`starting book deletion ${bookId}`)
        await this.cleanupTranslationRequests(bookId);
        console.log(`cleaned up translation requests ${bookId}`)
        await this.books.deleteBook(bookId);
        console.log(`deleted book ${bookId}`)
    }

    moveBook(bookId: BookId, newPath: string[]) {
        this.books.updateBookPath(bookId, newPath);
    }

    deleteBooksInBatch(bookUids: BookId[]) {
        bookUids.map(u => this.deleteBook(u));
    }

    moveBooksInBatch(bookUids: BookId[], newPath: string[]) {
        bookUids.map(u => this.moveBook(u, newPath));
    }
}
