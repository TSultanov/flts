import { liveQuery, type PromiseExtended } from "dexie";
import { db, type Book, type BookChapter } from "./data/db";
import { fromStore } from "svelte/store";

export type LibraryBook = Book & {
    chapters: BookChapter[]
}

export class Library {
    $libraryBooks: LibraryBook[] = []

    async refresh() {
        db.transaction(
            'r',
            [
                db.books,
                db.bookChapters,
            ],
            async () => {
                const books = await db.books.toArray();
                const lBooks: LibraryBook[] = await Promise.all(books.map(async b => {
                    const chapters = await db.bookChapters.where("bookId").equals(b.id).sortBy("order");
                    return {
                        ...b,
                        chapters,
                    }
                }));
                this.$libraryBooks = lBooks;
            }
        )
    }
}