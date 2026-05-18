import { invoke, type InvokeArgs } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import type { UUID } from "./v2/db";

export type UpdateEvent<TEvent = any> = {
    name: string;
    filter: (ev: TEvent) => boolean;
};

const eventCleanupRegistry =
    typeof FinalizationRegistry !== "undefined"
        ? new FinalizationRegistry<UnlistenFn[]>((unlisteners) => {
              for (const u of unlisteners) {
                  try { u(); } catch { /* ignore */ }
              }
          })
        : null;

export class Resource<T> {
    #current: T | undefined = $state(undefined);

    constructor(
        getterName: string,
        args: InvokeArgs = {},
        events: UpdateEvent[] = [],
        defaultValue?: T,
    ) {
        this.#current = defaultValue;
        const unlisteners: UnlistenFn[] = [];

        const fetch = () => {
            invoke<T>(getterName, args).then((v) => {
                this.#current = v;
            });
        };

        for (const ev of events) {
            listen(ev.name, (event) => {
                if (ev.filter((event as any).payload)) fetch();
            }).then((u) => unlisteners.push(u));
        }
        fetch();

        eventCleanupRegistry?.register(this, unlisteners);
    }

    get current(): T | undefined {
        return this.#current;
    }
}

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

export class ParagraphTranslationActivityResource {
    #current: ParagraphTranslationActivity | null = $state(null);

    constructor(bookId: UUID, paragraphId: number) {
        const unlisteners: UnlistenFn[] = [];
        eventCleanupRegistry?.register(this, unlisteners);

        const matches = (ev: { bookId: UUID; paragraphId: number }) =>
            ev.bookId === bookId && ev.paragraphId === paragraphId;

        // Register listeners first, then fetch the initial snapshot. The
        // backend orders state mutation (active_translations.remove) before
        // emitting "finished", so a snapshot taken after listeners are live
        // cannot contradict a finished event we'd subsequently receive.
        const setup = async () => {
            unlisteners.push(
                await listen<StartedEvent>(
                    "paragraph_translation_started",
                    (e) => {
                        const p = e.payload;
                        if (!matches(p)) return;
                        this.#current = {
                            requestId: p.requestId,
                            progressChars: 0,
                            expectedChars: p.expectedChars,
                        };
                    },
                ),
            );
            unlisteners.push(
                await listen<ProgressEvent>(
                    "paragraph_translation_progress",
                    (e) => {
                        const p = e.payload;
                        if (!matches(p)) return;
                        this.#current = {
                            requestId: p.requestId,
                            progressChars: p.progressChars,
                            expectedChars: p.expectedChars,
                        };
                    },
                ),
            );
            unlisteners.push(
                await listen<FinishedEvent>(
                    "paragraph_translation_finished",
                    (e) => {
                        const p = e.payload;
                        if (!matches(p)) return;
                        if (p.error) {
                            console.warn(
                                `Translation failed for paragraph ${paragraphId}:`,
                                p.error,
                            );
                        }
                        this.#current = null;
                    },
                ),
            );

            const v = await invoke<ParagraphTranslationActivity | null>(
                "get_paragraph_translation_activity",
                { bookId, paragraphId },
            );
            // Don't clobber state populated by events that landed first.
            if (this.#current === null && v !== null) {
                this.#current = v;
            }
        };
        setup().catch(() => {});
    }

    get current(): ParagraphTranslationActivity | null {
        return this.#current;
    }
}
