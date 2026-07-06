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

// Row shape of the `list_paragraph_translation_activity` snapshot command.
type ActiveTranslationRow = {
    bookId: UUID;
    paragraphId: number;
} & ParagraphTranslationActivity;

// Single source of truth for active paragraph translations, driven by the
// started/progress/finished Tauri events.
//
// Two hazards are handled beyond plain event mirroring:
//
// 1. Event ordering: `paragraph_translation_progress` is emitted from a
//    detached throttler task on the Rust side while
//    `paragraph_translation_finished` comes from the saver task — nothing
//    orders them, so a throttled progress event can arrive AFTER the
//    finished event for the same request. Only `started` may create an
//    entry; `progress` updates an existing entry with a matching requestId
//    and is otherwise dropped, so a late straggler cannot resurrect a
//    finished entry into an eternal spinner.
//
// 2. Event loss (iOS): while the app is backgrounded the WKWebView is
//    suspended and Tauri events emitted in that window are lost — stranding
//    entries whose `finished` we never saw AND hiding activity whose
//    `started` we never saw. On visibilitychange back to visible we fetch
//    the full snapshot from the Rust `list_paragraph_translation_activity`
//    command and replace our entries wholesale.
class ActiveTranslationsStore {
    #entries = new SvelteMap<string, ParagraphTranslationActivity>();
    // Monotonic token; a reconcile pass only applies its results if no newer
    // pass has started while it was awaiting the snapshot.
    #reconcileToken = 0;

    constructor() {
        eventHub.subscribe<StartedEvent>(
            "paragraph_translation_started",
            () => true,
            (p) => {
                // Any lifecycle change invalidates an in-flight reconcile
                // pass: its snapshot predates this event (events and invoke
                // responses travel on independent channels — nothing orders
                // them), so applying it could delete this fresh entry.
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
                // Only update an entry that `started` created for this exact
                // request; see hazard 1 above.
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
                // See the started handler: a snapshot captured before this
                // event must not be applied after it (it would resurrect
                // the entry as a phantom spinner).
                this.#reconcileToken++;
                const key = activityKey(p.bookId, p.paragraphId);
                const existing = this.#entries.get(key);
                // Delete only the entry this request created: a stale
                // finished emitted just before a rapid re-translate's
                // started must not kill the newer request's spinner. (An
                // entry stranded with a lost started is cleaned up by the
                // visibilitychange reconcile instead.)
                if (existing && existing.requestId === p.requestId) {
                    this.#entries.delete(key);
                }
            },
        );

        // Guarded so importing this module in tests / non-browser contexts
        // doesn't blow up.
        if (typeof document !== "undefined") {
            document.addEventListener("visibilitychange", () => {
                if (document.visibilityState === "visible") {
                    void this.#reconcile();
                }
            });
        }

        // Adopt translations already in flight at store creation: their
        // `started` events predate our subscription (webview launch/reload
        // while the backend keeps translating), and the progress handler
        // deliberately won't create entries (hazard 1), so only a snapshot
        // can reveal them. The token machinery protects against a live
        // `started` racing this initial pass.
        void this.#reconcile();
    }

    // Replace the store's entries wholesale with the Rust-side snapshot:
    // entries Rust no longer tracks are deleted, entries it does track are
    // adopted — including ones whose `started` event was lost during
    // suspension. A live event landing in the small window between snapshot
    // capture and apply is corrected by the next progress/finished event for
    // that request; a pass superseded by a newer one applies nothing.
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
