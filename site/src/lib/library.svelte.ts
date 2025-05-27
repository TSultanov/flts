import { db, type Book, type BookChapter } from "./data/db";
import type { ImportWorkerController } from "./data/importWorkerController";

export type LibraryBook = Book & {
    chapters: BookChapter[]
}

export class Library {
    libraryBooks: LibraryBook[] = $state([]);
    workerController: ImportWorkerController;

    constructor(workerController: ImportWorkerController) {
        this.workerController = workerController;
        this.workerController.addOnParagraphTranslatedHandler(() => this.refresh());
    }

    async refresh() {
            await db.transaction(
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
                    this.libraryBooks = lBooks;
                }
            );
        }

    async importText(title: string, text: string) {
            await db.transaction(
                'rw',
                [
                    db.books,
                    db.bookChapters,
                    db.paragraphs,
                ],
                async () => {
                    const bookId = await db.books.add({
                        title
                    });

                    const chapterId = await db.bookChapters.add({
                        bookId,
                        order: 0,
                    });

                    const paragraphs = this.splitParagraphs(text);

                    const paragraphIds = [];
                    let order = 0;
                    for (const paragraph of paragraphs) {
                        const paragraphId = await db.paragraphs.add({
                            chapterId,
                            order,
                            originalText: paragraph,
                        });
                        paragraphIds.push(paragraphId);
                        order += 1;
                    }
                }
            );
            await this.refresh();
        }

    private splitParagraphs(text: string): string[] {
            return text
                .split(/\n+/)
                .map(p => p.trim())
                .filter(p => p.length > 0);
        }
}