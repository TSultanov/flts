import { invoke, type InvokeArgs } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import type { UUID } from "./v2/db";

export type UpdateEvent<TEvent = any> = {
    name: string;
    filter: (ev: TEvent) => boolean;
};

const eventCleanupRegistry =
    typeof FinalizationRegistry !== "undefined"
        ? new FinalizationRegistry<Array<() => void>>((unsubscribers) => {
              for (const u of unsubscribers) {
                  try { u(); } catch { /* ignore */ }
              }
          })
        : null;

type Subscriber = {
    filter: (payload: any) => boolean;
    handler: (payload: any) => void;
};

// Singleton router: at most one Tauri `listen()` per event name, with
// in-process fan-out to subscribers. Each per-paragraph Resource subscribing
// here costs an O(1) Set insert instead of a ~10 ms IPC round-trip to
// register a native listener.
class TauriEventHub {
    #subs = new Map<string, Set<Subscriber>>();
    #ready = new Map<string, Promise<void>>();

    subscribe<T>(
        name: string,
        filter: (payload: T) => boolean,
        handler: (payload: T) => void,
    ): () => void {
        let set = this.#subs.get(name);
        if (!set) {
            set = new Set();
            this.#subs.set(name, set);
            this.#install(name);
        }
        const sub: Subscriber = {
            filter: filter as (p: any) => boolean,
            handler: handler as (p: any) => void,
        };
        set.add(sub);
        return () => set!.delete(sub);
    }

    async subscribeReady<T>(
        name: string,
        filter: (payload: T) => boolean,
        handler: (payload: T) => void,
    ): Promise<() => void> {
        const unsub = this.subscribe(name, filter, handler);
        await this.#ready.get(name);
        return unsub;
    }

    #install(name: string) {
        const p = listen(name, (event) => {
            const set = this.#subs.get(name);
            if (!set || set.size === 0) return;
            const payload = (event as any).payload;
            for (const sub of set) {
                try {
                    if (sub.filter(payload)) sub.handler(payload);
                } catch {
                    /* swallow — one bad subscriber must not break others */
                }
            }
        }).then(() => undefined);
        this.#ready.set(name, p);
    }
}

export const eventHub = new TauriEventHub();

export class Resource<T> {
    #current: T | undefined = $state(undefined);

    constructor(
        getterName: string,
        args: InvokeArgs = {},
        events: UpdateEvent[] = [],
        defaultValue?: T,
    ) {
        this.#current = defaultValue;
        const unsubscribers: Array<() => void> = [];

        const fetch = () => {
            invoke<T>(getterName, args).then((v) => {
                this.#current = v;
            });
        };

        for (const ev of events) {
            unsubscribers.push(eventHub.subscribe(ev.name, ev.filter, fetch));
        }
        fetch();

        eventCleanupRegistry?.register(this, unsubscribers);
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
        const unsubscribers: Array<() => void> = [];
        eventCleanupRegistry?.register(this, unsubscribers);

        const matches = (ev: { bookId: UUID; paragraphId: number }) =>
            ev.bookId === bookId && ev.paragraphId === paragraphId;

        // Register listeners first, then fetch the initial snapshot. The
        // backend orders state mutation (active_translations.remove) before
        // emitting "finished", so a snapshot taken after listeners are live
        // cannot contradict a finished event we'd subsequently receive.
        const setup = async () => {
            unsubscribers.push(
                await eventHub.subscribeReady<StartedEvent>(
                    "paragraph_translation_started",
                    matches,
                    (p) => {
                        this.#current = {
                            requestId: p.requestId,
                            progressChars: 0,
                            expectedChars: p.expectedChars,
                        };
                    },
                ),
            );
            unsubscribers.push(
                await eventHub.subscribeReady<ProgressEvent>(
                    "paragraph_translation_progress",
                    matches,
                    (p) => {
                        this.#current = {
                            requestId: p.requestId,
                            progressChars: p.progressChars,
                            expectedChars: p.expectedChars,
                        };
                    },
                ),
            );
            unsubscribers.push(
                await eventHub.subscribeReady<FinishedEvent>(
                    "paragraph_translation_finished",
                    matches,
                    (p) => {
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
