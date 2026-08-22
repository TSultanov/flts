import { invoke } from "@tauri-apps/api/core";
import { eventHub } from "../data/tauri.svelte";
import type { UUID } from "../data/uuid";

export const SUMMARY_STATUS_KEY = Symbol("BookSummaryStatusStore");

type SummaryGenerationProgress = {
  bookId: UUID;
  current: number;
  total: number;
  status: "in_progress" | "done" | "failed";
  error?: string;
};

type BookSummaryStatusView = {
  totalChapters: number;
  generated: boolean[];
  activelyGenerating?: number;
};

export class BookSummaryStatusStore {
  #bookId: UUID;
  #generated: boolean[] = $state([]);
  #totalChapters: number = $state(0);
  #activelyGenerating: number | null = $state(null);
  #unsubscribe: () => void;

  #onVisibilityChange: (() => void) | null = null;

  constructor(bookId: UUID) {
    this.#bookId = bookId;

    this.#refetch();

    this.#unsubscribe = eventHub.subscribe<SummaryGenerationProgress>(
      "summary_generation_progress",
      (ev) => ev.bookId === bookId,
      (ev) => this.#apply(ev),
    );

    // A suspended iOS WKWebView drops progress events, and this store is
    // per-book (a chapter switch won't rebuild it), so translate buttons
    // would stick on "waiting for chapter summaries" until reopen.
    if (typeof document !== "undefined") {
      this.#onVisibilityChange = () => {
        if (document.visibilityState === "visible") this.#refetch();
      };
      document.addEventListener("visibilitychange", this.#onVisibilityChange);
    }
  }

  #refetch(): void {
    invoke<BookSummaryStatusView>("get_book_summary_status", {
      bookId: this.#bookId,
    })
      .then((res) => {
        // `generated` only goes false→true, so OR-ing keeps a
        // snapshot that predates a progress event from regressing.
        const next = this.#generated.slice();
        while (next.length < res.generated.length) next.push(false);
        res.generated.forEach((g, i) => {
          if (g) next[i] = true;
        });
        this.#generated = next;
        this.#totalChapters = Math.max(this.#totalChapters, res.totalChapters);
        this.#activelyGenerating = res.activelyGenerating ?? null;
      })
      .catch((err) => console.error("Failed to load summary status", err));
  }

  #apply(ev: SummaryGenerationProgress): void {
    if (ev.total > this.#totalChapters) {
      // Backfill if the event arrived before the initial fetch landed.
      const next = this.#generated.slice();
      while (next.length < ev.total) next.push(false);
      this.#generated = next;
      this.#totalChapters = ev.total;
    }
    if (ev.status === "in_progress") {
      // The backend emits `current = idx` at chapter start and
      // `idx + 1` after save; marking everything strictly below
      // `current` handles both without double-counting.
      const next = this.#generated.slice();
      for (let i = 0; i < ev.current && i < next.length; i++) {
        next[i] = true;
      }
      this.#generated = next;
      this.#activelyGenerating = ev.current < ev.total ? ev.current : null;
    } else if (ev.status === "done") {
      const next = this.#generated.slice();
      for (let i = 0; i < ev.total && i < next.length; i++) {
        next[i] = true;
      }
      this.#generated = next;
      this.#activelyGenerating = null;
    } else {
      // "failed" — leave `generated` as-is.
      this.#activelyGenerating = null;
    }
  }

  isGenerated(chapterId: number): boolean {
    return this.#generated[chapterId] === true;
  }

  canTranslate(chapterId: number): boolean {
    if (chapterId === 0) return true;
    return this.#generated[chapterId - 1] === true;
  }

  isActivelyGenerating(chapterId: number): boolean {
    return this.#activelyGenerating === chapterId;
  }

  dispose(): void {
    this.#unsubscribe();
    if (this.#onVisibilityChange) {
      document.removeEventListener(
        "visibilitychange",
        this.#onVisibilityChange,
      );
      this.#onVisibilityChange = null;
    }
  }
}
