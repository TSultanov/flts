import type { EpubBook } from "../import/epubLoader";
import type { UUID } from "./uuid";
import {
    type BookMeta,
    type ChapterMetaView,
    type ParagraphOriginal,
    type ParagraphTranslationSlice,
    type SentenceWordTranslation,
} from "./types";
import { Resource } from "./tauri.svelte";
import { ParagraphTranslationActivityResource } from "./translationActivity.svelte";
import { invoke } from "@tauri-apps/api/core";
import { getConfig } from "../config/store";

export type LibraryBookMetadataView = {
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
    pageOffset: number,
}

export type LibraryFolder = {
    name?: string,
    folders: LibraryFolder[],
    books: BookMeta[],
}

export type SystemDefinition = {
    definition: string,
    transcription: string | null,
}

export function buildLibraryFolder(books: LibraryBookMetadataView[]): LibraryFolder {
    const root: LibraryFolder = { folders: [], books: [] };

    const getOrCreateFolder = (path: string[]): LibraryFolder => {
        if (path.length === 0) return root;
        let current = root;
        for (const folderName of path) {
            let folder = current.folders.find(f => f.name === folderName);
            if (!folder) {
                folder = { name: folderName, folders: [], books: [] };
                current.folders.push(folder);
            }
            current = folder;
        }
        return current;
    };

    for (const book of books) {
        const folderPath = book.path ?? [];
        const targetFolder = getOrCreateFolder(folderPath);
        targetFolder.books.push({
            uid: book.id,
            chapterCount: book.chaptersCount,
            translationRatio: book.translationRatio,
            title: book.title,
            path: [...folderPath],
        });
    }

    return root;
}

export class Library {
    getLibraryBooksMetadata(): Resource<LibraryBookMetadataView[]> {
        return new Resource<LibraryBookMetadataView[]>(
            "list_books",
            {},
            [{ name: "library_updated", filter: () => true }],
            [],
        );
    }

    getBookChapters(bookId: UUID): Resource<ChapterMetaView[]> {
        return new Resource<ChapterMetaView[]>(
            "list_book_chapters",
            { bookId },
            [{ name: "book_updated", filter: (updatedId: UUID) => updatedId === bookId }],
            [],
        );
    }

    getBookChapterParagraphIds(bookId: UUID, chapterId: number): Resource<number[]> {
        return new Resource<number[]>(
            "get_book_chapter_paragraph_ids",
            { bookId, chapterId },
            [{ name: "book_updated", filter: (updatedId: UUID) => updatedId === bookId }],
            [],
        );
    }

    getWordInfo(bookId: UUID, paragraphId: number, sentenceId: number, wordId: number): Resource<SentenceWordTranslation | undefined> {
        return new Resource<SentenceWordTranslation | undefined>(
            "get_word_info",
            { bookId, paragraphId, sentenceId, wordId },
            [{ name: "book_updated", filter: (updatedId: UUID) => updatedId === bookId }],
        );
    }

    // Get system dictionary definition for a word (macOS Dictionary Services)
    getSystemDefinition(word: string, sourceLang: string, targetLang: string): Resource<SystemDefinition | null> {
        return new Resource<SystemDefinition | null>(
            "get_system_definition",
            { word, sourceLang, targetLang },
            [],
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

    getParagraphTranslationActivity(bookId: UUID, paragraphId: number): ParagraphTranslationActivityResource {
        return new ParagraphTranslationActivityResource(bookId, paragraphId);
    }

    async getParagraphOriginalsBatch(
        bookId: UUID,
        paragraphIds: number[],
    ): Promise<ParagraphOriginal[]> {
        return await invoke<ParagraphOriginal[]>(
            "get_paragraph_originals_batch",
            { bookId, paragraphIds },
        );
    }

    async getParagraphTranslationsBatch(
        bookId: UUID,
        paragraphIds: number[],
    ): Promise<ParagraphTranslationSlice[]> {
        return await invoke<ParagraphTranslationSlice[]>(
            "get_paragraph_translations_batch",
            { bookId, paragraphIds },
        );
    }

    async getBookReadingState(bookId: UUID): Promise<BookReadingState | null> {
        return await invoke<BookReadingState | null>("get_book_reading_state", { bookId });
    }

    async saveBookReadingState(
        bookId: UUID,
        chapterId: number,
        paragraphId: number,
        pageOffset: number,
    ): Promise<void> {
        await invoke("save_book_reading_state", {
            bookId,
            chapterId,
            paragraphId,
            pageOffset,
        });
    }

    async deleteBook(bookUid: UUID) {
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
