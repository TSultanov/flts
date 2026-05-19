import { SvelteMap } from "svelte/reactivity";
import { eventHub } from "../data/tauri.svelte";
import type { Library } from "../data/library";
import type { UUID } from "../data/uuid";
import type { ParagraphSegment } from "../data/types";

export type ParagraphTranslationSliceCache = {
    segments: ParagraphSegment[] | null;
    visibleWords: number[];
};

export const CHAPTER_STORE_KEY = Symbol("ChapterParagraphsStore");

const BATCH_SIZE = 20;
const MAX_INFLIGHT_PER_KIND = 5;

export class ChapterParagraphsStore {
    #bookId: UUID;
    #library: Library;

    #originals = new SvelteMap<number, string>();
    #translations = new SvelteMap<number, ParagraphTranslationSliceCache>();

    #originalsQueue: number[] = [];
    #originalsEnqueued = new Set<number>();
    #originalsInflight = 0;

    #translationsQueue: number[] = [];
    #translationsEnqueued = new Set<number>();
    #translationsInflight = 0;

    constructor(bookId: UUID, library: Library) {
        this.#bookId = bookId;
        this.#library = library;

        eventHub.subscribe<{ bookId: UUID; paragraphId: number }>(
            "paragraph_updated",
            (p) => p.bookId === bookId,
            (p) => {
                // Originals never mutate after import, so we don't refetch
                // them. A translation update invalidates only the segments
                // cache, and only refetches if the paragraph is something
                // we'd previously fetched (i.e. has been in the mount
                // window). Paragraphs never visited stay un-enqueued.
                if (this.#translations.has(p.paragraphId)) {
                    this.#translations.delete(p.paragraphId);
                    this.#translationsEnqueued.delete(p.paragraphId);
                    this.enqueueTranslations([p.paragraphId]);
                }
            },
        );
    }

    getOriginal(id: number): string | undefined {
        return this.#originals.get(id);
    }

    hasOriginal(id: number): boolean {
        return this.#originals.has(id);
    }

    getTranslation(id: number): ParagraphTranslationSliceCache | null {
        return this.#translations.get(id) ?? null;
    }

    enqueueOriginals(ids: readonly number[]): void {
        for (const id of ids) {
            if (this.#originals.has(id)) continue;
            if (this.#originalsEnqueued.has(id)) continue;
            this.#originalsEnqueued.add(id);
            this.#originalsQueue.push(id);
        }
        this.#pumpOriginals();
    }

    enqueueTranslations(ids: readonly number[]): void {
        for (const id of ids) {
            if (this.#translations.has(id)) continue;
            if (this.#translationsEnqueued.has(id)) continue;
            this.#translationsEnqueued.add(id);
            this.#translationsQueue.push(id);
        }
        this.#pumpTranslations();
    }

    #pumpOriginals(): void {
        while (
            this.#originalsInflight < MAX_INFLIGHT_PER_KIND &&
            this.#originalsQueue.length > 0
        ) {
            const chunk = this.#originalsQueue.splice(0, BATCH_SIZE);
            this.#originalsInflight++;
            this.#library
                .getParagraphOriginalsBatch(this.#bookId, chunk)
                .then((rows) => {
                    for (const row of rows) {
                        this.#originals.set(row.id, row.original);
                    }
                })
                .catch((err) => {
                    console.error("Failed to fetch paragraph originals batch", err);
                    // Allow a future enqueue to retry these ids.
                    for (const id of chunk) this.#originalsEnqueued.delete(id);
                })
                .finally(() => {
                    this.#originalsInflight--;
                    this.#pumpOriginals();
                });
        }
    }

    #pumpTranslations(): void {
        while (
            this.#translationsInflight < MAX_INFLIGHT_PER_KIND &&
            this.#translationsQueue.length > 0
        ) {
            const chunk = this.#translationsQueue.splice(0, BATCH_SIZE);
            this.#translationsInflight++;
            this.#library
                .getParagraphTranslationsBatch(this.#bookId, chunk)
                .then((rows) => {
                    for (const row of rows) {
                        this.#translations.set(row.id, {
                            segments: row.segments ?? null,
                            visibleWords: row.visibleWords,
                        });
                    }
                })
                .catch((err) => {
                    console.error("Failed to fetch paragraph translations batch", err);
                    for (const id of chunk) this.#translationsEnqueued.delete(id);
                })
                .finally(() => {
                    this.#translationsInflight--;
                    this.#pumpTranslations();
                });
        }
    }
}
