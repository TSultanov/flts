import { SvelteMap } from "svelte/reactivity";
import { eventHub } from "../data/tauri.svelte";
import type { Library } from "../data/library";
import type { UUID } from "../data/uuid";

export type ParagraphTranslationSliceCache = {
  /** The paragraph as the backend drew it; null for one not translated. */
  html: string | null;
};

export const CHAPTER_STORE_KEY = Symbol("ChapterParagraphsStore");

const BATCH_SIZE = 20;
const MAX_INFLIGHT = 5;
// Past this a batch is presumed hung: its slot is released and its ids
// become retryable. A late response still applies.
const BATCH_WATCHDOG_MS = 30_000;

// Holds the responses to a group of requests until the last has come.
class Together {
  #waiting = 0;
  #held: Array<() => void> = [];

  // One per request; a second call (a late response) applies at once.
  expect(): Land {
    this.#waiting++;
    let landed = false;
    return (apply) => {
      if (landed) return apply();
      landed = true;
      this.#held.push(apply);
      if (--this.#waiting > 0) return;
      const held = this.#held;
      this.#held = [];
      for (const each of held) each();
    };
  }
}

type Land = (apply: () => void) => void;
const atOnce: Land = (apply) => apply();

export class ChapterParagraphsStore {
  #bookId: UUID;
  #library: Library;

  #originals = new SvelteMap<number, string>();
  #translations = new SvelteMap<number, ParagraphTranslationSliceCache>();

  #originalsRequested = new Set<number>();

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

  #together: Together | null = null;
  /** Called when translations have been put in: what is drawn may change size. */
  onTranslations: (() => void) | null = null;

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
  }

  dispose(): void {
    if (this.#disposed) return;
    this.#disposed = true;
    for (const unsub of this.#unsubscribers) unsub();
    this.#unsubscribers = [];
  }

  // Refetch in place, so no translation→original-text flicker. Must bypass
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

  // What `requests` asks for is applied in one go. Every change re-lays out
  // the whole chapter, so text and its translations landing apart cost two
  // layouts where one would do.
  together(requests: () => void): void {
    this.#together = new Together();
    try {
      requests();
    } finally {
      this.#together = null;
    }
  }

  // One request for all of `ids`: every arrival re-lays out the whole
  // chapter, so a chapter fetched in pieces is laid out once per piece.
  // Settles once what came has been applied.
  fetchOriginals(ids: readonly number[]): Promise<void> {
    const wanted = ids.filter((id) => !this.#originalsRequested.has(id));
    if (wanted.length === 0 || this.#disposed) return Promise.resolve();
    for (const id of wanted) this.#originalsRequested.add(id);
    const land = this.#together?.expect() ?? atOnce;
    return this.#library
      .getParagraphOriginalsBatch(this.#bookId, wanted)
      .then(
        (rows) =>
          new Promise<void>((applied) =>
            land(() => {
              for (const row of rows) {
                this.#originals.set(row.id, row.original);
              }
              applied();
            }),
          ),
      )
      .catch((err) => {
        console.error("Failed to fetch paragraph originals", err);
        // Make these ids retryable.
        for (const id of wanted) this.#originalsRequested.delete(id);
        land(() => {});
      });
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

  #pumpTranslations(): void {
    if (this.#disposed) return;
    while (
      this.#translationsInflight < MAX_INFLIGHT &&
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
      // Response or watchdog, whichever is first, frees the slot once.
      let settled = false;
      const settle = () => {
        if (settled) return;
        settled = true;
        clearTimeout(watchdog);
        this.#translationsInflight--;
        this.#pumpTranslations();
      };
      const land = this.#together?.expect() ?? atOnce;
      const watchdog = setTimeout(() => {
        for (const id of chunk) this.#translationsEnqueued.delete(id);
        land(() => {});
        settle();
      }, BATCH_WATCHDOG_MS);
      this.#library
        .getParagraphHtmlBatch(this.#bookId, chunk)
        .then((rows) =>
          land(() => {
            for (const row of rows) {
              if (
                (this.#translationsEpoch.get(row.id) ?? 0) !==
                dispatchEpochs.get(row.id)
              ) {
                continue;
              }
              this.#applyTranslationRow(row.id, row.html ?? null);
            }
            this.onTranslations?.();
          }),
        )
        .catch((err) => {
          console.error("Failed to fetch paragraph translations batch", err);
          for (const id of chunk) this.#translationsEnqueued.delete(id);
          land(() => {});
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

  // Sole write path. Translations are never removed, so an untranslated
  // row over a held translation is transient backend state, not a deletion.
  #applyTranslationRow(id: number, html: string | null): void {
    if (html === null && this.#translations.get(id)?.html != null) {
      return;
    }
    this.#translations.set(id, { html });
  }
}
