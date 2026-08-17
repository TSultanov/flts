import { SvelteMap } from "svelte/reactivity";
import { eventHub } from "../data/tauri.svelte";
import type { Library, ParagraphSegment } from "../data/library";
import type { UUID } from "../data/uuid";

export type ParagraphTranslationSliceCache = {
    segments: ParagraphSegment[] | null;
};

export const CHAPTER_STORE_KEY = Symbol("ChapterParagraphsStore");

const BATCH_SIZE = 20;
const MAX_INFLIGHT_PER_KIND = 5;
const CARDS_REFRESH_DEBOUNCE_MS = 500;
// Past this a batch is presumed hung: its slot is released and its ids
// become retryable. A late response still applies.
const BATCH_WATCHDOG_MS = 30_000;

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
    // Per-id ref-count of unsettled batches, decremented on RESPONSE, not
    // on watchdog. Without it, a paragraph_updated arriving after a timeout
    // but before the response would find the id in no collection and be
    // dropped, caching the paragraph as untranslated forever.
    #translationsInflightIds = new Map<number, number>();
    // Staleness counter bumped on `paragraph_updated`; responses apply only
    // rows whose dispatch-time epoch is still current, so an update racing
    // an in-flight fetch always wins.
    #translationsEpoch = new Map<number, number>();

    #cardsRefreshTimer: ReturnType<typeof setTimeout> | null = null;
    #unsubscribers: Array<() => void> = [];
    #disposed = false;

    constructor(bookId: UUID, library: Library) {
        this.#bookId = bookId;
        this.#library = library;

        this.#unsubscribers.push(
            eventHub.subscribe<{ bookId: UUID; paragraphId: number }>(
                "paragraph_updated",
                (p) => p.bookId === bookId,
                (p) => {
                    // Originals never mutate after import. Only paragraphs
                    // that entered the mount window are worth refetching.
                    const id = p.paragraphId;
                    if (
                        this.#translations.has(id) ||
                        this.#translationsEnqueued.has(id) ||
                        this.#translationsInflightIds.has(id)
                    ) {
                        this.#translationsEpoch.set(
                            id,
                            (this.#translationsEpoch.get(id) ?? 0) + 1,
                        );
                        this.#softEnqueueTranslations([id]);
                    }
                },
            ),
        );

        // Card-file changes shift per-word familiarity. Debounced so a long
        // sync_pass burst coalesces into one refresh.
        this.#unsubscribers.push(
            eventHub.subscribe<null>(
                "cards_updated",
                () => true,
                () => {
                    console.info("[cards_updated] event received; scheduling refresh");
                    this.#scheduleCardsRefresh();
                },
            ),
        );
    }

    dispose(): void {
        if (this.#disposed) return;
        this.#disposed = true;
        for (const unsub of this.#unsubscribers) unsub();
        this.#unsubscribers = [];
        if (this.#cardsRefreshTimer != null) {
            clearTimeout(this.#cardsRefreshTimer);
            this.#cardsRefreshTimer = null;
        }
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

    // Refetch in place, so no segments→original-text flicker. Must bypass
    // `#translationsEnqueued` (it never clears for fetched ids, and would
    // block every refresh); dedup against the live queue instead.
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

    // Returns a settle fn that releases the batch's slot exactly once —
    // response or watchdog, whichever fires first.
    #armBatchWatchdog(
        chunk: readonly number[],
        enqueued: Set<number>,
        release: () => void,
    ): () => void {
        let settled = false;
        const settleOnce = () => {
            if (settled) return;
            settled = true;
            release();
        };
        const timer = setTimeout(() => {
            for (const id of chunk) enqueued.delete(id);
            settleOnce();
        }, BATCH_WATCHDOG_MS);
        return () => {
            clearTimeout(timer);
            settleOnce();
        };
    }

    #pumpOriginals(): void {
        if (this.#disposed) return;
        while (
            this.#originalsInflight < MAX_INFLIGHT_PER_KIND &&
            this.#originalsQueue.length > 0
        ) {
            const chunk = this.#originalsQueue.splice(0, BATCH_SIZE);
            this.#originalsInflight++;
            const settle = this.#armBatchWatchdog(
                chunk,
                this.#originalsEnqueued,
                () => {
                    this.#originalsInflight--;
                    this.#pumpOriginals();
                },
            );
            this.#library
                .getParagraphOriginalsBatch(this.#bookId, chunk)
                .then((rows) => {
                    for (const row of rows) {
                        this.#originals.set(row.id, row.original);
                    }
                })
                .catch((err) => {
                    console.error("Failed to fetch paragraph originals batch", err);
                    // Make these ids retryable.
                    for (const id of chunk) this.#originalsEnqueued.delete(id);
                })
                .finally(settle);
        }
    }

    #pumpTranslations(): void {
        if (this.#disposed) return;
        while (
            this.#translationsInflight < MAX_INFLIGHT_PER_KIND &&
            this.#translationsQueue.length > 0
        ) {
            const chunk = this.#translationsQueue.splice(0, BATCH_SIZE);
            this.#translationsInflight++;
            // A row whose epoch advanced by resolve time is stale; its
            // refetch is already queued.
            const dispatchEpochs = new Map<number, number>();
            for (const id of chunk) {
                dispatchEpochs.set(id, this.#translationsEpoch.get(id) ?? 0);
            }
            for (const id of chunk) {
                this.#translationsInflightIds.set(
                    id,
                    (this.#translationsInflightIds.get(id) ?? 0) + 1,
                );
            }
            const settle = this.#armBatchWatchdog(
                chunk,
                this.#translationsEnqueued,
                () => {
                    this.#translationsInflight--;
                    this.#pumpTranslations();
                },
            );
            this.#library
                .getParagraphTranslationsBatch(this.#bookId, chunk)
                .then((rows) => {
                    for (const row of rows) {
                        if (
                            (this.#translationsEpoch.get(row.id) ?? 0) !==
                            dispatchEpochs.get(row.id)
                        ) {
                            continue;
                        }
                        this.#applyTranslationRow(row.id, row.segments ?? null);
                    }
                })
                .catch((err) => {
                    console.error("Failed to fetch paragraph translations batch", err);
                    for (const id of chunk) this.#translationsEnqueued.delete(id);
                })
                .finally(() => {
                    // Response side only — the watchdog must not drop the
                    // in-flight claim on these ids.
                    for (const id of chunk) {
                        const n = (this.#translationsInflightIds.get(id) ?? 1) - 1;
                        if (n <= 0) this.#translationsInflightIds.delete(id);
                        else this.#translationsInflightIds.set(id, n);
                    }
                    settle();
                });
        }
    }

    // Sole write path. Translations are never removed, so a null-segments
    // row over held segments is transient backend state, not a deletion.
    #applyTranslationRow(id: number, segments: ParagraphSegment[] | null): void {
        if (segments === null && this.#translations.get(id)?.segments != null) {
            return;
        }
        this.#translations.set(id, { segments });
    }
}
