import { derived, readable, type Readable } from 'svelte/store';
import type { EpubBook } from "./epubLoader";
import type { UUID } from "./v2/db";
import { type ChapterMetaView, type IBookMeta, type ParagraphView, type SentenceWordTranslation } from "./sql/book";
import { eventToReadable, getterToReadable, getterToReadableWithEvents } from './tauri';
import { invoke } from "@tauri-apps/api/core";
import { configStore, getConfig } from "../config";

type LibraryBookMetadataView = {
    id: UUID,
    title: string,
    chaptersCount: number,
    paragraphsCount: number,
    translationRatio: number,
    path: string[],
}

export type BookReadingState = {
    chapterId: number,
    paragraphId: number,
}

export type LibraryFolder = {
    name?: string,
    folders: LibraryFolder[],
    books: IBookMeta[],
}

export class Library {
    getLibraryBooks(): Readable<LibraryFolder> {
        const booksStore = eventToReadable<LibraryBookMetadataView[]>("library_updated", "list_books", [])
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
                const folderPath = book.path ?? [];
                const targetFolder = getOrCreateFolder(folderPath);
                targetFolder.books.push({
                    uid: book.id,
                    chapterCount: book.chaptersCount,
                    translationRatio: book.translationRatio,
                    title: book.title,
                    path: [...folderPath]
                });
            }

            return root;
        })
    }

    getBookChapters(bookId: UUID): Readable<ChapterMetaView[]> {
        return getterToReadable("list_book_chapters", { "bookId": bookId }, "book_updated", (updatedId: UUID) => updatedId === bookId, []);
    }

    getBookChapterParagraphs(bookId: UUID, chapterId: number): Readable<ParagraphView[]> {
        return getterToReadable("get_book_chapter_paragraphs", { "bookId": bookId, "chapterId": chapterId }, "book_updated", (updatedId: UUID) => updatedId === bookId, []);
    }

    getWordInfo(bookId: UUID, paragraphId: number, sentenceId: number, wordId: number): Readable<SentenceWordTranslation | undefined> {
        return getterToReadable<SentenceWordTranslation | undefined, UUID>(
            "get_word_info",
            { bookId, paragraphId, sentenceId, wordId },
            "book_updated", (updatedId: UUID) => updatedId === bookId,
        );
    }

    async importEpub(book: EpubBook, sourceLanguageId: string) {
        await invoke<UUID>("import_epub", { book, sourceLanguageId });
    }

    async importText(title: string, text: string, sourceLanguageId: string) {
        await invoke<UUID>("import_plain_text", { title, text, sourceLanguageId });
    }

    async translateParagraph(bookId: UUID, paragraphId: number, model: number | undefined = undefined, useCache: boolean = true) {
        let config = await getConfig();
        return await invoke<number>("translate_paragraph", { bookId, paragraphId, model: model ?? config.model, useCache });
    }

    async getParagraphTranslationRequestId(bookId: UUID, paragraphId: number) {
        return await invoke<number>("get_paragraph_translation_request_id", { bookId, paragraphId });
    }

    async getBookReadingState(bookId: UUID): Promise<BookReadingState | null> {
        return await invoke<BookReadingState | null>("get_book_reading_state", { bookId });
    }

    async saveBookReadingState(bookId: UUID, chapterId: number, paragraphId: number): Promise<void> {
        await invoke("save_book_reading_state", { bookId, chapterId, paragraphId });
    }

    private async cleanupTranslationRequests(bookUid: UUID): Promise<void> {
    }

    async deleteBook(bookUid: UUID) {
        console.log(`starting book deletion ${bookUid}`)
        await this.cleanupTranslationRequests(bookUid);
        console.log(`cleaned up translation requests ${bookUid}`)
        // await sqlBooks.deleteBook(bookUid);
        console.log(`deleted book ${bookUid}`)
    }

    async moveBook(bookUid: UUID, newPath: string[]) {
        await invoke("move_book", { bookId: bookUid, path: newPath });
    }

    async deleteBooksInBatch(bookUids: UUID[]) {
        await Promise.all(bookUids.map(u => this.deleteBook(u)));
    }

    async moveBooksInBatch(bookUids: UUID[], newPath: string[]) {
        await Promise.all(bookUids.map(u => this.moveBook(u, newPath)));
    }
}
