import { SvelteMap } from "svelte/reactivity";
import { eventHub } from "../data/tauri.svelte";
import type { Library, ParagraphSegment } from "../data/library";
import type { UUID } from "../data/uuid";

export type ParagraphTranslationSliceCache = {
    segments: ParagraphSegment[] | null;
    visibleWords: number[];
};

export const CHAPTER_STORE_KEY = Symbol("ChapterParagraphsStore");

const BATCH_SIZE = 20;
const MAX_INFLIGHT_PER_KIND = 5;
const CARDS_REFRESH_DEBOUNCE_MS = 500;

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

    #cardsRefreshTimer: ReturnType<typeof setTimeout> | null = null;

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

        // Card-file changes (Anki sync writes or Syncthing pushes) require a
        // backend re-read to update per-word familiarity. Debounced so a long
        // sync_pass burst coalesces into a single refresh.
        eventHub.subscribe<null>(
            "cards_updated",
            () => true,
            () => {
                console.info("[cards_updated] event received; scheduling refresh");
                this.#scheduleCardsRefresh();
            },
        );
    }

    #scheduleCardsRefresh(): void {
        if (this.#cardsRefreshTimer != null) {
            clearTimeout(this.#cardsRefreshTimer);
        }
        this.#cardsRefreshTimer = setTimeout(() => {
            this.#cardsRefreshTimer = null;
            const ids = [...this.#translations.keys()];
            if (ids.length === 0) {
                console.info(
                    "[cards_updated] no cached translations to refresh",
                );
                return;
            }
            console.info(
                `[cards_updated] refreshing ${ids.length} cached translations`,
            );
            this.#softEnqueueTranslations(ids);
        }, CARDS_REFRESH_DEBOUNCE_MS);
    }

    // Re-fetch cached translations without dropping them first. Overwrites
    // entries in place as the batch resolves, so the user sees no
    // segments→original-text flicker. Bypasses `#translationsEnqueued`
    // entirely — that set is the regular-enqueue dedup and never clears
    // for successfully-fetched ids, so checking it would block every
    // refresh. Dedup against the current queue contents only so a burst
    // of `cards_updated` events doesn't push the same ids multiple times.
    #softEnqueueTranslations(ids: readonly number[]): void {
        const alreadyQueued = new Set(this.#translationsQueue);
        for (const id of ids) {
            if (alreadyQueued.has(id)) continue;
            alreadyQueued.add(id);
            this.#translationsQueue.push(id);
        }
        this.#pumpTranslations();
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
