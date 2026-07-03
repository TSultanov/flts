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
// A batch that hasn't settled after this long is presumed hung: its
// in-flight slot is released so later paragraphs keep flowing, and its
// ids leave the dedup set so a future enqueue can retry them. If the
// response eventually arrives it is still applied — data is data.
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
    // Ref-count of dispatched-but-unsettled batches per id (overlapping
    // soft-refresh batches can carry the same id), decremented when the
    // RESPONSE arrives — not when the watchdog fires. The watchdog removes
    // a timed-out chunk's ids from #translationsEnqueued while the fetch is
    // still pending; without this map, a paragraph_updated arriving in that
    // window would find the id in no collection and be dropped, letting the
    // late stale null row cache the paragraph as untranslated forever.
    #translationsInflightIds = new Map<number, number>();
    // Per-paragraph staleness counter, bumped on `paragraph_updated`.
    // Batch responses only apply rows whose dispatch-time epoch is still
    // current, so an older in-flight response can never overwrite fresher
    // data, and an update racing an in-flight fetch always wins.
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
                    // Originals never mutate after import, so we don't
                    // refetch them. A translation update matters only for
                    // paragraphs we've cached, queued, or have in flight
                    // (i.e. ever been in the mount window); paragraphs
                    // never visited stay un-enqueued. Bump the epoch so
                    // any in-flight response for this id is discarded on
                    // arrival, then soft-enqueue a refetch — the cached
                    // entry stays visible until the replacement lands, so
                    // there's no segments→original-text flicker.
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

        // Card-file changes (Anki sync writes or Syncthing pushes) require a
        // backend re-read to update per-word familiarity. Debounced so a long
        // sync_pass burst coalesces into a single refresh.
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

    // Returns a settle function releasing the batch's in-flight slot
    // exactly once — whichever of response or watchdog fires first wins.
    // On timeout the chunk ids leave `enqueued` so a future enqueue can
    // retry them; a late response settling afterwards must not touch the
    // counter again (its rows still apply in the caller's `.then`).
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
                    // Allow a future enqueue to retry these ids.
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
            // Snapshot each id's epoch at dispatch; a row whose epoch has
            // advanced by resolve time is stale and must not apply — its
            // refetch is already queued or in flight.
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
                    // Response side (never the watchdog): drop this batch's
                    // in-flight claim on its ids, then release the slot.
                    for (const id of chunk) {
                        const n = (this.#translationsInflightIds.get(id) ?? 1) - 1;
                        if (n <= 0) this.#translationsInflightIds.delete(id);
                        else this.#translationsInflightIds.set(id, n);
                    }
                    settle();
                });
        }
    }

    // Single write path into `#translations`. Translations are only ever
    // added in this app, never removed, so a null-segments row for a
    // paragraph we already hold segments for is a transient backend state
    // and must never clobber the cache.
    #applyTranslationRow(id: number, segments: ParagraphSegment[] | null): void {
        if (segments === null && this.#translations.get(id)?.segments != null) {
            return;
        }
        this.#translations.set(id, { segments });
    }
}
