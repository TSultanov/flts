import { derived, readable, type Readable } from 'svelte/store';
import type { EpubBook } from "./epubLoader";
import type { UUID } from "./v2/db";
import { type ChapterMetaView, type IBookMeta, type ParagraphView, type SentenceWordTranslation } from "./sql/book";
import { eventToReadable, getterToReadable, getterToReadableWithEvents, getterToReadableWithEventsAndPatches } from './tauri';
import { invoke } from "@tauri-apps/api/core";
import { getConfig } from "../config";

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

export type TranslationStatus = {
    request_id: number;
    progress_chars: number;
    expected_chars: number;
    is_complete: boolean;
};

export type SystemDefinition = {
    definition: string,
    transcription: string | null,
}

type ParagraphUpdatedPayload = {
    book_id: UUID,
    paragraph: ParagraphView,
};

const translationStatusSubscribers = new Map<
    number,
    Set<(status: TranslationStatus | undefined) => void>
>();
const translationStatusPollers = new Map<
    number,
    {
        interval: ReturnType<typeof setInterval>;
        inFlight: boolean;
        last: TranslationStatus | undefined;
    }
>();

const TRANSLATION_STATUS_POLL_INTERVAL_MS = 500;

function ensureTranslationStatusPoller(requestId: number) {
    if (translationStatusPollers.has(requestId)) {
        return;
    }

    const poller = {
        interval: undefined as unknown as ReturnType<typeof setInterval>,
        inFlight: false,
        last: undefined as TranslationStatus | undefined,
    };

    const publish = (status: TranslationStatus | undefined) => {
        poller.last = status;
        const subscribers = translationStatusSubscribers.get(requestId);
        if (!subscribers) {
            return;
        }
        for (const cb of subscribers) {
            cb(status);
        }
    };

    const tick = async () => {
        if (poller.inFlight) {
            return;
        }
        poller.inFlight = true;
        try {
            const status = await invoke<TranslationStatus | null>(
                "get_translation_status",
                { requestId },
            );
            const next = status ?? undefined;
            if (
                poller.last &&
                next &&
                poller.last.request_id === next.request_id &&
                poller.last.progress_chars === next.progress_chars &&
                poller.last.expected_chars === next.expected_chars &&
                poller.last.is_complete === next.is_complete
            ) {
                return;
            }
            if (!poller.last && !next) {
                return;
            }
            publish(next);
        } catch {
        } finally {
            poller.inFlight = false;
        }
    };

    poller.interval = setInterval(tick, TRANSLATION_STATUS_POLL_INTERVAL_MS);
    translationStatusPollers.set(requestId, poller);
    void tick();
}

function maybeStopTranslationStatusPoller(requestId: number) {
    const subscribers = translationStatusSubscribers.get(requestId);
    if (subscribers && subscribers.size > 0) {
        return;
    }

    const poller = translationStatusPollers.get(requestId);
    if (!poller) {
        return;
    }
    clearInterval(poller.interval);
    translationStatusPollers.delete(requestId);
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
        return getterToReadableWithEventsAndPatches<ParagraphView[]>(
            "get_book_chapter_paragraphs",
            { bookId, chapterId },
            [
                {
                    name: "book_updated",
                    filter: (updatedId: UUID) => updatedId === bookId,
                },
            ],
            [
                {
                    name: "paragraph_updated",
                    filter: (ev: ParagraphUpdatedPayload) => ev.book_id === bookId,
                    patch: (current: ParagraphView[], ev: ParagraphUpdatedPayload) => {
                        const idx = current.findIndex((p) => p.id === ev.paragraph.id);
                        if (idx === -1) {
                            return current;
                        }
                        const next = current.slice();
                        next[idx] = ev.paragraph;
                        return next;
                    },
                },
            ],
            [],
        );
    }

    getWordInfo(bookId: UUID, paragraphId: number, sentenceId: number, wordId: number): Readable<SentenceWordTranslation | undefined> {
        return getterToReadableWithEvents<SentenceWordTranslation | undefined>(
            "get_word_info",
            { bookId, paragraphId, sentenceId, wordId },
            [
                {
                    name: "book_updated",
                    filter: (updatedId: UUID) => updatedId === bookId,
                },
                {
                    name: "paragraph_updated",
                    filter: (ev: ParagraphUpdatedPayload) =>
                        ev.book_id === bookId && ev.paragraph.id === paragraphId,
                },
            ],
        );
    }

    // Get system dictionary definition for a word (macOS Dictionary Services)
    getSystemDefinition(word: string, sourceLang: string, targetLang: string): Readable<SystemDefinition | null> {
        return getterToReadableWithEvents<SystemDefinition | null>(
            "get_system_definition",
            { word, sourceLang, targetLang },
            [], // No update events - this is a one-time fetch
            null,
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

    async getParagraphTranslationRequestId(bookId: UUID, paragraphId: number): Promise<number | null> {
        return await invoke<number | null>("get_paragraph_translation_request_id", { bookId, paragraphId });
    }

    getTranslationStatus(requestId: number | null): Readable<TranslationStatus | undefined> {
        if (requestId === null) {
            return readable(undefined);
        }

        ensureTranslationStatusPoller(requestId);

        return readable<TranslationStatus | undefined>(undefined, (set) => {
            let subs = translationStatusSubscribers.get(requestId);
            if (!subs) {
                subs = new Set();
                translationStatusSubscribers.set(requestId, subs);
            }

            subs.add(set);
            const poller = translationStatusPollers.get(requestId);
            if (poller && poller.last) {
                set(poller.last);
            }
            return () => {
                subs.delete(set);
                if (subs.size === 0) {
                    translationStatusSubscribers.delete(requestId);
                }
                maybeStopTranslationStatusPoller(requestId);
            };
        });
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
        await this.cleanupTranslationRequests(bookUid);
        await invoke('delete_book', { bookId: bookUid });
    }

    async moveBook(bookUid: UUID, newPath: string[]) {
        await invoke("move_book", { bookId: bookUid, path: newPath });
    }

    async markWordVisible(bookId: UUID, paragraphId: number, flatIndex: number): Promise<boolean> {
        return await invoke<boolean>("mark_word_visible", { bookId, paragraphId, flatIndex });
    }

    async deleteBooksInBatch(bookUids: UUID[]) {
        await Promise.all(bookUids.map(u => this.deleteBook(u)));
    }

    async moveBooksInBatch(bookUids: UUID[], newPath: string[]) {
        await Promise.all(bookUids.map(u => this.moveBook(u, newPath)));
    }
}
