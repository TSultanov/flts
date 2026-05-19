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

// Single source of truth for active paragraph translations. The Rust
// `TranslationQueue.active_translations` map starts empty at process boot
// and is purely in-memory, so subscribing once at module load is sufficient
// to capture every started/progress/finished event — no per-paragraph
// snapshot fetch is required.
class ActiveTranslationsStore {
    #entries = new SvelteMap<string, ParagraphTranslationActivity>();

    constructor() {
        eventHub.subscribe<StartedEvent>(
            "paragraph_translation_started",
            () => true,
            (p) => {
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
                this.#entries.set(activityKey(p.bookId, p.paragraphId), {
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
                this.#entries.delete(activityKey(p.bookId, p.paragraphId));
            },
        );
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
