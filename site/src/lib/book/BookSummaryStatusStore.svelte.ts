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

    constructor(bookId: UUID) {
        this.#bookId = bookId;

        invoke<BookSummaryStatusView>("get_book_summary_status", { bookId })
            .then((res) => {
                this.#totalChapters = res.totalChapters;
                this.#generated = res.generated.slice();
                this.#activelyGenerating = res.activelyGenerating ?? null;
            })
            .catch((err) =>
                console.error("Failed to load summary status", err),
            );

        this.#unsubscribe = eventHub.subscribe<SummaryGenerationProgress>(
            "summary_generation_progress",
            (ev) => ev.bookId === bookId,
            (ev) => this.#apply(ev),
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
    }
}
