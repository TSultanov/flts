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

        // summary_generation_progress events lost while iOS suspends the
        // WKWebView would otherwise leave translate buttons stuck on
        // "waiting for chapter summaries" until the book is reopened —
        // this store is per-book, so a chapter switch doesn't rebuild it.
        // Re-sync from the backend snapshot on resume.
        if (typeof document !== "undefined") {
            this.#onVisibilityChange = () => {
                if (document.visibilityState === "visible") this.#refetch();
            };
            document.addEventListener(
                "visibilitychange",
                this.#onVisibilityChange,
            );
        }
    }

    #refetch(): void {
        invoke<BookSummaryStatusView>("get_book_summary_status", {
            bookId: this.#bookId,
        })
            .then((res) => {
                // Merge rather than replace: `generated` only ever goes
                // false→true, so OR-ing with the current state means a
                // snapshot captured just before a progress event landed
                // can never regress the UI.
                const next = this.#generated.slice();
                while (next.length < res.generated.length) next.push(false);
                res.generated.forEach((g, i) => {
                    if (g) next[i] = true;
                });
                this.#generated = next;
                this.#totalChapters = Math.max(
                    this.#totalChapters,
                    res.totalChapters,
                );
                this.#activelyGenerating = res.activelyGenerating ?? null;
            })
            .catch((err) =>
                console.error("Failed to load summary status", err),
            );
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
            // The backend's post-save emit uses `current = idx + 1` for the
            // just-finished chapter; the start-of-chapter emit uses
            // `current = idx`. Marking everything strictly below `current`
            // as generated handles both shapes without double-counting.
            const next = this.#generated.slice();
            for (let i = 0; i < ev.current && i < next.length; i++) {
                next[i] = true;
            }
            this.#generated = next;
            this.#activelyGenerating =
                ev.current < ev.total ? ev.current : null;
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
