import type { Books } from './evolu/book';
import type { Evolu } from '@evolu/common';
import type { BookId, DatabaseSchema } from './evolu/schema';
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
    constructor(
        evolu: Evolu<DatabaseSchema>,
        private books: Books,
        private translationQueue: TranslationQueue,
    ) { }

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
