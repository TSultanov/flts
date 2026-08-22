import { invoke } from "@tauri-apps/api/core";
import { SvelteMap } from "svelte/reactivity";
import { eventHub } from "./tauri.svelte";
import type { UUID } from "./uuid";

export type ParagraphTranslationActivity = {
  requestId: number;
  progressChars: number;
  expectedChars: number;
};

type StartedEvent = {
  bookId: UUID;
  paragraphId: number;
  requestId: number;
  expectedChars: number;
};

type ProgressEvent = {
  bookId: UUID;
  paragraphId: number;
  requestId: number;
  progressChars: number;
  expectedChars: number;
};

type FinishedEvent = {
  bookId: UUID;
  paragraphId: number;
  requestId: number;
  error: string | null;
};

const activityKey = (bookId: UUID, paragraphId: number) =>
  `${bookId}:${paragraphId}`;

type ActiveTranslationRow = {
  bookId: UUID;
  paragraphId: number;
} & ParagraphTranslationActivity;

// Source of truth for active paragraph translations. Two hazards shape it:
//
// 1. Ordering: progress (throttler task) and finished (saver task) are
//    unordered, so progress can arrive after finished. Only `started` may
//    create an entry, so a straggler can't resurrect an eternal spinner.
// 2. Loss: a suspended iOS WKWebView drops events entirely, both stranding
//    and hiding entries — visibilitychange re-syncs from the snapshot.
class ActiveTranslationsStore {
  #entries = new SvelteMap<string, ParagraphTranslationActivity>();
  // A reconcile pass applies only if no newer pass started meanwhile.
  #reconcileToken = 0;

  constructor() {
    eventHub.subscribe<StartedEvent>(
      "paragraph_translation_started",
      () => true,
      (p) => {
        // An in-flight reconcile's snapshot predates this event and
        // would delete the fresh entry; invalidate it.
        this.#reconcileToken++;
        this.#entries.set(activityKey(p.bookId, p.paragraphId), {
          requestId: p.requestId,
          progressChars: 0,
          expectedChars: p.expectedChars,
        });
      },
    );
    eventHub.subscribe<ProgressEvent>(
      "paragraph_translation_progress",
      () => true,
      (p) => {
        const key = activityKey(p.bookId, p.paragraphId);
        const existing = this.#entries.get(key);
        // Hazard 1: never create, only update this exact request.
        if (!existing || existing.requestId !== p.requestId) return;
        this.#entries.set(key, {
          requestId: p.requestId,
          progressChars: p.progressChars,
          expectedChars: p.expectedChars,
        });
      },
    );
    eventHub.subscribe<FinishedEvent>(
      "paragraph_translation_finished",
      () => true,
      (p) => {
        if (p.error) {
          console.warn(
            `Translation failed for paragraph ${p.paragraphId}:`,
            p.error,
          );
        }
        // A snapshot captured before this event would resurrect the
        // entry as a phantom spinner.
        this.#reconcileToken++;
        const key = activityKey(p.bookId, p.paragraphId);
        const existing = this.#entries.get(key);
        // Delete only this request's entry — a stale finished must
        // not kill a rapid re-translate's newer spinner.
        if (existing && existing.requestId === p.requestId) {
          this.#entries.delete(key);
        }
      },
    );

    // Guarded for non-browser (test) contexts.
    if (typeof document !== "undefined") {
      document.addEventListener("visibilitychange", () => {
        if (document.visibilityState === "visible") {
          void this.#reconcile();
        }
      });
    }

    // Translations already running at webview launch have a `started`
    // predating our subscription, and progress won't create entries —
    // only a snapshot can reveal them.
    void this.#reconcile();
  }

  // Replaces entries wholesale with the backend snapshot, adopting any
  // whose `started` was lost.
  async #reconcile(): Promise<void> {
    const token = ++this.#reconcileToken;
    let snapshot: ActiveTranslationRow[];
    try {
      snapshot = await invoke<ActiveTranslationRow[]>(
        "list_paragraph_translation_activity",
      );
    } catch (err) {
      // An IPC failure must not clear live spinners.
      console.warn("Failed to reconcile translation activity:", err);
      return;
    }

    if (token !== this.#reconcileToken) return; // superseded by a newer pass

    const fresh = new Map(
      snapshot.map((row) => [
        activityKey(row.bookId, row.paragraphId),
        {
          requestId: row.requestId,
          progressChars: row.progressChars,
          expectedChars: row.expectedChars,
        },
      ]),
    );
    for (const key of [...this.#entries.keys()]) {
      if (!fresh.has(key)) this.#entries.delete(key);
    }
    for (const [key, entry] of fresh) {
      this.#entries.set(key, entry);
    }
  }

  get(bookId: UUID, paragraphId: number): ParagraphTranslationActivity | null {
    return this.#entries.get(activityKey(bookId, paragraphId)) ?? null;
  }
}

export const activeTranslations = new ActiveTranslationsStore();

export class ParagraphTranslationActivityResource {
  #bookId!: UUID;
  #paragraphId!: number;

  current: ParagraphTranslationActivity | null = $derived.by(() =>
    activeTranslations.get(this.#bookId, this.#paragraphId),
  );

  constructor(bookId: UUID, paragraphId: number) {
    this.#bookId = bookId;
    this.#paragraphId = paragraphId;
  }
}
