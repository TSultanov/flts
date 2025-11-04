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
                const targetFolder = getOrCreateFolder(/*book.path || */[]);
                targetFolder.books.push({
                    uid: book.id,
                    chapterCount: book.chaptersCount,
                    translationRatio: book.translationRatio,
                    title: book.title,
                    path: [] // TODO
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
        // Track current target language to filter dictionary updates
        let currentTarget: string | undefined;
        const unsub = configStore.subscribe((cfg) => {
            currentTarget = cfg?.targetLanguageId;
        });

        const store = getterToReadableWithEvents<SentenceWordTranslation | undefined>(
            "get_word_info",
            { bookId, paragraphId, sentenceId, wordId },
            [
                { name: "book_updated", filter: (updatedId: UUID) => updatedId === bookId },
                // dictionary_updated payload is [from, to] (639-3)
                { name: "dictionary_updated", filter: (payload: [string, string]) => !!currentTarget && payload?.[1] === currentTarget },
            ],
        );

        // Ensure we detach the config subscription when the consumer unsubscribes
        return {
            subscribe(run, invalidate) {
                const un = store.subscribe(run, invalidate);
                return () => { un(); unsub(); };
            }
        } as Readable<SentenceWordTranslation | undefined>;
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

    }

    async deleteBooksInBatch(bookUids: UUID[]) {
        await Promise.all(bookUids.map(u => this.deleteBook(u)));
    }

    async moveBooksInBatch(bookUids: UUID[], newPath: string[]) {
        await Promise.all(bookUids.map(u => this.moveBook(u, newPath)));
    }
}
